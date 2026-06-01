use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::{
    env,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
};

use reqwest::Url;
use reqwest::blocking::Client;
use rufin_core::{AppSettings, DiscordDisplayType, DiscordLinkType};
use rufin_playback::PlaybackState;
use serde_json::{Value, json};
use tracing::{debug, warn};

use crate::controller::PlaybackSnapshot;

use super::Shell;

const DEFAULT_LARGE_IMAGE_URL: &str = "https://raw.githubusercontent.com/screwys/Rufin/main/data/icons/hicolor/scalable/apps/io.github.screwys.Rufin.svg";
const LASTFM_ALBUM_INFO_URL: &str = "https://ws.audioscrobbler.com/2.0/";
const MUSICBRAINZ_RELEASE_SEARCH_URL: &str = "https://musicbrainz.org/ws/2/release/";
const MUSICBRAINZ_COVER_ART_URL: &str = "https://coverartarchive.org/release";
const MUSICBRAINZ_USER_AGENT: &str = "Rufin/0.1";
const MAX_DISCORD_TEXT_LENGTH: usize = 127;
const MAX_DISCORD_URL_LENGTH: usize = 256;

#[cfg(unix)]
const IPC_VERSION: u8 = 1;
#[cfg(unix)]
const OP_HANDSHAKE: u32 = 0;
#[cfg(unix)]
const OP_FRAME: u32 = 1;
#[cfg(unix)]
const OP_CLOSE: u32 = 2;
#[cfg(unix)]
const OP_PING: u32 = 3;
#[cfg(unix)]
const OP_PONG: u32 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PresencePlaybackState {
    Playing,
    Paused,
}

