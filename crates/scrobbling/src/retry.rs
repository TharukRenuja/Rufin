use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use library::{Library, NewScrobble, PendingScrobble, PendingScrobbleId, ScrobbleService};
use playback::{CompletedScrobble, ListeningTrack};
use reqwest::blocking::Client;
use tracing::warn;

use crate::services::{audioscrobbler, listenbrainz};
use crate::{AudioscrobblerSettings, ListenBrainzSettings, Settings};

const COMMAND_CAPACITY: usize = 32;
const DELIVERY_BATCH_SIZE: usize = 50;
const NOW_PLAYING_STABLE_DELAY: Duration = Duration::from_secs(1);
const RETRY_POLL: Duration = Duration::from_secs(30);
const USER_AGENT: &str = concat!("Rufin/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SubmissionTrack {
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) album: String,
    pub(crate) duration_millis: u64,
}

impl SubmissionTrack {
    fn capture(track: &ListeningTrack) -> Option<Self> {
        let title = track.title.trim();
        let artists = track
            .artists
            .iter()
            .map(|artist| artist.trim())
            .filter(|artist| !artist.is_empty())
            .collect::<Vec<_>>();
        if title.is_empty() || artists.is_empty() {
            return None;
        }
        Some(Self {
            title: title.to_string(),
            artist: artists.join(", "),
            album: track
                .album
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_string(),
            duration_millis: track.duration_millis,
        })
    }

    fn from_pending(pending: &PendingScrobble) -> Self {
        Self {
            title: pending.track_title.clone(),
            artist: pending.artist_name.clone(),
            album: pending.album_title.clone().unwrap_or_default(),
            duration_millis: pending.duration_millis,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Submission {
    NowPlaying(SubmissionTrack),
    Scrobble {
        track: SubmissionTrack,
        started_at_unix_seconds: i64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DeliveryError {
    Retry(String),
    CredentialBlocked(String),
    Stop(String),
}

impl DeliveryError {
    pub(crate) fn retry(error: impl Into<String>) -> Self {
        Self::Retry(error.into())
    }

    pub(crate) fn credential_blocked(error: impl Into<String>) -> Self {
        Self::CredentialBlocked(error.into())
    }

    pub(crate) fn stop(error: impl Into<String>) -> Self {
        Self::Stop(error.into())
    }
}

impl std::fmt::Display for DeliveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Retry(error) | Self::CredentialBlocked(error) | Self::Stop(error) => {
                formatter.write_str(error)
            }
        }
    }
}

#[derive(Clone)]
enum TargetSettings {
    Audioscrobbler {
        service: audioscrobbler::Service,
        settings: AudioscrobblerSettings,
    },
    ListenBrainz(ListenBrainzSettings),
}

#[derive(Clone)]
struct DeliveryTarget {
    service: ScrobbleService,
    account_id: String,
    settings: TargetSettings,
}

#[derive(Clone)]
struct DeliveryState {
    settings: Settings,
    private_mode: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeliveryFlow {
    Continue,
    StopAccount,
}

enum Command {
    NowPlaying(SubmissionTrack),
    Wake,
}

fn now_playing_is_stable(changed_at: Instant, now: Instant) -> bool {
    now >= changed_at + NOW_PLAYING_STABLE_DELAY
}

struct Worker {
    sender: SyncSender<Command>,
    _thread: JoinHandle<()>,
}

impl Worker {
    fn new(library: Library, state: Arc<Mutex<DeliveryState>>) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(6))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|error| error.to_string())?;
        let (sender, receiver) = sync_channel(COMMAND_CAPACITY);
        let thread = std::thread::Builder::new()
            .name("rufin-scrobbling".to_string())
            .spawn(move || run_worker(client, library, state, receiver))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            sender,
            _thread: thread,
        })
    }

    fn now_playing(&self, track: SubmissionTrack) {
        match self.sender.try_send(Command::NowPlaying(track)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                warn!("scrobbling worker is busy; dropping transient now-playing update");
            }
            Err(TrySendError::Disconnected(_)) => {
                warn!("scrobbling worker is unavailable; dropping transient now-playing update");
            }
        }
    }

    fn wake(&self) {
        let _ = self.sender.try_send(Command::Wake);
    }
}

