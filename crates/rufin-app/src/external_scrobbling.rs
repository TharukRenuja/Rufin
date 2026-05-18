use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use rufin_core::{
    AppSettings, AudioscrobblerScrobbleSettings, ListenBrainzScrobbleSettings, QueueEntry,
    ScrobblingSettings, TrackId,
};
use rufin_provider::PlaybackReportKind;
use serde_json::{Value, json};
use tracing::{debug, warn};

use crate::controller::PlaybackSnapshot;

const LASTFM_API_URL: &str = "https://ws.audioscrobbler.com/2.0/";
const LIBREFM_API_URL: &str = "https://libre.fm/2.0/";
const LISTENBRAINZ_API_URL: &str = "https://api.listenbrainz.org/1/submit-listens";
const USER_AGENT: &str = "Rufin/0.1";
const MIN_SCROBBLE_DURATION_SECONDS: u32 = 30;
const MAX_SCROBBLE_THRESHOLD_SECONDS: u32 = 4 * 60;

#[derive(Default)]
pub(crate) struct ExternalScrobbleState {
    current: Option<ActiveScrobble>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ActiveScrobble {
    track_id: TrackId,
    started_at_unix_seconds: u64,
    scrobbled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ScrobbleTrack {
    title: String,
    artist: String,
    album: String,
    duration_seconds: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ScrobbleAction {
    NowPlaying(ScrobbleTrack),
    Scrobble {
        track: ScrobbleTrack,
        started_at_unix_seconds: u64,
    },
}

#[derive(Clone, Copy, Debug)]
enum AudioscrobblerService {
    LastFm,
    LibreFm,
}

impl AudioscrobblerService {
    fn name(self) -> &'static str {
        match self {
            Self::LastFm => "Last.fm",
            Self::LibreFm => "Libre.fm",
        }
    }

    fn api_url(self) -> &'static str {
        match self {
            Self::LastFm => LASTFM_API_URL,
            Self::LibreFm => LIBREFM_API_URL,
        }
    }
}

impl ExternalScrobbleState {
    fn observe(
        &mut self,
        kind: PlaybackReportKind,
        snapshot: &PlaybackSnapshot,
        entry: &QueueEntry,
    ) -> Option<ScrobbleAction> {
        let started_at = unix_now_seconds().saturating_sub(u64::from(snapshot.position_seconds));
        if self
            .current
            .as_ref()
            .is_none_or(|active| active.track_id != entry.track_id)
        {
            self.current = Some(ActiveScrobble {
                track_id: entry.track_id.clone(),
                started_at_unix_seconds: started_at,
                scrobbled: false,
            });
        }

        let track = scrobble_track(entry)?;
        if kind == PlaybackReportKind::Started {
            return Some(ScrobbleAction::NowPlaying(track));
        }

        let active = self.current.as_mut()?;
        if active.scrobbled
            || !position_reaches_scrobble_threshold(
                track.duration_seconds,
                snapshot.position_seconds,
            )
        {
            return None;
        }

        active.scrobbled = true;
        Some(ScrobbleAction::Scrobble {
            track,
            started_at_unix_seconds: active.started_at_unix_seconds,
        })
    }
}

pub(crate) fn report(
    settings: &AppSettings,
    state: &Arc<Mutex<ExternalScrobbleState>>,
    kind: PlaybackReportKind,
    failed: bool,
    snapshot: &PlaybackSnapshot,
    entry: &QueueEntry,
) {
    if failed || settings.private_mode || !has_configured_target(&settings.scrobbling, false) {
        return;
    }

    let action = state
        .lock()
        .map(|mut state| state.observe(kind, snapshot, entry))
        .unwrap_or(None);
    let Some(action) = action else {
        return;
    };
    if matches!(action, ScrobbleAction::NowPlaying(_))
        && !has_configured_target(&settings.scrobbling, true)
    {
        return;
    }

    let scrobbling = settings.scrobbling.clone();
    thread::spawn(move || submit_action(scrobbling, action));
}

fn submit_action(settings: ScrobblingSettings, action: ScrobbleAction) {
    let client = Client::builder()
        .timeout(Duration::from_secs(6))
        .user_agent(USER_AGENT)
        .build()
        .unwrap_or_else(|error| {
            warn!(%error, "failed to build scrobbling HTTP client");
            Client::new()
        });

    if let Some(error) = submit_audioscrobbler(
        &client,
        AudioscrobblerService::LastFm,
        &settings.lastfm,
        &action,
    )
    .err()
    {
        warn!(%error, "Last.fm scrobbling failed");
    }
    if let Some(error) = submit_audioscrobbler(
        &client,
        AudioscrobblerService::LibreFm,
        &settings.librefm,
        &action,
    )
    .err()
    {
        warn!(%error, "Libre.fm scrobbling failed");
    }
    if let Some(error) = submit_listenbrainz(&client, &settings.listenbrainz, &action).err() {
        warn!(%error, "ListenBrainz scrobbling failed");
    }
}

fn has_configured_target(settings: &ScrobblingSettings, now_playing: bool) -> bool {
    audioscrobbler_configured(&settings.lastfm, now_playing)
        || audioscrobbler_configured(&settings.librefm, now_playing)
        || listenbrainz_configured(&settings.listenbrainz, now_playing)
}

fn audioscrobbler_configured(settings: &AudioscrobblerScrobbleSettings, now_playing: bool) -> bool {
    settings.enabled
        && (!now_playing || settings.now_playing_enabled)
        && !settings.api_key.trim().is_empty()
        && !settings.api_secret.trim().is_empty()
        && !settings.session_key.trim().is_empty()
}

fn listenbrainz_configured(settings: &ListenBrainzScrobbleSettings, now_playing: bool) -> bool {
    settings.enabled
        && (!now_playing || settings.now_playing_enabled)
        && !settings.user_token.trim().is_empty()
}

fn submit_audioscrobbler(
    client: &Client,
    service: AudioscrobblerService,
    settings: &AudioscrobblerScrobbleSettings,
    action: &ScrobbleAction,
) -> Result<(), String> {
    if !audioscrobbler_configured(settings, matches!(action, ScrobbleAction::NowPlaying(_))) {
        return Ok(());
    }

    let params = match action {
        ScrobbleAction::NowPlaying(track) => {
            audioscrobbler_params("track.updateNowPlaying", settings, track, None)
        }
        ScrobbleAction::Scrobble {
            track,
            started_at_unix_seconds,
        } => audioscrobbler_params(
            "track.scrobble",
            settings,
            track,
            Some(*started_at_unix_seconds),
        ),
    };

    let response = client
        .post(service.api_url())
        .form(&params)
        .send()
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let value = response.json::<Value>().unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(format!("{} returned HTTP {status}", service.name()));
    }
    if let Some(error) = value.get("error").and_then(Value::as_i64) {
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(format!("{} API error {error}: {message}", service.name()));
    }
    debug!(
        service = service.name(),
        "submitted external scrobbling event"
    );
    Ok(())
}

