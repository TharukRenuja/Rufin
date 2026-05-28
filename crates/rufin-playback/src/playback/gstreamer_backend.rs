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
impl GstEngine {
    fn new(events: Arc<Mutex<VecDeque<PlaybackEvent>>>) -> Result<Self, String> {
        let shared = Arc::new(Mutex::new(SharedPlaybackState::new()));
        let primary = PlayerPipeline::new(
            Slot::Primary,
            "rufin-primary-player",
            Arc::clone(&shared),
            Arc::clone(&events),
        )?;
        let secondary = PlayerPipeline::new(
            Slot::Secondary,
            "rufin-secondary-player",
            Arc::clone(&shared),
            Arc::clone(&events),
        )?;
        Ok(Self {
            primary,
            secondary,
            shared,
            events,
            last_position_tick: Instant::now(),
            state: PlaybackState::Stopped,
            pending_seek: None,
        })
    }

    fn handle_command(&mut self, command: PlaybackCommand) {
        let result = match command {
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
            PlaybackCommand::PrepareNext(next) => {
                if let Ok(mut shared) = self.shared.lock() {
                    shared.next = next;
                }
                Ok(())
            }
            PlaybackCommand::UpdateSettings(mut settings) => {
                settings.sanitize();
                if let Ok(mut shared) = self.shared.lock() {
                    shared.settings = settings;
                    shared.volume = shared.settings.volume;
                    shared.muted = shared.settings.muted;
                }
                let (volume, muted) = self.output_state();
                self.primary.set_output_volume(volume, muted);
                self.secondary.set_output_volume(volume, muted);
                push_event(&self.events, PlaybackEvent::VolumeChanged { volume, muted });
                Ok(())
            }
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

    fn play_prepared(
        &mut self,
        item: PreparedPlaybackItem,
        next: Option<PreparedPlaybackItem>,
        start_position_seconds: u32,
        mut settings: PlaybackSettings,
    ) -> Result<(), String> {
        self.pending_seek = None;
        settings.sanitize();
        self.secondary.stop();
        let volume = settings.volume;
        let muted = settings.muted;
        if let Ok(mut shared) = self.shared.lock() {
            shared.settings = settings.clone();
            shared.current = Some(item.clone());
            shared.next = next;
            shared.gapless_pending = None;
            shared.crossfade = None;
            shared.active = Slot::Primary;
            shared.volume = volume;
            shared.muted = muted;
        }
        self.push_state(PlaybackState::Buffering);
        self.primary
            .play_item(&item, &settings, volume, muted, start_position_seconds)?;
        if start_position_seconds > 0 {
            self.start_playback_seek(u64::from(start_position_seconds) * 1_000);
        } else {
            self.pending_seek = Some(PendingSeek::track_start(Instant::now()));
        }
        Ok(())
    }

    fn start_seek(&mut self, millis: u64) -> Result<(), String> {
        self.finish_crossfade_for_seek();
        self.active_pipeline().seek_millis(millis)?;
        self.pending_seek = Some(PendingSeek::interactive(millis, self.state, Instant::now()));
        Ok(())
    }

    fn start_playback_seek(&mut self, millis: u64) {
        let pending = PendingSeek::startup(millis, self.state, Instant::now());
        let _ = self.active_pipeline().seek_millis(millis);
        self.pending_seek = Some(pending);
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
                if let Some(duration) = self.active_pipeline().duration() {
                    push_event(
                        &self.events,
                        PlaybackEvent::DurationChanged(clock_seconds(duration)),
                    );
                }
            }
            MessageView::Buffering(buffering) if self.is_active_slot(slot) => {
                self.handle_buffering(buffering.percent().min(100) as u8);
            }
            MessageView::Eos(_) => self.handle_eos(slot),
            MessageView::Error(error_message) => {
                let message = error_message.error().to_string();
                error!(%message, "GStreamer playback error");
                push_event(&self.events, PlaybackEvent::Error(message));
            }
            _ => {}
        }
    }

    fn handle_stream_start(&mut self) {
        let started = self.shared.lock().ok().and_then(|mut shared| {
            let item = shared.gapless_pending.take()?;
            shared.current = Some(item.clone());
            Some(item.track)
        });
        self.handle_stream_started_track(started);
    }

    fn handle_stream_started_track(&mut self, started: Option<PlaybackTrack>) {
        let Some(track) = started else {
            return;
        };
        self.pending_seek = None;
        push_event(&self.events, position_event(0));
        push_event(
            &self.events,
            PlaybackEvent::DurationChanged(track.duration_seconds),
        );
        push_event(&self.events, PlaybackEvent::PreparedTrackStarted(track));
    }

    fn handle_state_changed(&mut self, state: PlaybackState) {
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
        if self.retry_pending_seek_after_async_done() {
            return;
        }
        if let Some(position) = self.active_pipeline().position() {
            self.push_position(clock_millis(position));
        }
    }