impl PresencePlaybackState {
    fn label(self) -> &'static str {
        match self {
            Self::Playing => "Playing",
            Self::Paused => "Paused",
        }
    }

    fn image_key(self) -> &'static str {
        match self {
            Self::Playing => "playing",
            Self::Paused => "paused",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PresenceKey {
    track_id: String,
    title: String,
    artist: String,
    album: String,
    duration_millis: u64,
    playback_state: PresencePlaybackState,
    display_type: DiscordDisplayType,
    link_type: DiscordLinkType,
    show_as_listening: bool,
    show_state_icon: bool,
    lastfm_api_key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PresenceActivity {
    key: PresenceKey,
    started_at_millis: Option<u64>,
    ended_at_millis: Option<u64>,
    timeline_millis: Option<u64>,
}

enum PresenceCommand {
    Set(PresenceActivity),
    Clear,
}

pub(super) struct DiscordPresence {
    sender: Option<Sender<PresenceCommand>>,
    client_id: Option<String>,
    last_key: Option<PresenceKey>,
    last_timeline_millis: Option<u64>,
    missing_client_id_logged: bool,
}

impl DiscordPresence {
    pub(super) fn new() -> Self {
        Self {
            sender: None,
            client_id: None,
            last_key: None,
            last_timeline_millis: None,
            missing_client_id_logged: false,
        }
    }

    pub(super) fn update(&mut self, settings: &AppSettings, snapshot: &PlaybackSnapshot) {
        if !settings.discord_presence_enabled || settings.private_mode {
            self.clear();
            return;
        }

        let Some(client_id) = discord_client_id(settings) else {
            if !self.missing_client_id_logged {
                warn!("Discord presence is enabled but no client ID is set");
                self.missing_client_id_logged = true;
            }
            self.clear();
            return;
        };
        self.missing_client_id_logged = false;

        let Some(key) = presence_key(settings, snapshot) else {
            self.clear();
            return;
        };
        let activity = presence_activity(key.clone(), snapshot);
        if self.last_key.as_ref() == Some(&key)
            && !timeline_changed(self.last_timeline_millis, activity.timeline_millis)
        {
            return;
        }

        let Some(sender) = self.sender(client_id) else {
            return;
        };
        let timeline_millis = activity.timeline_millis;
        if sender.send(PresenceCommand::Set(activity)).is_ok() {
            self.last_key = Some(key);
            self.last_timeline_millis = timeline_millis;
        }
    }

    fn clear(&mut self) {
        self.last_timeline_millis = None;
        if self.last_key.take().is_some()
            && let Some(sender) = &self.sender
        {
            let _sent = sender.send(PresenceCommand::Clear);
        }
    }

    fn sender(&mut self, client_id: String) -> Option<Sender<PresenceCommand>> {
        if self.client_id.as_deref() != Some(client_id.as_str()) {
            self.clear();
            self.sender = None;
            self.client_id = Some(client_id);
            self.last_key = None;
            self.last_timeline_millis = None;
        }

        if let Some(sender) = &self.sender {
            return Some(sender.clone());
        }

        let client_id = self.client_id.clone()?;
        let (sender, receiver) = channel();
        thread::spawn(move || run_discord_worker(client_id, receiver));
        self.sender = Some(sender.clone());
        Some(sender)
    }
}

impl Shell {
    pub(super) fn update_discord_presence(&self, snapshot: &PlaybackSnapshot) {
        self.state
            .discord_presence
            .borrow_mut()
            .update(&self.state.settings.borrow(), snapshot);
    }
}

fn discord_client_id(settings: &AppSettings) -> Option<String> {
    let client_id = settings.discord_client_id.trim();
    if client_id.is_empty() {
        None
    } else {
        Some(client_id.to_string())
    }
}

fn presence_key(settings: &AppSettings, snapshot: &PlaybackSnapshot) -> Option<PresenceKey> {
    let playback_state = match snapshot.state {
        PlaybackState::Playing | PlaybackState::Buffering => PresencePlaybackState::Playing,
        PlaybackState::Paused if settings.discord_show_paused => PresencePlaybackState::Paused,
        PlaybackState::Paused | PlaybackState::Stopped => return None,
    };
    let entry = snapshot.current.as_ref()?;
    Some(PresenceKey {
        track_id: entry.track_id.as_str().to_string(),
        title: entry.title.clone(),
        artist: entry.artist.clone(),
        album: entry.album.clone(),
        duration_millis: u64::from(entry.duration_seconds) * 1_000,
        playback_state,
        display_type: settings.discord_display_type,
        link_type: settings.discord_link_type,
        show_as_listening: settings.discord_show_as_listening,
        show_state_icon: settings.discord_show_state_icon,
        lastfm_api_key: settings.lastfm_api_key.trim().to_string(),
    })
}

fn presence_activity(key: PresenceKey, snapshot: &PlaybackSnapshot) -> PresenceActivity {
    let started_at_millis = match key.playback_state {
        PresencePlaybackState::Playing => Some(playback_started_at_millis(snapshot)),
        PresencePlaybackState::Paused => None,
    };
    let ended_at_millis = started_at_millis.and_then(|started| {
        if key.duration_millis == 0 {
            None
        } else {
            Some(started.saturating_add(key.duration_millis))
        }
    });
    let timeline_millis = started_at_millis.or_else(|| {
        if key.playback_state == PresencePlaybackState::Paused {
            Some(snapshot.position_millis)
        } else {
            None
        }
    });
    PresenceActivity {
        key,
        started_at_millis,
        ended_at_millis,
        timeline_millis,
    }
}

fn timeline_changed(previous: Option<u64>, current: Option<u64>) -> bool {
    match (previous, current) {
        (Some(previous), Some(current)) => previous.abs_diff(current) > 1_200,
        (previous, current) => previous != current,
    }
}

fn playback_started_at_millis(snapshot: &PlaybackSnapshot) -> u64 {
    unix_now_millis().saturating_sub(snapshot.position_millis)
}

fn unix_now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

#[cfg(unix)]
fn run_discord_worker(client_id: String, receiver: std::sync::mpsc::Receiver<PresenceCommand>) {
    let mut worker = IpcWorker::new(client_id);
    while let Ok(command) = receiver.recv() {
        match command {
            PresenceCommand::Set(activity) => worker.set_activity(&activity),
            PresenceCommand::Clear => worker.clear_activity(),
        }
    }
}

#[cfg(not(unix))]
fn run_discord_worker(_client_id: String, receiver: std::sync::mpsc::Receiver<PresenceCommand>) {
    for _command in receiver {
        debug!("Discord presence is not supported on this platform");
    }
}

#[cfg(unix)]
struct IpcWorker {
    client_id: String,
    http: Client,
    stream: Option<UnixStream>,
    nonce: u64,
}

#[cfg(unix)]
impl IpcWorker {
    fn new(client_id: String) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(4))
            .user_agent(MUSICBRAINZ_USER_AGENT)
            .build()
            .unwrap_or_else(|error| {
                warn!(%error, "failed to build Discord cover lookup client");
                Client::new()
            });
        Self {
            client_id,
            http,
            stream: None,
            nonce: 0,
        }
    }

    fn set_activity(&mut self, activity: &PresenceActivity) {
        let large_image = self
            .cover_image_url(&activity.key)
            .unwrap_or_else(|| DEFAULT_LARGE_IMAGE_URL.to_string());
        let payload = self.activity_payload(Some(activity), Some(large_image));
        self.send_frame(payload);
    }

    fn clear_activity(&mut self) {
        let payload = self.activity_payload(None, None);
        self.send_frame(payload);
    }

    fn cover_image_url(&self, key: &PresenceKey) -> Option<String> {
        lastfm_cover_url(&self.http, key).or_else(|| {
            if matches!(
                key.link_type,
                DiscordLinkType::MusicBrainz | DiscordLinkType::MusicBrainzLastFm
            ) {
                musicbrainz_cover_url(&self.http, &key.artist, &key.album)
            } else {
                None
            }
        })
    }

    fn activity_payload(
        &mut self,
        activity: Option<&PresenceActivity>,
        large_image: Option<String>,
    ) -> Value {
        self.nonce = self.nonce.wrapping_add(1);
        json!({
            "cmd": "SET_ACTIVITY",
            "args": {
                "pid": std::process::id(),
                "activity": activity.map(|activity| activity_json(activity, large_image.as_deref())),
            },
            "nonce": format!("rufin-{}", self.nonce),
        })
    }

    fn send_frame(&mut self, payload: Value) {
        if self.ensure_connected().is_err() {
            return;
        }

        let result = self
            .stream
            .as_mut()
            .ok_or_else(|| "Discord IPC is not connected".to_string())
            .and_then(|stream| write_packet(stream, OP_FRAME, &payload));
        if let Err(error) = result {
            debug!(%error, "Discord IPC write failed");
            self.stream = None;
        }
    }

    fn ensure_connected(&mut self) -> Result<(), String> {
        if self.stream.is_some() {
            return Ok(());
        }

        let mut stream = connect_discord_ipc()?;
        stream
            .set_read_timeout(Some(Duration::from_millis(750)))
            .map_err(|error| error.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_millis(750)))
            .map_err(|error| error.to_string())?;
        write_packet(
            &mut stream,
            OP_HANDSHAKE,
            &json!({
                "v": IPC_VERSION,
                "client_id": self.client_id,
            }),
        )?;
        if let Ok((opcode, response)) = read_packet(&mut stream)
            && opcode == OP_CLOSE
        {
            return Err(format!("Discord IPC closed during handshake: {response}"));
        }
        self.stream = Some(stream);
        Ok(())
    }
}

