use std::sync::Arc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

#[cfg(all(unix, not(test)))]
use std::env;
#[cfg(unix)]
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
};

use playback::{PlaybackView, RunId, SequenceEntry, TransportStatus};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use serde_json::{Value, json};
use tracing::debug;

use crate::{LatestReceiver, LatestSender, latest_slot};

pub const DEFAULT_CLIENT_ID: &str = "1505345384686419979";
pub(crate) const APP_ICON_URL: &str = "https://raw.githubusercontent.com/screwys/Rufin/main/data/icons/hicolor/scalable/apps/io.github.screwys.Rufin.svg";
pub(crate) const SUPPORTED: bool = cfg!(unix);

#[cfg(unix)]
const MAX_TEXT_LENGTH: usize = 127;
#[cfg(unix)]
const MAX_URL_LENGTH: usize = 256;
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

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

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum DisplayType {
    #[serde(rename = "artist")]
    Artist,
    #[serde(rename = "application", alias = "app")]
    #[default]
    Application,
    #[serde(rename = "song")]
    Song,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum LinkType {
    #[serde(rename = "last_fm")]
    LastFm,
    #[serde(rename = "musicbrainz")]
    #[default]
    MusicBrainz,
    #[serde(rename = "musicbrainz_last_fm")]
    MusicBrainzLastFm,
    #[serde(rename = "none")]
    None,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Settings {
    #[serde(rename = "discord_presence_enabled")]
    pub enabled: bool,
    #[serde(rename = "discord_client_id")]
    pub client_id: String,
    #[serde(rename = "discord_display_type")]
    pub display_type: DisplayType,
    #[serde(rename = "discord_link_type")]
    pub link_type: LinkType,
    #[serde(rename = "discord_show_paused")]
    pub show_paused: bool,
    #[serde(rename = "discord_show_as_listening")]
    pub show_as_listening: bool,
    #[serde(rename = "discord_show_state_icon")]
    pub show_state_icon: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            client_id: DEFAULT_CLIENT_ID.to_string(),
            display_type: DisplayType::Application,
            link_type: LinkType::MusicBrainz,
            show_paused: false,
            show_as_listening: true,
            show_state_icon: true,
        }
    }
}

impl Settings {
    pub fn sanitize(&mut self) {
        self.client_id = self.client_id.trim().to_string();
        if self.client_id.is_empty() {
            self.client_id = DEFAULT_CLIENT_ID.to_string();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlaybackState {
    Playing,
    Paused,
}

#[cfg(unix)]
impl PlaybackState {
    const fn label(self) -> &'static str {
        match self {
            Self::Playing => "Playing",
            Self::Paused => "Paused",
        }
    }

    const fn image_key(self) -> &'static str {
        match self {
            Self::Playing => "playing",
            Self::Paused => "paused",
        }
    }
}

#[derive(Clone)]
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) struct Activity {
    settings: Settings,
    source_id: String,
    pub(crate) run: RunId,
    entry: Arc<SequenceEntry>,
    playback_state: PlaybackState,
    pub(crate) started_at_millis: Option<u64>,
    pub(crate) ended_at_millis: Option<u64>,
    pub(crate) large_image: String,
}

impl Activity {
    pub(crate) fn new(
        settings: &Settings,
        view: &PlaybackView,
        now_millis: u64,
        large_image: String,
    ) -> Option<Self> {
        let playback_state = visible_playback_state(settings, view.transport.state)?;
        let run = view.transport.run?;
        let entry = Arc::clone(view.transport.current.as_ref()?);
        let duration_millis = duration_millis(view, &entry);
        let started_at_millis = match playback_state {
            PlaybackState::Playing => {
                Some(now_millis.saturating_sub(view.transport.position_millis))
            }
            PlaybackState::Paused => None,
        };
        Some(Self {
            settings: settings.clone(),
            source_id: view.transport.source_id.as_str().to_string(),
            run,
            entry,
            playback_state,
            started_at_millis,
            ended_at_millis: started_at_millis.and_then(|started| {
                (duration_millis > 0).then(|| started.saturating_add(duration_millis))
            }),
            large_image,
        })
    }

