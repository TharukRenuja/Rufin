use super::*;
use gstreamer_audio as gst_audio;
use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};

const CLASSIC_EQUALIZER_FREQUENCIES: [f64; EQUALIZER_BAND_COUNT] = [
    60.0, 170.0, 310.0, 600.0, 1000.0, 3000.0, 6000.0, 12000.0, 14000.0, 16000.0,
];
const EQUALIZER_DUMMY_LOW_FREQUENCY: f64 = 20.0;
const EQUALIZER_DUMMY_HIGH_FREQUENCY: f64 = 20_000.0;
const GAPLESS_BUFFERING_IGNORE_REMAINING_MS: u64 = 5_000;
const VISUALIZER_CHANNEL_CAPACITY: usize = 2;
const VISUALIZER_COPY_FRAMES: usize = 4_096;
const VISUALIZER_FFT_SIZE: usize = 2_048;
const VISUALIZER_MIN_EMIT_INTERVAL: Duration = Duration::from_millis(33);
const VISUALIZER_NOISE_FLOOR_DB: f32 = -72.0;
const VISUALIZER_CEILING_DB: f32 = -6.0;
const STATUS_FADE_DURATION: Duration = Duration::from_millis(300);

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
                PlaybackCommand::WarmUp(_)
                    | PlaybackCommand::Play { .. }
                    | PlaybackCommand::PlayPrepared { .. }
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
pub(super) enum Slot {
    Primary,
    Secondary,
}
#[derive(Clone, Debug)]
pub(super) struct CrossfadeState {
    pub(super) from: Slot,
    pub(super) to: Slot,
    pub(super) started_at: Instant,
    pub(super) duration: Duration,
    pub(super) item: PreparedPlaybackItem,
}
#[derive(Clone, Debug)]
pub(super) struct PendingSeek {
    target_millis: u64,
    expires_at: Instant,
    logical_state: PlaybackState,
    kind: PendingSeekKind,
    pub(super) retry_on_async_done: bool,
    pub(super) resume_after_seek: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingSeekKind {
    Interactive,
    Startup,
    TrackStart,
}
impl PendingSeek {
    pub(super) fn interactive(
        target_millis: u64,
        logical_state: PlaybackState,
        now: Instant,
    ) -> Self {
        Self {
            target_millis,
            expires_at: now + SEEK_SETTLE_WINDOW,
            logical_state,
            kind: PendingSeekKind::Interactive,
            retry_on_async_done: false,
            resume_after_seek: false,
        }
    }

    pub(super) fn startup(target_millis: u64, logical_state: PlaybackState, now: Instant) -> Self {
        Self {
            target_millis,
            expires_at: now + STARTUP_SEEK_SETTLE_WINDOW,
            logical_state,
            kind: PendingSeekKind::Startup,
            retry_on_async_done: true,
            resume_after_seek: true,
        }
    }

    pub(super) fn track_start(now: Instant) -> Self {
        Self {
            target_millis: 0,
            expires_at: now + TRACK_START_SETTLE_WINDOW,
            logical_state: PlaybackState::Buffering,
            kind: PendingSeekKind::TrackStart,
            retry_on_async_done: false,
            resume_after_seek: false,
        }
    }

    pub(super) fn accepts_position(&self, millis: u64, now: Instant) -> bool {
        now >= self.expires_at || seek_position_matches_target(self.target_millis, millis)
    }

    pub(super) fn suppresses_state(&self, state: PlaybackState, now: Instant) -> bool {
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

    pub(super) fn suppresses_buffering(&self, now: Instant) -> bool {
        now < self.expires_at
            && matches!(
                self.kind,
                PendingSeekKind::Interactive | PendingSeekKind::Startup
            )
    }

    pub(super) fn is_track_start(&self) -> bool {
        self.kind == PendingSeekKind::TrackStart
    }

    pub(super) fn blocks_timing_query(&self) -> bool {
        self.kind == PendingSeekKind::TrackStart
    }
}
#[derive(Debug)]
pub(super) struct SharedPlaybackState {
    pub(super) settings: PlaybackSettings,
    pub(super) current: Option<PreparedPlaybackItem>,
    pub(super) next: Option<PreparedPlaybackItem>,
    pub(super) gapless_pending: Option<PreparedPlaybackItem>,
    pub(super) about_to_finish_pending: bool,
    pub(super) active: Slot,
    pub(super) crossfade: Option<CrossfadeState>,
    pub(super) volume: f64,
    pub(super) muted: bool,
    pub(super) visualizer_enabled: bool,
}
impl SharedPlaybackState {
    pub(super) fn new() -> Self {
        let settings = PlaybackSettings::default();
        Self {
            current: None,
            next: None,
            gapless_pending: None,
            about_to_finish_pending: false,
            active: Slot::Primary,
            crossfade: None,
            volume: settings.volume,
            muted: settings.muted,
            visualizer_enabled: false,
            settings,
        }
    }
}
pub(super) struct PreparedNextClear {
    pub(super) gapless_current: Option<(Slot, PreparedPlaybackItem)>,
    pub(super) crossfade: Option<CrossfadeState>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StatusFadeTarget {
    Pause,
    Playing,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct StatusFade {
    slot: Slot,
    target: StatusFadeTarget,
    started_at: Instant,
    duration: Duration,
    start_volume: f64,
    end_volume: f64,
    muted: bool,
}
impl StatusFade {
    pub(super) fn new(
        slot: Slot,
        target: StatusFadeTarget,
        start_volume: f64,
        end_volume: f64,
        muted: bool,
        now: Instant,
    ) -> Self {
        Self {
            slot,
            target,
            started_at: now,
            duration: STATUS_FADE_DURATION,
            start_volume: start_volume.clamp(0.0, 1.0),
            end_volume: end_volume.clamp(0.0, 1.0),
            muted,
        }
    }

    pub(super) fn volume_at(&self, now: Instant) -> f64 {
        let progress = (now.saturating_duration_since(self.started_at).as_secs_f64()
            / self.duration.as_secs_f64())
        .clamp(0.0, 1.0);
        self.start_volume + (self.end_volume - self.start_volume) * progress
    }

    fn is_finished(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started_at) >= self.duration
    }
}
#[derive(Clone)]
struct VisualizerTap {
    slot: Slot,
    sender: SyncSender<VisualizerFrame>,
    generation: Arc<AtomicU64>,
}
impl VisualizerTap {
    fn install(&self, pad: &gst::Pad) -> Option<gst::PadProbeId> {
        let tap = self.clone();
        pad.add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
            let Some(buffer) = info.buffer() else {
                return gst::PadProbeReturn::Ok;
            };
            let Some(samples) = copy_visualizer_samples(pad, buffer) else {
                return gst::PadProbeReturn::Ok;
            };
            let frame = VisualizerFrame {
                slot: tap.slot,
                generation: tap.generation.load(Ordering::Acquire),
                samples,
            };
            match tap.sender.try_send(frame) {
                Ok(()) | Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => return gst::PadProbeReturn::Remove,
            }
            gst::PadProbeReturn::Ok
        })
    }
}
struct VisualizerFrame {
    slot: Slot,
    generation: u64,
    samples: Vec<f32>,
}
pub(super) struct VisualizerAnalyzer {
    sender: SyncSender<VisualizerFrame>,
    generation: Arc<AtomicU64>,
}
impl VisualizerAnalyzer {
    pub(super) fn new(
        events: Arc<Mutex<VecDeque<PlaybackEvent>>>,
        shared: Arc<Mutex<SharedPlaybackState>>,
    ) -> Self {
        let (sender, receiver) = sync_channel(VISUALIZER_CHANNEL_CAPACITY);
        let generation = Arc::new(AtomicU64::new(1));
        let worker_generation = Arc::clone(&generation);
        let _ = thread::Builder::new()
            .name("rufin-visualizer-fft".to_string())
            .spawn(move || run_visualizer_worker(receiver, events, shared, worker_generation))
            .inspect_err(|error| warn!(%error, "failed to start visualizer FFT worker"));
        Self { sender, generation }
    }

    fn tap(&self, slot: Slot) -> VisualizerTap {
        VisualizerTap {
            slot,
            sender: self.sender.clone(),
            generation: Arc::clone(&self.generation),
        }
    }

    fn next_generation(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}
pub(super) struct PlayerPipeline {
    pub(super) pipeline: gst::Element,
    bus: gst::Bus,
    _about_to_finish_id: glib::SignalHandlerId,
    audio_sink_config: Option<AudioSinkConfig>,
    equalizer: Option<gst::Element>,
    visualizer_pad: Option<gst::Pad>,
    visualizer_probe: Option<gst::PadProbeId>,
}
#[derive(Clone, Debug, PartialEq)]
struct AudioSinkConfig {
    replay_gain: ReplayGainMode,
    audio_output: Option<String>,
}
impl AudioSinkConfig {
    fn new(settings: &PlaybackSettings) -> Self {
        Self {
            replay_gain: settings.replay_gain,
            audio_output: settings.audio_output.clone(),
        }
    }
}
struct AudioSink {
    root: gst::Element,
    equalizer: Option<gst::Element>,
    visualizer_pad: Option<gst::Pad>,
}
#[derive(Debug, PartialEq)]
pub(super) enum AboutToFinishAction {
    Preload(Box<PreparedPlaybackItem>),
    Ignore,
}
impl PlayerPipeline {
    pub(super) fn new(name: &str, shared: Arc<Mutex<SharedPlaybackState>>) -> Result<Self, String> {
        let pipeline = make_playbin(name)?;
        let bus = pipeline
            .bus()
            .ok_or_else(|| "GStreamer playbin did not expose a bus".to_string())?;
        let fakesink = gst::ElementFactory::make("fakesink")
            .name(format!("{name}-video-sink"))
            .build()
            .map_err(|error| error.to_string())?;
        configure_playbin_for_audio(&pipeline);
        pipeline.set_property("video-sink", &fakesink);

        let pipeline_for_signal = pipeline.clone();
        let shared_for_signal = Arc::clone(&shared);
        let about_to_finish_id = pipeline.connect("about-to-finish", false, move |_| {
            handle_about_to_finish(&pipeline_for_signal, &shared_for_signal);
            None
        });

        Ok(Self {
            pipeline,
            bus,
            _about_to_finish_id: about_to_finish_id,
            audio_sink_config: None,
            equalizer: None,
            visualizer_pad: None,
            visualizer_probe: None,
        })
    }