#[cfg(unix)]
fn activity_json(activity: &PresenceActivity, large_image: Option<&str>) -> Value {
    let mut value = json!({
        "details": discord_text(&activity.key.title, "Idle"),
        "state": discord_text(&activity.key.artist, "Unknown artist"),
        "assets": {
            "large_image": large_image.unwrap_or(DEFAULT_LARGE_IMAGE_URL),
            "large_text": discord_text(&activity.key.album, "Unknown album"),
        },
        "timestamps": {},
        "instance": false,
        "status_display_type": status_display_type(activity.key.display_type),
        "type": if activity.key.show_as_listening { 2 } else { 0 },
    });

    if let Some(start) = activity.started_at_millis {
        value["timestamps"]["start"] = json!(start);
    }
    if let Some(end) = activity.ended_at_millis {
        value["timestamps"]["end"] = json!(end);
    }
    if let Some((details_url, state_url)) = activity_urls(&activity.key) {
        value["details_url"] = json!(details_url);
        value["state_url"] = json!(state_url);
    }
    if should_show_state_icon(&activity.key) {
        value["assets"]["small_image"] = json!(activity.key.playback_state.image_key());
        value["assets"]["small_text"] = json!(activity.key.playback_state.label());
    }

    value
}

#[cfg(unix)]
fn status_display_type(display_type: DiscordDisplayType) -> u8 {
    match display_type {
        DiscordDisplayType::Application => 0,
        DiscordDisplayType::Artist => 1,
        DiscordDisplayType::Song => 2,
    }
}