    pub(crate) fn matches(&self, view: &PlaybackView) -> bool {
        self.source_id == view.transport.source_id.as_str()
            && Some(self.run) == view.transport.run
            && Some(self.playback_state)
                == visible_playback_state(&self.settings, view.transport.state)
            && view
                .transport
                .current
                .as_ref()
                .is_some_and(|entry| entry.as_ref() == self.entry.as_ref())
    }
}

fn duration_millis(_view: &PlaybackView, entry: &SequenceEntry) -> u64 {
    u64::from(entry.track.duration_seconds).saturating_mul(1_000)
}

pub(crate) fn visible_playback_state(
    settings: &Settings,
    state: TransportStatus,
) -> Option<PlaybackState> {
    if !settings.enabled {
        return None;
    }
    Some(match state {
        TransportStatus::Playing | TransportStatus::Buffering => PlaybackState::Playing,
        TransportStatus::Paused if settings.show_paused => PlaybackState::Paused,
        TransportStatus::Stopped
        | TransportStatus::Resolving
        | TransportStatus::Paused
        | TransportStatus::Failed => return None,
    })
}

pub(crate) struct Worker {
    mailbox: Option<LatestSender<Option<Arc<Activity>>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    pub(crate) fn new() -> Self {
        let (mailbox, receiver) = latest_slot();
        let thread = std::thread::spawn(move || {
            run_worker(&receiver, Connection::new(), RECONNECT_DELAY);
        });
        Self {
            mailbox: Some(mailbox),
            thread: Some(thread),
        }
    }