    fn configure_audio(&mut self, settings: &PlaybackSettings) -> Result<(), String> {
        let config = AudioSinkConfig::new(settings);
        if self.audio_sink_config.as_ref() == Some(&config) {
            self.apply_equalizer(&settings.equalizer);
            return Ok(());
        }
        self.clear_visualizer_tap();
        let sink = build_audio_sink(settings)?;
        self.pipeline.set_property("audio-sink", &sink.root);
        self.equalizer = sink.equalizer;
        self.visualizer_pad = sink.visualizer_pad;
        self.audio_sink_config = Some(config);
        self.apply_equalizer(&settings.equalizer);
        Ok(())
    }

    fn apply_equalizer(&self, settings: &EqualizerSettings) {
        if let Some(equalizer) = self.equalizer.as_ref() {
            configure_equalizer(equalizer, settings);
        }
    }

    fn set_visualizer_tap(&mut self, tap: Option<VisualizerTap>) {
        self.clear_visualizer_tap();
        if let (Some(tap), Some(pad)) = (tap, self.visualizer_pad.as_ref()) {
            self.visualizer_probe = tap.install(pad);
        }
    }

    fn clear_visualizer_tap(&mut self) {
        if let (Some(pad), Some(probe)) =
            (self.visualizer_pad.as_ref(), self.visualizer_probe.take())
        {
            pad.remove_probe(probe);
        } else {
            self.visualizer_probe = None;
        }
    }

