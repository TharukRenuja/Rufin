mod discord;

use std::sync::{
    Arc, Mutex, Weak,
    mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use playback::{PlaybackView, TransportStatus};
use tracing::debug;

pub use discord::{DEFAULT_CLIENT_ID, DisplayType, LinkType, Settings};

pub(crate) struct LatestSender<T> {
    value: Arc<Mutex<Option<T>>>,
    wake: SyncSender<()>,
}

impl<T> LatestSender<T> {
    fn publish(&self, value: T) {
        let Ok(mut latest) = self.value.lock() else {
            return;
        };
        *latest = Some(value);
        drop(latest);
        if let Err(TrySendError::Disconnected(())) = self.wake.try_send(()) {
            self.clear();
        }
    }

    fn clear(&self) {
        if let Ok(mut latest) = self.value.lock() {
            *latest = None;
        }
    }
}

pub(crate) struct LatestReceiver<T> {
    value: Arc<Mutex<Option<T>>>,
    wake: Receiver<()>,
}

impl<T> LatestReceiver<T> {
    fn recv(&self) -> Option<T> {
        loop {
            self.wake.recv().ok()?;
            if let Some(value) = self.take() {
                return Some(value);
            }
        }
    }

    fn recv_timeout(&self, delay: Duration) -> Result<T, RecvTimeoutError> {
        loop {
            self.wake.recv_timeout(delay)?;
            if let Some(value) = self.take() {
                return Ok(value);
            }
        }
    }

    #[cfg(test)]
    fn try_recv(&self) -> Option<T> {
        while self.wake.try_recv().is_ok() {
            if let Some(value) = self.take() {
                return Some(value);
            }
        }
        None
    }

    fn take(&self) -> Option<T> {
        self.value.lock().ok()?.take()
    }
}

fn latest_slot<T>() -> (LatestSender<T>, LatestReceiver<T>) {
    let value = Arc::new(Mutex::new(None));
    let (wake, receiver) = sync_channel(1);
    (
        LatestSender {
            value: Arc::clone(&value),
            wake,
        },
        LatestReceiver {
            value,
            wake: receiver,
        },
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArtworkKey {
    artist: String,
    album: String,
    musicbrainz_album_id: Option<String>,
    musicbrainz_release_group_id: Option<String>,
    lastfm_api_key: String,
    allow_musicbrainz: bool,
}

pub struct ArtworkRequest {
    revision: u64,
    key: ArtworkKey,
    queued_at: Instant,
    owner: Weak<Inner>,
}

impl ArtworkRequest {
    pub fn artist(&self) -> &str {
        &self.key.artist
    }

    pub fn album(&self) -> &str {
        &self.key.album
    }

    pub fn lastfm_api_key(&self) -> &str {
        &self.key.lastfm_api_key
    }

    pub fn musicbrainz_album_id(&self) -> Option<&str> {
        self.key.musicbrainz_album_id.as_deref()
    }

    pub fn musicbrainz_release_group_id(&self) -> Option<&str> {
        self.key.musicbrainz_release_group_id.as_deref()
    }

    pub const fn allow_musicbrainz(&self) -> bool {
        self.key.allow_musicbrainz
    }

    pub fn queued_for(&self) -> Duration {
        self.queued_at.elapsed()
    }

    pub fn complete(self, result: Result<Option<String>, String>) {
        if let Some(owner) = self.owner.upgrade() {
            owner.complete_artwork(self.revision, &self.key, result);
        }
    }
}

pub struct ArtworkRequests {
    receiver: LatestReceiver<ArtworkRequest>,
}

impl ArtworkRequests {
    pub fn recv(&self) -> Option<ArtworkRequest> {
        self.receiver.recv()
    }

    #[cfg(test)]
    fn try_recv(&self) -> Option<ArtworkRequest> {
        self.receiver.try_recv()
    }
}

pub struct Presence {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<State>,
    artwork: LatestSender<ArtworkRequest>,
}

#[derive(Default)]
struct State {
    settings: Settings,
    lastfm_api_key: String,
    activity: Option<Arc<discord::Activity>>,
    artwork: ArtworkState,
    next_artwork_revision: u64,
    worker: Option<discord::Worker>,
}

#[derive(Default)]
enum ArtworkState {
    #[default]
    Empty,
    Pending {
        revision: u64,
        key: ArtworkKey,
    },
    Ready {
        key: ArtworkKey,
        url: Option<String>,
    },
}

impl Presence {
    pub fn new() -> (Self, ArtworkRequests) {
        let (artwork, receiver) = latest_slot();
        let inner = Arc::new(Inner {
            state: Mutex::new(State::default()),
            artwork,
        });
        (Self { inner }, ArtworkRequests { receiver })
    }

    pub fn update(
        &self,
        mut settings: Settings,
        delivery_enabled: bool,
        lastfm_api_key: &str,
        view: Option<&PlaybackView>,
    ) {
        settings.enabled &= delivery_enabled && discord::SUPPORTED;
        let Ok(mut state) = self.inner.state.lock() else {
            return;
        };
        state.settings = settings;
        state.lastfm_api_key = lastfm_api_key.to_string();
        self.refresh(&mut state, view, unix_now_millis());
    }

    pub fn observe(&self, view: Option<&PlaybackView>, position_discontinuity: bool) {
        let Ok(mut state) = self.inner.state.lock() else {
            return;
        };
        if !position_discontinuity && state.matches(view) {
            return;
        }
        self.refresh(&mut state, view, unix_now_millis());
    }

    fn refresh(&self, state: &mut State, view: Option<&PlaybackView>, now_millis: u64) {
        let Some(view) = view else {
            self.clear(state);
            return;
        };
        if state.settings.enabled && view.transport.state == TransportStatus::Resolving {
            if matches!(state.artwork, ArtworkState::Pending { .. }) {
                state.artwork = ArtworkState::Empty;
                self.inner.artwork.clear();
            }
            return;
        }
        let Some(mut activity) = discord::Activity::new(
            &state.settings,
            view,
            now_millis,
            discord::APP_ICON_URL.to_string(),
        ) else {
            self.clear(state);
            return;
        };
        activity.large_image = self.artwork_image(state, view);
        let activity = Arc::new(activity);
        state.activity = Some(Arc::clone(&activity));
        if !matches!(state.artwork, ArtworkState::Pending { .. }) {
            state.publish(Some(activity));
        }
    }

    fn clear(&self, state: &mut State) {
        if matches!(state.artwork, ArtworkState::Pending { .. }) {
            state.artwork = ArtworkState::Empty;
        }
        self.inner.artwork.clear();
        if state.activity.take().is_some() {
            state.publish(None);
        }
    }

    fn artwork_image(&self, state: &mut State, view: &PlaybackView) -> String {
        let Some(key) =
            ArtworkKey::from_view(view, &state.lastfm_api_key, state.settings.link_type)
        else {
            state.artwork = ArtworkState::Empty;
            self.inner.artwork.clear();
            return discord::APP_ICON_URL.to_string();
        };
        match &state.artwork {
            ArtworkState::Pending { key: pending, .. } if pending == &key => {
                return state
                    .activity
                    .as_ref()
                    .map(|activity| activity.large_image.clone())
                    .unwrap_or_else(|| discord::APP_ICON_URL.to_string());
            }
            ArtworkState::Ready { key: ready, url } if ready == &key => {
                return url
                    .clone()
                    .unwrap_or_else(|| discord::APP_ICON_URL.to_string());
            }
            ArtworkState::Empty | ArtworkState::Pending { .. } | ArtworkState::Ready { .. } => {}
        }

        state.next_artwork_revision = state.next_artwork_revision.wrapping_add(1);
        let revision = state.next_artwork_revision;
        state.artwork = ArtworkState::Pending {
            revision,
            key: key.clone(),
        };
        self.inner.artwork.publish(ArtworkRequest {
            revision,
            key,
            queued_at: Instant::now(),
            owner: Arc::downgrade(&self.inner),
        });
        discord::APP_ICON_URL.to_string()
    }
}

impl State {
    fn matches(&self, view: Option<&PlaybackView>) -> bool {
        match (&self.activity, view) {
            (None, None) => true,
            (Some(_), None) => false,
            (None, Some(view)) => {
                discord::visible_playback_state(&self.settings, view.transport.state).is_none()
                    || view.transport.run.is_none()
                    || view.transport.current.is_none()
            }
            (Some(activity), Some(view)) => activity.matches(view),
        }
    }

    fn publish(&mut self, activity: Option<Arc<discord::Activity>>) {
        match activity {
            Some(activity) => self
                .worker
                .get_or_insert_with(discord::Worker::new)
                .publish(Some(activity)),
            None => {
                if let Some(worker) = &self.worker {
                    worker.publish(None);
                }
            }
        }
    }

    fn complete_artwork(
        &mut self,
        revision: u64,
        key: &ArtworkKey,
        result: Result<Option<String>, String>,
    ) -> Option<Arc<discord::Activity>> {
        if !matches!(
            &self.artwork,
            ArtworkState::Pending { revision: pending, key: pending_key }
                if *pending == revision && pending_key == key
        ) {
            return None;
        }
        let url = match result {
            Ok(url) => url,
            Err(error) => {
                debug!(%error, "rich-presence artwork lookup failed");
                None
            }
        };
        self.artwork = ArtworkState::Ready {
            key: key.clone(),
            url: url.clone(),
        };
        let image = url.unwrap_or_else(|| discord::APP_ICON_URL.to_string());
        let activity = self.activity.as_mut()?;
        Arc::make_mut(activity).large_image = image;
        Some(Arc::clone(activity))
    }
}

impl ArtworkKey {
    fn from_view(view: &PlaybackView, lastfm_api_key: &str, link_type: LinkType) -> Option<Self> {
        if link_type == LinkType::None {
            return None;
        }
        let track = &view.transport.current.as_ref()?.track;
        let artist = track.artist.trim();
        let album = track.album.trim();
        if artist.is_empty() || album.is_empty() {
            return None;
        }
        let musicbrainz_album_id = track
            .album_artwork
            .as_ref()
            .and_then(|album| album.musicbrainz_album_id.clone());
        let musicbrainz_release_group_id = track
            .album_artwork
            .as_ref()
            .and_then(|album| album.musicbrainz_release_group_id.clone());
        let lastfm_api_key = if matches!(link_type, LinkType::LastFm | LinkType::MusicBrainzLastFm)
        {
            lastfm_api_key
        } else {
            ""
        };
        let allow_musicbrainz = matches!(
            link_type,
            LinkType::MusicBrainz | LinkType::MusicBrainzLastFm
        );
        Some(Self {
            artist: artist.to_string(),
            album: album.to_string(),
            musicbrainz_album_id,
            musicbrainz_release_group_id,
            lastfm_api_key: lastfm_api_key.to_string(),
            allow_musicbrainz,
        })
    }
}

impl Inner {
    fn complete_artwork(
        &self,
        revision: u64,
        key: &ArtworkKey,
        result: Result<Option<String>, String>,
    ) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if let Some(activity) = state.complete_artwork(revision, key, result) {
            state.publish(Some(activity));
        }
    }
}

fn unix_now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use library::{AlbumArtwork, AlbumId, ArtistCredit, ArtistId, SourceId, Track, TrackId};
    use playback::{
        ControlsView, OccurrenceId, PlaybackView, Provenance, QueueSummaryView, RepeatMode, RunId,
        SequenceEntry, TransportStatus, TransportView,
    };

    use super::*;

    #[cfg(unix)]
    #[test]
    fn delivery_block_clears_without_starting_more_artwork() {
        let (presence, requests) = Presence::new();
        let view = test_view(1, "Album One", TransportStatus::Playing, 0);
        presence.update(enabled_settings(), true, "first-key", Some(&view));
        assert!(requests.try_recv().is_some());

        presence.update(enabled_settings(), false, "first-key", Some(&view));
        let state = presence
            .inner
            .state
            .lock()
            .expect("presence state lock poisoned");
        assert!(state.activity.is_none());
        assert!(matches!(state.artwork, ArtworkState::Empty));
        assert!(requests.try_recv().is_none());
    }

    #[test]
    fn stale_artwork_cannot_replace_a_newer_album() {
        let (presence, requests) = Presence::new();
        let first_view = test_view(1, "Album One", TransportStatus::Playing, 0);
        refresh_presence(&presence, &first_view, "first-key", 100_000);
        let first = requests
            .try_recv()
            .unwrap_or_else(|| panic!("first album should request artwork"));

        let second_view = test_view(2, "Album Two", TransportStatus::Playing, 0);
        presence.observe(Some(&second_view), false);
        first.complete(Ok(Some("https://example.invalid/old.jpg".to_string())));
        let second = requests
            .try_recv()
            .unwrap_or_else(|| panic!("stale completion must not discard newer artwork"));
        assert_eq!(second.album(), "Album Two");

        second.complete(Ok(Some("https://images.example/new.jpg".to_string())));
        let state = presence
            .inner
            .state
            .lock()
            .expect("presence state lock poisoned");
        assert_eq!(
            state
                .activity
                .as_ref()
                .map(|activity| activity.large_image.as_str()),
            Some("https://images.example/new.jpg"),
        );
    }

    #[test]
    fn timeline_rebases_only_for_run_state_or_seek_changes() {
        let mut settings = enabled_settings();
        settings.show_paused = true;
        let mut view = test_view(1, "Album", TransportStatus::Playing, 5_000);
        let activity =
            discord::Activity::new(&settings, &view, 100_000, discord::APP_ICON_URL.to_string())
                .unwrap_or_else(|| panic!("playing run should publish activity"));
        assert_eq!(activity.started_at_millis, Some(95_000));
        assert_eq!(activity.ended_at_millis, Some(137_000));

        view.transport.position_millis = 6_000;
        assert!(activity.matches(&view));

        view.transport.position_millis = 20_000;
        let seeked =
            discord::Activity::new(&settings, &view, 101_000, discord::APP_ICON_URL.to_string())
                .unwrap_or_else(|| panic!("seek should rebase activity"));
        assert_eq!(seeked.started_at_millis, Some(81_000));
        assert_eq!(seeked.ended_at_millis, Some(123_000));

        view.transport.state = TransportStatus::Paused;
        let paused =
            discord::Activity::new(&settings, &view, 102_000, discord::APP_ICON_URL.to_string())
                .unwrap_or_else(|| panic!("visible pause should publish activity"));
        assert_eq!(paused.started_at_millis, None);
        assert_eq!(paused.ended_at_millis, None);

        view.transport.run = Some(RunId::new(2));
        view.transport.state = TransportStatus::Playing;
        view.transport.position_millis = 0;
        let replay =
            discord::Activity::new(&settings, &view, 103_000, discord::APP_ICON_URL.to_string())
                .unwrap_or_else(|| panic!("new run should publish a new timeline"));
        assert_eq!(replay.run, RunId::new(2));
        assert_eq!(replay.started_at_millis, Some(103_000));
    }

    #[test]
    fn only_current_track_changes_invalidate_same_run_metadata() {
        let mut view = test_view(1, "Album", TransportStatus::Playing, 0);
        let activity = discord::Activity::new(
            &enabled_settings(),
            &view,
            100_000,
            discord::APP_ICON_URL.to_string(),
        )
        .unwrap_or_else(|| panic!("playing run should publish activity"));

        view.queue.revision = view.queue.revision.wrapping_add(1);
        assert!(activity.matches(&view));

        Arc::make_mut(
            view.transport
                .current
                .as_mut()
                .unwrap_or_else(|| panic!("current entry missing")),
        )
        .track
        .title = "Corrected title".to_string();
        assert!(!activity.matches(&view));
    }

    #[test]
    fn changed_lastfm_key_requests_artwork_once() {
        let (presence, requests) = Presence::new();
        let view = test_view(1, "Album", TransportStatus::Playing, 0);
        let mut settings = enabled_settings();
        settings.link_type = LinkType::LastFm;
        presence.update(settings.clone(), true, "first-key", Some(&view));
        requests
            .try_recv()
            .unwrap_or_else(|| panic!("first key should request artwork"))
            .complete(Ok(None));

        presence.update(settings, true, "second-key", Some(&view));
        let second = requests
            .try_recv()
            .unwrap_or_else(|| panic!("changed key should request artwork"));
        assert_eq!(second.lastfm_api_key(), "second-key");
        presence.observe(Some(&view), false);
        assert!(requests.try_recv().is_none());
    }

    #[test]
    fn artwork_uses_displayed_artist_and_selected_metadata_source() {
        let (presence, requests) = Presence::new();
        let mut view = test_view(1, "Album", TransportStatus::Playing, 0);
        Arc::make_mut(
            view.transport
                .current
                .as_mut()
                .unwrap_or_else(|| panic!("current entry missing")),
        )
        .track
        .album_artwork = Some(AlbumArtwork {
            id: AlbumId::fake(1),
            title: "Album".to_string(),
            artist: "Album Artist".to_string(),
            image_ref: None,
            musicbrainz_album_id: Some("release-id".to_string()),
            musicbrainz_release_group_id: Some("release-group-id".to_string()),
        });
        refresh_presence(&presence, &view, "key", 100_000);
        let first = requests
            .try_recv()
            .unwrap_or_else(|| panic!("album should request artwork"));
        assert_eq!(first.artist(), "Artist");
        assert!(first.allow_musicbrainz());
        assert_eq!(first.lastfm_api_key(), "");
        assert_eq!(first.musicbrainz_album_id(), Some("release-id"));
        assert_eq!(
            first.musicbrainz_release_group_id(),
            Some("release-group-id")
        );
        first.complete(Ok(None));

        let mut settings = enabled_settings();
        settings.link_type = LinkType::LastFm;
        presence.update(settings, true, "key", Some(&view));
        let changed = requests
            .try_recv()
            .unwrap_or_else(|| panic!("changed metadata source should request artwork"));
        assert!(!changed.allow_musicbrainz());
        assert_eq!(changed.lastfm_api_key(), "key");
    }

    #[test]
    fn pending_artwork_delays_the_first_activity_until_completion() {
        let (presence, requests) = Presence::new();
        let view = test_view(1, "Album", TransportStatus::Playing, 0);
        refresh_presence(&presence, &view, "key", 100_000);
        {
            let state = presence
                .inner
                .state
                .lock()
                .expect("presence state lock poisoned");
            assert!(state.worker.is_none());
            assert!(matches!(state.artwork, ArtworkState::Pending { .. }));
        }

        requests
            .try_recv()
            .unwrap_or_else(|| panic!("album should request artwork"))
            .complete(Ok(Some("https://images.example/cover.jpg".to_string())));
        let state = presence
            .inner
            .state
            .lock()
            .expect("presence state lock poisoned");
        assert!(state.worker.is_some());
        assert_eq!(
            state
                .activity
                .as_ref()
                .map(|activity| activity.large_image.as_str()),
            Some("https://images.example/cover.jpg")
        );
    }

    #[test]
    fn resolving_next_track_retains_the_current_activity() {
        let (presence, requests) = Presence::new();
        let first = test_view(1, "Album One", TransportStatus::Playing, 0);
        refresh_presence(&presence, &first, "key", 100_000);
        requests
            .try_recv()
            .unwrap_or_else(|| panic!("first album should request artwork"))
            .complete(Ok(Some("https://images.example/one.jpg".to_string())));

        let mut second = test_view(2, "Album Two", TransportStatus::Resolving, 0);
        presence.observe(Some(&second), false);
        {
            let state = presence
                .inner
                .state
                .lock()
                .expect("presence state lock poisoned");
            assert_eq!(
                state.activity.as_ref().map(|activity| activity.run),
                Some(RunId::new(1))
            );
        }

        second.transport.state = TransportStatus::Buffering;
        presence.observe(Some(&second), false);
        let request = requests
            .try_recv()
            .unwrap_or_else(|| panic!("second album should request artwork"));
        request.complete(Ok(Some("https://images.example/two.jpg".to_string())));
        let state = presence
            .inner
            .state
            .lock()
            .expect("presence state lock poisoned");
        assert_eq!(
            state.activity.as_ref().map(|activity| activity.run),
            Some(RunId::new(2))
        );
    }

    #[test]
    fn newer_resolving_track_cancels_a_staged_replacement() {
        let (presence, requests) = Presence::new();
        let first = test_view(1, "Album One", TransportStatus::Playing, 0);
        refresh_presence(&presence, &first, "key", 100_000);
        requests
            .try_recv()
            .unwrap_or_else(|| panic!("first album should request artwork"))
            .complete(Ok(Some("https://images.example/one.jpg".to_string())));

        let second = test_view(2, "Album Two", TransportStatus::Buffering, 0);
        presence.observe(Some(&second), false);
        let second_request = requests
            .try_recv()
            .unwrap_or_else(|| panic!("second album should request artwork"));

        let mut third = test_view(3, "Album Three", TransportStatus::Resolving, 0);
        presence.observe(Some(&third), false);
        second_request.complete(Ok(Some("https://images.example/two.jpg".to_string())));
        {
            let state = presence
                .inner
                .state
                .lock()
                .expect("presence state lock poisoned");
            assert!(matches!(state.artwork, ArtworkState::Empty));
            assert_ne!(
                state
                    .activity
                    .as_ref()
                    .map(|activity| activity.large_image.as_str()),
                Some("https://images.example/two.jpg")
            );
        }

        third.transport.state = TransportStatus::Buffering;
        presence.observe(Some(&third), false);
        requests
            .try_recv()
            .unwrap_or_else(|| panic!("third album should request artwork"))
            .complete(Ok(Some("https://images.example/three.jpg".to_string())));
        let state = presence
            .inner
            .state
            .lock()
            .expect("presence state lock poisoned");
        assert_eq!(
            state.activity.as_ref().map(|activity| activity.run),
            Some(RunId::new(3))
        );
    }

    #[test]
    fn disabled_metadata_lookup_publishes_the_app_icon_immediately() {
        let (presence, requests) = Presence::new();
        let view = test_view(1, "Album", TransportStatus::Playing, 0);
        let mut settings = enabled_settings();
        settings.link_type = LinkType::None;
        presence.update(settings, true, "key", Some(&view));

        assert!(requests.try_recv().is_none());
        let state = presence
            .inner
            .state
            .lock()
            .expect("presence state lock poisoned");
        assert!(state.worker.is_some());
        assert_eq!(
            state
                .activity
                .as_ref()
                .map(|activity| activity.large_image.as_str()),
            Some(discord::APP_ICON_URL)
        );
    }

    #[test]
    fn failed_artwork_uses_and_caches_the_app_icon() {
        let (presence, requests) = Presence::new();
        let view = test_view(1, "Album", TransportStatus::Playing, 0);
        refresh_presence(&presence, &view, "first-key", 100_000);
        let request = requests
            .try_recv()
            .unwrap_or_else(|| panic!("album should request artwork"));
        let key = request.key.clone();
        request.complete(Err("offline".to_string()));
        let state = presence
            .inner
            .state
            .lock()
            .expect("presence state lock poisoned");
        assert!(state.worker.is_some());
        assert!(matches!(
            &state.artwork,
            ArtworkState::Ready { key: ready, url: None } if ready == &key
        ));
        assert_eq!(
            state
                .activity
                .as_ref()
                .map(|activity| activity.large_image.as_str()),
            Some(discord::APP_ICON_URL),
        );
        assert!(state.matches(Some(&view)));
        drop(state);

        presence.observe(Some(&view), false);
        assert!(requests.try_recv().is_none());
    }

    fn enabled_settings() -> Settings {
        Settings {
            enabled: true,
            ..Settings::default()
        }
    }

    fn refresh_presence(
        presence: &Presence,
        view: &PlaybackView,
        lastfm_api_key: &str,
        now_millis: u64,
    ) {
        let mut state = presence
            .inner
            .state
            .lock()
            .expect("presence state lock poisoned");
        state.settings = enabled_settings();
        state.lastfm_api_key = lastfm_api_key.to_string();
        presence.refresh(&mut state, Some(view), now_millis);
    }

    pub(crate) fn test_view(
        run: u64,
        album: &str,
        state: TransportStatus,
        position_millis: u64,
    ) -> PlaybackView {
        let occurrence = OccurrenceId::new(format!("presence:{run}"));
        let entry = Arc::new(SequenceEntry {
            occurrence: occurrence.clone(),
            provenance: Provenance::Manual,
            track: Track {
                id: TrackId::fake(run),
                album_id: AlbumId::fake(run),
                title: "Track".to_string(),
                artist: "Artist".to_string(),
                artist_id: Some(ArtistId::fake(1)),
                artist_credits: vec![ArtistCredit {
                    id: ArtistId::fake(1),
                    name: "Artist".to_string(),
                    musicbrainz_artist_id: Some("artist-id".to_string()),
                }],
                album_artist_credits: vec![ArtistCredit {
                    id: ArtistId::fake(2),
                    name: "Album Artist".to_string(),
                    musicbrainz_artist_id: None,
                }],
                album: album.to_string(),
                year: 2026,
                release_date: None,
                date_added: None,
                last_played: None,
                play_count: None,
                user_rating: None,
                duration_seconds: 42,
                favorite: false,
                disc_number: 1,
                track_number: 1,
                image_ref: None,
                album_artwork: None,
                genres: Vec::new(),
                musicbrainz_recording_id: Some("recording-id".to_string()),
                musicbrainz_release_track_id: Some("track-id".to_string()),
                local_path: None,
                source_format: None,
                comment: None,
                skip_count: None,
                bpm: None,
                moods: Vec::new(),
            },
        });
        PlaybackView {
            queue: QueueSummaryView {
                revision: run,
                total: 1,
                current_occurrence: Some(occurrence),
                current_index: Some(0),
                next_occurrence: None,
            },
            transport: TransportView {
                source_id: SourceId::fake(1),
                run: Some(RunId::new(run)),
                current: Some(entry),
                state,
                position_millis,
                duration_millis: 42_500,
                buffering_percent: None,
                error: None,
            },
            controls: ControlsView {
                repeat_mode: RepeatMode::Off,
                shuffle_enabled: false,
                auto_dj_enabled: false,
                volume: 1.0,
                muted: false,
                audio_output: None,
            },
        }
    }
}