    pub(crate) fn publish(&self, activity: Option<Arc<Activity>>) {
        if let Some(mailbox) = &self.mailbox {
            mailbox.publish(activity);
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(mailbox) = self.mailbox.take() {
            mailbox.publish(None);
            drop(mailbox);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_worker(
    receiver: &LatestReceiver<Option<Arc<Activity>>>,
    mut connection: Connection,
    reconnect_delay: Duration,
) {
    let mut current = receiver.recv();
    while let Some(activity) = current {
        if connection.apply(activity.as_deref()) {
            current = match receiver.recv_timeout(reconnect_delay) {
                Ok(next) => Some(next),
                Err(RecvTimeoutError::Timeout) => Some(activity),
                Err(RecvTimeoutError::Disconnected) => None,
            };
        } else {
            drop(activity);
            current = receiver.recv();
        }
    }
}

struct Connection {
    #[cfg(unix)]
    stream: Option<UnixStream>,
    #[cfg(unix)]
    client_id: Option<String>,
    #[cfg(unix)]
    paths: Vec<PathBuf>,
    #[cfg(unix)]
    nonce: u64,
}

impl Connection {
    fn new() -> Self {
        Self {
            #[cfg(unix)]
            stream: None,
            #[cfg(unix)]
            client_id: None,
            #[cfg(unix)]
            paths: worker_ipc_paths(),
            #[cfg(unix)]
            nonce: 0,
        }
    }

    fn apply(&mut self, activity: Option<&Activity>) -> bool {
        #[cfg(not(unix))]
        {
            let _ = activity;
            debug!("Discord rich presence is not supported on this platform");
            false
        }
        #[cfg(unix)]
        {
            let Some(activity) = activity else {
                if self.stream.is_some() {
                    let payload = self.activity_payload(None);
                    if self.send_payload(&payload).is_err() {
                        self.disconnect();
                    }
                }
                return false;
            };
            let client_id = activity.settings.client_id.as_str();
            if self
                .client_id
                .as_deref()
                .is_some_and(|old| old != client_id)
            {
                let payload = self.activity_payload(None);
                let _ = self.send_payload(&payload);
                self.disconnect();
            }
            if let Err(error) = self.ensure_connected(client_id) {
                debug!(%error, "Discord IPC connection unavailable");
                return true;
            }
            let payload = self.activity_payload(Some(activity));
            if let Err(error) = self.send_payload(&payload) {
                debug!(%error, "Discord IPC update failed");
                self.disconnect();
                return true;
            }
            false
        }
    }

    #[cfg(unix)]
    fn activity_payload(&mut self, activity: Option<&Activity>) -> Value {
        self.nonce = self.nonce.wrapping_add(1);
        json!({
            "cmd": "SET_ACTIVITY",
            "args": {
                "pid": std::process::id(),
                "activity": activity.map(activity_json),
            },
            "nonce": format!("rufin-{}", self.nonce),
        })
    }

    #[cfg(unix)]
    fn ensure_connected(&mut self, client_id: &str) -> Result<(), String> {
        if self.stream.is_some() {
            return Ok(());
        }
        let mut stream = connect_paths(&self.paths)?;
        stream
            .set_read_timeout(Some(Duration::from_millis(750)))
            .map_err(|error| error.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_millis(750)))
            .map_err(|error| error.to_string())?;
        write_packet(
            &mut stream,
            OP_HANDSHAKE,
            &json!({ "v": IPC_VERSION, "client_id": client_id }),
        )?;
        read_response(&mut stream)?;
        self.stream = Some(stream);
        self.client_id = Some(client_id.to_string());
        Ok(())
    }

    #[cfg(unix)]
    fn send_payload(&mut self, payload: &Value) -> Result<(), String> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| "Discord IPC is not connected".to_string())?;
        write_packet(stream, OP_FRAME, payload)?;
        read_response(stream)
    }

    #[cfg(unix)]
    fn disconnect(&mut self) {
        self.stream = None;
        self.client_id = None;
    }
}

#[cfg(unix)]
fn worker_ipc_paths() -> Vec<PathBuf> {
    #[cfg(test)]
    {
        Vec::new()
    }
    #[cfg(not(test))]
    {
        discord_ipc_paths()
    }
}

#[cfg(unix)]
fn activity_json(activity: &Activity) -> Value {
    let track = &activity.entry.track;
    let mut value = json!({
        "details": discord_text(&track.title, "Idle"),
        "state": discord_text(&track.artist, "Unknown artist"),
        "assets": {
            "large_image": activity.large_image,
            "large_text": discord_text(&track.album, "Unknown album"),
        },
        "timestamps": {},
        "instance": false,
        "status_display_type": status_display_type(activity.settings.display_type),
        "type": if activity.settings.show_as_listening { 2 } else { 0 },
    });
    if let Some(start) = activity.started_at_millis {
        value["timestamps"]["start"] = json!(start / 1_000);
    }
    if let Some(end) = activity.ended_at_millis {
        value["timestamps"]["end"] = json!(end / 1_000);
    }
    let (details_url, state_url) = activity_urls(activity);
    if let Some(details_url) = details_url {
        value["details_url"] = json!(details_url);
    }
    if let Some(state_url) = state_url {
        value["state_url"] = json!(state_url);
    }
    if activity.playback_state == PlaybackState::Paused || activity.settings.show_state_icon {
        value["assets"]["small_image"] = json!(activity.playback_state.image_key());
        value["assets"]["small_text"] = json!(activity.playback_state.label());
    }
    value
}

#[cfg(unix)]
const fn status_display_type(display_type: DisplayType) -> u8 {
    match display_type {
        DisplayType::Application => 0,
        DisplayType::Artist => 1,
        DisplayType::Song => 2,
    }
}

#[cfg(unix)]
fn activity_urls(activity: &Activity) -> (Option<String>, Option<String>) {
    let track = &activity.entry.track;
    let track_artist = track
        .artist_credits
        .first()
        .map(|credit| credit.name.trim())
        .filter(|artist| !artist.is_empty())
        .unwrap_or_else(|| track.artist.trim());
    let album_artist = track
        .album_artist_credits
        .first()
        .map(|credit| credit.name.trim())
        .filter(|artist| !artist.is_empty())
        .unwrap_or(track_artist);
    let mut details = None;
    let mut state = None;
    if matches!(
        activity.settings.link_type,
        LinkType::LastFm | LinkType::MusicBrainzLastFm
    ) {
        state = lastfm_artist_url(track_artist);
        details = lastfm_track_url(album_artist, &track.album, &track.title);
    }
    if matches!(
        activity.settings.link_type,
        LinkType::MusicBrainz | LinkType::MusicBrainzLastFm
    ) {
        if activity.settings.link_type == LinkType::MusicBrainz {
            state = track
                .artist_credits
                .first()
                .and_then(|artist| artist.musicbrainz_artist_id.as_deref())
                .and_then(|id| musicbrainz_url("artist", id));
        }
        details = track
            .musicbrainz_release_track_id
            .as_deref()
            .and_then(|id| musicbrainz_url("track", id))
            .or_else(|| {
                track
                    .musicbrainz_recording_id
                    .as_deref()
                    .and_then(|id| musicbrainz_url("recording", id))
            })
            .or(details);
    }
    (details, state)
}

#[cfg(unix)]
fn lastfm_artist_url(artist: &str) -> Option<String> {
    let artist = artist.trim();
    (!artist.is_empty()).then(|| format!("https://www.last.fm/music/{}", encode_segment(artist)))
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
        encode_segment(artist),
        encode_segment(album),
        encode_segment(title)
    );
    (url.len() <= MAX_URL_LENGTH).then_some(url)
}