#[cfg(unix)]
fn should_show_state_icon(key: &PresenceKey) -> bool {
    key.playback_state == PresencePlaybackState::Paused || key.show_state_icon
}

#[cfg(unix)]
fn activity_urls(key: &PresenceKey) -> Option<(String, String)> {
    if !matches!(
        key.link_type,
        DiscordLinkType::LastFm | DiscordLinkType::MusicBrainzLastFm
    ) {
        return None;
    }
    let state_url = lastfm_artist_url(&key.artist)?;
    let details_url = lastfm_track_url(&key.artist, &key.album, &key.title)?;
    Some((details_url, state_url))
}

#[cfg(unix)]
fn lastfm_artist_url(artist: &str) -> Option<String> {
    let artist = artist.trim();
    if artist.is_empty() {
        return None;
    }
    Some(format!(
        "https://www.last.fm/music/{}",
        url_encode_segment(artist)
    ))
}

#[cfg(unix)]
fn lastfm_track_url(artist: &str, album: &str, title: &str) -> Option<String> {
    let artist = artist.trim();
    let title = title.trim();
    if artist.is_empty() || title.is_empty() {
        return None;
    }
    let album = if album.trim().is_empty() { "_" } else { album };
    let url = format!(
        "https://www.last.fm/music/{}/{}/{}",
        url_encode_segment(artist),
        url_encode_segment(album),
        url_encode_segment(title)
    );
    if url.len() <= MAX_DISCORD_URL_LENGTH {
        Some(url)
    } else {
        None
    }
}

#[cfg(unix)]
fn discord_text(value: &str, fallback: &str) -> String {
    let text = if value.trim().is_empty() {
        fallback
    } else {
        value.trim()
    };
    let mut truncated = text
        .chars()
        .take(MAX_DISCORD_TEXT_LENGTH)
        .collect::<String>();
    if truncated.len() < 2 {
        truncated.push(' ');
    }
    truncated
}

