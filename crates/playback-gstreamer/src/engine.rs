use super::pipeline::{AboutToFinishAction, PlayerPipeline, SourceClock};
#[cfg(test)]
use super::waveform::visualizer_pipeline_is_live;
use super::waveform::{VisualizerAnalyzer, VisualizerTap};
use super::*;
use std::collections::HashMap;
use std::sync::mpsc::{SyncSender, sync_channel};

const GAPLESS_BUFFERING_IGNORE_REMAINING_MS: u64 = 5_000;
const STATUS_FADE_DURATION: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum TelemetryKind {
    Position,
    Duration,
    Buffering,
    Visualizer,
}

struct PendingTelemetry {
    sequence: u64,
    event: BackendEvent,
}

#[derive(Default)]
pub(super) struct EventMailbox {
    next_sequence: u64,
    ready: VecDeque<BackendEvent>,
    latest: HashMap<(RunId, TelemetryKind), PendingTelemetry>,
}

impl EventMailbox {
    fn push(&mut self, event: BackendEvent) {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        if let Some(key) = telemetry_key(&event) {
            self.latest
                .insert(key, PendingTelemetry { sequence, event });
        } else {
            self.flush_telemetry();
            self.ready.push_back(event);
        }
    }

    fn drain(&mut self) -> Vec<BackendEvent> {
        self.flush_telemetry();
        self.ready.drain(..).collect()
    }

    fn flush_telemetry(&mut self) {
        let mut telemetry = self
            .latest
            .drain()
            .map(|(_, event)| event)
            .collect::<Vec<_>>();
        telemetry.sort_unstable_by_key(|event| event.sequence);
        self.ready
            .extend(telemetry.into_iter().map(|event| event.event));
    }
}

fn telemetry_key(event: &BackendEvent) -> Option<(RunId, TelemetryKind)> {
    match event {
        BackendEvent::Position { run, .. } => Some((*run, TelemetryKind::Position)),
        BackendEvent::Duration { run, .. } => Some((*run, TelemetryKind::Duration)),
        BackendEvent::Buffering { run, .. } => Some((*run, TelemetryKind::Buffering)),
        BackendEvent::Visualizer { run, levels } if !levels.is_empty() => {
            Some((*run, TelemetryKind::Visualizer))
        }
        BackendEvent::Started { .. }
        | BackendEvent::State { .. }
        | BackendEvent::Ended { .. }
        | BackendEvent::Transitioned { .. }
        | BackendEvent::NextNeeded { .. }
        | BackendEvent::NextUnavailable { .. }
        | BackendEvent::AudioApplied { .. }
        | BackendEvent::Visualizer { .. }
        | BackendEvent::Error { .. } => None,
    }
}

pub struct GStreamerPlaybackBackend {
    commands: Option<Sender<BackendCommand>>,
    events: Arc<Mutex<EventMailbox>>,
    thread: Option<thread::JoinHandle<()>>,
}
impl GStreamerPlaybackBackend {
    pub fn new() -> Result<Self, BackendError> {
        let (commands, receiver) = channel();
        let events = Arc::new(Mutex::new(EventMailbox::default()));
        let thread_events = Arc::clone(&events);
        let (ready_sender, ready_receiver) = sync_channel(1);
        let thread = thread::Builder::new()
            .name("rufin-gstreamer-playback".to_string())
            .spawn(move || run_gstreamer_thread(receiver, thread_events, ready_sender))
            .map_err(|error| BackendError::Backend(error.to_string()))?;
        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                commands: Some(commands),
                events,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(BackendError::Backend(error))
            }
            Err(_) => {
                let _ = thread.join();
                Err(BackendError::ChannelClosed)
            }
        }
    }
}
impl PlaybackBackend for GStreamerPlaybackBackend {
    fn send(&mut self, command: BackendCommand) -> Result<(), BackendError> {
        self.commands
            .as_ref()
            .ok_or(BackendError::ChannelClosed)?
            .send(command)
            .map_err(|_| BackendError::ChannelClosed)
    }

    fn drain_events(&mut self) -> Vec<BackendEvent> {
        lock_recover(&self.events).drain()
    }

    fn shutdown(&mut self) -> Result<(), BackendError> {
        self.commands.take();
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        thread
            .join()
            .map_err(|_| BackendError::Backend("GStreamer playback worker panicked".to_string()))
    }
}