#[cfg(unix)]
fn musicbrainz_url(entity: &str, id: &str) -> Option<String> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    Some(format!(
        "https://musicbrainz.org/{entity}/{}",
        encode_segment(id)
    ))
}

#[cfg(unix)]
fn discord_text(value: &str, fallback: &str) -> String {
    let text = if value.trim().is_empty() {
        fallback
    } else {
        value.trim()
    };
    let mut text = text.chars().take(MAX_TEXT_LENGTH).collect::<String>();
    if text.chars().count() < 2 {
        text.push(' ');
    }
    text
}

#[cfg(unix)]
fn encode_segment(value: &str) -> String {
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

#[cfg(unix)]
fn connect_paths(paths: &[PathBuf]) -> Result<UnixStream, String> {
    for path in paths {
        if let Ok(stream) = UnixStream::connect(path) {
            return Ok(stream);
        }
    }
    Err("Discord IPC socket was not found".to_string())
}

#[cfg(all(unix, not(test)))]
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
fn read_response(stream: &mut UnixStream) -> Result<(), String> {
    loop {
        match read_packet(stream)? {
            (OP_PING, _) => {}
            (OP_CLOSE, response) => return Err(format!("Discord IPC closed: {response}")),
            (OP_FRAME, _) => return Ok(()),
            (opcode, _) => return Err(format!("unexpected Discord IPC opcode {opcode}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::tests::test_view;

    use super::*;

    #[test]
    fn settings_keep_persisted_defaults_and_display_alias() {
        let settings = serde_json::from_str::<Settings>("{}")
            .unwrap_or_else(|error| panic!("deserialize defaults: {error}"));
        assert_eq!(settings, Settings::default());
        assert_eq!(settings.link_type, LinkType::MusicBrainz);

        let alias = serde_json::from_str::<DisplayType>("\"app\"")
            .unwrap_or_else(|error| panic!("deserialize display alias: {error}"));
        assert_eq!(alias, DisplayType::Application);

        let mut disabled = Settings {
            enabled: false,
            client_id: "  ".to_string(),
            ..Settings::default()
        };
        disabled.sanitize();
        assert_eq!(disabled.client_id, DEFAULT_CLIENT_ID);
        assert!(!disabled.enabled);
    }

    #[cfg(unix)]
    #[test]
    fn payload_uses_musicbrainz_facts_without_a_lookup() {
        let mut activity = test_activity(1, "Track");
        activity.settings.link_type = LinkType::MusicBrainzLastFm;
        let payload = activity_json(&activity);

        assert_eq!(
            payload["details_url"],
            "https://musicbrainz.org/track/track-id"
        );
        assert_eq!(payload["state_url"], "https://www.last.fm/music/Artist");
        assert_eq!(payload["timestamps"]["end"], 52);
        assert_eq!(payload["assets"]["large_image"], APP_ICON_URL);

        activity.settings.link_type = LinkType::MusicBrainz;
        Arc::make_mut(&mut activity.entry)
            .track
            .musicbrainz_release_track_id = None;
        let payload = activity_json(&activity);
        assert_eq!(
            payload["details_url"],
            "https://musicbrainz.org/recording/recording-id"
        );
        assert_eq!(
            payload["state_url"],
            "https://musicbrainz.org/artist/artist-id"
        );

        activity.settings.link_type = LinkType::LastFm;
        let payload = activity_json(&activity);
        assert_eq!(payload["state_url"], "https://www.last.fm/music/Artist");
        assert_eq!(
            payload["details_url"],
            "https://www.last.fm/music/Album%20Artist/Album/Track"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_ipc_handshake_answers_ping_and_reconnects() {
        use std::os::unix::net::UnixListener;

        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| panic!("create IPC test directory: {error}"));
        let path = directory.path().join("discord-ipc-0");
        let first_listener = UnixListener::bind(&path)
            .unwrap_or_else(|error| panic!("bind first IPC socket: {error}"));
        let first_server = std::thread::spawn(move || serve_one_update(first_listener, true));

        let mut connection = Connection {
            stream: None,
            client_id: None,
            paths: vec![path.clone()],
            nonce: 0,
        };
        let first = test_activity(1, "First");
        assert!(!connection.apply(Some(&first)));
        first_server
            .join()
            .unwrap_or_else(|_| panic!("first IPC server panicked"));

        let second = test_activity(2, "Latest");
        assert!(connection.apply(Some(&second)));
        std::fs::remove_file(&path)
            .unwrap_or_else(|error| panic!("remove first IPC socket: {error}"));
        let second_listener = UnixListener::bind(&path)
            .unwrap_or_else(|error| panic!("bind replacement IPC socket: {error}"));
        let second_server = std::thread::spawn(move || serve_one_update(second_listener, false));
        assert!(!connection.apply(Some(&second)));
        second_server
            .join()
            .unwrap_or_else(|_| panic!("replacement IPC server panicked"));
    }

    #[cfg(unix)]
    fn serve_one_update(listener: std::os::unix::net::UnixListener, ping: bool) {
        let (mut stream, _) = listener
            .accept()
            .unwrap_or_else(|error| panic!("accept IPC connection: {error}"));
        let (opcode, handshake) =
            read_packet(&mut stream).unwrap_or_else(|error| panic!("read IPC handshake: {error}"));
        assert_eq!(opcode, OP_HANDSHAKE);
        assert_eq!(handshake["client_id"], DEFAULT_CLIENT_ID);
        write_packet(&mut stream, OP_FRAME, &json!({ "evt": "READY" }))
            .unwrap_or_else(|error| panic!("write IPC ready response: {error}"));

        let (opcode, update) =
            read_packet(&mut stream).unwrap_or_else(|error| panic!("read IPC activity: {error}"));
        assert_eq!(opcode, OP_FRAME);
        if ping {
            write_packet(&mut stream, OP_PING, &json!({ "ping": 1 }))
                .unwrap_or_else(|error| panic!("write IPC ping: {error}"));
            let (opcode, pong) =
                read_packet(&mut stream).unwrap_or_else(|error| panic!("read IPC pong: {error}"));
            assert_eq!(opcode, OP_PONG);
            assert_eq!(pong["ping"], 1);
            write_packet(&mut stream, OP_FRAME, &json!({ "evt": "SET_ACTIVITY" }))
                .unwrap_or_else(|error| panic!("write IPC activity response: {error}"));
        } else {
            assert_eq!(update["args"]["activity"]["details"], "Latest");
            write_packet(&mut stream, OP_FRAME, &json!({ "evt": "SET_ACTIVITY" }))
                .unwrap_or_else(|error| panic!("write IPC activity response: {error}"));
        }
    }

    fn test_activity(run: u64, title: &str) -> Activity {
        let mut view = test_view(run, "Album", TransportStatus::Playing, 0);
        Arc::make_mut(
            view.transport
                .current
                .as_mut()
                .unwrap_or_else(|| panic!("current entry missing")),
        )
        .track
        .title = title.to_string();
        let mut activity = Activity::new(
            &Settings {
                enabled: true,
                link_type: LinkType::None,
                ..Settings::default()
            },
            &view,
            10_000,
            APP_ICON_URL.to_string(),
        )
        .unwrap_or_else(|| panic!("test activity should be visible"));
        activity.started_at_millis = Some(10_000);
        activity.ended_at_millis = Some(52_500);
        activity
    }
}