pub struct Scrobbler {
    library: Library,
    state: Arc<Mutex<DeliveryState>>,
    worker: Worker,
}

impl Scrobbler {
    pub fn new(
        library: Library,
        mut settings: Settings,
        private_mode: bool,
    ) -> Result<Self, String> {
        settings.sanitize();
        let state = Arc::new(Mutex::new(DeliveryState {
            settings,
            private_mode,
        }));
        let worker = Worker::new(library.clone(), Arc::clone(&state))?;
        Ok(Self {
            library,
            state,
            worker,
        })
    }

    pub fn update_settings(
        &self,
        mut settings: Settings,
        private_mode: bool,
    ) -> Result<(), String> {
        settings.sanitize();
        let (previous_accounts, current_accounts, reauthorized) = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "scrobbling settings lock was poisoned".to_string())?;
            let previous = known_accounts(&state.settings);
            let current = known_accounts(&settings);
            let reauthorized = reauthorized_accounts(&state.settings, &settings);
            state.settings = settings;
            state.private_mode = private_mode;
            (previous, current, reauthorized)
        };
        for (service, account_id) in previous_accounts {
            if !current_accounts
                .iter()
                .any(|current| current == &(service, account_id.clone()))
            {
                self.library
                    .discard_scrobbles(service, &account_id)
                    .map_err(|error| error.to_string())?;
            }
        }
        let now = unix_seconds();
        for (service, account_id) in reauthorized {
            self.library
                .wake_scrobbles(service, &account_id, now)
                .map_err(|error| error.to_string())?;
        }
        if !private_mode {
            self.worker.wake();
        }
        Ok(())
    }

    pub fn now_playing(&self, track: &ListeningTrack) {
        let enabled = self
            .state
            .lock()
            .is_ok_and(|state| !state.private_mode && !targets(&state.settings, true).is_empty());
        if enabled && let Some(track) = SubmissionTrack::capture(track) {
            self.worker.now_playing(track);
        }
    }

    pub fn completed_play(&self, completed: &CompletedScrobble) -> Result<usize, String> {
        let targets = {
            let state = self
                .state
                .lock()
                .map_err(|_| "scrobbling settings lock was poisoned".to_string())?;
            if state.private_mode {
                return Ok(0);
            }
            targets(&state.settings, false)
        };
        let Some(track) = SubmissionTrack::capture(&completed.track) else {
            return Ok(0);
        };
        let scrobbles = targets
            .into_iter()
            .map(|target| NewScrobble {
                id: PendingScrobbleId {
                    service: target.service,
                    account_id: target.account_id,
                    play_id: completed.play_id.clone(),
                },
                track_title: track.title.clone(),
                artist_name: track.artist.clone(),
                album_title: (!track.album.is_empty()).then(|| track.album.clone()),
                duration_millis: track.duration_millis,
                started_at: completed.started_at_unix_seconds,
            })
            .collect::<Vec<_>>();
        let accepted = self
            .library
            .queue_scrobbles(scrobbles)
            .map_err(|error| error.to_string())?;
        if accepted > 0 {
            self.worker.wake();
        }
        Ok(accepted)
    }
}

