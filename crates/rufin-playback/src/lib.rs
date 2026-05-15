use std::collections::VecDeque;
use std::fmt;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use gstreamer as gst;
use gstreamer_play as gst_play;
use rufin_core::TrackId;
use thiserror::Error;
use tracing::{debug, error, instrument};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaybackTrack {
    pub id: TrackId,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_seconds: u32,
}

#[derive(Clone, Eq, PartialEq)]
pub struct StreamDescriptor {
    uri: String,
    redacted_uri: String,
}

impl StreamDescriptor {
    pub fn new(uri: impl Into<String>) -> Self {
        let uri = uri.into();
        let redacted_uri = redact_sensitive_uri(&uri);
        Self { uri, redacted_uri }
    }

    pub fn with_redacted(uri: impl Into<String>, redacted_uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            redacted_uri: redacted_uri.into(),
        }
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn redacted_uri(&self) -> &str {
        &self.redacted_uri
    }
}

impl fmt::Debug for StreamDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StreamDescriptor")
            .field("uri", &self.redacted_uri)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PlaybackState {
    #[default]
    Stopped,
    Buffering,
    Paused,
    Playing,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlaybackCommand {
    Play {
        track: PlaybackTrack,
        stream: StreamDescriptor,
        start_position_seconds: u32,
    },
    Resume,
    Pause,
    Stop,
    Seek(u32),
    SetVolume(f64),
    SetMuted(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlaybackEvent {
    StateChanged(PlaybackState),
    PositionChanged(u32),
    DurationChanged(u32),
    Buffering(u8),
    EndOfStream,
    VolumeChanged { volume: f64, muted: bool },
    Error(String),
}

#[derive(Debug, Error)]
pub enum PlaybackError {
    #[error("playback backend failed: {0}")]
    Backend(String),
    #[error("playback command channel closed")]
    ChannelClosed,
}

pub trait PlaybackBackend: Send {
    fn send(&mut self, command: PlaybackCommand) -> Result<(), PlaybackError>;
    fn drain_events(&mut self) -> Vec<PlaybackEvent>;
}

#[derive(Default)]
pub struct FakePlaybackBackend {
    state: PlaybackState,
    current: Option<PlaybackTrack>,
    position_seconds: u32,
    duration_seconds: u32,
    volume: f64,
    muted: bool,
    events: VecDeque<PlaybackEvent>,
}

impl FakePlaybackBackend {
    pub fn new() -> Self {
        Self {
            state: PlaybackState::Stopped,
            current: None,
            position_seconds: 0,
            duration_seconds: 0,
            volume: 1.0,
            muted: false,
            events: VecDeque::new(),
        }
    }

    pub fn emit_end_of_stream_for_test(&mut self) {
        self.events.push_back(PlaybackEvent::EndOfStream);
    }

    fn set_state(&mut self, state: PlaybackState) {
        self.state = state;
        self.events.push_back(PlaybackEvent::StateChanged(state));
    }
}

impl PlaybackBackend for FakePlaybackBackend {
    fn send(&mut self, command: PlaybackCommand) -> Result<(), PlaybackError> {
        match command {
            PlaybackCommand::Play {
                track,
                start_position_seconds,
                ..
            } => {
                self.duration_seconds = track.duration_seconds;
                self.position_seconds = start_position_seconds.min(self.duration_seconds);
                self.current = Some(track);
                self.events
                    .push_back(PlaybackEvent::DurationChanged(self.duration_seconds));
                self.events
                    .push_back(PlaybackEvent::PositionChanged(self.position_seconds));
                self.set_state(PlaybackState::Playing);
            }
            PlaybackCommand::Resume => self.set_state(PlaybackState::Playing),
            PlaybackCommand::Pause => self.set_state(PlaybackState::Paused),
            PlaybackCommand::Stop => {
                self.position_seconds = 0;
                self.set_state(PlaybackState::Stopped);
                self.events.push_back(PlaybackEvent::PositionChanged(0));
            }
            PlaybackCommand::Seek(seconds) => {
                self.position_seconds = seconds.min(self.duration_seconds);
                self.events
                    .push_back(PlaybackEvent::PositionChanged(self.position_seconds));
            }
            PlaybackCommand::SetVolume(volume) => {
                self.volume = volume.clamp(0.0, 1.0);
                self.events.push_back(PlaybackEvent::VolumeChanged {
                    volume: self.volume,
                    muted: self.muted,
                });
            }
            PlaybackCommand::SetMuted(muted) => {
                self.muted = muted;
                self.events.push_back(PlaybackEvent::VolumeChanged {
                    volume: self.volume,
                    muted: self.muted,
                });
            }
        }
        Ok(())
    }

    fn drain_events(&mut self) -> Vec<PlaybackEvent> {
        self.events.drain(..).collect()
    }
}

pub struct LazyGStreamerPlaybackBackend {
    inner: Option<Box<dyn PlaybackBackend>>,
}

impl LazyGStreamerPlaybackBackend {
    pub fn new() -> Self {
        Self { inner: None }
    }

    fn backend(&mut self) -> Result<&mut Box<dyn PlaybackBackend>, PlaybackError> {
        if self.inner.is_none() {
            debug!("initializing GStreamer playback backend");
            self.inner = Some(Box::new(GStreamerPlaybackBackend::new()?));
        }
        Ok(self
            .inner
            .as_mut()
            .expect("lazy playback backend was just initialized"))
    }
}

impl Default for LazyGStreamerPlaybackBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaybackBackend for LazyGStreamerPlaybackBackend {
    fn send(&mut self, command: PlaybackCommand) -> Result<(), PlaybackError> {
        if self.inner.is_none() && !matches!(command, PlaybackCommand::Play { .. }) {
            return Ok(());
        }
        self.backend()?.send(command)
    }

    fn drain_events(&mut self) -> Vec<PlaybackEvent> {
        self.inner
            .as_mut()
            .map(|backend| backend.drain_events())
            .unwrap_or_default()
    }
}

pub struct GStreamerPlaybackBackend {
    commands: Sender<PlaybackCommand>,
    events: Arc<Mutex<VecDeque<PlaybackEvent>>>,
}

impl GStreamerPlaybackBackend {
    pub fn new() -> Result<Self, PlaybackError> {
        let (commands, receiver) = channel();
        let events = Arc::new(Mutex::new(VecDeque::new()));
        let thread_events = Arc::clone(&events);
        thread::Builder::new()
            .name("rufin-gstreamer-playback".to_string())
            .spawn(move || run_gstreamer_thread(receiver, thread_events))
            .map_err(|error| PlaybackError::Backend(error.to_string()))?;
        Ok(Self { commands, events })
    }
}

impl PlaybackBackend for GStreamerPlaybackBackend {
    fn send(&mut self, command: PlaybackCommand) -> Result<(), PlaybackError> {
        self.commands
            .send(command)
            .map_err(|_| PlaybackError::ChannelClosed)
    }

    fn drain_events(&mut self) -> Vec<PlaybackEvent> {
        self.events
            .lock()
            .map(|mut events| events.drain(..).collect())
            .unwrap_or_default()
    }
}

#[instrument(skip(receiver, events))]
fn run_gstreamer_thread(
    receiver: Receiver<PlaybackCommand>,
    events: Arc<Mutex<VecDeque<PlaybackEvent>>>,
) {
    if let Err(error) = gst::init() {
        push_event(
            &events,
            PlaybackEvent::Error(format!("GStreamer init failed: {error}")),
        );
        return;
    }

    let play = gst_play::Play::default();
    play.set_video_track_enabled(false);
    let bus = play.message_bus();
    let mut last_position_tick = Instant::now();
    let mut volume = 1.0;
    let mut muted = false;

    loop {
        while let Some(message) = bus.pop() {
            handle_gstreamer_message(&events, &message, &mut volume, &mut muted);
        }

        match receiver.recv_timeout(Duration::from_millis(80)) {
            Ok(command) => {
                if handle_gstreamer_command(&play, &events, command).is_err() {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if last_position_tick.elapsed() >= Duration::from_millis(500) {
            last_position_tick = Instant::now();
            if let Some(position) = play.position() {
                push_event(
                    &events,
                    PlaybackEvent::PositionChanged(clock_seconds(position)),
                );
            }
            if let Some(duration) = play.duration() {
                push_event(
                    &events,
                    PlaybackEvent::DurationChanged(clock_seconds(duration)),
                );
            }
        }
    }
}

fn handle_gstreamer_command(
    play: &gst_play::Play,
    events: &Arc<Mutex<VecDeque<PlaybackEvent>>>,
    command: PlaybackCommand,
) -> Result<(), ()> {
    match command {
        PlaybackCommand::Play {
            track,
            stream,
            start_position_seconds,
        } => {
            debug!(
                track_id = %track.id.as_str(),
                uri = %stream.redacted_uri(),
                "starting GStreamer playback"
            );
            push_event(
                events,
                PlaybackEvent::StateChanged(PlaybackState::Buffering),
            );
            play.stop();
            play.set_uri(Some(stream.uri()));
            play.play();
            if start_position_seconds > 0 {
                play.seek(gst::ClockTime::from_seconds(u64::from(
                    start_position_seconds,
                )));
            }
        }
        PlaybackCommand::Resume => {
            play.play();
            push_event(events, PlaybackEvent::StateChanged(PlaybackState::Playing));
        }
        PlaybackCommand::Pause => {
            play.pause();
            push_event(events, PlaybackEvent::StateChanged(PlaybackState::Paused));
        }
        PlaybackCommand::Stop => {
            play.stop();
            push_event(events, PlaybackEvent::PositionChanged(0));
            push_event(events, PlaybackEvent::StateChanged(PlaybackState::Stopped));
        }
        PlaybackCommand::Seek(seconds) => {
            play.seek(gst::ClockTime::from_seconds(u64::from(seconds)));
            push_event(events, PlaybackEvent::PositionChanged(seconds));
        }
        PlaybackCommand::SetVolume(volume) => play.set_volume(volume.clamp(0.0, 1.0)),
        PlaybackCommand::SetMuted(muted) => play.set_mute(muted),
    }
    Ok(())
}

fn handle_gstreamer_message(
    events: &Arc<Mutex<VecDeque<PlaybackEvent>>>,
    message: &gst::Message,
    volume: &mut f64,
    muted: &mut bool,
) {
    let Ok(message) = gst_play::PlayMessage::parse(message) else {
        return;
    };
    match message {
        gst_play::PlayMessage::StateChanged(state) => {
            push_event(
                events,
                PlaybackEvent::StateChanged(match state.state() {
                    gst_play::PlayState::Stopped => PlaybackState::Stopped,
                    gst_play::PlayState::Buffering => PlaybackState::Buffering,
                    gst_play::PlayState::Paused => PlaybackState::Paused,
                    gst_play::PlayState::Playing => PlaybackState::Playing,
                    _ => PlaybackState::Stopped,
                }),
            );
        }
        gst_play::PlayMessage::PositionUpdated(position) => {
            if let Some(position) = position.position() {
                push_event(
                    events,
                    PlaybackEvent::PositionChanged(clock_seconds(position)),
                );
            }
        }
        gst_play::PlayMessage::DurationChanged(duration) => {
            if let Some(duration) = duration.duration() {
                push_event(
                    events,
                    PlaybackEvent::DurationChanged(clock_seconds(duration)),
                );
            }
        }
        gst_play::PlayMessage::Buffering(buffering) => {
            push_event(
                events,
                PlaybackEvent::Buffering(buffering.percent().min(100) as u8),
            );
        }
        gst_play::PlayMessage::EndOfStream(_) => {
            push_event(events, PlaybackEvent::EndOfStream);
        }
        gst_play::PlayMessage::VolumeChanged(volume_message) => {
            *volume = volume_message.volume().clamp(0.0, 1.0);
            push_event(
                events,
                PlaybackEvent::VolumeChanged {
                    volume: *volume,
                    muted: *muted,
                },
            );
        }
        gst_play::PlayMessage::MuteChanged(mute) => {
            *muted = mute.is_muted();
            push_event(
                events,
                PlaybackEvent::VolumeChanged {
                    volume: *volume,
                    muted: *muted,
                },
            );
        }
        gst_play::PlayMessage::Error(error_message) => {
            let message = error_message.error().to_string();
            error!(%message, "GStreamer playback error");
            push_event(events, PlaybackEvent::Error(message));
        }
        _ => {}
    }
}

fn push_event(events: &Arc<Mutex<VecDeque<PlaybackEvent>>>, event: PlaybackEvent) {
    if let Ok(mut events) = events.lock() {
        events.push_back(event);
    }
}

fn clock_seconds(clock_time: gst::ClockTime) -> u32 {
    clock_time.seconds().min(u64::from(u32::MAX)) as u32
}

fn redact_sensitive_uri(uri: &str) -> String {
    let Some((base, query)) = uri.split_once('?') else {
        return uri.to_string();
    };
    let query = query
        .split('&')
        .map(|pair| {
            let Some((key, value)) = pair.split_once('=') else {
                return pair.to_string();
            };
            let lower = key.to_ascii_lowercase();
            if lower.contains("token") || lower.contains("key") {
                format!("{key}=<redacted>")
            } else {
                format!("{key}={value}")
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}?{query}")
}

#[cfg(test)]
mod tests {
    use super::{
        FakePlaybackBackend, PlaybackBackend, PlaybackCommand, PlaybackEvent, PlaybackState,
        PlaybackTrack, StreamDescriptor,
    };
    use rufin_core::TrackId;

    #[test]
    fn fake_backend_reports_basic_state_transitions() {
        let mut backend = FakePlaybackBackend::new();
        let track = track(1);

        backend
            .send(PlaybackCommand::Play {
                track,
                stream: StreamDescriptor::new("fake://track/1"),
                start_position_seconds: 12,
            })
            .expect("play");
        backend.send(PlaybackCommand::Pause).expect("pause");
        backend.send(PlaybackCommand::Resume).expect("resume");
        backend.send(PlaybackCommand::Seek(42)).expect("seek");
        backend.send(PlaybackCommand::Stop).expect("stop");

        let events = backend.drain_events();
        assert!(events.contains(&PlaybackEvent::StateChanged(PlaybackState::Playing)));
        assert!(events.contains(&PlaybackEvent::StateChanged(PlaybackState::Paused)));
        assert!(events.contains(&PlaybackEvent::PositionChanged(42)));
        assert!(events.contains(&PlaybackEvent::StateChanged(PlaybackState::Stopped)));
    }

    #[test]
    fn stream_descriptor_redacts_sensitive_query_values() {
        let stream = StreamDescriptor::new(
            "https://music.example/Audio/track/stream?UserId=user&api_key=secret-token&DeviceId=device",
        );

        assert_eq!(
            stream.redacted_uri(),
            "https://music.example/Audio/track/stream?UserId=user&api_key=<redacted>&DeviceId=device"
        );
        assert!(!format!("{stream:?}").contains("secret-token"));
    }

    fn track(number: u32) -> PlaybackTrack {
        PlaybackTrack {
            id: TrackId::fake(number),
            title: format!("Track {number}"),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            duration_seconds: 180,
        }
    }
}