impl Drop for GStreamerPlaybackBackend {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Slot {
    Primary,
    Secondary,
}

impl Slot {
    fn index(self) -> usize {
        match self {
            Self::Primary => 0,
            Self::Secondary => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct PipelineId(pub(super) u64);

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PreparedRun {
    pub(super) run: RunId,
    pub(super) stream: ResolvedStream,
}

impl PreparedRun {
    fn from_next(next: &PreparedNext) -> Self {
        Self {
            run: next.run,
            stream: next.stream.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct CrossfadeState {
    pub(super) from: Slot,
    pub(super) to: Slot,
    pub(super) old_run: RunId,
    pub(super) started_at: Instant,
    pub(super) duration: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IncomingPhase {
    Prerolling,
    Seeking,
    Ready,
}

#[derive(Clone, Debug)]
struct IncomingPipeline {
    id: PipelineId,
    slot: Slot,
    item: PreparedNext,
    phase: IncomingPhase,
}
#[derive(Clone, Debug)]
pub(super) struct PendingSeek {
    target_millis: u64,
    expires_at: Instant,
    logical_state: BackendState,
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
        logical_state: BackendState,
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

    pub(super) fn startup(target_millis: u64, logical_state: BackendState, now: Instant) -> Self {
        Self::startup_with_resume(target_millis, logical_state, now, true)
    }

    pub(super) fn startup_with_resume(
        target_millis: u64,
        logical_state: BackendState,
        now: Instant,
        resume_after_seek: bool,
    ) -> Self {
        Self {
            target_millis,
            expires_at: now + STARTUP_SEEK_SETTLE_WINDOW,
            logical_state,
            kind: PendingSeekKind::Startup,
            retry_on_async_done: true,
            resume_after_seek,
        }
    }

    pub(super) fn track_start(now: Instant) -> Self {
        Self {
            target_millis: 0,
            expires_at: now + TRACK_START_SETTLE_WINDOW,
            logical_state: BackendState::Buffering,
            kind: PendingSeekKind::TrackStart,
            retry_on_async_done: false,
            resume_after_seek: false,
        }
    }

    pub(super) fn accepts_position(&self, millis: u64, now: Instant) -> bool {
        now >= self.expires_at || seek_position_matches_target(self.target_millis, millis)
    }

    pub(super) fn suppresses_state(&self, state: BackendState, now: Instant) -> bool {
        if now >= self.expires_at || state == self.logical_state {
            return false;
        }

        match self.kind {
            PendingSeekKind::Interactive => matches!(
                state,
                BackendState::Stopped
                    | BackendState::Buffering
                    | BackendState::Paused
                    | BackendState::Playing
            ),
            PendingSeekKind::Startup => matches!(
                state,
                BackendState::Stopped | BackendState::Paused | BackendState::Playing
            ),
            PendingSeekKind::TrackStart => {
                matches!(state, BackendState::Stopped | BackendState::Paused)
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

    fn set_desired_playing(&mut self, playing: bool) {
        self.logical_state = if playing {
            BackendState::Playing
        } else {
            BackendState::Paused
        };
        if self.kind == PendingSeekKind::Startup {
            self.resume_after_seek = playing;
        }
    }
}
#[derive(Debug)]
pub(super) struct SharedBackendState {
    pub(super) settings: BackendAudioSettings,
    pub(super) current: Option<PreparedRun>,
    pub(super) next: Option<PreparedNext>,
    pub(super) gapless_pending: Option<PreparedNext>,
    pub(super) about_to_finish_pending: bool,
    pub(super) next_needed: Option<RunId>,
    pub(super) active: Slot,
    pub(super) crossfade: Option<CrossfadeState>,
    pub(super) visualizer_enabled: bool,
    pipeline_ids: [Option<PipelineId>; 2],
}
impl SharedBackendState {
    pub(super) fn new() -> Self {
        let settings = BackendAudioSettings::default();
        Self {
            current: None,
            next: None,
            gapless_pending: None,
            about_to_finish_pending: false,
            next_needed: None,
            active: Slot::Primary,
            crossfade: None,
            visualizer_enabled: false,
            pipeline_ids: [None, None],
            settings,
        }
    }

    fn pipeline_id(&self, slot: Slot) -> Option<PipelineId> {
        self.pipeline_ids[slot.index()]
    }

    fn set_pipeline_id(&mut self, slot: Slot, id: Option<PipelineId>) {
        self.pipeline_ids[slot.index()] = id;
    }

    fn pipeline_is_live(&self, slot: Slot, id: PipelineId) -> bool {
        self.pipeline_id(slot) == Some(id)
    }

    pub(super) fn pipeline_is_current(&self, slot: Slot, id: PipelineId) -> bool {
        self.active == slot && self.pipeline_is_live(slot, id)
    }
}
pub(super) struct PreparedNextClear {
    pub(super) gapless_current: Option<(Slot, PreparedRun)>,
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
pub(super) struct GstEngine {
    pub(super) primary: PlayerPipeline,
    pub(super) secondary: PlayerPipeline,
    pub(super) shared: Arc<Mutex<SharedBackendState>>,
    events: Arc<Mutex<EventMailbox>>,
    pub(super) visualizer: VisualizerAnalyzer,
    pub(super) last_position_tick: Instant,
    pub(super) state: BackendState,
    pub(super) pending_seek: Option<PendingSeek>,
    pub(super) status_fade: Option<StatusFade>,
    pub(super) restore_output_on_playing: bool,
    pub(super) play_command_started_at: Option<Instant>,
    ended_run: Option<RunId>,
    next_pipeline_number: u64,
    incoming: Option<IncomingPipeline>,
}
impl GstEngine {
    fn new(events: Arc<Mutex<EventMailbox>>) -> Self {
        let shared = Arc::new(Mutex::new(SharedBackendState::new()));
        let primary = PlayerPipeline::new("rufin-primary-player", Arc::clone(&shared));
        let secondary = PlayerPipeline::new("rufin-secondary-player", Arc::clone(&shared));
        let visualizer = VisualizerAnalyzer::new(Arc::clone(&events), Arc::clone(&shared));
        Self {
            primary,
            secondary,
            shared,
            events,
            visualizer,
            last_position_tick: Instant::now(),
            state: BackendState::Stopped,
            pending_seek: None,
            status_fade: None,
            restore_output_on_playing: false,
            play_command_started_at: None,
            ended_run: None,
            next_pipeline_number: 1,
            incoming: None,
        }
    }

    fn next_pipeline_id(&mut self) -> PipelineId {
        let id = PipelineId(self.next_pipeline_number);
        self.next_pipeline_number = self.next_pipeline_number.wrapping_add(1).max(1);
        id
    }

    fn start_pipeline(
        &mut self,
        slot: Slot,
        item: &PreparedRun,
        settings: &BackendAudioSettings,
        volume: f64,
        muted: bool,
        startup_state: gst::State,
    ) -> Result<PipelineId, String> {
        let id = self.next_pipeline_id();
        lock_recover(&self.shared).set_pipeline_id(slot, Some(id));
        let result = self.pipeline_for_slot_mut(slot).play_item(
            id,
            slot,
            item,
            settings,
            volume,
            muted,
            startup_state,
        );
        if result.is_err() {
            let mut shared = lock_recover(&self.shared);
            if shared.pipeline_id(slot) == Some(id) {
                shared.set_pipeline_id(slot, None);
            }
        }
        result.map(|()| id)
    }

    fn stop_pipeline(&mut self, slot: Slot) {
        lock_recover(&self.shared).set_pipeline_id(slot, None);
        self.pipeline_for_slot_mut(slot).stop();
    }

    fn clear_incoming(&mut self) {
        if let Some(incoming) = self.incoming.take() {
            self.stop_pipeline(incoming.slot);
        }
    }

    fn prepare_incoming(&mut self, next: &PreparedNext) -> Result<(), String> {
        if self
            .incoming
            .as_ref()
            .is_some_and(|incoming| incoming.item == *next)
        {
            return Ok(());
        }
        let context = (|| {
            let shared = lock_recover(&self.shared);
            if shared.crossfade.is_some() {
                return None;
            }
            let current = shared.current.as_ref()?;
            let should_prepare = match next.transition {
                NextTransition::Crossfade { .. } => true,
                NextTransition::Gapless => {
                    next.stream.window().is_some()
                        && !streams_are_adjacent_windows(&current.stream, &next.stream)
                }
                NextTransition::Default => false,
            };
            should_prepare.then(|| {
                (
                    inactive_slot(shared.active),
                    shared.settings.clone(),
                    shared.settings.muted,
                )
            })
        })();
        let Some((slot, settings, muted)) = context else {
            self.clear_incoming();
            return Ok(());
        };

        self.clear_incoming();
        self.stop_pipeline(slot);
        let item = PreparedRun::from_next(next);
        let id = self.start_pipeline(slot, &item, &settings, 0.0, muted, gst::State::Paused)?;
        self.incoming = Some(IncomingPipeline {
            id,
            slot,
            item: next.clone(),
            phase: IncomingPhase::Prerolling,
        });
        Ok(())
    }

    fn incoming_matches(&self, slot: Slot, id: PipelineId) -> bool {
        self.incoming
            .as_ref()
            .is_some_and(|incoming| incoming.slot == slot && incoming.id == id)
    }

    fn handle_incoming_async_done(&mut self, slot: Slot, id: PipelineId) {
        let Some(incoming) = self
            .incoming
            .as_mut()
            .filter(|incoming| incoming.slot == slot && incoming.id == id)
        else {
            return;
        };
        match incoming.phase {
            IncomingPhase::Prerolling if incoming.item.stream.end_millis().is_some() => {
                incoming.phase = IncomingPhase::Seeking;
                if let Err(error) = self.pipeline_for_slot(slot).seek_millis(0) {
                    self.fail_incoming(slot, id, error);
                }
            }
            IncomingPhase::Prerolling | IncomingPhase::Seeking => {
                incoming.phase = IncomingPhase::Ready;
            }
            IncomingPhase::Ready => {}
        }
    }

    fn fail_incoming(&mut self, slot: Slot, id: PipelineId, error: String) {
        let Some(incoming) = self
            .incoming
            .take()
            .filter(|incoming| incoming.slot == slot && incoming.id == id)
        else {
            return;
        };
        self.stop_pipeline(slot);
        let current_run = self.timing_run_id();
        let mut shared = lock_recover(&self.shared);
        if shared
            .next
            .as_ref()
            .is_some_and(|next| next.run == incoming.item.run)
        {
            shared.next = None;
        }
        drop(shared);
        if let Some(current_run) = current_run {
            push_event(
                &self.events,
                BackendEvent::NextUnavailable {
                    current_run,
                    next_run: incoming.item.run,
                    error: BackendFailure::new(error),
                },
            );
        }
    }

    fn handle_command(&mut self, command: BackendCommand) {
        let command_run = command.run();
        let result = match command {
            BackendCommand::Start {
                run,
                current,
                next,
                start_position_millis,
            } => self.play_prepared(
                PreparedRun {
                    run,
                    stream: current,
                },
                next,
                start_position_millis,
            ),
            BackendCommand::PrepareNext { current_run, next } => {
                if self.run_is_current(current_run) {
                    self.prepare_next(next)
                } else {
                    Ok(())
                }
            }
            BackendCommand::ConfigureAudio(settings) => {
                self.cancel_status_fade();
                (|| -> Result<(), String> {
                    let visualizer_enabled = self.visualizer_enabled();
                    lock_recover(&self.shared).settings = settings.clone();
                    self.primary.configure_audio(&settings)?;
                    self.secondary.configure_audio(&settings)?;
                    self.sync_visualizer_taps(visualizer_enabled);
                    let (volume, muted) = self.output_state();
                    self.primary.set_output_volume(volume, muted);
                    self.secondary.set_output_volume(volume, muted);
                    push_event(
                        &self.events,
                        BackendEvent::AudioApplied {
                            volume,
                            muted,
                            output: settings.audio_output.clone(),
                        },
                    );
                    Ok(())
                })()
            }
            BackendCommand::SetOutputVolume { volume, muted } => {
                let volume = if volume.is_finite() {
                    volume.clamp(0.0, 1.0)
                } else {
                    1.0
                };
                {
                    let mut shared = lock_recover(&self.shared);
                    shared.settings.volume = volume;
                    shared.settings.muted = muted;
                }
                self.primary.set_output_volume(volume, muted);
                self.secondary.set_output_volume(volume, muted);
                push_event(
                    &self.events,
                    BackendEvent::AudioApplied {
                        volume,
                        muted,
                        output: lock_recover(&self.shared).settings.audio_output.clone(),
                    },
                );
                Ok(())
            }
            BackendCommand::SetVisualizerEnabled(enabled) => self.set_visualizer_enabled(enabled),
            BackendCommand::Play { run } => {
                if self.run_is_current(run) {
                    self.start_status_resume()
                } else {
                    Ok(())
                }
            }
            BackendCommand::Pause { run } => {
                if self.run_is_current(run) {
                    self.start_status_pause()
                } else {
                    Ok(())
                }
            }
            BackendCommand::Stop { run } => {
                if !self.run_is_current(run) {
                    return;
                }
                let _ = self.cancel_status_fade();
                self.pending_seek = None;
                self.ended_run = None;
                self.incoming = None;
                self.stop_pipeline(Slot::Primary);
                self.stop_pipeline(Slot::Secondary);
                {
                    let mut shared = lock_recover(&self.shared);
                    shared.current = None;
                    shared.next = None;
                    shared.gapless_pending = None;
                    shared.about_to_finish_pending = false;
                    shared.next_needed = None;
                    shared.crossfade = None;
                    shared.active = Slot::Primary;
                }
                self.primary.set_visualizer_tap(None);
                self.secondary.set_visualizer_tap(None);
                push_event(&self.events, BackendEvent::Position { run, millis: 0 });
                self.state = BackendState::Stopped;
                push_event(
                    &self.events,
                    BackendEvent::State {
                        run,
                        state: BackendState::Stopped,
                    },
                );
                Ok(())
            }
            BackendCommand::Seek {
                run,
                position_millis,
            } => {
                if self.run_is_current(run) {
                    self.start_seek(position_millis)
                } else {
                    Ok(())
                }
            }
        };

        if let Err(error) = result
            && let Some(run) = command_run.or_else(|| self.timing_run_id())
        {
            push_event(
                &self.events,
                BackendEvent::Error {
                    run,
                    error: BackendFailure::new(error),
                },
            );
        }
    }

    fn play_prepared(
        &mut self,
        item: PreparedRun,
        next: Option<PreparedNext>,
        start_position_millis: u64,
    ) -> Result<(), String> {
        let incoming_next = next.clone();
        let command_started_at = Instant::now();
        self.play_command_started_at = Some(command_started_at);
        let _ = self.cancel_status_fade();
        self.pending_seek = None;
        self.ended_run = None;
        self.clear_incoming();
        self.restore_output_on_playing = false;
        let settings = self.settings();
        self.stop_pipeline(Slot::Primary);
        self.stop_pipeline(Slot::Secondary);
        self.secondary.set_visualizer_tap(None);
        let volume = settings.volume;
        let muted = settings.muted;
        let start_millis =
            SourceClock::from_stream(&item.stream).physical_seek(start_position_millis);
        let visualizer_enabled = {
            let mut shared = lock_recover(&self.shared);
            shared.settings = settings.clone();
            shared.current = Some(item.clone());
            shared.next = next;
            shared.gapless_pending = None;
            shared.about_to_finish_pending = false;
            shared.crossfade = None;
            shared.active = Slot::Primary;
            shared.visualizer_enabled
        };
        if visualizer_enabled {
            push_event(
                &self.events,
                BackendEvent::Visualizer {
                    run: item.run,
                    levels: Vec::new(),
                },
            );
        }
        self.push_state(BackendState::Buffering);
        let pipeline_started_at = Instant::now();
        let needs_preroll_seek = start_millis > 0 || item.stream.end_millis().is_some();
        let startup_state = if needs_preroll_seek {
            gst::State::Paused
        } else {
            gst::State::Playing
        };
        self.start_pipeline(
            Slot::Primary,
            &item,
            &settings,
            volume,
            muted,
            startup_state,
        )?;
        self.restore_output_on_playing = true;
        let primary_tap = self.visualizer_tap(Slot::Primary, visualizer_enabled);
        self.primary.set_visualizer_tap(primary_tap);
        info!(
            run = %item.run,
            uri_scheme = %stream_uri_scheme(item.stream.uri()),
            stream_windowed = item.stream.end_millis().is_some(),
            start_millis,
            audio_output = self.primary.audio_output_factory().as_deref().unwrap_or("unknown"),
            elapsed_ms = command_started_at.elapsed().as_millis(),
            pipeline_ms = pipeline_started_at.elapsed().as_millis(),
            "queued GStreamer playback item"
        );
        if needs_preroll_seek {
            self.start_playback_seek(start_millis);
        } else {
            self.pending_seek = Some(PendingSeek::track_start(Instant::now()));
        }
        if let Some(duration) = self.primary.fixed_duration() {
            self.push_duration(duration);
        }
        if let Some(next) = incoming_next.as_ref() {
            self.prepare_incoming(next)?;
        }
        Ok(())
    }

    fn prepare_next(&mut self, next: Option<PreparedNext>) -> Result<(), String> {
        let Some(next) = next else {
            self.clear_prepared_next();
            return Ok(());
        };

        let late_preload = {
            let mut shared = lock_recover(&self.shared);
            let mut late_preload = None;
            shared.next = Some(next.clone());
            shared.next_needed = None;
            if shared.about_to_finish_pending && gapless_preload_should_run(&shared, &next) {
                if next.stream.end_millis().is_none()
                    && gapless_preload_source_is_supported(next.stream.uri())
                    && let Some(item) = shared.next.take()
                {
                    shared.gapless_pending = Some(item.clone());
                    shared.about_to_finish_pending = false;
                    late_preload = Some(item);
                }
                shared.about_to_finish_pending = false;
            }
            if shared.about_to_finish_pending && !gapless_preload_should_run(&shared, &next) {
                shared.about_to_finish_pending = false;
            }
            late_preload
        };
        if let Some(item) = late_preload {
            info!(
                next_run = %item.run,
                uri = %item.stream.redacted_uri(),
                "preloading late gapless next stream"
            );
            self.active_pipeline().set_stream(&item.stream)?;
        }
        self.prepare_incoming(&next)
    }

    fn clear_prepared_next(&mut self) {
        self.clear_incoming();
        let clear = clear_prepared_next_state(&mut lock_recover(&self.shared));
        if let Some((slot, current)) = clear.gapless_current {
            debug!(
                run = %current.run,
                "cleared pending gapless next stream"
            );
            if let Err(error) = self.pipeline_for_slot(slot).set_stream(&current.stream) {
                warn!(
                    %error,
                    run = %current.run,
                    "failed to restore current stream after clearing pending gapless next"
                );
            }
        }
    }

    pub(super) fn start_seek(&mut self, millis: u64) -> Result<(), String> {
        let logical_state = self.state;
        self.ended_run = None;
        let _ = self.cancel_status_fade();
        self.finish_crossfade_for_seek();
        let current_after_gapless_cancel = self.cancel_gapless_pending_for_seek();
        let target_state = match logical_state {
            BackendState::Paused | BackendState::Stopped => gst::State::Paused,
            BackendState::Buffering | BackendState::Playing => gst::State::Playing,
        };
        if let Some(current) = current_after_gapless_cancel {
            let (start_millis, needs_preroll_seek) =
                self.start_item_session_at_millis(current, millis, target_state)?;
            self.pending_seek = pending_seek_for_session_restart(
                start_millis,
                millis,
                logical_state,
                target_state,
                needs_preroll_seek,
                Instant::now(),
            );
            self.push_logical_position(millis);
            return Ok(());
        }
        if millis == 0 {
            let current = self.current_item()?;
            let (start_millis, needs_preroll_seek) =
                self.start_item_session_at_millis(current, 0, target_state)?;
            self.pending_seek = pending_seek_for_session_restart(
                start_millis,
                0,
                logical_state,
                target_state,
                needs_preroll_seek,
                Instant::now(),
            );
            self.push_logical_position(0);
            return Ok(());
        }
        if logical_state == BackendState::Paused {
            self.active_pipeline().set_state(gst::State::Paused)?;
        }
        let physical_target = self.active_pipeline().physical_seek_target(millis);
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
        self.pending_seek = Some(PendingSeek::interactive(
            physical_target,
            logical_state,
            Instant::now(),
        ));
        Ok(())
    }

    fn start_playback_seek(&mut self, millis: u64) {
        debug!(
            target_millis = millis,
            "deferring startup seek until GStreamer preroll completes"
        );
        self.pending_seek = Some(PendingSeek::startup(millis, self.state, Instant::now()));
    }

    fn cancel_gapless_pending_for_seek(&mut self) -> Option<PreparedRun> {
        cancel_gapless_pending(&mut lock_recover(&self.shared)).map(|(current, _pending)| current)
    }

    fn current_item(&self) -> Result<PreparedRun, String> {
        lock_recover(&self.shared)
            .current
            .clone()
            .ok_or_else(|| "No current playback item is active".to_string())
    }

    fn start_item_session_at_millis(
        &mut self,
        item: PreparedRun,
        position_millis: u64,
        target_state: gst::State,
    ) -> Result<(u64, bool), String> {
        let (settings, volume, muted, visualizer_enabled, slot) = self.session_context();
        let start_millis = SourceClock::from_stream(&item.stream).physical_seek(position_millis);
        let needs_preroll_seek = start_millis > 0 || item.stream.end_millis().is_some();
        let startup_state = if needs_preroll_seek {
            gst::State::Paused
        } else {
            target_state
        };
        self.stop_pipeline(slot);
        self.start_pipeline(slot, &item, &settings, volume, muted, startup_state)?;
        let tap = self.visualizer_tap(slot, visualizer_enabled);
        self.pipeline_for_slot_mut(slot).set_visualizer_tap(tap);
        Ok((start_millis, needs_preroll_seek))
    }

    fn session_context(&self) -> (BackendAudioSettings, f64, bool, bool, Slot) {
        let shared = lock_recover(&self.shared);
        (
            shared.settings.clone(),
            shared.settings.volume,
            shared.settings.muted,
            shared.visualizer_enabled,
            shared.active,
        )
    }

    fn poll_bus(&mut self) {
        while let Some((id, message)) = self.primary.pop_bus_message() {
            self.handle_message(Slot::Primary, id, &message);
        }
        while let Some((id, message)) = self.secondary.pop_bus_message() {
            self.handle_message(Slot::Secondary, id, &message);
        }
    }

    fn handle_message(&mut self, slot: Slot, id: PipelineId, message: &gst::Message) {
        if !lock_recover(&self.shared).pipeline_is_live(slot, id) {
            return;
        }
        use gst::MessageView;

        match message.view() {
            MessageView::AsyncDone(_) if self.incoming_matches(slot, id) => {
                self.handle_incoming_async_done(slot, id);
                return;
            }
            MessageView::Error(error) if self.incoming_matches(slot, id) => {
                self.fail_incoming(slot, id, error.error().to_string());
                return;
            }
            _ => {}
        }

        match message.view() {
            MessageView::StateChanged(state)
                if self.message_source_is_pipeline(slot, message) && self.is_active_slot(slot) =>
            {
                if let Some(started_at) = self.play_command_started_at {
                    let run = self.timing_run_id();
                    debug!(
                        run = run.map(RunId::get).unwrap_or_default(),
                        ?slot,
                        old = ?state.old(),
                        current = ?state.current(),
                        pending = ?state.pending(),
                        elapsed_ms = started_at.elapsed().as_millis(),
                        "GStreamer startup state changed"
                    );
                }
                let playback_state = match state.current() {
                    gst::State::Null | gst::State::Ready => BackendState::Stopped,
                    gst::State::Paused => BackendState::Paused,
                    gst::State::Playing => BackendState::Playing,
                    gst::State::VoidPending => BackendState::Buffering,
                };
                self.handle_state_changed(playback_state);
            }
            MessageView::AsyncDone(_) if self.is_active_slot(slot) => {
                if let Some(started_at) = self.play_command_started_at {
                    let run = self.timing_run_id();
                    debug!(
                        run = run.map(RunId::get).unwrap_or_default(),
                        ?slot,
                        elapsed_ms = started_at.elapsed().as_millis(),
                        "GStreamer startup async done"
                    );
                }
                self.handle_async_done();
            }
            MessageView::StreamStart(_) if self.is_active_slot(slot) => {
                if let Some(started_at) = self.play_command_started_at {
                    let run = self.timing_run_id();
                    debug!(
                        run = run.map(RunId::get).unwrap_or_default(),
                        ?slot,
                        elapsed_ms = started_at.elapsed().as_millis(),
                        "GStreamer startup stream start"
                    );
                }
                self.handle_stream_start();
            }
            MessageView::Tag(tag) if self.is_active_slot(slot) => {
                self.log_stream_diagnostics(slot, &tag.tags());
            }
            MessageView::DurationChanged(_) if self.is_active_slot(slot) => {
                if self.pending_seek.is_none()
                    && let Some(duration) = self.active_pipeline().duration()
                {
                    self.push_physical_duration(clock_millis(duration));
                }
            }
            MessageView::Buffering(buffering) if self.is_active_slot(slot) => {
                let percent = buffering.percent().min(100) as u8;
                if matches!(percent, 1 | 25 | 50 | 75 | 100)
                    && let Some(started_at) = self.play_command_started_at
                {
                    let run = self.timing_run_id();
                    debug!(
                        run = run.map(RunId::get).unwrap_or_default(),
                        ?slot,
                        percent,
                        elapsed_ms = started_at.elapsed().as_millis(),
                        "GStreamer startup buffering"
                    );
                }
                self.handle_buffering(percent);
            }
            MessageView::SegmentDone(_) => self.handle_end(slot, true),
            MessageView::Eos(_) => self.handle_end(slot, false),
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
                    let run = self.run_for_slot(slot).or_else(|| self.timing_run_id());
                    self.stop_after_playback_error();
                    if let Some(run) = run {
                        push_event(
                            &self.events,
                            BackendEvent::Error {
                                run,
                                error: BackendFailure::new(error),
                            },
                        );
                        self.state = BackendState::Stopped;
                        push_event(
                            &self.events,
                            BackendEvent::State {
                                run,
                                state: BackendState::Stopped,
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn log_stream_diagnostics(&self, slot: Slot, tags: &gst::TagListRef) {
        let codec = tags
            .get::<gst::tags::AudioCodec>()
            .map(|value| value.get().to_string())
            .or_else(|| {
                tags.get::<gst::tags::Codec>()
                    .map(|value| value.get().to_string())
            })
            .or_else(|| {
                tags.get::<gst::tags::ContainerFormat>()
                    .map(|value| value.get().to_string())
            });
        let bitrate = tags
            .get::<gst::tags::Bitrate>()
            .map(|value| value.get())
            .or_else(|| {
                tags.get::<gst::tags::NominalBitrate>()
                    .map(|value| value.get())
            })
            .map(|bits_per_second| bits_per_second / 1_000);
        if codec.is_none() && bitrate.is_none() {
            return;
        }
        let run = self.run_for_slot(slot).or_else(|| self.timing_run_id());
        debug!(
            run = run.map(RunId::get).unwrap_or_default(),
            codec = codec.as_deref().unwrap_or("unknown"),
            reported_bitrate_kbps = bitrate,
            "received GStreamer stream metadata"
        );
    }

    fn handle_transition_error(&mut self, slot: Slot, error: &str) -> bool {
        self.handle_gapless_preload_error(slot, error)
            || self.handle_crossfade_next_error(slot, error)
    }

    fn handle_gapless_preload_error(&mut self, slot: Slot, error: &str) -> bool {
        let reset = (|| {
            let mut shared = lock_recover(&self.shared);
            if shared.active != slot {
                return None;
            }
            cancel_gapless_pending(&mut shared)
        })();
        let Some((current, pending)) = reset else {
            return false;
        };
        warn!(
            next_run = %pending.run,
            error = %error,
            "gapless next stream failed before commit"
        );
        let target_state = match self.state {
            BackendState::Paused | BackendState::Stopped => gst::State::Paused,
            BackendState::Buffering | BackendState::Playing => gst::State::Playing,
        };
        if let Err(reset_error) =
            self.start_item_session_at_millis(current.clone(), 0, target_state)
        {
            warn!(
                %reset_error,
                run = %current.run,
                "failed to restart current stream after gapless preload error"
            );
            return false;
        }
        push_event(
            &self.events,
            BackendEvent::NextUnavailable {
                current_run: current.run,
                next_run: pending.run,
                error: BackendFailure::new(error),
            },
        );
        true
    }

    fn handle_crossfade_next_error(&mut self, slot: Slot, error: &str) -> bool {
        let crossfade = lock_recover(&self.shared).crossfade.clone();
        let Some(crossfade) = crossfade else {
            return false;
        };
        if slot == crossfade.from {
            warn!(%error, old_run = %crossfade.old_run, "outgoing crossfade tail failed");
            self.finish_crossfade(crossfade);
            return true;
        }
        false
    }

    fn handle_stream_start(&mut self) {
        let started = (|| {
            let mut shared = lock_recover(&self.shared);
            let item = shared.gapless_pending.take()?;
            let old_run = shared.current.as_ref()?.run;
            shared.current = Some(PreparedRun::from_next(&item));
            shared.about_to_finish_pending = false;
            Some((old_run, item.run, item.stream))
        })();
        self.handle_stream_started_run(started);
    }

    fn handle_stream_started_run(&mut self, started: Option<(RunId, RunId, ResolvedStream)>) {
        let Some((old_run, new_run, stream)) = started else {
            return;
        };
        info!(
            old_run = %old_run,
            new_run = %new_run,
            "gapless stream started"
        );
        self.pending_seek = None;
        self.ended_run = None;
        self.last_position_tick = Instant::now();
        self.pipeline_for_slot_mut(self.active_slot())
            .set_source_clock(&stream);
        let visualizer_enabled = self.visualizer_enabled();
        self.sync_visualizer_taps(visualizer_enabled);
        if visualizer_enabled {
            push_event(
                &self.events,
                BackendEvent::Visualizer {
                    run: new_run,
                    levels: Vec::new(),
                },
            );
        }
        push_event(
            &self.events,
            BackendEvent::Transitioned { old_run, new_run },
        );
        push_event(
            &self.events,
            BackendEvent::Position {
                run: new_run,
                millis: 0,
            },
        );
    }

    pub(super) fn handle_state_changed(&mut self, state: BackendState) {
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
        if state == BackendState::Playing
            && self
                .pending_seek
                .as_ref()
                .is_some_and(PendingSeek::is_track_start)
        {
            self.pending_seek = None;
        }
        if state == BackendState::Playing
            && self.status_fade.is_none()
            && self.restore_output_on_playing
        {
            let (volume, muted) = self.output_state();
            self.active_pipeline().set_output_volume(volume, muted);
            self.restore_output_on_playing = false;
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
        self.state = BackendState::Buffering;
        if let Some(run) = self.timing_run_id() {
            push_event(&self.events, BackendEvent::Buffering { run, percent });
        }
    }

    fn gapless_preload_near_end(&self) -> bool {
        if lock_recover(&self.shared).gapless_pending.is_none() {
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
        let remaining_ms = self
            .active_pipeline()
            .logical_remaining(position_ms, duration_ms);
        duration_ms > 0 && position_ms > 0 && remaining_ms < GAPLESS_BUFFERING_IGNORE_REMAINING_MS
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
        let seek_result = self.active_pipeline().seek_physical_millis(target_millis);
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
            if self.restore_output_on_playing {
                let (volume, muted) = self.output_state();
                self.active_pipeline().set_output_volume(volume, muted);
                self.restore_output_on_playing = false;
            }
            self.push_state(BackendState::Playing);
        }
    }

    fn push_state(&mut self, state: BackendState) {
        let run = self.timing_run_id();
        if state == BackendState::Playing
            && let Some(started_at) = self.play_command_started_at.take()
        {
            info!(
                run = run.map(RunId::get).unwrap_or_default(),
                elapsed_ms = started_at.elapsed().as_millis(),
                "GStreamer playback reached playing"
            );
            if let Some(run) = run {
                push_event(&self.events, BackendEvent::Started { run });
            }
        }
        self.state = state;
        if let Some(run) = run {
            push_event(&self.events, BackendEvent::State { run, state });
        }
    }

    fn handle_end(&mut self, slot: Slot, stream_window: bool) {
        if self.finish_crossfade_if_needed(slot) {
            return;
        }
        if stream_window && self.is_active_slot(slot) && self.promote_adjacent_stream_window() {
            return;
        }
        if self.is_active_slot(slot) && self.promote_prepared_gapless() {
            return;
        }
        if self.is_active_slot(slot) {
            let run = self.timing_run_id();
            info!(
                run = run.map(RunId::get).unwrap_or_default(),
                stream_window, "playback reached end"
            );
            if let Some(run) = run {
                self.emit_ended_once(run);
            }
        }
    }

    fn start_status_pause(&mut self) -> Result<(), String> {
        let _ = self.cancel_status_fade();
        if let Some(pending) = self.pending_seek.as_mut() {
            pending.set_desired_playing(false);
        }
        self.state = BackendState::Paused;
        self.finish_crossfade_for_visible_current();
        let (volume, muted, enabled) = self.status_fade_settings();
        if !self.active_pipeline().has_session() {
            self.push_state(BackendState::Paused);
            return Ok(());
        }
        if !enabled || muted || volume <= 0.0 {
            self.active_pipeline().set_state(gst::State::Paused)?;
            self.push_state(BackendState::Paused);
            return Ok(());
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
        let _ = self.cancel_status_fade();
        let waiting_for_preroll = if let Some(pending) = self.pending_seek.as_mut() {
            pending.set_desired_playing(true);
            pending.retry_on_async_done
        } else {
            false
        };
        if waiting_for_preroll {
            self.push_state(BackendState::Buffering);
            return Ok(());
        }
        if !self.active_pipeline().has_session() {
            return if self.current_item().is_ok() {
                Err("No active GStreamer session to resume".to_string())
            } else {
                Ok(())
            };
        }
        let (volume, muted, enabled) = self.status_fade_settings();
        if !enabled || muted || volume <= 0.0 {
            return self
                .active_pipeline()
                .set_state(gst::State::Playing)
                .map(|_| {
                    self.push_state(BackendState::Playing);
                });
        }
        let slot = self.active_slot();
        self.pipeline_for_slot(slot).set_output_volume(0.0, muted);
        self.pipeline_for_slot(slot)
            .set_state(gst::State::Playing)
            .map(|_| {
                self.push_state(BackendState::Playing);
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
                    if let Some(run) = self.timing_run_id() {
                        push_event(
                            &self.events,
                            BackendEvent::Error {
                                run,
                                error: BackendFailure::new(error),
                            },
                        );
                    }
                    return;
                }
                self.push_state(BackendState::Paused);
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

    fn cancel_status_fade(&mut self) -> Option<StatusFade> {
        let fade = self.status_fade.take();
        if let Some(fade) = fade {
            let (volume, muted) = self.output_state();
            self.pipeline_for_slot(fade.slot)
                .set_output_volume(volume, muted);
        }
        fade
    }

    fn status_fade_settings(&self) -> (f64, bool, bool) {
        let shared = lock_recover(&self.shared);
        (
            shared.settings.volume,
            shared.settings.muted,
            shared.settings.fade_on_status_change,
        )
    }

    fn tick(&mut self) {
        let next_needed = lock_recover(&self.shared).next_needed.take();
        if let Some(run) = next_needed {
            push_event(&self.events, BackendEvent::NextNeeded { run });
        }
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
                && let Some(duration) = self.active_pipeline().duration()
            {
                self.push_physical_duration(clock_millis(duration));
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
        let logical_millis = self.active_pipeline().logical_position(millis);
        if let Some(run) = self.timing_run_id() {
            if self.ended_run == Some(run) {
                return;
            }
            self.push_logical_position(logical_millis);
        }
    }

    fn promote_adjacent_stream_window(&mut self) -> bool {
        let candidate = (|| {
            let shared = lock_recover(&self.shared);
            let current = shared.current.as_ref()?;
            let boundary = current.stream.end_millis()?;
            let next = shared.next.as_ref()?;
            if next.transition != NextTransition::Gapless
                || next.stream.uri() != current.stream.uri()
                || next.stream.start_millis() != boundary
            {
                return None;
            }
            Some((current.run, next.clone()))
        })();
        let Some((old_run, next)) = candidate else {
            return false;
        };

        let slot = self.active_slot();
        if let Err(error) = self
            .pipeline_for_slot_mut(slot)
            .rearm_stream_window(&next.stream)
        {
            push_event(
                &self.events,
                BackendEvent::NextUnavailable {
                    current_run: old_run,
                    next_run: next.run,
                    error: BackendFailure::new(error),
                },
            );
            return false;
        }
        let visualizer_enabled = {
            let mut shared = lock_recover(&self.shared);
            let Some(committed) = shared.next.take() else {
                return false;
            };
            shared.current = Some(PreparedRun::from_next(&committed));
            shared.gapless_pending = None;
            shared.about_to_finish_pending = false;
            shared.next_needed = None;
            shared.visualizer_enabled
        };
        let new_run = next.run;
        self.pending_seek = None;
        self.ended_run = None;
        self.last_position_tick = Instant::now();
        self.sync_visualizer_taps(visualizer_enabled);
        if visualizer_enabled {
            push_event(
                &self.events,
                BackendEvent::Visualizer {
                    run: new_run,
                    levels: Vec::new(),
                },
            );
        }
        push_event(
            &self.events,
            BackendEvent::Transitioned { old_run, new_run },
        );
        push_event(
            &self.events,
            BackendEvent::Position {
                run: new_run,
                millis: 0,
            },
        );
        true
    }

    fn promote_prepared_gapless(&mut self) -> bool {
        let Some(incoming) = self.incoming.as_ref().filter(|incoming| {
            incoming.phase == IncomingPhase::Ready
                && incoming.item.transition == NextTransition::Gapless
        }) else {
            return false;
        };
        let slot = incoming.slot;
        let id = incoming.id;
        if let Err(error) = self.pipeline_for_slot(slot).set_state(gst::State::Playing) {
            self.fail_incoming(slot, id, error);
            return false;
        }
        let Some(incoming) = self.incoming.take() else {
            return false;
        };
        let old_slot = self.active_slot();
        let old_run = self.timing_run_id();
        let new_run = incoming.item.run;
        self.stop_pipeline(old_slot);
        let (volume, muted) = self.output_state();
        self.pipeline_for_slot(slot)
            .set_output_volume(volume, muted);
        let visualizer_enabled = {
            let mut shared = lock_recover(&self.shared);
            shared.active = slot;
            shared.current = Some(PreparedRun::from_next(&incoming.item));
            shared.next = None;
            shared.gapless_pending = None;
            shared.about_to_finish_pending = false;
            shared.visualizer_enabled
        };
        let Some(old_run) = old_run else {
            return false;
        };
        self.pending_seek = None;
        self.ended_run = None;
        self.last_position_tick = Instant::now();
        self.sync_visualizer_taps(visualizer_enabled);
        if visualizer_enabled {
            push_event(
                &self.events,
                BackendEvent::Visualizer {
                    run: new_run,
                    levels: Vec::new(),
                },
            );
        }
        push_event(
            &self.events,
            BackendEvent::Transitioned { old_run, new_run },
        );
        push_event(
            &self.events,
            BackendEvent::Position {
                run: new_run,
                millis: 0,
            },
        );
        true
    }

    fn push_logical_position(&self, millis: u64) {
        if let Some(run) = self.timing_run_id() {
            push_event(&self.events, BackendEvent::Position { run, millis });
        }
    }

    pub(super) fn push_duration(&self, millis: u64) {
        if let Some(run) = self.timing_run_id() {
            push_event(&self.events, BackendEvent::Duration { run, millis });
        }
    }

    fn push_physical_duration(&self, millis: u64) {
        self.push_duration(self.active_pipeline().logical_duration(millis));
    }

    fn emit_ended_once(&mut self, run: RunId) {
        if self.ended_run == Some(run) {
            return;
        }
        self.ended_run = Some(run);
        push_event(&self.events, BackendEvent::Ended { run });
    }

    fn timing_run_id(&self) -> Option<RunId> {
        lock_recover(&self.shared)
            .current
            .as_ref()
            .map(|item| item.run)
    }

    fn run_is_current(&self, run: RunId) -> bool {
        self.timing_run_id() == Some(run)
    }

    fn maybe_start_crossfade(&mut self) {
        if self.pending_seek.is_some() {
            return;
        }
        let request = (|| {
            let shared = lock_recover(&self.shared);
            if shared.crossfade.is_some() {
                return None;
            }
            let next = shared.next.clone()?;
            let NextTransition::Crossfade {
                duration_millis: crossfade_ms,
            } = next.transition
            else {
                return None;
            };
            Some((
                next,
                shared.active,
                shared.settings.volume,
                shared.settings.muted,
                crossfade_ms,
            ))
        })();

        let Some((next, from, volume, muted, crossfade_ms)) = request else {
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
        let logical_position = self.active_pipeline().logical_position(position_ms);
        let logical_duration = self.active_pipeline().logical_duration(duration_ms);
        let remaining = self
            .active_pipeline()
            .logical_remaining(position_ms, duration_ms);
        if logical_duration == 0
            || logical_position >= logical_duration
            || remaining > crossfade_ms
            || logical_duration <= crossfade_ms + 1_000
        {
            return;
        }
        if !self
            .incoming
            .as_ref()
            .is_some_and(|incoming| incoming.item == next && incoming.phase == IncomingPhase::Ready)
        {
            return;
        }
        let Some(incoming_plan) = self.incoming.take() else {
            return;
        };
        let to = incoming_plan.slot;
        if let Err(error) = self.pipeline_for_slot(to).set_state(gst::State::Playing) {
            self.stop_pipeline(to);
            if let Some(current_run) = self.timing_run_id() {
                push_event(
                    &self.events,
                    BackendEvent::NextUnavailable {
                        current_run,
                        next_run: next.run,
                        error: BackendFailure::new(error),
                    },
                );
            }
            return;
        }
        let visualizer_enabled = self.visualizer_enabled();
        let old_run = {
            let mut shared = lock_recover(&self.shared);
            let Some(old_run) = shared.current.as_ref().map(|current| current.run) else {
                return;
            };
            shared.next = None;
            shared.active = to;
            shared.current = Some(PreparedRun::from_next(&next));
            shared.crossfade = Some(CrossfadeState {
                from,
                to,
                old_run,
                started_at: Instant::now(),
                duration: Duration::from_millis(crossfade_ms),
            });
            old_run
        };
        self.pipeline_for_slot(from)
            .set_output_volume(volume, muted);
        self.ended_run = None;
        let tap = self.visualizer_tap(to, visualizer_enabled);
        self.pipeline_for_slot_mut(to).set_visualizer_tap(tap);
        push_event(
            &self.events,
            BackendEvent::Transitioned {
                old_run,
                new_run: next.run,
            },
        );
        push_event(
            &self.events,
            BackendEvent::Position {
                run: next.run,
                millis: 0,
            },
        );
    }

    fn update_crossfade(&mut self) {
        let Some(crossfade) = lock_recover(&self.shared).crossfade.clone() else {
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
        let crossfade = lock_recover(&self.shared).crossfade.clone();
        if let Some(crossfade) = crossfade
            && crossfade.from == eos_slot
        {
            self.finish_crossfade(crossfade);
            return true;
        }
        false
    }

    pub(super) fn finish_crossfade_for_seek(&mut self) {
        self.finish_crossfade_for_visible_current();
    }

    fn finish_crossfade_for_visible_current(&mut self) {
        let crossfade = lock_recover(&self.shared).crossfade.clone();
        if let Some(crossfade) = crossfade {
            self.finish_crossfade(crossfade);
        }
    }

    fn finish_crossfade(&mut self, crossfade: CrossfadeState) {
        self.pending_seek = None;
        self.stop_pipeline(crossfade.from);
        let (volume, muted) = self.output_state();
        self.pipeline_for_slot(crossfade.to)
            .set_output_volume(volume, muted);
        let retained_next = {
            let mut shared = lock_recover(&self.shared);
            shared.active = crossfade.to;
            shared.crossfade = None;
            shared.gapless_pending = None;
            shared.about_to_finish_pending = false;
            shared.next.clone()
        };
        if let Some(next) = retained_next
            && let Err(error) = self.prepare_incoming(&next)
        {
            warn!(
                %error,
                current_run = self.timing_run_id().map(RunId::get),
                next_run = %next.run,
                "failed to prepare the next stream after crossfade"
            );
        }
    }

    fn settings(&self) -> BackendAudioSettings {
        lock_recover(&self.shared).settings.clone()
    }

    fn output_state(&self) -> (f64, bool) {
        let shared = lock_recover(&self.shared);
        (shared.settings.volume, shared.settings.muted)
    }

    fn visualizer_enabled(&self) -> bool {
        lock_recover(&self.shared).visualizer_enabled
    }

    fn visualizer_tap(&self, slot: Slot, enabled: bool) -> Option<VisualizerTap> {
        if !enabled {
            return None;
        }
        let (pipeline_id, run) = (|| {
            let shared = lock_recover(&self.shared);
            if shared.active != slot {
                return None;
            }
            Some((shared.pipeline_id(slot)?, shared.current.as_ref()?.run))
        })()?;
        Some(self.visualizer.tap(slot, pipeline_id, run))
    }

    fn sync_visualizer_taps(&mut self, enabled: bool) {
        let primary_tap = self.visualizer_tap(Slot::Primary, enabled);
        let secondary_tap = self.visualizer_tap(Slot::Secondary, enabled);
        self.primary.set_visualizer_tap(primary_tap);
        self.secondary.set_visualizer_tap(secondary_tap);
    }

    fn set_visualizer_enabled(&mut self, enabled: bool) -> Result<(), String> {
        let changed = {
            let mut shared = lock_recover(&self.shared);
            let changed = shared.visualizer_enabled != enabled;
            shared.visualizer_enabled = enabled;
            changed
        };
        if changed && let Some(run) = self.timing_run_id() {
            push_event(
                &self.events,
                BackendEvent::Visualizer {
                    run,
                    levels: Vec::new(),
                },
            );
        }
        if enabled {
            self.sync_visualizer_taps(true);
        } else if changed {
            self.sync_visualizer_taps(false);
        }
        Ok(())
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
        lock_recover(&self.shared).active
    }

    fn is_active_slot(&self, slot: Slot) -> bool {
        self.active_slot() == slot
    }

    fn error_is_relevant_slot(&self, slot: Slot) -> bool {
        if self.is_active_slot(slot) {
            return true;
        }
        lock_recover(&self.shared)
            .crossfade
            .clone()
            .is_some_and(|crossfade| crossfade.from == slot || crossfade.to == slot)
    }

    fn run_for_slot(&self, slot: Slot) -> Option<RunId> {
        let shared = lock_recover(&self.shared);
        if let Some(crossfade) = shared.crossfade.as_ref()
            && crossfade.from == slot
        {
            return Some(crossfade.old_run);
        }
        (shared.active == slot)
            .then(|| shared.current.as_ref().map(|current| current.run))
            .flatten()
    }

    fn stop_after_playback_error(&mut self) {
        self.pending_seek = None;
        self.incoming = None;
        self.stop_pipeline(Slot::Primary);
        self.stop_pipeline(Slot::Secondary);
        {
            let mut shared = lock_recover(&self.shared);
            shared.next = None;
            shared.gapless_pending = None;
            shared.about_to_finish_pending = false;
            shared.crossfade = None;
            shared.active = Slot::Primary;
        }
    }

    fn message_source_is_pipeline(&self, slot: Slot, message: &gst::Message) -> bool {
        self.pipeline_for_slot(slot)
            .message_source_is_pipeline(message)
    }

    fn shutdown(&mut self) {
        self.incoming = None;
        self.stop_pipeline(Slot::Primary);
        self.stop_pipeline(Slot::Secondary);
    }
}
#[instrument(skip(receiver, events))]
fn run_gstreamer_thread(
    receiver: Receiver<BackendCommand>,
    events: Arc<Mutex<EventMailbox>>,
    ready: SyncSender<Result<(), String>>,
) {
    let startup_started_at = Instant::now();
    if let Err(error) = ensure_gstreamer_initialized() {
        let _ = ready.send(Err(format!("GStreamer init failed: {error}")));
        return;
    }

    let mut engine = GstEngine::new(Arc::clone(&events));
    if ready.send(Ok(())).is_err() {
        return;
    }
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
pub(super) fn handle_about_to_finish(
    pipeline: &gst::Element,
    shared: &Arc<Mutex<SharedBackendState>>,
    trust_invalid_certificate: &AtomicBool,
    slot: Slot,
    id: PipelineId,
) {
    let action = about_to_finish_action_for_pipeline(&mut lock_recover(shared), slot, id);

    match action {
        AboutToFinishAction::Preload(next) => {
            info!(
                next_run = %next.run,
                uri = %next.stream.redacted_uri(),
                "preloading gapless next stream"
            );
            trust_invalid_certificate
                .store(next.stream.trust_invalid_certificate(), Ordering::SeqCst);
            pipeline.set_property("uri", next.stream.uri());
        }
        AboutToFinishAction::Ignore => {}
    }
}

fn about_to_finish_action_for_pipeline(
    shared: &mut SharedBackendState,
    slot: Slot,
    id: PipelineId,
) -> AboutToFinishAction {
    if !shared.pipeline_is_current(slot, id) {
        return AboutToFinishAction::Ignore;
    }
    about_to_finish_action(shared)
}

pub(super) fn about_to_finish_action(shared: &mut SharedBackendState) -> AboutToFinishAction {
    if shared.gapless_pending.is_some() {
        return AboutToFinishAction::Ignore;
    }

    let Some(next) = shared.next.as_ref() else {
        shared.about_to_finish_pending = true;
        shared.next_needed = shared.current.as_ref().map(|current| current.run);
        return AboutToFinishAction::Ignore;
    };

    if !gapless_preload_should_run(shared, next) {
        shared.about_to_finish_pending = false;
        return AboutToFinishAction::Ignore;
    }

    if next.stream.end_millis().is_some() || !gapless_preload_source_is_supported(next.stream.uri())
    {
        debug!(
            next_run = %next.run,
            uri = %next.stream.redacted_uri(),
            "skipping gapless preload for non-local stream"
        );
        shared.about_to_finish_pending = false;
        return AboutToFinishAction::Ignore;
    }

    let Some(next) = shared.next.take() else {
        shared.about_to_finish_pending = false;
        return AboutToFinishAction::Ignore;
    };
    shared.gapless_pending = Some(next.clone());
    shared.about_to_finish_pending = false;
    AboutToFinishAction::Preload(Box::new(next))
}

pub(super) fn cancel_gapless_pending(
    shared: &mut SharedBackendState,
) -> Option<(PreparedRun, PreparedNext)> {
    let pending = shared.gapless_pending.take()?;
    let current = shared.current.clone()?;
    if shared.next.is_none() {
        shared.next = Some(pending.clone());
    }
    shared.about_to_finish_pending = false;
    Some((current, pending))
}

pub(super) fn clear_prepared_next_state(shared: &mut SharedBackendState) -> PreparedNextClear {
    let gapless_current = shared.gapless_pending.take().and_then(|_| {
        shared
            .current
            .clone()
            .map(|current| (shared.active, current))
    });
    shared.next = None;
    shared.about_to_finish_pending = false;
    PreparedNextClear { gapless_current }
}

fn gapless_preload_should_run(_shared: &SharedBackendState, next: &PreparedNext) -> bool {
    next.transition == NextTransition::Gapless
}

pub(super) fn gapless_preload_source_is_supported(uri: &str) -> bool {
    uri.starts_with("file://") || uri.starts_with("http://") || uri.starts_with("https://")
}
fn inactive_slot(slot: Slot) -> Slot {
    match slot {
        Slot::Primary => Slot::Secondary,
        Slot::Secondary => Slot::Primary,
    }
}

fn streams_are_adjacent_windows(current: &ResolvedStream, next: &ResolvedStream) -> bool {
    current
        .end_millis()
        .is_some_and(|boundary| current.uri() == next.uri() && next.start_millis() == boundary)
}
pub(super) fn push_event(events: &Arc<Mutex<EventMailbox>>, event: BackendEvent) {
    lock_recover(events).push(event);
}
fn clock_millis(clock_time: gst::ClockTime) -> u64 {
    clock_time.mseconds()
}
fn seek_position_matches_target(target_millis: u64, millis: u64) -> bool {
    let lower = target_millis.saturating_sub(SEEK_POSITION_TOLERANCE_MILLIS);
    let upper = target_millis.saturating_add(SEEK_POSITION_TOLERANCE_MILLIS);
    (lower..=upper).contains(&millis)
}
fn pending_seek_for_session_restart(
    absolute_start_millis: u64,
    logical_position_millis: u64,
    logical_state: BackendState,
    target_state: gst::State,
    needs_preroll_seek: bool,
    now: Instant,
) -> Option<PendingSeek> {
    if needs_preroll_seek {
        return Some(PendingSeek::startup_with_resume(
            absolute_start_millis,
            logical_state,
            now,
            target_state == gst::State::Playing,
        ));
    }
    if target_state == gst::State::Playing {
        return Some(PendingSeek::track_start(now));
    }
    Some(PendingSeek::interactive(
        logical_position_millis,
        logical_state,
        now,
    ))
}
fn stream_uri_scheme(uri: &str) -> &str {
    uri.split_once(':')
        .map(|(scheme, _)| scheme)
        .unwrap_or("unknown")
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_clock_maps_one_cue_window_everywhere() {
        let stream = ResolvedStream::new("file:///music/cue.flac").with_window(60_000, 90_000);
        let clock = SourceClock::from_stream(&stream);

        assert_eq!(clock.physical_seek(12_345), 72_345);
        assert_eq!(clock.physical_seek(40_000), 90_000);
        assert_eq!(clock.logical_position(72_345), 12_345);
        assert_eq!(clock.logical_position(95_000), 30_000);
        assert_eq!(clock.logical_duration(180_000), 30_000);
        assert_eq!(clock.remaining(72_345, 180_000), 17_655);
    }

    #[test]
    fn replaced_pipeline_cannot_consume_next_or_relabel_visualizer_work() {
        let old = PipelineId(4);
        let current = PipelineId(5);
        let current_run = RunId::new(10);
        let next = PreparedNext::new(
            RunId::new(11),
            ResolvedStream::new("file:///music/next.flac"),
            NextTransition::Gapless,
        );
        let mut shared = SharedBackendState::new();
        shared.active = Slot::Primary;
        shared.current = Some(PreparedRun {
            run: current_run,
            stream: ResolvedStream::new("file:///music/current.flac"),
        });
        shared.next = Some(next.clone());
        shared.visualizer_enabled = true;
        shared.set_pipeline_id(Slot::Primary, Some(current));

        assert_eq!(
            about_to_finish_action_for_pipeline(&mut shared, Slot::Primary, old),
            AboutToFinishAction::Ignore
        );
        assert_eq!(shared.next, Some(next));

        let shared = Arc::new(Mutex::new(shared));
        assert!(!visualizer_pipeline_is_live(
            &shared,
            Slot::Primary,
            old,
            current_run,
        ));
        assert!(visualizer_pipeline_is_live(
            &shared,
            Slot::Primary,
            current,
            current_run,
        ));
    }

    #[test]
    fn event_mailbox_coalesces_telemetry_without_crossing_ordered_events() {
        let first = RunId::new(1);
        let second = RunId::new(2);
        let mut mailbox = EventMailbox::default();

        mailbox.push(BackendEvent::Position {
            run: first,
            millis: 10,
        });
        mailbox.push(BackendEvent::Position {
            run: first,
            millis: 20,
        });
        mailbox.push(BackendEvent::Duration {
            run: first,
            millis: 90,
        });
        mailbox.push(BackendEvent::Position {
            run: second,
            millis: 30,
        });
        mailbox.push(BackendEvent::State {
            run: first,
            state: BackendState::Playing,
        });
        mailbox.push(BackendEvent::Position {
            run: first,
            millis: 40,
        });
        mailbox.push(BackendEvent::Position {
            run: first,
            millis: 50,
        });
        mailbox.push(BackendEvent::Visualizer {
            run: first,
            levels: vec![0.5],
        });
        mailbox.push(BackendEvent::Visualizer {
            run: first,
            levels: vec![0.75],
        });
        mailbox.push(BackendEvent::Visualizer {
            run: first,
            levels: Vec::new(),
        });
        mailbox.push(BackendEvent::Buffering {
            run: first,
            percent: 10,
        });
        mailbox.push(BackendEvent::Buffering {
            run: first,
            percent: 100,
        });
        mailbox.push(BackendEvent::Ended { run: first });

        assert_eq!(
            mailbox.drain(),
            vec![
                BackendEvent::Position {
                    run: first,
                    millis: 20,
                },
                BackendEvent::Duration {
                    run: first,
                    millis: 90,
                },
                BackendEvent::Position {
                    run: second,
                    millis: 30,
                },
                BackendEvent::State {
                    run: first,
                    state: BackendState::Playing,
                },
                BackendEvent::Position {
                    run: first,
                    millis: 50,
                },
                BackendEvent::Visualizer {
                    run: first,
                    levels: vec![0.75],
                },
                BackendEvent::Visualizer {
                    run: first,
                    levels: Vec::new(),
                },
                BackendEvent::Buffering {
                    run: first,
                    percent: 100,
                },
                BackendEvent::Ended { run: first },
            ]
        );
    }
}