#[cfg(unix)]
fn lastfm_cover_url(http: &Client, key: &PresenceKey) -> Option<String> {
    let api_key = key.lastfm_api_key.trim();
    let artist = key.artist.trim();
    let album = key.album.trim();
    if api_key.is_empty() || artist.is_empty() || album.is_empty() {
        return None;
    }
    let url = Url::parse_with_params(
        LASTFM_ALBUM_INFO_URL,
        [
            ("method", "album.getinfo"),
            ("api_key", api_key),
            ("artist", artist),
            ("album", album),
            ("format", "json"),
        ],
    )
    .ok()?;
    let response = http.get(url).send().ok()?;
    let value = response.json::<Value>().ok()?;
    for index in (0..=3).rev() {
        let Some(image) = value
            .pointer(&format!("/album/image/{index}/#text"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|image| !image.is_empty())
        else {
            continue;
        };
        return Some(image.to_string());
    }
    None
}

#[cfg(unix)]
fn musicbrainz_cover_url(http: &Client, artist: &str, album: &str) -> Option<String> {
    let artist = artist.trim();
    let album = album.trim();
    if artist.is_empty() || album.is_empty() {
        return None;
    }
    let query = format!("artist:\"{artist}\" AND release:\"{album}\"");
    let url = Url::parse_with_params(
        MUSICBRAINZ_RELEASE_SEARCH_URL,
        [("query", query.as_str()), ("fmt", "json"), ("limit", "1")],
    )
    .ok()?;
    let response = http.get(url).send().ok()?;
    let value = response.json::<Value>().ok()?;
    let release_id = value
        .pointer("/releases/0/id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|release_id| !release_id.is_empty())?;
    Some(format!(
        "{MUSICBRAINZ_COVER_ART_URL}/{release_id}/front-250"
    ))
}

#[cfg(unix)]
fn connect_discord_ipc() -> Result<UnixStream, String> {
    for path in discord_ipc_paths() {
        match UnixStream::connect(&path) {
            Ok(stream) => return Ok(stream),
            Err(error) => debug!(path = %path.display(), %error, "Discord IPC socket unavailable"),
        }
    }
    Err("Discord IPC socket was not found".to_string())
}

#[cfg(unix)]
fn discord_ipc_paths() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for key in ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"] {
        if let Some(path) = env::var_os(key).map(PathBuf::from)
            && !roots.contains(&path)
        {
            roots.push(path);
        }
    }
    let tmp = PathBuf::from("/tmp");
    if !roots.contains(&tmp) {
        roots.push(tmp);
    }

    let mut paths = Vec::new();
    for root in roots {
        for index in 0..10 {
            paths.push(root.join(format!("discord-ipc-{index}")));
        }
    }
    paths
}

#[cfg(unix)]
fn write_packet(stream: &mut UnixStream, opcode: u32, payload: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(payload).map_err(|error| error.to_string())?;
    let length = u32::try_from(bytes.len()).map_err(|_| "Discord IPC payload is too large")?;
    stream
        .write_all(&opcode.to_le_bytes())
        .and_then(|()| stream.write_all(&length.to_le_bytes()))
        .and_then(|()| stream.write_all(&bytes))
        .map_err(|error| error.to_string())
}

#[cfg(unix)]
fn read_packet(stream: &mut UnixStream) -> Result<(u32, Value), String> {
    let mut header = [0_u8; 8];
    stream
        .read_exact(&mut header)
        .map_err(|error| error.to_string())?;
    let opcode = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    let mut bytes = vec![0_u8; length];
    stream
        .read_exact(&mut bytes)
        .map_err(|error| error.to_string())?;
    let value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if opcode == OP_PING {
        write_packet(stream, OP_PONG, &value)?;
    }
    Ok((opcode, value))
}