    fn play_item(
        &mut self,
        item: &PreparedPlaybackItem,
        settings: &PlaybackSettings,
        volume: f64,
        muted: bool,
        start_position_millis: u64,
    ) -> Result<(), String> {
        self.pipeline
            .set_state(gst::State::Ready)
            .map_err(|error| error.to_string())?;
        self.configure_audio(settings)?;
        self.pipeline.set_property("uri", item.stream.uri());
        self.set_output_volume(volume, muted);
        let startup_state = if start_position_millis > 0 {
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

    fn restart_from_beginning(&self, target_state: gst::State) -> Result<(), String> {
        self.pipeline
            .set_state(gst::State::Ready)
            .map_err(|error| error.to_string())?;
        self.pipeline
            .set_state(target_state)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn reset_uri(
        &self,
        item: &PreparedPlaybackItem,
        target_state: gst::State,
    ) -> Result<(), String> {
        self.pipeline
            .set_state(gst::State::Ready)
            .map_err(|error| error.to_string())?;
        self.pipeline.set_property("uri", item.stream.uri());
        self.pipeline
            .set_state(target_state)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn seek_millis(&self, millis: u64) -> Result<(), String> {
        self.pipeline
            .seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
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
pub(super) struct GstEngine {
    pub(super) primary: PlayerPipeline,
    pub(super) secondary: PlayerPipeline,
    pub(super) shared: Arc<Mutex<SharedPlaybackState>>,
    pub(super) events: Arc<Mutex<VecDeque<PlaybackEvent>>>,
    pub(super) visualizer: VisualizerAnalyzer,
    pub(super) last_position_tick: Instant,
    pub(super) state: PlaybackState,
    pub(super) pending_seek: Option<PendingSeek>,
    pub(super) status_fade: Option<StatusFade>,
    pub(super) play_command_started_at: Option<Instant>,
}
impl GstEngine {
    fn new(events: Arc<Mutex<VecDeque<PlaybackEvent>>>) -> Result<Self, String> {
        let shared = Arc::new(Mutex::new(SharedPlaybackState::new()));
        let primary = PlayerPipeline::new("rufin-primary-player", Arc::clone(&shared))?;
        let secondary = PlayerPipeline::new("rufin-secondary-player", Arc::clone(&shared))?;
        let visualizer = VisualizerAnalyzer::new(Arc::clone(&events), Arc::clone(&shared));
        Ok(Self {
            primary,
            secondary,
            shared,
            events,
            visualizer,
            last_position_tick: Instant::now(),
            state: PlaybackState::Stopped,
            pending_seek: None,
            status_fade: None,
            play_command_started_at: None,
        })
    }

    fn handle_command(&mut self, command: PlaybackCommand) {
        let result = match command {
            PlaybackCommand::WarmUp(mut settings) => {
                settings.sanitize();
                self.warm_up(&settings)
            }
            PlaybackCommand::Play {
                track,
                stream,
                start_position_seconds,
            } => {
                let settings = self.settings();
                self.play_prepared(
                    PreparedPlaybackItem::new(track, stream),
                    None,
                    start_position_seconds,
                    settings,
                )
            }
            PlaybackCommand::PlayPrepared {
                item,
                next,
                start_position_seconds,
                settings,
            } => self.play_prepared(item, next, start_position_seconds, settings),
            PlaybackCommand::PrepareNext(next) => self.prepare_next(next),
            PlaybackCommand::UpdateSettings(mut settings) => {
                self.cancel_status_fade();
                settings.sanitize();
                (|| -> Result<(), String> {
                    let visualizer_enabled = self.visualizer_enabled();
                    if let Ok(mut shared) = self.shared.lock() {
                        shared.settings = settings.clone();
                        shared.volume = shared.settings.volume;
                        shared.muted = shared.settings.muted;
                    }
                    self.primary.configure_audio(&settings)?;
                    self.secondary.configure_audio(&settings)?;
                    self.sync_visualizer_taps(visualizer_enabled);
                    let (volume, muted) = self.output_state();
                    self.primary.set_output_volume(volume, muted);
                    self.secondary.set_output_volume(volume, muted);
                    push_event(&self.events, PlaybackEvent::VolumeChanged { volume, muted });
                    Ok(())
                })()
            }
            PlaybackCommand::SetVisualizerEnabled(enabled) => self.set_visualizer_enabled(enabled),
            PlaybackCommand::Resume => self.start_status_resume(),
            PlaybackCommand::Pause => self.start_status_pause(),
            PlaybackCommand::Silence => {
                self.primary.set_output_volume(0.0, true);
                self.secondary.set_output_volume(0.0, true);
                Ok(())
            }
            PlaybackCommand::Stop => {
                self.cancel_status_fade();
                self.pending_seek = None;
                self.primary.stop();
                self.secondary.stop();
                self.visualizer.next_generation();
                if let Ok(mut shared) = self.shared.lock() {
                    shared.current = None;
                    shared.next = None;
                    shared.gapless_pending = None;
                    shared.about_to_finish_pending = false;
                    shared.crossfade = None;
                    shared.active = Slot::Primary;
                }
                self.primary.set_visualizer_tap(None);
                self.secondary.set_visualizer_tap(None);
                push_event(&self.events, position_event(0));
                self.push_state(PlaybackState::Stopped);
                Ok(())
            }
            PlaybackCommand::Seek(seconds) => self.start_seek(u64::from(seconds) * 1_000),
            PlaybackCommand::SeekMillis(millis) => self.start_seek(millis),
            PlaybackCommand::SetVolume(volume) => {
                self.cancel_status_fade();
                let volume = volume.clamp(0.0, 1.0);
                let muted = self.set_volume(volume);
                push_event(&self.events, PlaybackEvent::VolumeChanged { volume, muted });
                Ok(())
            }
            PlaybackCommand::SetMuted(muted) => {
                self.cancel_status_fade();
                let volume = self.set_muted(muted);
                push_event(&self.events, PlaybackEvent::VolumeChanged { volume, muted });
                Ok(())
            }
        };

        if let Err(error) = result {
            push_event(&self.events, PlaybackEvent::Error(error));
        }
    }

    fn warm_up(&mut self, settings: &PlaybackSettings) -> Result<(), String> {
        let visualizer_enabled = self.visualizer_enabled();
        self.primary.configure_audio(settings)?;
        self.secondary.configure_audio(settings)?;
        self.sync_visualizer_taps(visualizer_enabled);
        if let Ok(mut shared) = self.shared.lock() {
            shared.settings = settings.clone();
            shared.volume = settings.volume;
            shared.muted = settings.muted;
        }
        self.primary
            .set_output_volume(settings.volume, settings.muted);
        self.secondary
            .set_output_volume(settings.volume, settings.muted);
        Ok(())
    }

    fn play_prepared(
        &mut self,
        item: PreparedPlaybackItem,
        next: Option<PreparedPlaybackItem>,
        start_position_seconds: u32,
        mut settings: PlaybackSettings,
    ) -> Result<(), String> {
        let command_started_at = Instant::now();
        self.play_command_started_at = Some(command_started_at);
        self.cancel_status_fade();
        self.pending_seek = None;
        settings.sanitize();
        self.secondary.stop();
        self.secondary.set_visualizer_tap(None);
        let volume = settings.volume;
        let muted = settings.muted;
        let start_millis = item
            .stream
            .source_start_millis()
            .saturating_add(u64::from(start_position_seconds) * 1_000);
        let mut visualizer_enabled = false;
        if let Ok(mut shared) = self.shared.lock() {
            shared.settings = settings.clone();
            shared.current = Some(item.clone());
            shared.next = next;
            shared.gapless_pending = None;
            shared.about_to_finish_pending = false;
            shared.crossfade = None;
            shared.active = Slot::Primary;
            shared.volume = volume;
            shared.muted = muted;
            visualizer_enabled = shared.visualizer_enabled;
        }
        self.visualizer.next_generation();
        if visualizer_enabled {
            push_event(&self.events, PlaybackEvent::Visualizer(Vec::new()));
        }
        self.push_state(PlaybackState::Buffering);
        let pipeline_started_at = Instant::now();
        self.primary
            .play_item(&item, &settings, volume, muted, start_millis)?;
        let primary_tap = self.visualizer_tap(Slot::Primary, visualizer_enabled);
        self.primary.set_visualizer_tap(primary_tap);
        info!(
            track_id = %item.track.id.as_str(),
            elapsed_ms = command_started_at.elapsed().as_millis(),
            pipeline_ms = pipeline_started_at.elapsed().as_millis(),
            "queued GStreamer playback item"
        );
        if start_millis > 0 {
            self.start_playback_seek(start_millis);
        } else {
            self.pending_seek = Some(PendingSeek::track_start(Instant::now()));
        }
        if item.stream.source_end_millis().is_some() {
            self.push_duration(item.track.duration_seconds);
        }
        Ok(())
    }

    fn prepare_next(&mut self, next: Option<PreparedPlaybackItem>) -> Result<(), String> {
        let Some(next) = next else {
            self.clear_prepared_next();
            return Ok(());
        };

        let mut late_preload = None;
        if let Ok(mut shared) = self.shared.lock() {
            shared.next = Some(next.clone());
            if shared.about_to_finish_pending && gapless_preload_should_run(&shared, &next) {
                if gapless_preload_source_is_supported(next.stream.uri()) {
                    let item = shared
                        .next
                        .take()
                        .expect("late gapless preload just stored next item");
                    shared.gapless_pending = Some(item.clone());
                    shared.about_to_finish_pending = false;
                    late_preload = Some(item);
                }
                shared.about_to_finish_pending = false;
            }
            if shared.about_to_finish_pending && !gapless_preload_should_run(&shared, &next) {
                shared.about_to_finish_pending = false;
            }
        }
        if let Some(item) = late_preload {
            info!(
                track_id = %item.track.id.as_str(),
                uri = %item.stream.redacted_uri(),
                "preloading late gapless next stream"
            );
            self.active_pipeline()
                .pipeline
                .set_property("uri", item.stream.uri());
        }
        Ok(())
    }

    fn clear_prepared_next(&mut self) {
        let clear = self
            .shared
            .lock()
            .map(|mut shared| clear_prepared_next_state(&mut shared))
            .unwrap_or_else(|_| PreparedNextClear {
                gapless_current: None,
                crossfade: None,
            });
        if let Some((slot, current)) = clear.gapless_current {
            debug!(
                track_id = %current.track.id.as_str(),
                "cleared pending gapless next stream"
            );
            self.pipeline_for_slot(slot)
                .pipeline
                .set_property("uri", current.stream.uri());
        }
        if let Some(crossfade) = clear.crossfade {
            debug!(
                track_id = %crossfade.item.track.id.as_str(),
                "cleared pending crossfade next stream"
            );
            self.pipeline_for_slot(crossfade.to).stop();
            let (volume, muted) = self.output_state();
            self.pipeline_for_slot(crossfade.from)
                .set_output_volume(volume, muted);
        }
    }

    fn start_seek(&mut self, millis: u64) -> Result<(), String> {
        self.cancel_status_fade();
        self.finish_crossfade_for_seek();
        self.cancel_gapless_preload_for_seek()?;
        if millis == 0 {
            let target_state = match self.state {
                PlaybackState::Paused | PlaybackState::Stopped => gst::State::Paused,
                PlaybackState::Buffering | PlaybackState::Playing => gst::State::Playing,
            };
            self.active_pipeline()
                .restart_from_beginning(target_state)?;
            self.pending_seek = Some(PendingSeek::track_start(Instant::now()));
            push_event(&self.events, position_event(0));
            return Ok(());
        }
        if let Err(error) = self.active_pipeline().seek_millis(millis) {
            warn!(
                %error,
                target_millis = millis,
                "GStreamer seek request failed"
            );
            if let Some(position) = self.active_pipeline().position() {
                self.push_position(clock_millis(position));
            }
            return Ok(());
        }
        self.pending_seek = Some(PendingSeek::interactive(millis, self.state, Instant::now()));
        Ok(())
    }

    fn start_playback_seek(&mut self, millis: u64) {
        debug!(
            target_millis = millis,
            "deferring startup seek until GStreamer preroll completes"
        );
        self.pending_seek = Some(PendingSeek::startup(millis, self.state, Instant::now()));
    }

    fn cancel_gapless_preload_for_seek(&mut self) -> Result<(), String> {
        let reset = self.shared.lock().ok().and_then(|mut shared| {
            cancel_gapless_pending(&mut shared).map(|(current, _pending)| current)
        });
        let Some(current) = reset else {
            return Ok(());
        };
        let target_state = match self.state {
            PlaybackState::Paused | PlaybackState::Stopped => gst::State::Paused,
            PlaybackState::Buffering | PlaybackState::Playing => gst::State::Playing,
        };
        self.active_pipeline().reset_uri(&current, target_state)
    }

    fn poll_bus(&mut self) {
        while let Some(message) = self.primary.bus.pop() {
            self.handle_message(Slot::Primary, &message);
        }
        while let Some(message) = self.secondary.bus.pop() {
            self.handle_message(Slot::Secondary, &message);
        }
    }

    fn handle_message(&mut self, slot: Slot, message: &gst::Message) {
        use gst::MessageView;

        match message.view() {
            MessageView::StateChanged(state)
                if self.message_source_is_pipeline(slot, message) && self.is_active_slot(slot) =>
            {
                let playback_state = match state.current() {
                    gst::State::Null | gst::State::Ready => PlaybackState::Stopped,
                    gst::State::Paused => PlaybackState::Paused,
                    gst::State::Playing => PlaybackState::Playing,
                    _ => PlaybackState::Buffering,
                };
                self.handle_state_changed(playback_state);
            }
            MessageView::AsyncDone(_) if self.is_active_slot(slot) => {
                self.handle_async_done();
            }
            MessageView::StreamStart(_) if self.is_active_slot(slot) => {
                self.handle_stream_start();
            }
            MessageView::DurationChanged(_) if self.is_active_slot(slot) => {
                if self.pending_seek.is_none()
                    && let Some(duration) = self.active_pipeline().duration()
                {
                    self.push_duration(clock_seconds(duration));
                }
            }
            MessageView::Buffering(buffering) if self.is_active_slot(slot) => {
                self.handle_buffering(buffering.percent().min(100) as u8);
            }
            MessageView::Eos(_) => self.handle_eos(slot),
            MessageView::Error(error_message) => {
                let error = error_message.error().to_string();
                let source = message
                    .src()
                    .map(|source| source.name().to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let active_slot = self.active_slot();
                let relevant = self.error_is_relevant_slot(slot);
                error!(
                    message = %error,
                    %source,
                    ?slot,
                    ?active_slot,
                    relevant,
                    "GStreamer playback error"
                );
                if relevant && self.handle_transition_error(slot, &error) {
                    return;
                }
                if relevant {
                    self.stop_after_playback_error();
                    push_event(&self.events, PlaybackEvent::Error(error));
                    self.push_state(PlaybackState::Stopped);
                }
            }
            _ => {}
        }
    }

    fn handle_transition_error(&mut self, slot: Slot, error: &str) -> bool {
        self.handle_gapless_preload_error(slot, error)
            || self.handle_crossfade_next_error(slot, error)
    }

    fn handle_gapless_preload_error(&mut self, slot: Slot, error: &str) -> bool {
        let reset = self.shared.lock().ok().and_then(|mut shared| {
            if shared.active != slot {
                return None;
            }
            cancel_gapless_pending(&mut shared)
        });
        let Some((current, pending)) = reset else {
            return false;
        };
        warn!(
            track_id = %pending.track.id.as_str(),
            error = %error,
            "gapless next stream failed before commit"
        );
        let target_state = match self.state {
            PlaybackState::Paused | PlaybackState::Stopped => gst::State::Paused,
            PlaybackState::Buffering | PlaybackState::Playing => gst::State::Playing,
        };
        if let Err(reset_error) = self.active_pipeline().reset_uri(&current, target_state) {
            warn!(
                %reset_error,
                track_id = %current.track.id.as_str(),
                "failed to restore current stream after gapless preload error"
            );
            return false;
        }
        push_event(&self.events, PlaybackEvent::EndOfStream);
        true
    }

    fn handle_crossfade_next_error(&mut self, slot: Slot, error: &str) -> bool {
        let crossfade = self
            .shared
            .lock()
            .ok()
            .and_then(|mut shared| cancel_crossfade_next(&mut shared, slot));
        let Some(crossfade) = crossfade else {
            return false;
        };
        warn!(
            track_id = %crossfade.item.track.id.as_str(),
            error = %error,
            "crossfade next stream failed before commit"
        );
        self.pipeline_for_slot(crossfade.to).stop();
        let (volume, muted) = self.output_state();
        self.pipeline_for_slot(crossfade.from)
            .set_output_volume(volume, muted);
        true
    }

    fn handle_stream_start(&mut self) {
        let started = self.shared.lock().ok().and_then(|mut shared| {
            let item = shared.gapless_pending.take()?;
            shared.current = Some(item.clone());
            shared.about_to_finish_pending = false;
            Some(item.track)
        });
        self.handle_stream_started_track(started);
    }

    pub(super) fn handle_stream_started_track(&mut self, started: Option<PlaybackTrack>) {
        let Some(track) = started else {
            return;
        };
        info!(
            track_id = %track.id.as_str(),
            "gapless stream started"
        );
        let track_id = track.id.clone();
        self.pending_seek = None;
        self.last_position_tick = Instant::now();
        push_event(
            &self.events,
            PlaybackEvent::PreparedTrackStarted(track.clone()),
        );
        push_event(
            &self.events,
            position_event_for_track(0, Some(track_id.clone())),
        );
        push_event(
            &self.events,
            PlaybackEvent::DurationChanged {
                track_id: Some(track_id),
                seconds: track.duration_seconds,
            },
        );
    }

    pub(super) fn handle_state_changed(&mut self, state: PlaybackState) {
        let now = Instant::now();
        if self
            .pending_seek
            .as_ref()
            .is_some_and(|pending| pending.suppresses_state(state, now))
        {
            return;
        }
        if self
            .pending_seek
            .as_ref()
            .is_some_and(|pending| now >= pending.expires_at)
        {
            self.pending_seek = None;
        }
        if state == PlaybackState::Playing
            && self
                .pending_seek
                .as_ref()
                .is_some_and(PendingSeek::is_track_start)
        {
            self.pending_seek = None;
        }
        self.push_state(state);
    }

    fn handle_buffering(&mut self, percent: u8) {
        if percent < 100 && self.gapless_preload_near_end() {
            debug!(
                percent,
                "ignoring buffering while gapless handoff is pending near end"
            );
            return;
        }
        let now = Instant::now();
        if self
            .pending_seek
            .as_ref()
            .is_some_and(|pending| pending.suppresses_buffering(now))
        {
            return;
        }
        if self
            .pending_seek
            .as_ref()
            .is_some_and(|pending| now >= pending.expires_at)
        {
            self.pending_seek = None;
        }
        self.state = PlaybackState::Buffering;
        push_event(&self.events, PlaybackEvent::Buffering(percent));
    }

    fn gapless_preload_near_end(&self) -> bool {
        if !self
            .shared
            .lock()
            .map(|shared| shared.gapless_pending.is_some())
            .unwrap_or(false)
        {
            return false;
        }
        let Some(position) = self.active_pipeline().position() else {
            return false;
        };
        let Some(duration) = self.active_pipeline().duration() else {
            return false;
        };
        let position_ms = clock_millis(position);
        let duration_ms = clock_millis(duration);
        duration_ms > 0
            && position_ms > 0
            && duration_ms.saturating_sub(position_ms) < GAPLESS_BUFFERING_IGNORE_REMAINING_MS
    }

    fn handle_async_done(&mut self) {
        if self.retry_pending_seek() {
            return;
        }
        if self
            .pending_seek
            .as_ref()
            .is_some_and(PendingSeek::blocks_timing_query)
        {
            return;
        }
        if let Some(position) = self.active_pipeline().position() {
            self.push_position(clock_millis(position));
        }
    }

    fn retry_pending_seek(&mut self) -> bool {
        let Some(pending) = self.pending_seek.as_mut() else {
            return false;
        };
        if !pending.retry_on_async_done {
            return false;
        }
        let now = Instant::now();
        if now >= pending.expires_at {
            let resume_after_seek = pending.resume_after_seek;
            self.pending_seek = None;
            if resume_after_seek {
                self.resume_after_startup_seek();
            }
            return false;
        }
        let target_millis = pending.target_millis;
        pending.retry_on_async_done = false;
        pending.expires_at = now + STARTUP_SEEK_SETTLE_WINDOW;
        let resume_after_seek = pending.resume_after_seek;
        let seek_result = self.active_pipeline().seek_millis(target_millis);
        if let Err(error) = seek_result {
            warn!(
                %error,
                target_millis,
                "deferred startup seek failed; resuming from current position"
            );
            self.pending_seek = None;
            if resume_after_seek {
                self.resume_after_startup_seek();
            }
            if let Some(position) = self.active_pipeline().position() {
                self.push_position(clock_millis(position));
            }
        } else {
            if let Some(pending) = self.pending_seek.as_mut() {
                pending.resume_after_seek = false;
            }
            debug!(target_millis, "deferred startup seek started");
            if resume_after_seek {
                self.resume_after_startup_seek();
            }
        }
        true
    }

    fn resume_after_startup_seek(&mut self) {
        if self
            .active_pipeline()
            .set_state(gst::State::Playing)
            .is_ok()
        {
            self.push_state(PlaybackState::Playing);
        }
    }

    fn push_state(&mut self, state: PlaybackState) {
        if state == PlaybackState::Playing
            && let Some(started_at) = self.play_command_started_at.take()
        {
            let track_id = self
                .shared
                .lock()
                .ok()
                .and_then(|shared| shared.current.as_ref().map(|item| item.track.id.clone()));
            info!(
                track_id = track_id.as_ref().map(|id| id.as_str()).unwrap_or("unknown"),
                elapsed_ms = started_at.elapsed().as_millis(),
                "GStreamer playback reached playing"
            );
        }
        self.state = state;
        push_event(&self.events, PlaybackEvent::StateChanged(state));
    }

    fn handle_eos(&mut self, slot: Slot) {
        if self.finish_crossfade_if_needed(slot) {
            return;
        }
        if self.is_active_slot(slot) {
            let track_id = self.timing_track_id();
            info!(
                track_id = track_id.as_ref().map(|id| id.as_str()).unwrap_or("unknown"),
                "playback reached end of stream"
            );
            push_event(&self.events, PlaybackEvent::EndOfStream);
        }
    }

    fn start_status_pause(&mut self) -> Result<(), String> {
        self.cancel_status_fade();
        self.pending_seek = None;
        let (volume, muted, enabled) = self.status_fade_settings();
        if !enabled || muted || volume <= 0.0 {
            return self
                .active_pipeline()
                .set_state(gst::State::Paused)
                .map(|_| {
                    self.push_state(PlaybackState::Paused);
                });
        }
        let slot = self.active_slot();
        self.status_fade = Some(StatusFade::new(
            slot,
            StatusFadeTarget::Pause,
            volume,
            0.0,
            muted,
            Instant::now(),
        ));
        self.pipeline_for_slot(slot)
            .set_output_volume(volume, muted);
        Ok(())
    }

    fn start_status_resume(&mut self) -> Result<(), String> {
        self.cancel_status_fade();
        self.pending_seek = None;
        let (volume, muted, enabled) = self.status_fade_settings();
        if !enabled || muted || volume <= 0.0 {
            return self
                .active_pipeline()
                .set_state(gst::State::Playing)
                .map(|_| {
                    self.push_state(PlaybackState::Playing);
                });
        }
        let slot = self.active_slot();
        self.pipeline_for_slot(slot).set_output_volume(0.0, muted);
        self.pipeline_for_slot(slot)
            .set_state(gst::State::Playing)
            .map(|_| {
                self.push_state(PlaybackState::Playing);
                self.status_fade = Some(StatusFade::new(
                    slot,
                    StatusFadeTarget::Playing,
                    0.0,
                    volume,
                    muted,
                    Instant::now(),
                ));
            })
    }

    fn update_status_fade(&mut self) {
        let Some(fade) = self.status_fade else {
            return;
        };
        let now = Instant::now();
        self.pipeline_for_slot(fade.slot)
            .set_output_volume(fade.volume_at(now), fade.muted);
        if !fade.is_finished(now) {
            return;
        }

        self.status_fade = None;
        match fade.target {
            StatusFadeTarget::Pause => {
                if let Err(error) = self
                    .pipeline_for_slot(fade.slot)
                    .set_state(gst::State::Paused)
                {
                    push_event(&self.events, PlaybackEvent::Error(error));
                    return;
                }
                self.push_state(PlaybackState::Paused);
                let (volume, muted) = self.output_state();
                self.pipeline_for_slot(fade.slot)
                    .set_output_volume(volume, muted);
            }
            StatusFadeTarget::Playing => {
                let (volume, muted) = self.output_state();
                self.pipeline_for_slot(fade.slot)
                    .set_output_volume(volume, muted);
            }
        }
    }

    fn cancel_status_fade(&mut self) {
        if self.status_fade.take().is_some() {
            let (volume, muted) = self.output_state();
            self.active_pipeline().set_output_volume(volume, muted);
        }
    }

    fn status_fade_settings(&self) -> (f64, bool, bool) {
        self.shared
            .lock()
            .map(|shared| {
                (
                    shared.volume,
                    shared.muted,
                    shared.settings.audio_fade_on_status_change,
                )
            })
            .unwrap_or((1.0, false, true))
    }

    fn tick(&mut self) {
        self.update_status_fade();
        if self.status_fade.is_some() {
            return;
        }
        self.maybe_start_crossfade();
        self.update_crossfade();

        if self.last_position_tick.elapsed() >= Duration::from_millis(500) {
            self.last_position_tick = Instant::now();
            if self
                .pending_seek
                .as_ref()
                .is_some_and(PendingSeek::blocks_timing_query)
            {
                return;
            }
            if let Some(position) = self.active_pipeline().position() {
                self.push_position(clock_millis(position));
            }
            if self.pending_seek.is_none()
                && !self.active_stream_is_windowed()
                && let Some(duration) = self.active_pipeline().duration()
            {
                self.push_duration(clock_seconds(duration));
            }
        }
    }

    pub(super) fn push_position(&mut self, millis: u64) {
        let now = Instant::now();
        if let Some(pending) = self.pending_seek.as_ref() {
            if !pending.accepts_position(millis, now) {
                return;
            }
            let resume_after_seek = pending.resume_after_seek;
            self.pending_seek = None;
            if resume_after_seek {
                self.resume_after_startup_seek();
            }
        }
        let millis = if let Some((start_millis, end_millis)) = self.active_source_window() {
            if millis >= end_millis {
                push_event(&self.events, PlaybackEvent::EndOfStream);
                return;
            }
            millis.saturating_sub(start_millis)
        } else {
            millis
        };
        push_event(
            &self.events,
            position_event_for_track(millis, self.timing_track_id()),
        );
    }

    pub(super) fn push_duration(&self, seconds: u32) {
        push_event(
            &self.events,
            PlaybackEvent::DurationChanged {
                track_id: self.timing_track_id(),
                seconds,
            },
        );
    }

    fn active_stream_is_windowed(&self) -> bool {
        self.active_source_window().is_some()
    }

    fn active_source_window(&self) -> Option<(u64, u64)> {
        let shared = self.shared.lock().ok()?;
        let current = shared.current.as_ref()?;
        let end = current.stream.source_end_millis()?;
        Some((current.stream.source_start_millis(), end))
    }

    fn timing_track_id(&self) -> Option<TrackId> {
        self.shared
            .lock()
            .ok()
            .and_then(|shared| shared.current.as_ref().map(|item| item.track.id.clone()))
    }

    fn maybe_start_crossfade(&mut self) {
        if self.pending_seek.is_some() {
            return;
        }
        let request = self.shared.lock().ok().and_then(|shared| {
            if shared.settings.transition_mode != PlaybackTransitionMode::Crossfade
                || shared.crossfade.is_some()
            {
                return None;
            }
            let next = shared.next.clone()?;
            if same_album_crossfade_is_skipped(&shared.settings, shared.current.as_ref(), &next) {
                return None;
            }
            let crossfade_ms = u64::from(shared.settings.crossfade_seconds) * 1_000;
            Some((
                next,
                shared.settings.clone(),
                shared.active,
                inactive_slot(shared.active),
                shared.volume,
                shared.muted,
                crossfade_ms,
            ))
        });

        let Some((next, settings, from, to, volume, muted, crossfade_ms)) = request else {
            return;
        };
        let Some(position) = self.active_pipeline().position() else {
            return;
        };
        let Some(duration) = self.active_pipeline().duration() else {
            return;
        };
        let position_ms = clock_millis(position);
        let duration_ms = clock_millis(duration);
        if duration_ms == 0
            || position_ms >= duration_ms
            || duration_ms.saturating_sub(position_ms) > crossfade_ms
            || duration_ms <= crossfade_ms + 1_000
        {
            return;
        }
        let visualizer_enabled = self.visualizer_enabled();
        let tap = self.visualizer_tap(to, visualizer_enabled);
        let inactive = self.pipeline_for_slot_mut(to);
        if let Err(error) = inactive.play_item(&next, &settings, 0.0, muted, 0) {
            push_event(&self.events, PlaybackEvent::Error(error));
            return;
        }
        inactive.set_visualizer_tap(tap);

        if let Ok(mut shared) = self.shared.lock() {
            shared.next = None;
            shared.crossfade = Some(CrossfadeState {
                from,
                to,
                started_at: Instant::now(),
                duration: Duration::from_millis(crossfade_ms),
                item: next.clone(),
            });
        }
        self.pipeline_for_slot(from)
            .set_output_volume(volume, muted);
        push_event(
            &self.events,
            PlaybackEvent::PreparedTrackStarted(next.track.clone()),
        );
        push_event(
            &self.events,
            position_event_for_track(0, Some(next.track.id.clone())),
        );
        push_event(
            &self.events,
            PlaybackEvent::DurationChanged {
                track_id: Some(next.track.id),
                seconds: next.track.duration_seconds,
            },
        );
    }

    fn update_crossfade(&mut self) {
        let Some(crossfade) = self
            .shared
            .lock()
            .ok()
            .and_then(|shared| shared.crossfade.clone())
        else {
            return;
        };
        let elapsed = crossfade.started_at.elapsed();
        let progress = (elapsed.as_secs_f64() / crossfade.duration.as_secs_f64()).clamp(0.0, 1.0);
        let (volume, muted) = self.output_state();
        let from_volume = (progress * FRAC_PI_2).cos() * volume;
        let to_volume = (progress * FRAC_PI_2).sin() * volume;
        self.pipeline_for_slot(crossfade.from)
            .set_output_volume(from_volume, muted);
        self.pipeline_for_slot(crossfade.to)
            .set_output_volume(to_volume, muted);
        if progress >= 1.0 {
            self.finish_crossfade(crossfade);
        }
    }

    fn finish_crossfade_if_needed(&mut self, eos_slot: Slot) -> bool {
        let crossfade = self
            .shared
            .lock()
            .ok()
            .and_then(|shared| shared.crossfade.clone());
        if let Some(crossfade) = crossfade
            && crossfade.from == eos_slot
        {
            self.finish_crossfade(crossfade);
            return true;
        }
        false
    }

    pub(super) fn finish_crossfade_for_seek(&mut self) {
        let crossfade = self
            .shared
            .lock()
            .ok()
            .and_then(|shared| shared.crossfade.clone());
        if let Some(crossfade) = crossfade {
            self.finish_crossfade(crossfade);
        }
    }

    fn finish_crossfade(&mut self, crossfade: CrossfadeState) {
        self.pending_seek = None;
        self.pipeline_for_slot(crossfade.from).stop();
        let (volume, muted) = self.output_state();
        self.pipeline_for_slot(crossfade.to)
            .set_output_volume(volume, muted);
        if let Ok(mut shared) = self.shared.lock() {
            shared.active = crossfade.to;
            shared.current = Some(crossfade.item);
            shared.crossfade = None;
            shared.gapless_pending = None;
            shared.about_to_finish_pending = false;
        }
    }

    fn settings(&self) -> PlaybackSettings {
        self.shared
            .lock()
            .map(|shared| shared.settings.clone())
            .unwrap_or_default()
    }

    fn output_state(&self) -> (f64, bool) {
        self.shared
            .lock()
            .map(|shared| (shared.volume, shared.muted))
            .unwrap_or((1.0, false))
    }

    fn visualizer_enabled(&self) -> bool {
        self.shared
            .lock()
            .map(|shared| shared.visualizer_enabled)
            .unwrap_or(false)
    }

    fn visualizer_tap(&self, slot: Slot, enabled: bool) -> Option<VisualizerTap> {
        enabled.then(|| self.visualizer.tap(slot))
    }

    fn sync_visualizer_taps(&mut self, enabled: bool) {
        let primary_tap = self.visualizer_tap(Slot::Primary, enabled);
        let secondary_tap = self.visualizer_tap(Slot::Secondary, enabled);
        self.primary.set_visualizer_tap(primary_tap);
        self.secondary.set_visualizer_tap(secondary_tap);
    }

    fn set_visualizer_enabled(&mut self, enabled: bool) -> Result<(), String> {
        let changed = {
            let Ok(mut shared) = self.shared.lock() else {
                return Ok(());
            };
            let changed = shared.visualizer_enabled != enabled;
            shared.visualizer_enabled = enabled;
            changed
        };
        if changed {
            self.visualizer.next_generation();
            push_event(&self.events, PlaybackEvent::Visualizer(Vec::new()));
        }
        if enabled {
            self.sync_visualizer_taps(true);
        } else if changed {
            self.sync_visualizer_taps(false);
        }
        Ok(())
    }

    fn set_volume(&mut self, volume: f64) -> bool {
        let muted = self
            .shared
            .lock()
            .map(|mut shared| {
                shared.volume = volume;
                shared.settings.volume = volume;
                shared.muted
            })
            .unwrap_or(false);
        self.primary.set_output_volume(volume, muted);
        self.secondary.set_output_volume(volume, muted);
        muted
    }

    fn set_muted(&mut self, muted: bool) -> f64 {
        let volume = self
            .shared
            .lock()
            .map(|mut shared| {
                shared.muted = muted;
                shared.settings.muted = muted;
                shared.volume
            })
            .unwrap_or(1.0);
        self.primary.set_output_volume(volume, muted);
        self.secondary.set_output_volume(volume, muted);
        volume
    }

    fn active_pipeline(&self) -> &PlayerPipeline {
        self.pipeline_for_slot(self.active_slot())
    }

    fn pipeline_for_slot(&self, slot: Slot) -> &PlayerPipeline {
        match slot {
            Slot::Primary => &self.primary,
            Slot::Secondary => &self.secondary,
        }
    }

    fn pipeline_for_slot_mut(&mut self, slot: Slot) -> &mut PlayerPipeline {
        match slot {
            Slot::Primary => &mut self.primary,
            Slot::Secondary => &mut self.secondary,
        }
    }

    fn active_slot(&self) -> Slot {
        self.shared
            .lock()
            .map(|shared| shared.active)
            .unwrap_or(Slot::Primary)
    }

    fn is_active_slot(&self, slot: Slot) -> bool {
        self.active_slot() == slot
    }

    fn error_is_relevant_slot(&self, slot: Slot) -> bool {
        if self.is_active_slot(slot) {
            return true;
        }
        self.shared
            .lock()
            .ok()
            .and_then(|shared| shared.crossfade.clone())
            .is_some_and(|crossfade| crossfade.from == slot || crossfade.to == slot)
    }

    fn stop_after_playback_error(&mut self) {
        self.pending_seek = None;
        self.primary.stop();
        self.secondary.stop();
        if let Ok(mut shared) = self.shared.lock() {
            shared.next = None;
            shared.gapless_pending = None;
            shared.about_to_finish_pending = false;
            shared.crossfade = None;
            shared.active = Slot::Primary;
        }
    }

    fn message_source_is_pipeline(&self, slot: Slot, message: &gst::Message) -> bool {
        message.src().is_some_and(|source| {
            source
                == self
                    .pipeline_for_slot(slot)
                    .pipeline
                    .upcast_ref::<gst::Object>()
        })
    }

    fn shutdown(&self) {
        self.primary.stop();
        self.secondary.stop();
    }
}
#[instrument(skip(receiver, events))]
fn run_gstreamer_thread(
    receiver: Receiver<PlaybackCommand>,
    events: Arc<Mutex<VecDeque<PlaybackEvent>>>,
) {
    let startup_started_at = Instant::now();
    if let Err(error) = gst::init() {
        push_event(
            &events,
            PlaybackEvent::Error(format!("GStreamer init failed: {error}")),
        );
        return;
    }

    let mut engine = match GstEngine::new(Arc::clone(&events)) {
        Ok(engine) => engine,
        Err(error) => {
            push_event(&events, PlaybackEvent::Error(error));
            return;
        }
    };
    info!(
        elapsed_ms = startup_started_at.elapsed().as_millis(),
        "GStreamer playback backend is ready"
    );

    loop {
        engine.poll_bus();
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(command) => engine.handle_command(command),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        engine.tick();
    }
    engine.shutdown();
}
fn handle_about_to_finish(pipeline: &gst::Element, shared: &Arc<Mutex<SharedPlaybackState>>) {
    let action = shared
        .lock()
        .map(|mut shared| about_to_finish_action(&mut shared))
        .unwrap_or(AboutToFinishAction::Ignore);

    match action {
        AboutToFinishAction::Preload(next) => {
            info!(
                track_id = %next.track.id.as_str(),
                uri = %next.stream.redacted_uri(),
                "preloading gapless next stream"
            );
            debug!(
                track_id = %next.track.id.as_str(),
                uri = %next.stream.redacted_uri(),
                "preloading gapless next stream"
            );
            pipeline.set_property("uri", next.stream.uri());
        }
        AboutToFinishAction::Ignore => {}
    }
}

pub(super) fn about_to_finish_action(shared: &mut SharedPlaybackState) -> AboutToFinishAction {
    if shared.gapless_pending.is_some() {
        return AboutToFinishAction::Ignore;
    }

    let Some(next) = shared.next.as_ref() else {
        shared.about_to_finish_pending = true;
        return AboutToFinishAction::Ignore;
    };

    if !gapless_preload_should_run(shared, next) {
        shared.about_to_finish_pending = false;
        return AboutToFinishAction::Ignore;
    }

    if next.stream.source_end_millis().is_some()
        || !gapless_preload_source_is_supported(next.stream.uri())
    {
        debug!(
            track_id = %next.track.id.as_str(),
            uri = %next.stream.redacted_uri(),
            "skipping gapless preload for non-local stream"
        );
        shared.about_to_finish_pending = false;
        return AboutToFinishAction::Ignore;
    }

    let next = shared
        .next
        .take()
        .expect("gapless preload checked that next item exists");
    shared.gapless_pending = Some(next.clone());
    shared.about_to_finish_pending = false;
    AboutToFinishAction::Preload(Box::new(next))
}

pub(super) fn cancel_gapless_pending(
    shared: &mut SharedPlaybackState,
) -> Option<(PreparedPlaybackItem, PreparedPlaybackItem)> {
    let pending = shared.gapless_pending.take()?;
    let current = shared.current.clone()?;
    if shared.next.is_none() {
        shared.next = Some(pending.clone());
    }
    shared.about_to_finish_pending = false;
    Some((current, pending))
}

pub(super) fn cancel_crossfade_next(
    shared: &mut SharedPlaybackState,
    slot: Slot,
) -> Option<CrossfadeState> {
    let crossfade = shared.crossfade.clone()?;
    if crossfade.to != slot {
        return None;
    }
    if shared.next.is_none() {
        shared.next = Some(crossfade.item.clone());
    }
    shared.crossfade = None;
    shared.gapless_pending = None;
    shared.about_to_finish_pending = false;
    Some(crossfade)
}

pub(super) fn clear_prepared_next_state(shared: &mut SharedPlaybackState) -> PreparedNextClear {
    let gapless_current = shared.gapless_pending.take().and_then(|_| {
        shared
            .current
            .clone()
            .map(|current| (shared.active, current))
    });
    let crossfade = shared.crossfade.take();
    shared.next = None;
    shared.about_to_finish_pending = false;
    PreparedNextClear {
        gapless_current,
        crossfade,
    }
}

fn gapless_preload_should_run(shared: &SharedPlaybackState, next: &PreparedPlaybackItem) -> bool {
    shared.settings.transition_mode == PlaybackTransitionMode::Gapless
        || same_album_crossfade_is_skipped(&shared.settings, shared.current.as_ref(), next)
}

pub(super) fn same_album_crossfade_is_skipped(
    settings: &PlaybackSettings,
    current: Option<&PreparedPlaybackItem>,
    next: &PreparedPlaybackItem,
) -> bool {
    if settings.transition_mode != PlaybackTransitionMode::Crossfade
        || !settings.skip_same_album_crossfade
    {
        return false;
    }
    let Some(current) = current else {
        return false;
    };
    if let (Some(current_album_id), Some(next_album_id)) =
        (&current.track.album_id, &next.track.album_id)
    {
        return current_album_id == next_album_id;
    }
    same_album_text(&current.track, &next.track)
}

fn same_album_text(current: &PlaybackTrack, next: &PlaybackTrack) -> bool {
    let current_album = current.album.trim();
    let next_album = next.album.trim();
    if current_album.is_empty() || !current_album.eq_ignore_ascii_case(next_album) {
        return false;
    }
    let current_artist = current.artist.trim();
    let next_artist = next.artist.trim();
    current_artist.is_empty()
        || next_artist.is_empty()
        || current_artist.eq_ignore_ascii_case(next_artist)
}

pub(super) fn gapless_preload_source_is_supported(uri: &str) -> bool {
    uri.starts_with("file://") || uri.starts_with("http://") || uri.starts_with("https://")
}
fn make_playbin(name: &str) -> Result<gst::Element, String> {
    gst::ElementFactory::make("playbin3")
        .name(name)
        .build()
        .or_else(|_| gst::ElementFactory::make("playbin").name(name).build())
        .map_err(|error| error.to_string())
}
fn configure_playbin_for_audio(pipeline: &gst::Element) {
    let current = pipeline.property_value("flags");
    let Some(flags_class) = glib::FlagsClass::with_type(current.type_()) else {
        return;
    };
    let Some(flags) = flags_class
        .builder()
        .set_by_nick("audio")
        .set_by_nick("soft-volume")
        .set_by_nick("buffering")
        .build()
    else {
        return;
    };
    pipeline.set_property_from_value("flags", &flags);
}
fn build_audio_sink(settings: &PlaybackSettings) -> Result<AudioSink, String> {
    let has_replay_gain = settings.replay_gain != ReplayGainMode::Off;

    let bin = gst::Bin::new();
    let convert_in = make_element("audioconvert", "rufin-audio-convert-in")?;
    let convert_out = make_element("audioconvert", "rufin-audio-convert-out")?;
    let sink = make_audio_output(settings.audio_output.as_deref())?;
    let mut elements = vec![convert_in.clone()];

    let equalizer = optional_element("equalizer-nbands", "rufin-equalizer");
    if let Some(equalizer) = equalizer.as_ref() {
        equalizer.set_property("num-bands", (EQUALIZER_BAND_COUNT + 2) as u32);
        configure_equalizer(equalizer, &settings.equalizer);
        elements.push(equalizer.clone());
    }

    if has_replay_gain && let Some(rgvolume) = optional_element("rgvolume", "rufin-replaygain") {
        if settings.replay_gain == ReplayGainMode::Album {
            rgvolume.set_property("album-mode", true);
        }
        elements.push(rgvolume);
        if let Some(rglimiter) = optional_element("rglimiter", "rufin-replaygain-limiter") {
            elements.push(rglimiter);
        }
    }

    let visualizer_pad = convert_out.static_pad("src");
    elements.push(convert_out.clone());
    elements.push(sink.clone());
    for element in &elements {
        bin.add(element).map_err(|error| error.to_string())?;
    }
    let refs = elements.iter().collect::<Vec<_>>();
    gst::Element::link_many(&refs).map_err(|error| error.to_string())?;

    let sink_pad = convert_in
        .static_pad("sink")
        .ok_or_else(|| "audio chain is missing an input pad".to_string())?;
    let ghost_sink = gst::GhostPad::with_target(&sink_pad).map_err(|error| error.to_string())?;
    ghost_sink
        .set_active(true)
        .map_err(|error| error.to_string())?;
    bin.add_pad(&ghost_sink)
        .map_err(|error| error.to_string())?;
    Ok(AudioSink {
        root: bin.upcast(),
        equalizer,
        visualizer_pad,
    })
}
fn copy_visualizer_samples(pad: &gst::Pad, buffer: &gst::Buffer) -> Option<Vec<f32>> {
    let caps = pad.current_caps()?;
    let info = gst_audio::AudioInfo::from_caps(caps.as_ref()).ok()?;
    let map = buffer.map_readable().ok()?;
    copy_audio_samples(map.as_slice(), &info, VISUALIZER_COPY_FRAMES)
}
fn copy_audio_samples(
    bytes: &[u8],
    info: &gst_audio::AudioInfo,
    max_frames: usize,
) -> Option<Vec<f32>> {
    if info.layout() != gst_audio::AudioLayout::Interleaved {
        return None;
    }
    let channels = usize::try_from(info.channels()).ok()?.max(1);
    let frame_size = usize::try_from(info.bpf()).ok()?;
    let sample_size = visualizer_sample_size(info.format())?;
    if frame_size == 0 || sample_size == 0 || sample_size.saturating_mul(channels) > frame_size {
        return None;
    }
    let frames = (bytes.len() / frame_size).min(max_frames);
    let mut samples = Vec::with_capacity(frames);
    for frame_index in 0..frames {
        let frame_start = frame_index * frame_size;
        let mut total = 0.0;
        for channel in 0..channels {
            let sample_start = frame_start + channel * sample_size;
            let sample_end = sample_start + sample_size;
            let sample = bytes
                .get(sample_start..sample_end)
                .and_then(|slice| decode_visualizer_sample(info.format(), slice))
                .unwrap_or(0.0);
            total += sample.clamp(-1.0, 1.0);
        }
        let mono = total / channels as f32;
        samples.push(mono);
    }
    (!samples.is_empty()).then_some(samples)
}
fn visualizer_sample_size(format: gst_audio::AudioFormat) -> Option<usize> {
    Some(match format {
        gst_audio::AudioFormat::S8 | gst_audio::AudioFormat::U8 => 1,
        gst_audio::AudioFormat::S16le | gst_audio::AudioFormat::U16le => 2,
        gst_audio::AudioFormat::S24le | gst_audio::AudioFormat::U24le => 3,
        gst_audio::AudioFormat::S2432le
        | gst_audio::AudioFormat::U2432le
        | gst_audio::AudioFormat::S32le
        | gst_audio::AudioFormat::U32le
        | gst_audio::AudioFormat::F32le => 4,
        gst_audio::AudioFormat::F64le => 8,
        _ => return None,
    })
}
fn decode_visualizer_sample(format: gst_audio::AudioFormat, bytes: &[u8]) -> Option<f32> {
    let sample = match format {
        gst_audio::AudioFormat::S8 => i8::from_ne_bytes([bytes[0]]) as f32 / i8::MAX as f32,
        gst_audio::AudioFormat::U8 => (bytes[0] as f32 - 128.0) / 128.0,
        gst_audio::AudioFormat::S16le => {
            i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / i16::MAX as f32
        }
        gst_audio::AudioFormat::U16le => {
            (u16::from_le_bytes([bytes[0], bytes[1]]) as f32 - 32_768.0) / 32_768.0
        }
        gst_audio::AudioFormat::S24le => decode_s24le(bytes) as f32 / 8_388_607.0,
        gst_audio::AudioFormat::U24le => (decode_u24le(bytes) as f32 - 8_388_608.0) / 8_388_608.0,
        gst_audio::AudioFormat::S2432le => {
            (i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) >> 8) as f32 / 8_388_607.0
        }
        gst_audio::AudioFormat::U2432le => {
            ((u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) >> 8) as f32
                - 8_388_608.0)
                / 8_388_608.0
        }
        gst_audio::AudioFormat::S32le => {
            i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32 / i32::MAX as f32
        }
        gst_audio::AudioFormat::U32le => {
            (u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32 - 2_147_483_648.0)
                / 2_147_483_648.0
        }
        gst_audio::AudioFormat::F32le => {
            f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        }
        gst_audio::AudioFormat::F64le => f64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]) as f32,
        _ => return None,
    };
    Some(if sample.is_finite() { sample } else { 0.0 })
}
fn decode_s24le(bytes: &[u8]) -> i32 {
    let sign = if bytes[2] & 0x80 == 0 { 0x00 } else { 0xff };
    i32::from_le_bytes([bytes[0], bytes[1], bytes[2], sign])
}
fn decode_u24le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0])
}
fn run_visualizer_worker(
    receiver: Receiver<VisualizerFrame>,
    events: Arc<Mutex<VecDeque<PlaybackEvent>>>,
    shared: Arc<Mutex<SharedPlaybackState>>,
    generation: Arc<AtomicU64>,
) {
    let mut fft = VisualizerFft::new();
    let mut current_generation = 0;
    while let Ok(frame) = receiver.recv() {
        if frame.generation != generation.load(Ordering::Acquire) {
            continue;
        }
        if frame.generation != current_generation {
            fft.clear();
            current_generation = frame.generation;
        }
        if !visualizer_frame_is_current(&shared, frame.slot) {
            continue;
        }
        fft.push_samples(&frame.samples);
        let Some(levels) = fft.maybe_levels() else {
            continue;
        };
        if frame.generation != generation.load(Ordering::Acquire)
            || !visualizer_frame_is_current(&shared, frame.slot)
        {
            continue;
        }
        push_event(&events, PlaybackEvent::Visualizer(levels));
    }
}
fn visualizer_frame_is_current(shared: &Arc<Mutex<SharedPlaybackState>>, slot: Slot) -> bool {
    shared
        .lock()
        .map(|shared| shared.visualizer_enabled && shared.active == slot)
        .unwrap_or(false)
}
struct VisualizerFft {
    samples: VecDeque<f32>,
    input: Vec<Complex<f32>>,
    window: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
    last_emit: Option<Instant>,
}
impl VisualizerFft {
    fn new() -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(VISUALIZER_FFT_SIZE);
        let window = (0..VISUALIZER_FFT_SIZE)
            .map(|index| {
                let position = index as f32 / (VISUALIZER_FFT_SIZE - 1) as f32;
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * position).cos()
            })
            .collect();
        Self {
            samples: VecDeque::with_capacity(VISUALIZER_FFT_SIZE),
            input: vec![Complex::new(0.0, 0.0); VISUALIZER_FFT_SIZE],
            window,
            fft,
            last_emit: None,
        }
    }

