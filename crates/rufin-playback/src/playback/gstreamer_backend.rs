use super::*;

const CLASSIC_EQUALIZER_FREQUENCIES: [f64; EQUALIZER_BAND_COUNT] = [
    60.0, 170.0, 310.0, 600.0, 1000.0, 3000.0, 6000.0, 12000.0, 14000.0, 16000.0,
];
const EQUALIZER_DUMMY_LOW_FREQUENCY: f64 = 20.0;
const EQUALIZER_DUMMY_HIGH_FREQUENCY: f64 = 20_000.0;

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
pub(super) struct PlayerPipeline {
    pub(super) pipeline: gst::Element,
    bus: gst::Bus,
    _about_to_finish_id: glib::SignalHandlerId,
    audio_sink_config: Option<AudioSinkConfig>,
    visualizer: Option<gst::Element>,
}
#[derive(Clone, Debug, PartialEq)]
struct AudioSinkConfig {
    replay_gain: ReplayGainMode,
    audio_output: Option<String>,
    equalizer: EqualizerSettings,
}
impl AudioSinkConfig {
    fn new(settings: &PlaybackSettings) -> Self {
        Self {
            replay_gain: settings.replay_gain,
            audio_output: settings.audio_output.clone(),
            equalizer: settings.equalizer.clone(),
        }
    }
}
struct AudioSink {
    root: gst::Element,
    visualizer: Option<gst::Element>,
}
#[derive(Debug, PartialEq)]
pub(super) enum AboutToFinishAction {
    Preload(PreparedPlaybackItem),
    Ignore,
}
impl PlayerPipeline {
    pub(super) fn new(
        slot: Slot,
        name: &str,
        shared: Arc<Mutex<SharedPlaybackState>>,
    ) -> Result<Self, String> {
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

        let _ = slot;
        Ok(Self {
            pipeline,
            bus,
            _about_to_finish_id: about_to_finish_id,
            audio_sink_config: None,
            visualizer: None,
        })
    }

    fn configure_audio(
        &mut self,
        settings: &PlaybackSettings,
        visualizer_enabled: bool,
    ) -> Result<(), String> {
        let config = AudioSinkConfig::new(settings);
        if self.audio_sink_config.as_ref() == Some(&config) {
            self.set_visualizer_enabled(visualizer_enabled);
            return Ok(());
        }
        let sink = build_audio_sink(settings, visualizer_enabled)?;
        self.pipeline.set_property("audio-sink", &sink.root);
        self.visualizer = sink.visualizer;
        self.audio_sink_config = Some(config);
        Ok(())
    }

    fn set_visualizer_enabled(&self, enabled: bool) {
        if let Some(visualizer) = &self.visualizer {
            visualizer.set_property("post-messages", enabled);
        }
    }