fn audioscrobbler_params(
    method: &str,
    settings: &AudioscrobblerScrobbleSettings,
    track: &ScrobbleTrack,
    started_at_unix_seconds: Option<u64>,
) -> Vec<(String, String)> {
    let mut params = vec![
        ("api_key".to_string(), settings.api_key.trim().to_string()),
        ("artist".to_string(), track.artist.clone()),
        ("duration".to_string(), track.duration_seconds.to_string()),
        ("method".to_string(), method.to_string()),
        ("sk".to_string(), settings.session_key.trim().to_string()),
        ("track".to_string(), track.title.clone()),
    ];
    if !track.album.trim().is_empty() {
        params.push(("album".to_string(), track.album.clone()));
    }
    if let Some(started_at_unix_seconds) = started_at_unix_seconds {
        params.push(("timestamp".to_string(), started_at_unix_seconds.to_string()));
    }
    let signature = api_signature(&params, settings.api_secret.trim());
    params.push(("api_sig".to_string(), signature));
    params.push(("format".to_string(), "json".to_string()));
    params
}

fn submit_listenbrainz(
    client: &Client,
    settings: &ListenBrainzScrobbleSettings,
    action: &ScrobbleAction,
) -> Result<(), String> {
    if !listenbrainz_configured(settings, matches!(action, ScrobbleAction::NowPlaying(_))) {
        return Ok(());
    }

    let (listen_type, listened_at, track) = match action {
        ScrobbleAction::NowPlaying(track) => ("playing_now", None, track),
        ScrobbleAction::Scrobble {
            track,
            started_at_unix_seconds,
        } => ("single", Some(*started_at_unix_seconds), track),
    };
    let mut listen = json!({
        "track_metadata": {
            "artist_name": track.artist.as_str(),
            "track_name": track.title.as_str(),
            "additional_info": {
                "duration_ms": u64::from(track.duration_seconds) * 1_000,
                "submission_client": "Rufin",
            },
        },
    });
    if !track.album.trim().is_empty() {
        listen["track_metadata"]["release_name"] = json!(track.album.as_str());
    }
    if let Some(listened_at) = listened_at {
        listen["listened_at"] = json!(listened_at);
    }
    let payload = json!({
        "listen_type": listen_type,
        "payload": [listen],
    });
    let response = client
        .post(LISTENBRAINZ_API_URL)
        .bearer_auth(settings.user_token.trim())
        .json(&payload)
        .send()
        .map_err(|error| error.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("ListenBrainz returned HTTP {status}"));
    }
    debug!("submitted ListenBrainz scrobbling event");
    Ok(())
}