#[cfg(unix)]
fn url_encode_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(*byte));
            }
            b' ' => encoded.push_str("%20"),
            byte => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{
        PresenceKey, PresencePlaybackState, activity_urls, discord_text, lastfm_track_url,
        presence_key, timeline_changed,
    };
    use crate::controller::PlaybackSnapshot;
    use rufin_core::{
        AppSettings, DiscordDisplayType, DiscordLinkType, QueueEntry, QueueEntryId, RepeatMode,
        TrackId,
    };
    use rufin_playback::PlaybackState;

    #[test]
    fn presence_key_requires_enabled_playback_with_track() {
        let mut settings = AppSettings {
            discord_presence_enabled: true,
            ..AppSettings::default()
        };
        let mut snapshot = PlaybackSnapshot::default();
        assert_eq!(presence_key(&settings, &snapshot), None);

        snapshot.current = Some(queue_entry());
        settings.discord_show_paused = false;
        snapshot.state = PlaybackState::Paused;
        assert_eq!(presence_key(&settings, &snapshot), None);

        settings.discord_show_paused = true;
        assert_eq!(
            presence_key(&settings, &snapshot).map(|key| key.playback_state),
            Some(PresencePlaybackState::Paused)
        );

        snapshot.state = PlaybackState::Playing;
        assert_eq!(
            presence_key(&settings, &snapshot),
            Some(PresenceKey {
                track_id: "jellyfin:track:one".to_string(),
                title: "Track One".to_string(),
                artist: "Artist".to_string(),
                album: "Album".to_string(),
                duration_millis: 180_000,
                playback_state: PresencePlaybackState::Playing,
                display_type: DiscordDisplayType::Application,
                link_type: DiscordLinkType::MusicBrainz,
                show_as_listening: true,
                show_state_icon: true,
                lastfm_api_key: String::new(),
            })
        );
    }

    #[test]
    fn presence_key_ignores_normal_position_ticks() {
        let settings = AppSettings {
            discord_presence_enabled: true,
            ..AppSettings::default()
        };
        let mut snapshot = PlaybackSnapshot {
            current: Some(queue_entry()),
            state: PlaybackState::Playing,
            ..PlaybackSnapshot::default()
        };
        let initial = presence_key(&settings, &snapshot);

        snapshot.position_millis = 1_000;
        snapshot.position_seconds = 1;

        assert_eq!(presence_key(&settings, &snapshot), initial);
    }

    #[test]
    fn timeline_changes_only_for_real_seek_drift() {
        assert!(!timeline_changed(Some(10_000), Some(11_000)));
        assert!(timeline_changed(Some(10_000), Some(12_000)));
        assert!(timeline_changed(None, Some(12_000)));
    }

    #[test]
    fn lastfm_urls_are_encoded_and_limited() {
        assert_eq!(
            lastfm_track_url("M83", "Hurry Up, We're Dreaming", "Midnight City"),
            Some(
                "https://www.last.fm/music/M83/Hurry%20Up%2C%20We%27re%20Dreaming/Midnight%20City"
                    .to_string()
            )
        );
        assert_eq!(lastfm_track_url("artist", "album", &"x".repeat(300)), None);
    }

    #[test]
    fn activity_urls_follow_link_setting() {
        let mut key = test_presence_key();
        assert_eq!(activity_urls(&key), None);

        key.link_type = DiscordLinkType::LastFm;
        assert_eq!(
            activity_urls(&key),
            Some((
                "https://www.last.fm/music/Artist/Album/Track%20One".to_string(),
                "https://www.last.fm/music/Artist".to_string(),
            ))
        );
    }

    #[test]
    fn discord_text_truncates_and_pads_short_values() {
        assert_eq!(discord_text("", "A"), "A ");
        assert_eq!(discord_text(&"x".repeat(140), "fallback").len(), 127);
    }

    fn test_presence_key() -> PresenceKey {
        PresenceKey {
            track_id: "jellyfin:track:one".to_string(),
            title: "Track One".to_string(),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            duration_millis: 180_000,
            playback_state: PresencePlaybackState::Playing,
            display_type: DiscordDisplayType::Application,
            link_type: DiscordLinkType::None,
            show_as_listening: false,
            show_state_icon: true,
            lastfm_api_key: String::new(),
        }
    }

    fn queue_entry() -> QueueEntry {
        QueueEntry {
            id: QueueEntryId::new("entry-one"),
            track_id: TrackId::new("jellyfin:track:one"),
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

    #[test]
    fn default_playback_snapshot_stays_stopped_for_presence_tests() {
        let snapshot = PlaybackSnapshot::default();
        assert_eq!(snapshot.state, PlaybackState::Stopped);
        assert_eq!(snapshot.repeat_mode, RepeatMode::All);
    }
}