    fn play_item(
        &mut self,
        item: &PreparedPlaybackItem,
        settings: &PlaybackSettings,
        volume: f64,
        muted: bool,
        visualizer_enabled: bool,
        start_position_seconds: u32,
    ) -> Result<(), String> {
        self.pipeline
            .set_state(gst::State::Ready)
            .map_err(|error| error.to_string())?;
        self.configure_audio(settings, visualizer_enabled)?;
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

    fn restart_from_beginning(&self, target_state: gst::State) -> Result<(), String> {
        self.pipeline
            .set_state(gst::State::Ready)
            .map_err(|error| error.to_string())?;
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
    pub(super) last_position_tick: Instant,
    pub(super) state: PlaybackState,
    pub(super) pending_seek: Option<PendingSeek>,
    pub(super) play_command_started_at: Option<Instant>,
}
impl GstEngine {
    fn new(events: Arc<Mutex<VecDeque<PlaybackEvent>>>) -> Result<Self, String> {
        let shared = Arc::new(Mutex::new(SharedPlaybackState::new()));
        let primary =
            PlayerPipeline::new(Slot::Primary, "rufin-primary-player", Arc::clone(&shared))?;
        let secondary = PlayerPipeline::new(
            Slot::Secondary,
            "rufin-secondary-player",
            Arc::clone(&shared),
        )?;
        Ok(Self {
            primary,
            secondary,
            shared,
            events,
            last_position_tick: Instant::now(),
            state: PlaybackState::Stopped,
            pending_seek: None,
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
                settings.sanitize();
                (|| -> Result<(), String> {
                    let visualizer_enabled = self.visualizer_enabled();
                    if let Ok(mut shared) = self.shared.lock() {
                        shared.settings = settings.clone();
                        shared.volume = shared.settings.volume;
                        shared.muted = shared.settings.muted;
                    }
                    self.primary
                        .configure_audio(&settings, visualizer_enabled)?;
                    self.secondary
                        .configure_audio(&settings, visualizer_enabled)?;
                    let (volume, muted) = self.output_state();
                    self.primary.set_output_volume(volume, muted);
                    self.secondary.set_output_volume(volume, muted);
                    push_event(&self.events, PlaybackEvent::VolumeChanged { volume, muted });
                    Ok(())
                })()
            }
            PlaybackCommand::SetVisualizerEnabled(enabled) => self.set_visualizer_enabled(enabled),
            PlaybackCommand::Resume => {
                self.pending_seek = None;
                self.active_pipeline()
                    .set_state(gst::State::Playing)
                    .map(|_| {
                        self.push_state(PlaybackState::Playing);
                    })
            }
            PlaybackCommand::Pause => {
                self.pending_seek = None;
                self.active_pipeline()
                    .set_state(gst::State::Paused)
                    .map(|_| {
                        self.push_state(PlaybackState::Paused);
                    })
            }
            PlaybackCommand::Stop => {
                self.pending_seek = None;
                self.primary.stop();
                self.secondary.stop();
                if let Ok(mut shared) = self.shared.lock() {
                    shared.current = None;
                    shared.next = None;
                    shared.gapless_pending = None;
                    shared.about_to_finish_pending = false;
                    shared.crossfade = None;
                    shared.active = Slot::Primary;
                }
                push_event(&self.events, position_event(0));
                self.push_state(PlaybackState::Stopped);
                Ok(())
            }
            PlaybackCommand::Seek(seconds) => self.start_seek(u64::from(seconds) * 1_000),
            PlaybackCommand::SeekMillis(millis) => self.start_seek(millis),
            PlaybackCommand::SetVolume(volume) => {
                let volume = volume.clamp(0.0, 1.0);
                let muted = self.set_volume(volume);
                push_event(&self.events, PlaybackEvent::VolumeChanged { volume, muted });
                Ok(())
            }
            PlaybackCommand::SetMuted(muted) => {
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
        self.primary.configure_audio(settings, visualizer_enabled)?;
        self.secondary
            .configure_audio(settings, visualizer_enabled)?;
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
        self.pending_seek = None;
        settings.sanitize();
        self.secondary.stop();
        let volume = settings.volume;
        let muted = settings.muted;
        let start_millis = u64::from(start_position_seconds) * 1_000;
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
        self.push_state(PlaybackState::Buffering);
        let pipeline_started_at = Instant::now();
        self.primary.play_item(
            &item,
            &settings,
            volume,
            muted,
            visualizer_enabled,
            start_position_seconds,
        )?;
        info!(
            track_id = %item.track.id.as_str(),
            elapsed_ms = command_started_at.elapsed().as_millis(),
            pipeline_ms = pipeline_started_at.elapsed().as_millis(),
            "queued GStreamer playback item"
        );
        if start_position_seconds > 0 {
            self.start_playback_seek(start_millis);
        } else {
            self.pending_seek = Some(PendingSeek::track_start(Instant::now()));
        }
        Ok(())
    }

    fn prepare_next(&mut self, next: Option<PreparedPlaybackItem>) -> Result<(), String> {
        let Some(next) = next else {
            if let Ok(mut shared) = self.shared.lock() {
                shared.next = None;
                shared.about_to_finish_pending = false;
            }
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

    fn start_seek(&mut self, millis: u64) -> Result<(), String> {
        self.finish_crossfade_for_seek();
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
                if !self.gapless_timing_is_pending()
                    && self.pending_seek.is_none()
                    && let Some(duration) = self.active_pipeline().duration()
                {
                    self.push_duration(clock_seconds(duration));
                }
            }
            MessageView::Buffering(buffering) if self.is_active_slot(slot) => {
                self.handle_buffering(buffering.percent().min(100) as u8);
            }
            MessageView::Element(element) if self.is_active_slot(slot) => {
                self.handle_element_message(element);
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
                if relevant {
                    self.stop_after_playback_error();
                    push_event(&self.events, PlaybackEvent::Error(error));
                    self.push_state(PlaybackState::Stopped);
                }
            }
            _ => {}
        }
    }

    fn handle_element_message(&self, element: &gst::message::Element) {
        if !self.visualizer_enabled() {
            return;
        }
        let Some(structure) = element.structure() else {
            return;
        };
        if structure.name() != "spectrum" {
            return;
        }
        if let Some(levels) = spectrum_levels(structure) {
            push_event(&self.events, PlaybackEvent::Visualizer(levels));
        }
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

    fn tick(&mut self) {
        self.maybe_start_crossfade();
        self.update_crossfade();

        if self.last_position_tick.elapsed() >= Duration::from_millis(500) {
            self.last_position_tick = Instant::now();
            if self.gapless_timing_is_pending() {
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
            if self.pending_seek.is_none()
                && let Some(duration) = self.active_pipeline().duration()
            {
                self.push_duration(clock_seconds(duration));
            }
        }
    }

    pub(super) fn push_position(&mut self, millis: u64) {
        if self.gapless_timing_is_pending() {
            return;
        }
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
        push_event(
            &self.events,
            position_event_for_track(millis, self.timing_track_id()),
        );
    }

    pub(super) fn push_duration(&self, seconds: u32) {
        if self.gapless_timing_is_pending() {
            return;
        }
        push_event(
            &self.events,
            PlaybackEvent::DurationChanged {
                track_id: self.timing_track_id(),
                seconds,
            },
        );
    }

    fn gapless_timing_is_pending(&self) -> bool {
        self.shared
            .lock()
            .map(|shared| shared.gapless_pending.is_some())
            .unwrap_or(false)
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
        let inactive = self.pipeline_for_slot_mut(to);
        if let Err(error) = inactive.play_item(&next, &settings, 0.0, muted, visualizer_enabled, 0)
        {
            push_event(&self.events, PlaybackEvent::Error(error));
            return;
        }

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

    fn set_visualizer_enabled(&mut self, enabled: bool) -> Result<(), String> {
        let changed = {
            let Ok(mut shared) = self.shared.lock() else {
                return Ok(());
            };
            let changed = shared.visualizer_enabled != enabled;
            shared.visualizer_enabled = enabled;
            changed
        };
        self.primary.set_visualizer_enabled(enabled);
        self.secondary.set_visualizer_enabled(enabled);
        if changed && !enabled {
            push_event(&self.events, PlaybackEvent::Visualizer(Vec::new()));
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

    if !gapless_preload_source_is_supported(next.stream.uri()) {
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
    AboutToFinishAction::Preload(next)
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
fn build_audio_sink(
    settings: &PlaybackSettings,
    visualizer_enabled: bool,
) -> Result<AudioSink, String> {
    let has_replay_gain = settings.replay_gain != ReplayGainMode::Off;
    let has_equalizer = settings.equalizer.enabled;
    let visualizer = optional_element("spectrum", "rufin-spectrum");
    if !has_replay_gain && !has_equalizer && visualizer.is_none() {
        return Ok(AudioSink {
            root: make_audio_output(settings.audio_output.as_deref())?,
            visualizer: None,
        });
    }

    let bin = gst::Bin::new();
    let convert_in = make_element("audioconvert", "rufin-audio-convert-in")?;
    let convert_out = make_element("audioconvert", "rufin-audio-convert-out")?;
    let sink = make_audio_output(settings.audio_output.as_deref())?;
    let mut elements = vec![convert_in.clone()];

    if has_replay_gain && let Some(rgvolume) = optional_element("rgvolume", "rufin-replaygain") {
        if settings.replay_gain == ReplayGainMode::Album {
            rgvolume.set_property("album-mode", true);
        }
        elements.push(rgvolume);
        if let Some(rglimiter) = optional_element("rglimiter", "rufin-replaygain-limiter") {
            elements.push(rglimiter);
        }
    }

    if has_equalizer
        && let Some(equalizer) = optional_element("equalizer-nbands", "rufin-equalizer")
    {
        equalizer.set_property("num-bands", (EQUALIZER_BAND_COUNT + 2) as u32);
        set_equalizer_band(&equalizer, 0, EQUALIZER_DUMMY_LOW_FREQUENCY, 0.0, 0.0);
        set_equalizer_band(
            &equalizer,
            EQUALIZER_BAND_COUNT + 1,
            EQUALIZER_DUMMY_HIGH_FREQUENCY,
            0.0,
            0.0,
        );
        let mut previous = 0.0;
        for (index, frequency) in CLASSIC_EQUALIZER_FREQUENCIES.iter().copied().enumerate() {
            let gain = settings.equalizer.bands.get(index).copied().unwrap_or(0.0);
            set_equalizer_band(&equalizer, index + 1, frequency, frequency - previous, gain);
            previous = frequency;
        }
        elements.push(equalizer);
    }

    if let Some(spectrum) = &visualizer {
        spectrum.set_property("bands", 1024u32);
        spectrum.set_property("threshold", -85i32);
        spectrum.set_property("post-messages", visualizer_enabled);
        spectrum.set_property("message-magnitude", true);
        spectrum.set_property("message-phase", false);
        spectrum.set_property("interval", 50_000_000u64);
        elements.push(spectrum.clone());
    }

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
        visualizer,
    })
}
fn spectrum_levels(structure: &gst::StructureRef) -> Option<Vec<f64>> {
    if let Ok(list) = structure.get::<gst::List>("magnitude") {
        let levels = list
            .iter()
            .filter_map(value_level)
            .map(normalize_spectrum_level)
            .collect::<Vec<_>>();
        return (!levels.is_empty()).then_some(levels);
    }
    if let Ok(array) = structure.get::<gst::Array>("magnitude") {
        let levels = array
            .iter()
            .filter_map(value_level)
            .map(normalize_spectrum_level)
            .collect::<Vec<_>>();
        return (!levels.is_empty()).then_some(levels);
    }
    structure
        .get::<String>("magnitude")
        .ok()
        .and_then(|raw| spectrum_levels_from_text(&raw))
}
fn value_level(value: &glib::SendValue) -> Option<f64> {
    value
        .get::<f64>()
        .ok()
        .or_else(|| value.get::<f32>().ok().map(f64::from))
        .or_else(|| value.get::<i32>().ok().map(f64::from))
}
fn spectrum_levels_from_text(raw: &str) -> Option<Vec<f64>> {
    let start = raw.find('{').map(|index| index + 1).unwrap_or(0);
    let end = raw.rfind('}').unwrap_or(raw.len());
    let levels = raw[start..end]
        .split(',')
        .filter_map(|part| part.trim().parse::<f64>().ok())
        .map(normalize_spectrum_level)
        .collect::<Vec<_>>();
    (!levels.is_empty()).then_some(levels)
}
fn normalize_spectrum_level(level: f64) -> f64 {
    ((level + 85.0) / 60.0).clamp(0.0, 1.0)
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
pub(super) fn redact_sensitive_uri(uri: &str) -> String {
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
    use super::*;

    #[test]
    fn visualizer_audio_sink_builds_when_available() {
        gst::init().expect("initialize GStreamer");
        if gst::ElementFactory::find("spectrum").is_none() {
            return;
        }
        let sink = build_audio_sink(&PlaybackSettings::default(), true)
            .expect("build visualizer audio sink");
        let visualizer = sink.visualizer.expect("visualizer element");
        assert!(visualizer.property::<bool>("post-messages"));
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
    fn spectrum_text_levels_are_normalized() {
        let levels =
            spectrum_levels_from_text("(float){ -30.0, -45.0, -70.0 }").expect("parse levels");
        assert!(levels[0] > levels[1]);
        assert!(levels[1] > levels[2]);
        assert!(levels.iter().all(|level| (0.0..=1.0).contains(level)));
        assert!(normalize_spectrum_level(-45.0) > 0.0);
    }
}