fn scrobble_track(entry: &QueueEntry) -> Option<ScrobbleTrack> {
    let title = entry.title.trim();
    let artist = entry.artist.trim();
    if title.is_empty() || artist.is_empty() {
        return None;
    }
    Some(ScrobbleTrack {
        title: title.to_string(),
        artist: artist.to_string(),
        album: entry.album.trim().to_string(),
        duration_seconds: entry.duration_seconds,
    })
}

fn position_reaches_scrobble_threshold(duration_seconds: u32, position_seconds: u32) -> bool {
    scrobble_threshold_seconds(duration_seconds)
        .is_some_and(|threshold| position_seconds >= threshold)
}

fn scrobble_threshold_seconds(duration_seconds: u32) -> Option<u32> {
    if duration_seconds <= MIN_SCROBBLE_DURATION_SECONDS {
        return None;
    }
    Some((duration_seconds / 2).min(MAX_SCROBBLE_THRESHOLD_SECONDS))
}

fn api_signature(params: &[(String, String)], secret: &str) -> String {
    let mut params = params
        .iter()
        .filter(|(key, _)| key != "format" && key != "callback" && key != "api_sig")
        .collect::<Vec<_>>();
    params.sort_by(|left, right| left.0.cmp(&right.0));
    let mut input = String::new();
    for (key, value) in params {
        input.push_str(key);
        input.push_str(value);
    }
    input.push_str(secret);
    format!("{:x}", md5::compute(input))
}

fn unix_now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        ExternalScrobbleState, ScrobbleAction, ScrobbleTrack, api_signature,
        position_reaches_scrobble_threshold, scrobble_threshold_seconds,
    };
    use crate::controller::PlaybackSnapshot;
    use rufin_core::{QueueEntry, QueueEntryId, TrackId};
    use rufin_playback::PlaybackState;
    use rufin_provider::PlaybackReportKind;

    #[test]
    fn scrobble_threshold_follows_common_service_rules() {
        assert_eq!(scrobble_threshold_seconds(30), None);
        assert_eq!(scrobble_threshold_seconds(31), Some(15));
        assert_eq!(scrobble_threshold_seconds(180), Some(90));
        assert_eq!(scrobble_threshold_seconds(900), Some(240));
        assert!(!position_reaches_scrobble_threshold(180, 89));
        assert!(position_reaches_scrobble_threshold(180, 90));
    }

    #[test]
    fn state_emits_now_playing_then_one_scrobble_per_track() {
        let mut state = ExternalScrobbleState::default();
        let entry = queue_entry("track-one");
        let mut snapshot = PlaybackSnapshot {
            current: Some(entry.clone()),
            state: PlaybackState::Playing,
            duration_seconds: 180,
            ..PlaybackSnapshot::default()
        };

        assert_eq!(
            state.observe(PlaybackReportKind::Started, &snapshot, &entry),
            Some(ScrobbleAction::NowPlaying(ScrobbleTrack {
                title: "Track One".to_string(),
                artist: "Artist".to_string(),
                album: "Album".to_string(),
                duration_seconds: 180,
            }))
        );

        snapshot.position_seconds = 89;
        assert_eq!(
            state.observe(PlaybackReportKind::Progress, &snapshot, &entry),
            None
        );

        snapshot.position_seconds = 90;
        assert!(matches!(
            state.observe(PlaybackReportKind::Progress, &snapshot, &entry),
            Some(ScrobbleAction::Scrobble { .. })
        ));
        snapshot.position_seconds = 120;
        assert_eq!(
            state.observe(PlaybackReportKind::Progress, &snapshot, &entry),
            None
        );
    }

    #[test]
    fn api_signature_sorts_params_and_excludes_format() {
        let params = vec![
            ("track".to_string(), "Track".to_string()),
            ("format".to_string(), "json".to_string()),
            ("method".to_string(), "track.scrobble".to_string()),
            ("artist".to_string(), "Artist".to_string()),
            ("api_key".to_string(), "key".to_string()),
            ("timestamp".to_string(), "10".to_string()),
            ("sk".to_string(), "session".to_string()),
        ];

        assert_eq!(
            api_signature(&params, "secret"),
            "9ea42092c79325d9e328dcc8c3fa4eeb"
        );
    }

    fn queue_entry(id: &str) -> QueueEntry {
        QueueEntry {
            id: QueueEntryId::new("entry-one"),
            track_id: TrackId::new(id),
            album_id: None,
            title: "Track One".to_string(),
            artist: "Artist".to_string(),
            artist_id: None,
            album: "Album".to_string(),
            year: 2026,
            duration_seconds: 180,
            favorite: false,
            image_ref: None,
        }
    }
}
