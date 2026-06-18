use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use domain::{
    AppSettings, AudioscrobblerScrobbleSettings, ListenBrainzScrobbleSettings, QueueEntry,
    ScrobblingSettings, TrackId,
};
use reqwest::{blocking::Client, header::AUTHORIZATION};
use serde_json::{Value, json};
use source::PlaybackReportKind;
use tracing::{debug, warn};

use crate::{controller::PlaybackSnapshot, external_activity};

const LASTFM_API_URL: &str = "https://ws.audioscrobbler.com/2.0/";
const LIBREFM_API_URL: &str = "https://libre.fm/2.0/";
const LISTENBRAINZ_API_URL: &str = "https://api.listenbrainz.org/1/submit-listens";
const LASTFM_AUTH_URL: &str = "https://www.last.fm/api/auth/";
const LIBREFM_AUTH_URL: &str = "https://libre.fm/api/auth/";
const LIBREFM_API_KEY: &str = "rufin";
const LIBREFM_API_SECRET: &str = "rufin";
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AudioscrobblerSession {
    pub(crate) username: String,
    pub(crate) session_key: String,
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

pub(crate) fn request_lastfm_auth_token(api_key: &str, api_secret: &str) -> Result<String, String> {
    let api_key = api_key.trim();
    let api_secret = api_secret.trim();
    if api_key.is_empty() || api_secret.is_empty() {
        return Err("Enter a Last.fm API key and shared secret first.".to_string());
    }

    request_audioscrobbler_auth_token(AudioscrobblerService::LastFm, api_key, api_secret)
}

pub(crate) fn request_librefm_auth_token() -> Result<String, String> {
    request_audioscrobbler_auth_token(
        AudioscrobblerService::LibreFm,
        LIBREFM_API_KEY,
        LIBREFM_API_SECRET,
    )
}

fn request_audioscrobbler_auth_token(
    service: AudioscrobblerService,
    api_key: &str,
    api_secret: &str,
) -> Result<String, String> {
    let params = vec![
        ("api_key".to_string(), api_key.to_string()),
        ("method".to_string(), "auth.getToken".to_string()),
    ];
    let mut form = params.clone();
    form.push(("api_sig".to_string(), api_signature(&params, api_secret)));
    form.push(("format".to_string(), "json".to_string()));

    let value = post_audioscrobbler_form(service.api_url(), form)?;
    audioscrobbler_error(&value, service.name())?;
    value
        .get("token")
        .and_then(Value::as_str)
        .filter(|token| !token.trim().is_empty())
        .map(|token| token.trim().to_string())
        .ok_or_else(|| format!("{} did not return an auth token.", service.name()))
}

pub(crate) fn lastfm_auth_url(api_key: &str, token: &str) -> String {
    format!(
        "{LASTFM_AUTH_URL}?api_key={}&token={}",
        api_key.trim(),
        token.trim()
    )
}

pub(crate) fn librefm_auth_url(token: &str) -> String {
    format!(
        "{LIBREFM_AUTH_URL}?api_key={}&token={}",
        LIBREFM_API_KEY,
        token.trim()
    )
}

pub(crate) fn request_lastfm_session(
    api_key: &str,
    api_secret: &str,
    token: &str,
) -> Result<Option<AudioscrobblerSession>, String> {
    let api_key = api_key.trim();
    let api_secret = api_secret.trim();
    let token = token.trim();
    if api_key.is_empty() || api_secret.is_empty() || token.is_empty() {
        return Err("Last.fm authorization is missing required fields.".to_string());
    }

    request_audioscrobbler_session(AudioscrobblerService::LastFm, api_key, api_secret, token)
}

pub(crate) fn request_librefm_session(
    token: &str,
) -> Result<Option<AudioscrobblerSession>, String> {
    let token = token.trim();
    if token.is_empty() {
        return Err("Libre.fm authorization is missing a token.".to_string());
    }

    request_audioscrobbler_session(
        AudioscrobblerService::LibreFm,
        LIBREFM_API_KEY,
        LIBREFM_API_SECRET,
        token,
    )
}

fn request_audioscrobbler_session(
    service: AudioscrobblerService,
    api_key: &str,
    api_secret: &str,
    token: &str,
) -> Result<Option<AudioscrobblerSession>, String> {
    let params = vec![
        ("api_key".to_string(), api_key.to_string()),
        ("method".to_string(), "auth.getSession".to_string()),
        ("token".to_string(), token.to_string()),
    ];
    let mut form = params.clone();
    form.push(("api_sig".to_string(), api_signature(&params, api_secret)));
    form.push(("format".to_string(), "json".to_string()));

    let value = post_audioscrobbler_form(service.api_url(), form)?;
    if let Some((code, message)) = audioscrobbler_error_value(&value) {
        if code == Some(14) {
            return Ok(None);
        }
        return Err(format!(
            "{} API error {}: {message}",
            service.name(),
            code.unwrap_or(0)
        ));
    }
    audioscrobbler_session_from_value(&value, service.name()).map(Some)
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
    if failed
        || !external_activity::playback_reporting(settings)
        || !has_configured_target(&settings.scrobbling, false)
    {
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

fn post_audioscrobbler_form(api_url: &str, form: Vec<(String, String)>) -> Result<Value, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .post(api_url)
        .form(&form)
        .send()
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let value = response.json::<Value>().unwrap_or(Value::Null);
    if !status.is_success() && audioscrobbler_error_value(&value).is_none() {
        return Err(format!("Audioscrobbler auth returned HTTP {status}"));
    }
    Ok(value)
}

fn audioscrobbler_error(value: &Value, service: &str) -> Result<(), String> {
    if let Some((code, message)) = audioscrobbler_error_value(value) {
        return Err(format!(
            "{service} API error {}: {message}",
            code.unwrap_or(0)
        ));
    }
    Ok(())
}

fn audioscrobbler_error_value(value: &Value) -> Option<(Option<i64>, String)> {
    let code = value.get("error").and_then(Value::as_i64);
    code.map(|code| {
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_string();
        (Some(code), message)
    })
}

fn audioscrobbler_session_from_value(
    value: &Value,
    service: &str,
) -> Result<AudioscrobblerSession, String> {
    let session = value
        .get("session")
        .ok_or_else(|| format!("{service} did not return a session."))?;
    let username = session
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let session_key = session
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if session_key.is_empty() {
        return Err(format!("{service} did not return a session key."));
    }
    Ok(AudioscrobblerSession {
        username,
        session_key,
    })
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
        .header(
            AUTHORIZATION,
            listenbrainz_authorization_header(settings.user_token.trim()),
        )
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

fn listenbrainz_authorization_header(user_token: &str) -> String {
    format!("Token {}", user_token.trim())
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
        AudioscrobblerSession, ExternalScrobbleState, ScrobbleAction, ScrobbleTrack, api_signature,
        audioscrobbler_session_from_value, lastfm_auth_url, librefm_auth_url,
        listenbrainz_authorization_header, position_reaches_scrobble_threshold,
        scrobble_threshold_seconds,
    };
    use crate::controller::PlaybackSnapshot;
    use domain::{QueueEntry, QueueEntryId, TrackId};
    use playback::PlaybackState;
    use serde_json::json;
    use source::PlaybackReportKind;

    #[test]
    fn external_follow_rules() {
        assert_eq!(scrobble_threshold_seconds(30), None);
        assert_eq!(scrobble_threshold_seconds(31), Some(15));
        assert_eq!(scrobble_threshold_seconds(180), Some(90));
        assert_eq!(scrobble_threshold_seconds(900), Some(240));
        assert!(!position_reaches_scrobble_threshold(180, 89));
        assert!(position_reaches_scrobble_threshold(180, 90));
    }

    #[test]
    fn external_track_per() {
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
    fn external_exclude_format() {
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

    #[test]
    fn lastfm_auth_case() {
        assert_eq!(
            lastfm_auth_url("api-key", "auth-token"),
            "https://www.last.fm/api/auth/?api_key=api-key&token=auth-token"
        );
    }

    #[test]
    fn librefm_auth_case() {
        assert_eq!(
            librefm_auth_url("auth-token"),
            "https://libre.fm/api/auth/?api_key=rufin&token=auth-token"
        );
    }

    #[test]
    fn external_audioscrobbler_session() {
        let value = json!({
            "session": {
                "name": "listener",
                "key": "session-key",
            },
        });

        assert_eq!(
            audioscrobbler_session_from_value(&value, "Last.fm").expect("audioscrobbler session"),
            AudioscrobblerSession {
                username: "listener".to_string(),
                session_key: "session-key".to_string(),
            }
        );
    }

    #[test]
    fn external_audioscrobbler_error() {
        let value = json!({
            "error": 14,
            "message": "Unauthorized Token",
        });

        assert_eq!(
            super::audioscrobbler_error_value(&value),
            Some((Some(14), "Unauthorized Token".to_string()))
        );
    }

    #[test]
    fn external_use_scheme() {
        assert_eq!(
            listenbrainz_authorization_header(" user-token "),
            "Token user-token"
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
            local_path: None,
            source_format: None,
            origin: None,
        }
    }
}