fn run_worker(
    client: Client,
    library: Library,
    state: Arc<Mutex<DeliveryState>>,
    receiver: Receiver<Command>,
) {
    let mut pending_now_playing = None;
    let mut retry_at = Instant::now() + RETRY_POLL;
    loop {
        let now = Instant::now();
        if now >= retry_at {
            deliver_due(&client, &library, &state);
            retry_at = Instant::now() + RETRY_POLL;
            continue;
        }
        if pending_now_playing
            .as_ref()
            .is_some_and(|(_, changed_at)| now_playing_is_stable(*changed_at, now))
        {
            let (track, _) = pending_now_playing
                .take()
                .expect("stable now-playing update must be pending");
            deliver_now_playing(&client, &state, track);
            continue;
        }
        let deadline = pending_now_playing
            .as_ref()
            .map(|(_, changed_at)| (*changed_at + NOW_PLAYING_STABLE_DELAY).min(retry_at))
            .unwrap_or(retry_at);
        match receiver.recv_timeout(deadline.saturating_duration_since(now)) {
            Ok(Command::NowPlaying(track)) => {
                pending_now_playing = Some((track, Instant::now()));
            }
            Ok(Command::Wake) => {
                // Durable completed scrobbles do not wait for the transient now-playing window.
                deliver_due(&client, &library, &state);
                retry_at = Instant::now() + RETRY_POLL;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn deliver_now_playing(client: &Client, state: &Arc<Mutex<DeliveryState>>, track: SubmissionTrack) {
    let targets = state
        .lock()
        .ok()
        .filter(|state| !state.private_mode)
        .map(|state| targets(&state.settings, true))
        .unwrap_or_default();
    let submission = Submission::NowPlaying(track);
    for target in targets {
        if let Err(error) = submit(client, &target, &submission) {
            warn!(%error, service = ?target.service, "now-playing update failed");
        }
    }
}

fn deliver_due(client: &Client, library: &Library, state: &Arc<Mutex<DeliveryState>>) {
    let targets = state
        .lock()
        .ok()
        .filter(|state| !state.private_mode)
        .map(|state| targets(&state.settings, false))
        .unwrap_or_default();
    let now = unix_seconds();
    for target in targets {
        let pending = match library.due_scrobbles(
            target.service,
            &target.account_id,
            now,
            DELIVERY_BATCH_SIZE,
        ) {
            Ok(pending) => pending,
            Err(error) => {
                warn!(%error, service = ?target.service, "could not read external scrobbling work");
                continue;
            }
        };
        for pending in pending {
            let Some(current) = current_target(state, target.service, &target.account_id) else {
                break;
            };
            let submission = Submission::Scrobble {
                track: SubmissionTrack::from_pending(&pending),
                started_at_unix_seconds: pending.started_at,
            };
            if finish_delivery(library, pending, now, submit(client, &current, &submission))
                == DeliveryFlow::StopAccount
            {
                break;
            }
        }
    }
}

fn finish_delivery(
    library: &Library,
    pending: PendingScrobble,
    now: i64,
    result: Result<(), DeliveryError>,
) -> DeliveryFlow {
    match result {
        Ok(()) => {
            if let Err(error) = library.complete_scrobble(pending.id) {
                warn!(%error, "could not complete external scrobbling work");
            }
            DeliveryFlow::Continue
        }
        Err(DeliveryError::Retry(error)) => {
            let service = pending.id.service;
            let next_attempt_at = now.saturating_add(retry_delay(pending.attempts));
            if let Err(store_error) = library.defer_scrobble(pending.id, next_attempt_at) {
                warn!(%store_error, "could not defer external scrobbling work");
            }
            warn!(
                %error,
                ?service,
                "external scrobble will be retried"
            );
            DeliveryFlow::StopAccount
        }
        Err(DeliveryError::CredentialBlocked(error)) => {
            let service = pending.id.service;
            if let Err(store_error) =
                library.block_scrobbles(service, &pending.id.account_id, &error)
            {
                warn!(%store_error, "could not preserve credential-blocked scrobbles");
            }
            warn!(
                %error,
                ?service,
                "external scrobbling credentials need attention"
            );
            DeliveryFlow::StopAccount
        }
        Err(DeliveryError::Stop(error)) => {
            let service = pending.id.service;
            warn!(
                %error,
                ?service,
                "external scrobble was rejected"
            );
            if let Err(store_error) = library.complete_scrobble(pending.id) {
                warn!(%store_error, "could not discard rejected external scrobble");
            }
            DeliveryFlow::Continue
        }
    }
}

fn current_target(
    state: &Arc<Mutex<DeliveryState>>,
    service: ScrobbleService,
    account_id: &str,
) -> Option<DeliveryTarget> {
    state
        .lock()
        .ok()
        .filter(|state| !state.private_mode)
        .and_then(|state| {
            targets(&state.settings, false)
                .into_iter()
                .find(|target| target.service == service && target.account_id == account_id)
        })
}

fn submit(
    client: &Client,
    target: &DeliveryTarget,
    submission: &Submission,
) -> Result<(), DeliveryError> {
    match &target.settings {
        TargetSettings::Audioscrobbler { service, settings } => {
            audioscrobbler::submit(client, *service, settings, submission)
        }
        TargetSettings::ListenBrainz(settings) => {
            listenbrainz::submit(client, settings, submission)
        }
    }
}

fn targets(settings: &Settings, now_playing: bool) -> Vec<DeliveryTarget> {
    let mut targets = Vec::with_capacity(3);
    if settings.lastfm.configured(now_playing) {
        targets.push(DeliveryTarget {
            service: ScrobbleService::LastFm,
            account_id: audioscrobbler_account_id(ScrobbleService::LastFm, &settings.lastfm),
            settings: TargetSettings::Audioscrobbler {
                service: audioscrobbler::Service::LastFm,
                settings: settings.lastfm.clone(),
            },
        });
    }
    if settings.librefm.configured(now_playing) {
        targets.push(DeliveryTarget {
            service: ScrobbleService::LibreFm,
            account_id: audioscrobbler_account_id(ScrobbleService::LibreFm, &settings.librefm),
            settings: TargetSettings::Audioscrobbler {
                service: audioscrobbler::Service::LibreFm,
                settings: settings.librefm.clone(),
            },
        });
    }
    if settings.listenbrainz.configured(now_playing) {
        targets.push(DeliveryTarget {
            service: ScrobbleService::ListenBrainz,
            account_id: opaque_account_id(
                ScrobbleService::ListenBrainz,
                &settings.listenbrainz.user_token,
            ),
            settings: TargetSettings::ListenBrainz(settings.listenbrainz.clone()),
        });
    }
    targets
}

fn known_accounts(settings: &Settings) -> Vec<(ScrobbleService, String)> {
    let mut accounts = Vec::with_capacity(3);
    if !settings.lastfm.session_key.is_empty() {
        accounts.push((
            ScrobbleService::LastFm,
            audioscrobbler_account_id(ScrobbleService::LastFm, &settings.lastfm),
        ));
    }
    if !settings.librefm.session_key.is_empty() {
        accounts.push((
            ScrobbleService::LibreFm,
            audioscrobbler_account_id(ScrobbleService::LibreFm, &settings.librefm),
        ));
    }
    if !settings.listenbrainz.user_token.is_empty() {
        accounts.push((
            ScrobbleService::ListenBrainz,
            opaque_account_id(
                ScrobbleService::ListenBrainz,
                &settings.listenbrainz.user_token,
            ),
        ));
    }
    accounts
}

fn reauthorized_accounts(
    previous: &Settings,
    current: &Settings,
) -> Vec<(ScrobbleService, String)> {
    let mut accounts = Vec::with_capacity(2);
    for (service, previous, current) in [
        (ScrobbleService::LastFm, &previous.lastfm, &current.lastfm),
        (
            ScrobbleService::LibreFm,
            &previous.librefm,
            &current.librefm,
        ),
    ] {
        if previous.session_key.is_empty() || current.session_key.is_empty() {
            continue;
        }
        let previous_account = audioscrobbler_account_id(service, previous);
        let current_account = audioscrobbler_account_id(service, current);
        let credentials_changed = previous.api_key != current.api_key
            || previous.api_secret != current.api_secret
            || previous.session_key != current.session_key;
        if previous_account == current_account && credentials_changed {
            accounts.push((service, current_account));
        }
    }
    accounts
}

fn audioscrobbler_account_id(
    service: ScrobbleService,
    settings: &AudioscrobblerSettings,
) -> String {
    let identity = if settings.username.trim().is_empty() {
        settings.session_key.as_str()
    } else {
        settings.username.as_str()
    };
    opaque_account_id(service, &identity.to_lowercase())
}

fn opaque_account_id(service: ScrobbleService, identity: &str) -> String {
    let service = match service {
        ScrobbleService::LastFm => "lastfm",
        ScrobbleService::LibreFm => "librefm",
        ScrobbleService::ListenBrainz => "listenbrainz",
    };
    let value = format!("rufin-scrobbling-account\0{service}\0{}", identity.trim());
    format!("{:x}", md5::compute(value))
}

fn retry_delay(attempts: u32) -> i64 {
    let exponent = attempts.min(7);
    (30_i64.saturating_mul(1_i64 << exponent)).min(3_600)
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use library::{SourceId, TrackId};

    use super::*;

    #[test]
    fn now_playing_waits_for_one_quiet_second() {
        let changed_at = Instant::now();
        assert!(!now_playing_is_stable(
            changed_at,
            changed_at + Duration::from_millis(999)
        ));
        assert!(now_playing_is_stable(
            changed_at,
            changed_at + Duration::from_secs(1)
        ));
    }

    #[test]
    fn account_identity_is_service_scoped_and_does_not_store_the_secret() {
        let lastfm = opaque_account_id(ScrobbleService::LastFm, "secret");
        let listenbrainz = opaque_account_id(ScrobbleService::ListenBrainz, "secret");
        assert_ne!(lastfm, listenbrainz);
        assert!(!lastfm.contains("secret"));
    }

    #[test]
    fn retry_delay_is_bounded() {
        assert_eq!(retry_delay(0), 30);
        assert_eq!(retry_delay(3), 240);
        assert_eq!(retry_delay(100), 3_600);
    }

    #[test]
    fn completed_track_capture_uses_canonical_artist_credit_text() {
        let track = ListeningTrack {
            source_id: SourceId::new("source"),
            track_id: TrackId::new("track"),
            recording_id: None,
            title: " Track ".to_string(),
            artists: vec!["Artist".to_string(), "Guest".to_string()],
            album: Some(" Album ".to_string()),
            track_number: None,
            disc_number: None,
            duration_millis: 180_000,
        };
        assert_eq!(
            SubmissionTrack::capture(&track),
            Some(SubmissionTrack {
                title: "Track".to_string(),
                artist: "Artist, Guest".to_string(),
                album: "Album".to_string(),
                duration_millis: 180_000,
            })
        );
    }

    #[test]
    fn delivery_results_delete_or_preserve_the_original_completed_play() {
        let directory = tempfile::tempdir().expect("temporary scrobbling directory");
        let path = directory.path().join("library.db");
        let library = Library::open(&path).expect("open Library");
        let account_id = "listener";

        let success = queue_one(&library, account_id, "success", 11);
        assert_eq!(
            finish_delivery(&library, success, 20, Ok(())),
            DeliveryFlow::Continue
        );
        assert!(
            library
                .due_scrobbles(ScrobbleService::LastFm, account_id, i64::MAX, 10)
                .expect("read after immediate success")
                .is_empty()
        );

        let retry = queue_one(&library, account_id, "retry", 12);
        assert_eq!(
            finish_delivery(
                &library,
                retry,
                20,
                Err(DeliveryError::retry("service unavailable")),
            ),
            DeliveryFlow::StopAccount
        );
        assert!(
            library
                .due_scrobbles(ScrobbleService::LastFm, account_id, 49, 10)
                .expect("read before retry deadline")
                .is_empty()
        );
        let retry = library
            .due_scrobbles(ScrobbleService::LastFm, account_id, 50, 10)
            .expect("read retry")
            .into_iter()
            .next()
            .expect("transient failure stays queued");
        assert_eq!(retry.attempts, 1);
        library
            .complete_scrobble(retry.id)
            .expect("finish retry fixture");

        let rejected = queue_one(&library, account_id, "rejected", 12);
        assert_eq!(
            finish_delivery(
                &library,
                rejected,
                20,
                Err(DeliveryError::stop("invalid track fields")),
            ),
            DeliveryFlow::Continue
        );
        assert!(
            library
                .due_scrobbles(ScrobbleService::LastFm, account_id, i64::MAX, 10)
                .expect("read after permanent rejection")
                .is_empty()
        );

        let blocked = queue_one(&library, account_id, "blocked", 13);
        library
            .queue_scrobbles(vec![new_scrobble(account_id, "also-blocked", 14)])
            .expect("queue second account delivery");
        assert_eq!(
            finish_delivery(
                &library,
                blocked,
                20,
                Err(DeliveryError::credential_blocked("invalid session")),
            ),
            DeliveryFlow::StopAccount
        );
        drop(library);

        let reopened = Library::open(&path).expect("reopen Library");
        assert!(
            reopened
                .due_scrobbles(ScrobbleService::LastFm, account_id, i64::MAX, 10)
                .expect("read credential-blocked work")
                .is_empty()
        );
        assert_eq!(
            reopened
                .wake_scrobbles(ScrobbleService::LastFm, "another-listener", 21)
                .expect("wake other account"),
            0
        );
        assert_eq!(
            reopened
                .wake_scrobbles(ScrobbleService::LastFm, account_id, 21)
                .expect("wake reauthorized account"),
            2
        );
        let due = reopened
            .due_scrobbles(ScrobbleService::LastFm, account_id, 21, 10)
            .expect("read reauthorized work");
        assert_eq!(
            due.iter()
                .map(|pending| (pending.id.play_id.as_str(), pending.started_at))
                .collect::<Vec<_>>(),
            vec![("blocked", 13), ("also-blocked", 14)]
        );
    }

    #[test]
    fn only_changed_credentials_for_the_same_account_wake_blocked_work() {
        let previous = audioscrobbler_settings("listener", "session-one");
        let mut current = previous.clone();
        current.session_key = "session-two".to_string();
        let previous = Settings {
            lastfm: previous,
            ..Settings::default()
        };
        let current = Settings {
            lastfm: current,
            ..Settings::default()
        };
        let account_id = audioscrobbler_account_id(ScrobbleService::LastFm, &previous.lastfm);

        let directory = tempfile::tempdir().expect("temporary scrobbling directory");
        let library = Library::open(directory.path().join("library.db")).expect("open Library");
        library
            .queue_scrobbles(vec![new_scrobble(&account_id, "blocked", 10)])
            .expect("queue completed play");
        library
            .block_scrobbles(ScrobbleService::LastFm, &account_id, "invalid session")
            .expect("block account work");
        let scrobbler =
            Scrobbler::new(library.clone(), previous.clone(), true).expect("start Scrobbler");
        scrobbler
            .update_settings(current.clone(), true)
            .expect("replace account credentials");
        assert_eq!(
            library
                .due_scrobbles(ScrobbleService::LastFm, &account_id, i64::MAX, 10)
                .expect("read woken work")
                .len(),
            1
        );

        assert_eq!(
            reauthorized_accounts(&previous, &current),
            vec![(ScrobbleService::LastFm, account_id)]
        );

        let mut other_account = current.clone();
        other_account.lastfm.username = "another-listener".to_string();
        assert!(reauthorized_accounts(&previous, &other_account).is_empty());

        let mut presentation_only = previous.clone();
        presentation_only.lastfm.now_playing_enabled = false;
        assert!(reauthorized_accounts(&previous, &presentation_only).is_empty());
    }

    fn queue_one(
        library: &Library,
        account_id: &str,
        play_id: &str,
        started_at: i64,
    ) -> PendingScrobble {
        library
            .queue_scrobbles(vec![new_scrobble(account_id, play_id, started_at)])
            .expect("queue completed play");
        library
            .due_scrobbles(ScrobbleService::LastFm, account_id, started_at, 10)
            .expect("read completed play")
            .into_iter()
            .find(|pending| pending.id.play_id == play_id)
            .expect("queued play is due immediately")
    }

    fn new_scrobble(account_id: &str, play_id: &str, started_at: i64) -> NewScrobble {
        NewScrobble {
            id: PendingScrobbleId {
                service: ScrobbleService::LastFm,
                account_id: account_id.to_string(),
                play_id: play_id.to_string(),
            },
            track_title: "Track".to_string(),
            artist_name: "Artist".to_string(),
            album_title: Some("Album".to_string()),
            duration_millis: 180_000,
            started_at,
        }
    }

    fn audioscrobbler_settings(username: &str, session_key: &str) -> AudioscrobblerSettings {
        AudioscrobblerSettings {
            enabled: true,
            username: username.to_string(),
            api_key: "api-key".to_string(),
            api_secret: "api-secret".to_string(),
            session_key: session_key.to_string(),
            now_playing_enabled: true,
        }
    }
}