    fn retry_pending_seek_after_async_done(&mut self) -> bool {
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
        let resume_after_seek = pending.resume_after_seek;
        pending.retry_on_async_done = false;
        pending.expires_at = now + STARTUP_SEEK_SETTLE_WINDOW;
        let seek_result = self.active_pipeline().seek_millis(target_millis);
        if let Some(pending) = self.pending_seek.as_mut() {
            if seek_result.is_err() {
                pending.retry_on_async_done = true;
            } else {
                pending.resume_after_seek = false;
            }
        }
        if resume_after_seek {
            self.resume_after_startup_seek();
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
        self.state = state;
        push_event(&self.events, PlaybackEvent::StateChanged(state));
    }

    fn handle_eos(&mut self, slot: Slot) {
        if self.finish_crossfade_if_needed(slot) {
            return;
        }
        if self.is_active_slot(slot) {
            push_event(&self.events, PlaybackEvent::EndOfStream);
        }
    }

    fn tick(&mut self) {
        self.maybe_start_crossfade();
        self.update_crossfade();

        if self.last_position_tick.elapsed() >= Duration::from_millis(500) {
            self.last_position_tick = Instant::now();
            if let Some(position) = self.active_pipeline().position() {
                self.push_position(clock_millis(position));
            }
            if let Some(duration) = self.active_pipeline().duration() {
                push_event(
                    &self.events,
                    PlaybackEvent::DurationChanged(clock_seconds(duration)),
                );
            }
        }
    }

    fn push_position(&mut self, millis: u64) {
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
        push_event(&self.events, position_event(millis));
    }

    fn maybe_start_crossfade(&mut self) {
        if self.pending_seek.is_some() {
            return;
        }
        let Some(position) = self.active_pipeline().position() else {
            return;
        };
        let Some(duration) = self.active_pipeline().duration() else {
            return;
        };
        let position_ms = clock_millis(position);
        let duration_ms = clock_millis(duration);
        if duration_ms == 0 || position_ms >= duration_ms {
            return;
        }

        let request = self.shared.lock().ok().and_then(|shared| {
            if shared.settings.transition_mode != PlaybackTransitionMode::Crossfade
                || shared.crossfade.is_some()
            {
                return None;
            }
            let crossfade_ms = u64::from(shared.settings.crossfade_seconds) * 1_000;
            if duration_ms.saturating_sub(position_ms) > crossfade_ms
                || duration_ms <= crossfade_ms + 1_000
            {
                return None;
            }
            Some((
                shared.next.clone()?,
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
        let inactive = self.pipeline_for_slot(to);
        if let Err(error) = inactive.play_item(&next, &settings, 0.0, muted, 0) {
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
        push_event(&self.events, position_event(0));
        push_event(
            &self.events,
            PlaybackEvent::DurationChanged(next.track.duration_seconds),
        );
        push_event(
            &self.events,
            PlaybackEvent::PreparedTrackStarted(next.track.clone()),
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

    fn finish_crossfade_for_seek(&mut self) {
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

    fn active_slot(&self) -> Slot {
        self.shared
            .lock()
            .map(|shared| shared.active)
            .unwrap_or(Slot::Primary)
    }

    fn is_active_slot(&self, slot: Slot) -> bool {
        self.active_slot() == slot
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
fn handle_about_to_finish(
    pipeline: &gst::Element,
    shared: &Arc<Mutex<SharedPlaybackState>>,
    events: &Arc<Mutex<VecDeque<PlaybackEvent>>>,
) {
    let next = shared.lock().ok().and_then(|mut shared| {
        if shared.settings.transition_mode != PlaybackTransitionMode::Gapless
            || shared.gapless_pending.is_some()
        {
            return None;
        }
        let next = shared.next.take()?;
        shared.gapless_pending = Some(next.clone());
        Some(next)
    });

    if let Some(next) = next {
        debug!(
            track_id = %next.track.id.as_str(),
            uri = %next.stream.redacted_uri(),
            "preloading gapless next stream"
        );
        pipeline.set_property("uri", next.stream.uri());
    } else {
        push_event(
            events,
            PlaybackEvent::StateChanged(PlaybackState::Buffering),
        );
    }
}
fn make_playbin(name: &str) -> Result<gst::Element, String> {
    gst::ElementFactory::make("playbin3")
        .name(name)
        .build()
        .or_else(|_| gst::ElementFactory::make("playbin").name(name).build())
        .map_err(|error| error.to_string())
}
fn build_audio_sink(settings: &PlaybackSettings) -> Result<gst::Element, String> {
    let bin = gst::Bin::new();
    let convert_in = make_element("audioconvert", "rufin-audio-convert-in")?;
    let convert_out = make_element("audioconvert", "rufin-audio-convert-out")?;
    let sink = make_audio_output(settings.audio_output.as_deref())?;
    let mut elements = vec![convert_in.clone()];

    if settings.replay_gain != ReplayGainMode::Off
        && let Some(rgvolume) = optional_element("rgvolume", "rufin-replaygain")
    {
        if settings.replay_gain == ReplayGainMode::Album {
            rgvolume.set_property("album-mode", true);
        }
        elements.push(rgvolume);
        if let Some(rglimiter) = optional_element("rglimiter", "rufin-replaygain-limiter") {
            elements.push(rglimiter);
        }
    }

    if settings.equalizer.enabled
        && let Some(equalizer) = optional_element("equalizer-10bands", "rufin-equalizer")
    {
        for (index, gain) in settings
            .equalizer
            .bands
            .iter()
            .copied()
            .take(EQUALIZER_BAND_COUNT)
            .enumerate()
        {
            equalizer.set_property(format!("band{index}").as_str(), gain);
        }
        elements.push(equalizer);
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
    Ok(bin.upcast())
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
fn position_event(millis: u64) -> PlaybackEvent {
    PlaybackEvent::PositionChanged {
        seconds: clock_seconds_from_millis(millis),
        millis,
    }
}
fn clock_seconds_from_millis(millis: u64) -> u32 {
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