    fn clear(&mut self) {
        self.samples.clear();
        self.last_emit = None;
    }

    fn push_samples(&mut self, samples: &[f32]) {
        let start = samples.len().saturating_sub(VISUALIZER_FFT_SIZE);
        for sample in &samples[start..] {
            if self.samples.len() == VISUALIZER_FFT_SIZE {
                self.samples.pop_front();
            }
            self.samples.push_back(*sample);
        }
    }

    fn maybe_levels(&mut self) -> Option<Vec<f64>> {
        if self.samples.len() < VISUALIZER_FFT_SIZE {
            return None;
        }
        let now = Instant::now();
        if self
            .last_emit
            .is_some_and(|last| now.duration_since(last) < VISUALIZER_MIN_EMIT_INTERVAL)
        {
            return None;
        }
        self.last_emit = Some(now);
        Some(self.levels())
    }

    fn levels(&mut self) -> Vec<f64> {
        for ((slot, sample), window) in self
            .input
            .iter_mut()
            .zip(self.samples.iter().copied())
            .zip(self.window.iter().copied())
        {
            *slot = Complex::new(sample * window, 0.0);
        }
        self.fft.process(&mut self.input);
        fft_levels(&self.input)
    }
}
fn fft_levels(input: &[Complex<f32>]) -> Vec<f64> {
    let half = VISUALIZER_FFT_SIZE / 2;
    input
        .iter()
        .take(half)
        .enumerate()
        .map(|(index, value)| {
            if index == 0 {
                return 0.0;
            }
            let magnitude = value.norm() / VISUALIZER_FFT_SIZE as f32;
            let db = 20.0 * magnitude.max(1.0e-6).log10();
            let level = ((db - VISUALIZER_NOISE_FLOOR_DB)
                / (VISUALIZER_CEILING_DB - VISUALIZER_NOISE_FLOOR_DB))
                .clamp(0.0, 1.0);
            f64::from(level.powf(1.25))
        })
        .collect()
}
fn set_equalizer_band(
    equalizer: &gst::Element,
    index: usize,
    frequency: f64,
    bandwidth: f64,
    gain: f64,
) {
    let Some(proxy) = equalizer.dynamic_cast_ref::<gst::ChildProxy>() else {
        return;
    };
    if let Some(band) = proxy.child_by_index(index as u32) {
        band.set_property("freq", frequency);
        band.set_property("bandwidth", bandwidth);
        band.set_property("gain", gain);
    }
}
fn configure_equalizer(equalizer: &gst::Element, settings: &EqualizerSettings) {
    set_equalizer_band(equalizer, 0, EQUALIZER_DUMMY_LOW_FREQUENCY, 0.0, 0.0);
    set_equalizer_band(
        equalizer,
        EQUALIZER_BAND_COUNT + 1,
        EQUALIZER_DUMMY_HIGH_FREQUENCY,
        0.0,
        0.0,
    );
    let mut previous = 0.0;
    for (index, frequency) in CLASSIC_EQUALIZER_FREQUENCIES.iter().copied().enumerate() {
        let gain = if settings.enabled {
            settings.bands.get(index).copied().unwrap_or(0.0)
        } else {
            0.0
        };
        set_equalizer_band(equalizer, index + 1, frequency, frequency - previous, gain);
        previous = frequency;
    }
}
fn make_audio_output(selected: Option<&str>) -> Result<gst::Element, String> {
    if let Some(selected) = selected
        && gst::ElementFactory::find(selected).is_some()
    {
        return make_element(selected, "rufin-audio-output");
    }
    make_element("autoaudiosink", "rufin-audio-output")
}
fn make_element(factory: &str, name: &str) -> Result<gst::Element, String> {
    gst::ElementFactory::make(factory)
        .name(name)
        .build()
        .map_err(|error| error.to_string())
}
fn optional_element(factory: &str, name: &str) -> Option<gst::Element> {
    gst::ElementFactory::find(factory)?;
    make_element(factory, name)
        .inspect_err(|error| warn!(%error, factory, "failed to create optional GStreamer element"))
        .ok()
}
fn inactive_slot(slot: Slot) -> Slot {
    match slot {
        Slot::Primary => Slot::Secondary,
        Slot::Secondary => Slot::Primary,
    }
}
fn push_event(events: &Arc<Mutex<VecDeque<PlaybackEvent>>>, event: PlaybackEvent) {
    if let Ok(mut events) = events.lock() {
        events.push_back(event);
    }
}
pub(super) fn position_event(millis: u64) -> PlaybackEvent {
    position_event_for_track(millis, None)
}
pub(super) fn position_event_for_track(millis: u64, track_id: Option<TrackId>) -> PlaybackEvent {
    PlaybackEvent::PositionChanged {
        track_id,
        seconds: clock_seconds_from_millis(millis),
        millis,
    }
}

