use std::collections::VecDeque;
use std::f64::consts::FRAC_PI_2;
use std::fmt;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use gst::glib;
use gst::prelude::*;
use gstreamer as gst;
use rufin_core::{
    EQUALIZER_BAND_COUNT, PlaybackSettings, PlaybackTransitionMode, ReplayGainMode, TrackId,
};
use thiserror::Error;
use tracing::{debug, error, instrument, warn};
const SEEK_SETTLE_WINDOW: Duration = Duration::from_millis(1_000);
const TRACK_START_SETTLE_WINDOW: Duration = Duration::from_millis(10_000);
const STARTUP_SEEK_SETTLE_WINDOW: Duration = Duration::from_millis(10_000);
const SEEK_POSITION_TOLERANCE_MILLIS: u64 = 1_500;
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
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedPlaybackItem {
    pub track: PlaybackTrack,
    pub stream: StreamDescriptor,
}
impl PreparedPlaybackItem {
    pub fn new(track: PlaybackTrack, stream: StreamDescriptor) -> Self {
        Self { track, stream }
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
#[allow(clippy::large_enum_variant)]
pub enum PlaybackCommand {
    Play {
        track: PlaybackTrack,
        stream: StreamDescriptor,
        start_position_seconds: u32,
    },
    PlayPrepared {
        item: PreparedPlaybackItem,
        next: Option<PreparedPlaybackItem>,
        start_position_seconds: u32,
        settings: PlaybackSettings,
    },
    PrepareNext(Option<PreparedPlaybackItem>),
    UpdateSettings(PlaybackSettings),
    Resume,
    Pause,
    Stop,
    Seek(u32),
    SeekMillis(u64),
    SetVolume(f64),
    SetMuted(bool),
}
#[derive(Clone, Debug, PartialEq)]
pub enum PlaybackEvent {
    StateChanged(PlaybackState),
    PositionChanged { seconds: u32, millis: u64 },
    DurationChanged(u32),
    Buffering(u8),
    EndOfStream,
    PreparedTrackStarted(PlaybackTrack),
    VolumeChanged { volume: f64, muted: bool },
    Error(String),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioOutput {
    pub id: String,
    pub name: String,
}
pub fn available_audio_outputs() -> Vec<AudioOutput> {
    let _ = gst::init();
    let candidates = [
        ("autoaudiosink", "System default"),
        ("pipewiresink", "PipeWire"),
        ("pulsesink", "PulseAudio"),
        ("alsasink", "ALSA"),
        ("jackaudiosink", "JACK"),
        ("osxaudiosink", "macOS"),
        ("wasapisink", "WASAPI"),
        ("directsoundsink", "DirectSound"),
    ];
    candidates
        .into_iter()
        .filter(|(id, _)| gst::ElementFactory::find(id).is_some())
        .map(|(id, name)| AudioOutput {
            id: id.to_string(),
            name: name.to_string(),
        })
        .collect()
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
    next: Option<PreparedPlaybackItem>,
    settings: PlaybackSettings,
    position_seconds: u32,
    position_millis: u64,
    duration_seconds: u32,
    volume: f64,
    muted: bool,
    events: VecDeque<PlaybackEvent>,
}
impl FakePlaybackBackend {
    pub fn new() -> Self {
        let settings = PlaybackSettings::default();
        Self {
            state: PlaybackState::Stopped,
            current: None,
            next: None,
            volume: settings.volume,
            muted: settings.muted,
            settings,
            position_seconds: 0,
            position_millis: 0,
            duration_seconds: 0,
            events: VecDeque::new(),
        }
    }

    pub fn emit_end_of_stream_for_test(&mut self) {
        self.events.push_back(PlaybackEvent::EndOfStream);
    }

    pub fn emit_prepared_track_started_for_test(&mut self) {
        if let Some(next) = self.next.take() {
            self.current = Some(next.track.clone());
            self.duration_seconds = next.track.duration_seconds;
            self.position_seconds = 0;
            self.position_millis = 0;
            self.events
                .push_back(PlaybackEvent::PreparedTrackStarted(next.track));
        }
    }

    fn set_state(&mut self, state: PlaybackState) {
        self.state = state;
        self.events.push_back(PlaybackEvent::StateChanged(state));
    }

    fn play_item(&mut self, item: PreparedPlaybackItem, start_position_seconds: u32) {
        self.duration_seconds = item.track.duration_seconds;
        self.position_seconds = start_position_seconds.min(self.duration_seconds);
        self.position_millis = u64::from(self.position_seconds) * 1_000;
        self.current = Some(item.track);
        self.events
            .push_back(PlaybackEvent::DurationChanged(self.duration_seconds));
        self.events.push_back(position_event(self.position_millis));
        self.set_state(PlaybackState::Playing);
    }
}
impl PlaybackBackend for FakePlaybackBackend {
    fn send(&mut self, command: PlaybackCommand) -> Result<(), PlaybackError> {
        match command {
            PlaybackCommand::Play {
                track,
                stream,
                start_position_seconds,
            } => self.play_item(
                PreparedPlaybackItem::new(track, stream),
                start_position_seconds,
            ),
            PlaybackCommand::PlayPrepared {
                item,
                next,
                start_position_seconds,
                settings,
            } => {
                self.settings = settings;
                self.volume = self.settings.volume;
                self.muted = self.settings.muted;
                self.next = next;
                self.play_item(item, start_position_seconds);
            }
            PlaybackCommand::PrepareNext(next) => self.next = next,
            PlaybackCommand::UpdateSettings(settings) => {
                self.settings = settings;
                self.volume = self.settings.volume;
                self.muted = self.settings.muted;
                self.events.push_back(PlaybackEvent::VolumeChanged {
                    volume: self.volume,
                    muted: self.muted,
                });
            }
            PlaybackCommand::Resume => self.set_state(PlaybackState::Playing),
            PlaybackCommand::Pause => self.set_state(PlaybackState::Paused),
            PlaybackCommand::Stop => {
                self.position_seconds = 0;
                self.position_millis = 0;
                self.next = None;
                self.set_state(PlaybackState::Stopped);
                self.events.push_back(position_event(0));
            }
            PlaybackCommand::Seek(seconds) => {
                self.position_seconds = seconds.min(self.duration_seconds);
                self.position_millis = u64::from(self.position_seconds) * 1_000;
                self.events.push_back(position_event(self.position_millis));
            }
            PlaybackCommand::SeekMillis(millis) => {
                self.position_millis =
                    millis.min(u64::from(self.duration_seconds).saturating_mul(1_000));
                self.position_seconds = clock_seconds_from_millis(self.position_millis);
                self.events.push_back(position_event(self.position_millis));
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
        if self.inner.is_none()
            && !matches!(
                command,
                PlaybackCommand::Play { .. } | PlaybackCommand::PlayPrepared { .. }
            )
        {
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Slot {
    Primary,
    Secondary,
}
#[derive(Clone, Debug)]
struct CrossfadeState {
    from: Slot,
    to: Slot,
    started_at: Instant,
    duration: Duration,
    item: PreparedPlaybackItem,
}
#[derive(Clone, Debug)]
struct PendingSeek {
    target_millis: u64,
    expires_at: Instant,
    logical_state: PlaybackState,
    kind: PendingSeekKind,
    retry_on_async_done: bool,
    resume_after_seek: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingSeekKind {
    Interactive,
    Startup,
    TrackStart,
}
impl PendingSeek {
    fn interactive(target_millis: u64, logical_state: PlaybackState, now: Instant) -> Self {
        Self {
            target_millis,
            expires_at: now + SEEK_SETTLE_WINDOW,
            logical_state,
            kind: PendingSeekKind::Interactive,
            retry_on_async_done: false,
            resume_after_seek: false,
        }
    }

    fn startup(target_millis: u64, logical_state: PlaybackState, now: Instant) -> Self {
        Self {
            target_millis,
            expires_at: now + STARTUP_SEEK_SETTLE_WINDOW,
            logical_state,
            kind: PendingSeekKind::Startup,
            retry_on_async_done: true,
            resume_after_seek: true,
        }
    }

    fn track_start(now: Instant) -> Self {
        Self {
            target_millis: 0,
            expires_at: now + TRACK_START_SETTLE_WINDOW,
            logical_state: PlaybackState::Buffering,
            kind: PendingSeekKind::TrackStart,
            retry_on_async_done: false,
            resume_after_seek: false,
        }
    }

    fn accepts_position(&self, millis: u64, now: Instant) -> bool {
        now >= self.expires_at || seek_position_matches_target(self.target_millis, millis)
    }

    fn suppresses_state(&self, state: PlaybackState, now: Instant) -> bool {
        if now >= self.expires_at || state == self.logical_state {
            return false;
        }

        match self.kind {
            PendingSeekKind::Interactive => matches!(
                state,
                PlaybackState::Stopped
                    | PlaybackState::Buffering
                    | PlaybackState::Paused
                    | PlaybackState::Playing
            ),
            PendingSeekKind::Startup => matches!(
                state,
                PlaybackState::Stopped | PlaybackState::Paused | PlaybackState::Playing
            ),
            PendingSeekKind::TrackStart => {
                matches!(state, PlaybackState::Stopped | PlaybackState::Paused)
            }
        }
    }

    fn suppresses_buffering(&self, now: Instant) -> bool {
        now < self.expires_at
            && matches!(
                self.kind,
                PendingSeekKind::Interactive | PendingSeekKind::Startup
            )
    }
}
#[derive(Debug)]
struct SharedPlaybackState {
    settings: PlaybackSettings,
    current: Option<PreparedPlaybackItem>,
    next: Option<PreparedPlaybackItem>,
    gapless_pending: Option<PreparedPlaybackItem>,
    active: Slot,
    crossfade: Option<CrossfadeState>,
    volume: f64,
    muted: bool,
}
impl SharedPlaybackState {
    fn new() -> Self {
        let settings = PlaybackSettings::default();
        Self {
            current: None,
            next: None,
            gapless_pending: None,
            active: Slot::Primary,
            crossfade: None,
            volume: settings.volume,
            muted: settings.muted,
            settings,
        }
    }
}
struct PlayerPipeline {
    pipeline: gst::Element,
    bus: gst::Bus,
    _about_to_finish_id: glib::SignalHandlerId,
}
impl PlayerPipeline {
    fn new(
        slot: Slot,
        name: &str,
        shared: Arc<Mutex<SharedPlaybackState>>,
        events: Arc<Mutex<VecDeque<PlaybackEvent>>>,
    ) -> Result<Self, String> {
        let pipeline = make_playbin(name)?;
        let bus = pipeline
            .bus()
            .ok_or_else(|| "GStreamer playbin did not expose a bus".to_string())?;
        let fakesink = gst::ElementFactory::make("fakesink")
            .name(format!("{name}-video-sink"))
            .build()
            .map_err(|error| error.to_string())?;
        pipeline.set_property("video-sink", &fakesink);

        let pipeline_for_signal = pipeline.clone();
        let shared_for_signal = Arc::clone(&shared);
        let events_for_signal = Arc::clone(&events);
        let about_to_finish_id = pipeline.connect("about-to-finish", false, move |_| {
            handle_about_to_finish(&pipeline_for_signal, &shared_for_signal, &events_for_signal);
            None
        });

        let _ = slot;
        Ok(Self {
            pipeline,
            bus,
            _about_to_finish_id: about_to_finish_id,
        })
    }

    fn configure_audio(&self, settings: &PlaybackSettings) -> Result<(), String> {
        let sink = build_audio_sink(settings)?;
        self.pipeline.set_property("audio-sink", &sink);
        Ok(())
    }

    fn play_item(
        &self,
        item: &PreparedPlaybackItem,
        settings: &PlaybackSettings,
        volume: f64,
        muted: bool,
        start_position_seconds: u32,
    ) -> Result<(), String> {
        self.pipeline
            .set_state(gst::State::Ready)
            .map_err(|error| error.to_string())?;
        self.configure_audio(settings)?;
        self.pipeline.set_property("uri", item.stream.uri());
        self.set_output_volume(volume, muted);
        let startup_state = if start_position_seconds > 0 {
            gst::State::Paused
        } else {
            gst::State::Playing
        };
        self.pipeline
            .set_state(startup_state)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    fn set_output_volume(&self, volume: f64, muted: bool) {
        self.pipeline.set_property("volume", volume.clamp(0.0, 1.0));
        self.pipeline.set_property("mute", muted);
    }

    fn set_state(&self, state: gst::State) -> Result<(), String> {
        self.pipeline
            .set_state(state)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn stop(&self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }

    fn seek_millis(&self, millis: u64) -> Result<(), String> {
        self.pipeline
            .seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                gst::ClockTime::from_mseconds(millis),
            )
            .map_err(|error| error.to_string())
    }

    fn position(&self) -> Option<gst::ClockTime> {
        self.pipeline.query_position::<gst::ClockTime>()
    }

    fn duration(&self) -> Option<gst::ClockTime> {
        self.pipeline.query_duration::<gst::ClockTime>()
    }
}
struct GstEngine {
    primary: PlayerPipeline,
    secondary: PlayerPipeline,
    shared: Arc<Mutex<SharedPlaybackState>>,
    events: Arc<Mutex<VecDeque<PlaybackEvent>>>,
    last_position_tick: Instant,
    state: PlaybackState,
    pending_seek: Option<PendingSeek>,
}