pub(super) fn clock_seconds_from_millis(millis: u64) -> u32 {
    (millis / 1_000).min(u64::from(u32::MAX)) as u32
}
fn clock_seconds(clock_time: gst::ClockTime) -> u32 {
    clock_time.seconds().min(u64::from(u32::MAX)) as u32
}
fn clock_millis(clock_time: gst::ClockTime) -> u64 {
    clock_time.mseconds()
}
fn seek_position_matches_target(target_millis: u64, millis: u64) -> bool {
    let lower = target_millis.saturating_sub(SEEK_POSITION_TOLERANCE_MILLIS);
    let upper = target_millis.saturating_add(SEEK_POSITION_TOLERANCE_MILLIS);
    (lower..=upper).contains(&millis)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_audio_sink_is_tap_ready_without_spectrum() {
        gst::init().expect("initialize GStreamer");
        let settings = PlaybackSettings {
            audio_output: Some("fakesink".to_string()),
            ..PlaybackSettings::default()
        };
        let sink = build_audio_sink(&settings).expect("build default audio sink");
        assert!(sink.visualizer_pad.is_some());
        let bin = sink.root.downcast::<gst::Bin>().expect("audio sink bin");
        let factories = bin
            .children()
            .iter()
            .filter_map(|element| element.factory().map(|factory| factory.name().to_string()))
            .collect::<Vec<_>>();
        assert!(factories.iter().any(|factory| factory == "audioconvert"));
        assert!(!factories.iter().any(|factory| factory == "spectrum"));
    }

    #[test]
    fn visualizer_audio_sink_omits_visualizer_caps_chain() {
        gst::init().expect("initialize GStreamer");
        let settings = PlaybackSettings {
            audio_output: Some("fakesink".to_string()),
            ..PlaybackSettings::default()
        };
        let sink = build_audio_sink(&settings).expect("build visualizer audio sink");
        let bin = sink.root.downcast::<gst::Bin>().expect("audio sink bin");
        let factories = bin
            .children()
            .iter()
            .filter_map(|element| element.factory().map(|factory| factory.name().to_string()))
            .collect::<Vec<_>>();
        assert!(!factories.iter().any(|factory| factory == "capsfilter"));
        assert!(!factories.iter().any(|factory| factory == "spectrum"));
    }

    #[test]
    fn audio_sink_config_ignores_equalizer_changes() {
        let previous = PlaybackSettings::default();
        let mut next = previous.clone();
        next.equalizer.enabled = true;
        next.equalizer.bands = vec![4.0; EQUALIZER_BAND_COUNT];

        assert_eq!(AudioSinkConfig::new(&previous), AudioSinkConfig::new(&next));
    }

    #[test]
    fn equalizer_band_helper_configures_child_band() {
        gst::init().expect("initialize GStreamer");
        let Some(equalizer) = optional_element("equalizer-nbands", "test-equalizer") else {
            return;
        };
        equalizer.set_property("num-bands", 12u32);
        set_equalizer_band(&equalizer, 1, 60.0, 60.0, 8.0);
        let proxy = equalizer
            .dynamic_cast_ref::<gst::ChildProxy>()
            .expect("equalizer child proxy");
        let band = proxy.child_by_index(1).expect("configured band");
        assert_eq!(band.property::<f64>("freq"), 60.0);
        assert_eq!(band.property::<f64>("bandwidth"), 60.0);
        assert_eq!(band.property::<f64>("gain"), 8.0);
    }

    #[test]
    fn equalizer_update_applies_live_gain_and_disable() {
        gst::init().expect("initialize GStreamer");
        let Some(equalizer) = optional_element("equalizer-nbands", "test-live-equalizer") else {
            return;
        };
        equalizer.set_property("num-bands", (EQUALIZER_BAND_COUNT + 2) as u32);
        let mut settings = EqualizerSettings {
            enabled: true,
            bands: vec![5.0; EQUALIZER_BAND_COUNT],
            ..EqualizerSettings::default()
        };
        configure_equalizer(&equalizer, &settings);
        assert_eq!(equalizer_band_gain(&equalizer, 1), Some(5.0));

        settings.enabled = false;
        configure_equalizer(&equalizer, &settings);
        assert_eq!(equalizer_band_gain(&equalizer, 1), Some(0.0));
    }

    fn equalizer_band_gain(equalizer: &gst::Element, index: usize) -> Option<f64> {
        equalizer
            .dynamic_cast_ref::<gst::ChildProxy>()?
            .child_by_index(index as u32)
            .map(|band| band.property::<f64>("gain"))
    }

    #[test]
    fn visualizer_pcm_copy_mixes_stereo_frames() {
        gst::init().expect("initialize GStreamer");
        let info = gst_audio::AudioInfo::builder(gst_audio::AudioFormat::F32le, 48_000, 2)
            .layout(gst_audio::AudioLayout::Interleaved)
            .build()
            .expect("audio info");
        let mut bytes = Vec::new();
        for sample in [(0.5_f32, -0.25_f32), (2.0_f32, 0.0_f32)] {
            bytes.extend_from_slice(&sample.0.to_le_bytes());
            bytes.extend_from_slice(&sample.1.to_le_bytes());
        }
        let samples = copy_audio_samples(&bytes, &info, 8).expect("copy samples");
        assert_eq!(samples.len(), 2);
        assert!((samples[0] - 0.125).abs() < 0.001);
        assert!((samples[1] - 0.5).abs() < 0.001);
    }

    #[test]
    fn visualizer_pcm_copy_mixes_s16_stereo_frames() {
        gst::init().expect("initialize GStreamer");
        let info = gst_audio::AudioInfo::builder(gst_audio::AudioFormat::S16le, 48_000, 2)
            .layout(gst_audio::AudioLayout::Interleaved)
            .build()
            .expect("audio info");
        let mut bytes = Vec::new();
        for sample in [(16_384_i16, -8_192_i16), (i16::MAX, 0_i16)] {
            bytes.extend_from_slice(&sample.0.to_le_bytes());
            bytes.extend_from_slice(&sample.1.to_le_bytes());
        }
        let samples = copy_audio_samples(&bytes, &info, 8).expect("copy samples");
        assert_eq!(samples.len(), 2);
        assert!((samples[0] - 0.125).abs() < 0.001);
        assert!((samples[1] - 0.5).abs() < 0.001);
    }

    #[test]
    fn visualizer_fft_silence_is_quiet() {
        let mut fft = VisualizerFft::new();
        fft.push_samples(&vec![0.0; VISUALIZER_FFT_SIZE]);
        let levels = fft.levels();
        assert_eq!(levels.len(), VISUALIZER_FFT_SIZE / 2);
        assert!(levels.iter().all(|level| (0.0..=0.01).contains(level)));
    }

    #[test]
    fn visualizer_fft_sine_produces_bounded_energy() {
        let mut fft = VisualizerFft::new();
        let samples = (0..VISUALIZER_FFT_SIZE)
            .map(|index| {
                let phase =
                    2.0 * std::f32::consts::PI * 16.0 * index as f32 / VISUALIZER_FFT_SIZE as f32;
                phase.sin() * 0.8
            })
            .collect::<Vec<_>>();
        fft.push_samples(&samples);
        let levels = fft.levels();
        let peak = levels.iter().copied().fold(0.0_f64, f64::max);
        assert!(peak > 0.3);
        assert!(levels.iter().all(|level| (0.0..=1.0).contains(level)));
    }
}
