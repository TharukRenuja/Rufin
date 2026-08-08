use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use library::{ResolvedStream, SourceId, StreamRequest, Track, TrackId};

use crate::{
    BackendCommand, BackendEvent, BackendState, Batch, BatchItem, ListeningFact, ListeningOutcome,
    ListeningTrack, NextTransition, OccurrenceId, Placement, PlaybackSettings,
    PlaybackTransitionMode, PreparedNext, Provenance, RepeatMode, RunEndReason, RunId, Sequence,
    SequenceEntry, SequenceError, external_scrobble_threshold_millis, manual_end_is_skip,
    qualified_play_threshold_millis,
};

const AUTO_DJ_HISTORY_LIMIT: usize = 10;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MaterializationId(u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterializationReservation {
    pub id: MaterializationId,
    pub source_id: SourceId,
    pub current_track_id: Option<TrackId>,
    pub queued_track_ids: Vec<TrackId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClockSample {
    pub monotonic_millis: u64,
    pub unix_seconds: i64,
    pub local_period: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TransportStatus {
    #[default]
    Stopped,
    Resolving,
    Buffering,
    Playing,
    Paused,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourceSessionEpoch(u64);

impl SourceSessionEpoch {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceReportPhase {
    Started,
    Progress,
    QualifiedPlay,
    Ended,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceReportFact {
    pub run: RunId,
    pub source_id: SourceId,
    pub track_id: TrackId,
    pub phase: SourceReportPhase,
    pub started_at_unix_seconds: i64,
    pub position_millis: u64,
    pub paused: bool,
    pub muted: bool,
    pub volume: f64,
    pub shuffle: bool,
    pub repeat_mode: RepeatMode,
    pub failed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PositionDiscontinuity {
    pub run: RunId,
    pub position_millis: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutoDjRequest {
    pub source_id: SourceId,
    pub seed_occurrence: OccurrenceId,
    pub seed_track_id: TrackId,
    pub requested_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionEffect {
    ResolveStream {
        run: RunId,
        source_id: SourceId,
        occurrence: OccurrenceId,
        request: StreamRequest,
    },
    Backend(BackendCommand),
    PersistProgress {
        source_id: SourceId,
        revision: u64,
        occurrence: Option<OccurrenceId>,
        progress_millis: u64,
    },
    PersistState {
        source_id: SourceId,
        revision: u64,
        occurrence: Option<OccurrenceId>,
        progress_millis: u64,
    },
    PersistOutputState {
        volume: f64,
        muted: bool,
    },
    FlushPersistence {
        source_id: SourceId,
    },
    Listening(ListeningFact),
    Activity(ListeningOutcome),
    ExternalScrobble(crate::CompletedScrobble),
    SourceReport(SourceReportFact),
    CurrentMediaChanged,
    PositionDiscontinuity(PositionDiscontinuity),
    RequestAutoDj(AutoDjRequest),
    Visualizer {
        run: RunId,
        levels: Vec<f64>,
    },
    NonfatalError(String),
    FatalError(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionCommand {
    Activate(OccurrenceId),
    Remove(OccurrenceId),
    Reorder {
        occurrence: OccurrenceId,
        target_index: usize,
        after: bool,
    },
    MoveAfterCurrent(OccurrenceId),
    ClearUpcoming,
    PlayPause,
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    Seek(u64),
    SetVolume(f64),
    SetMuted(bool),
    PersistOutputState,
    SetRepeat(RepeatMode),
    SetShuffle {
        enabled: bool,
        seed: u64,
    },
    SetAutoDj {
        enabled: bool,
        refill_threshold: usize,
    },
    UpdateSettings(PlaybackSettings),
    SetVisualizerEnabled(bool),
    RefreshTracks {
        source_session_epoch: SourceSessionEpoch,
        tracks: Vec<Track>,
    },
    StreamInputsChanged,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionUpdate {
    pub effects: Vec<SessionEffect>,
    pub view_changed: bool,
    pub structure_changed: bool,
}

impl SessionUpdate {
    fn changed() -> Self {
        Self {
            view_changed: true,
            ..Self::default()
        }
    }

    fn structural() -> Self {
        Self {
            view_changed: true,
            structure_changed: true,
            effects: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct RunContext {
    id: RunId,
    play_id: String,
    occurrence: OccurrenceId,
    track: ListeningTrack,
    status: TransportStatus,
    duration_millis: u64,
    audible_millis: u64,
    started_at_unix_seconds: Option<i64>,
    local_period: Option<String>,
    last_monotonic_millis: Option<u64>,
    qualified: bool,
    external_scrobble_emitted: bool,
    last_progress_bucket: Option<u64>,
    desired_playing: bool,
    resolved_stream: Option<ResolvedStream>,
}

impl RunContext {
    fn resolving(id: RunId, play_id: String, source_id: SourceId, entry: &SequenceEntry) -> Self {
        let track = ListeningTrack::capture(source_id, &entry.track);
        Self {
            id,
            play_id,
            occurrence: entry.occurrence.clone(),
            duration_millis: track.duration_millis,
            track,
            status: TransportStatus::Resolving,
            audible_millis: 0,
            started_at_unix_seconds: None,
            local_period: None,
            last_monotonic_millis: None,
            qualified: false,
            external_scrobble_emitted: false,
            last_progress_bucket: None,
            desired_playing: true,
            resolved_stream: None,
        }
    }

    fn advance_clock(&mut self, monotonic_millis: u64) {
        if self.status == TransportStatus::Playing
            && let Some(previous) = self.last_monotonic_millis
        {
            self.audible_millis = self
                .audible_millis
                .saturating_add(monotonic_millis.saturating_sub(previous));
        }
        self.last_monotonic_millis = Some(monotonic_millis);
    }
}

#[derive(Clone, Debug)]
enum NextResolution {
    Resolving,
    Ready(ResolvedStream),
}

#[derive(Clone, Debug)]
struct NextPlan {
    current_run: RunId,
    next_run: RunId,
    occurrence: OccurrenceId,
    request: StreamRequest,
    transition: NextTransition,
    resolution: NextResolution,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct AutoDjKey {
    source_id: SourceId,
    seed_occurrence: OccurrenceId,
}

#[derive(Clone, Debug)]
pub(crate) struct PlaybackSession {
    sequence: Sequence,
    source_session_epoch: SourceSessionEpoch,
    play_id_prefix: Arc<str>,
    current_run: Option<RunContext>,
    next_plan: Option<NextPlan>,
    next_run_number: u64,
    next_materialization_number: u64,
    pending_replacement: Option<MaterializationId>,
    pending_additive: HashMap<MaterializationId, Placement>,
    settings: PlaybackSettings,
    auto_dj_enabled: bool,
    auto_dj_refill_threshold: usize,
    auto_dj_in_flight: Option<AutoDjKey>,
    auto_dj_waiting_for_continuation: bool,
    buffering_percent: Option<u8>,
    last_error: Option<String>,
}

impl PlaybackSession {
    pub fn new(
        sequence: Sequence,
        source_session_epoch: SourceSessionEpoch,
        play_id_prefix: impl Into<Arc<str>>,
        mut settings: PlaybackSettings,
        auto_dj_enabled: bool,
        auto_dj_refill_threshold: usize,
    ) -> Self {
        settings.sanitize();
        Self {
            sequence,
            source_session_epoch,
            play_id_prefix: play_id_prefix.into(),
            current_run: None,
            next_plan: None,
            next_run_number: 1,
            next_materialization_number: 1,
            pending_replacement: None,
            pending_additive: HashMap::new(),
            settings,
            auto_dj_enabled,
            auto_dj_refill_threshold: auto_dj_refill_threshold.max(1),
            auto_dj_in_flight: None,
            auto_dj_waiting_for_continuation: false,
            buffering_percent: None,
            last_error: None,
        }
    }

    pub fn sequence(&self) -> &Sequence {
        &self.sequence
    }

    pub const fn source_session_epoch(&self) -> SourceSessionEpoch {
        self.source_session_epoch
    }

    pub fn status(&self) -> TransportStatus {
        self.current_run
            .as_ref()
            .map(|run| {
                if run.status == TransportStatus::Resolving && !run.desired_playing {
                    TransportStatus::Paused
                } else {
                    run.status
                }
            })
            .unwrap_or_else(|| {
                if self.last_error.is_some() {
                    TransportStatus::Failed
                } else {
                    TransportStatus::Stopped
                }
            })
    }

    pub fn desired_playing(&self) -> bool {
        self.current_run
            .as_ref()
            .is_some_and(|run| run.desired_playing)
    }

    pub fn current_run(&self) -> Option<RunId> {
        self.current_run.as_ref().map(|run| run.id)
    }

    pub fn position_millis(&self) -> u64 {
        self.sequence.progress_millis()
    }

    pub fn duration_millis(&self) -> u64 {
        self.current_run
            .as_ref()
            .map(|run| run.duration_millis)
            .or_else(|| {
                self.sequence
                    .selected()
                    .map(|entry| u64::from(entry.track.duration_seconds) * 1_000)
            })
            .unwrap_or_default()
    }

    pub fn buffering_percent(&self) -> Option<u8> {
        self.buffering_percent
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn settings(&self) -> &PlaybackSettings {
        &self.settings
    }

    pub fn auto_dj_enabled(&self) -> bool {
        self.auto_dj_enabled
    }

    pub fn reserve_materialization(&mut self, placement: Placement) -> MaterializationReservation {
        let id = MaterializationId(self.next_materialization_number);
        self.next_materialization_number = self.next_materialization_number.wrapping_add(1).max(1);
        match placement {
            Placement::Replace { .. } => {
                self.pending_additive.clear();
                self.pending_replacement = Some(id);
                self.auto_dj_in_flight = None;
                self.auto_dj_waiting_for_continuation = false;
            }
            Placement::AfterCurrent | Placement::End => {
                self.pending_additive.insert(id, placement);
            }
        }
        MaterializationReservation {
            id,
            source_id: self.sequence.source_id().clone(),
            current_track_id: self.sequence.selected().map(|entry| entry.track.id.clone()),
            queued_track_ids: self.sequence.unique_track_ids(),
        }
    }

    pub fn apply_materialization(
        &mut self,
        id: MaterializationId,
        source_id: &SourceId,
        batch: Batch,
        placement: Placement,
        sample: &ClockSample,
    ) -> Result<Option<SessionUpdate>, SequenceError> {
        if self.sequence.source_id() != source_id {
            return Ok(None);
        }
        let accepted = match placement {
            Placement::Replace { .. } => self.pending_replacement == Some(id),
            Placement::AfterCurrent | Placement::End => {
                self.pending_additive.get(&id) == Some(&placement)
            }
        };
        if !accepted {
            return Ok(None);
        }
        if matches!(placement, Placement::Replace { .. }) {
            self.pending_replacement = None;
        } else {
            self.pending_additive.remove(&id);
        }
        self.apply_batch(batch, placement, sample).map(Some)
    }

    pub fn fail_materialization(
        &mut self,
        id: MaterializationId,
        source_id: &SourceId,
        placement: Placement,
        message: String,
    ) -> Option<SessionUpdate> {
        self.cancel_materialization(id, source_id, placement)
            .then(|| SessionUpdate {
                effects: vec![SessionEffect::NonfatalError(message)],
                ..SessionUpdate::default()
            })
    }

    pub fn cancel_materialization(
        &mut self,
        id: MaterializationId,
        source_id: &SourceId,
        placement: Placement,
    ) -> bool {
        if self.sequence.source_id() != source_id {
            return false;
        }
        match placement {
            Placement::Replace { .. } if self.pending_replacement == Some(id) => {
                self.pending_replacement = None;
                true
            }
            Placement::AfterCurrent | Placement::End
                if self.pending_additive.get(&id) == Some(&placement) =>
            {
                self.pending_additive.remove(&id);
                true
            }
            _ => false,
        }
    }

    pub fn handle_command(
        &mut self,
        command: SessionCommand,
        sample: &ClockSample,
    ) -> Result<SessionUpdate, SequenceError> {
        match command {
            SessionCommand::Activate(occurrence) => Ok(self.activate(&occurrence, sample)),
            SessionCommand::Remove(occurrence) => Ok(self.remove(&occurrence, sample)),
            SessionCommand::Reorder {
                occurrence,
                target_index,
                after,
            } => Ok(self.reorder(&occurrence, target_index, after)),
            SessionCommand::MoveAfterCurrent(occurrence) => {
                Ok(self.move_after_current(&occurrence))
            }
            SessionCommand::ClearUpcoming => Ok(self.clear_upcoming()),
            SessionCommand::PlayPause => Ok(self.play_pause()),
            SessionCommand::Play => Ok(self.set_playing(true)),
            SessionCommand::Pause => Ok(self.set_playing(false)),
            SessionCommand::Stop => Ok(self.stop(sample)),
            SessionCommand::Next => Ok(self.next(sample)),
            SessionCommand::Previous => Ok(self.previous(sample)),
            SessionCommand::Seek(position_millis) => Ok(self.seek(position_millis)),
            SessionCommand::SetVolume(volume) => Ok(self.set_volume(volume)),
            SessionCommand::SetMuted(muted) => Ok(self.set_muted(muted)),
            SessionCommand::PersistOutputState => Ok(SessionUpdate {
                effects: vec![SessionEffect::PersistOutputState {
                    volume: self.settings.volume,
                    muted: self.settings.muted,
                }],
                ..SessionUpdate::default()
            }),
            SessionCommand::SetRepeat(repeat) => Ok(self.set_repeat(repeat)),
            SessionCommand::SetShuffle { enabled, seed } => Ok(self.set_shuffle(enabled, seed)),
            SessionCommand::SetAutoDj {
                enabled,
                refill_threshold,
            } => Ok(self.set_auto_dj(enabled, refill_threshold)),
            SessionCommand::UpdateSettings(settings) => Ok(self.update_settings(settings)),
            SessionCommand::SetVisualizerEnabled(enabled) => Ok(SessionUpdate {
                effects: vec![SessionEffect::Backend(
                    BackendCommand::SetVisualizerEnabled(enabled),
                )],
                ..SessionUpdate::default()
            }),
            SessionCommand::RefreshTracks {
                source_session_epoch,
                tracks,
            } => Ok(self.refresh_tracks(source_session_epoch, tracks)),
            SessionCommand::StreamInputsChanged => Ok(self.stream_inputs_changed()),
        }
    }

    pub fn stream_resolved(&mut self, run: RunId, stream: ResolvedStream) -> SessionUpdate {
        if self.current_run.as_ref().is_some_and(|current| {
            current.id == run && current.status == TransportStatus::Resolving
        }) {
            return self.current_stream_resolved(run, stream);
        }
        let Some(plan) = self.next_plan.as_mut().filter(|plan| {
            plan.next_run == run
                && self
                    .current_run
                    .as_ref()
                    .is_some_and(|current| current.id == plan.current_run)
        }) else {
            return SessionUpdate::default();
        };
        plan.resolution = NextResolution::Ready(stream.clone());
        let mut update = SessionUpdate::default();
        if self.current_run.as_ref().is_some_and(|current| {
            matches!(
                current.status,
                TransportStatus::Buffering | TransportStatus::Playing | TransportStatus::Paused
            )
        }) {
            update
                .effects
                .push(SessionEffect::Backend(BackendCommand::PrepareNext {
                    current_run: plan.current_run,
                    next: Some(PreparedNext::new(plan.next_run, stream, plan.transition)),
                }));
        }
        update
    }

    pub fn stream_failed(
        &mut self,
        run: RunId,
        error: String,
        sample: &ClockSample,
    ) -> SessionUpdate {
        if self
            .current_run
            .as_ref()
            .is_some_and(|current| current.id == run)
        {
            let mut update = SessionUpdate::changed();
            self.finish_current(RunEndReason::Failed, sample, &mut update.effects);
            self.last_error = Some(error.clone());
            self.buffering_percent = None;
            update.effects.push(SessionEffect::FatalError(error));
            return update;
        }
        if self
            .next_plan
            .as_ref()
            .is_some_and(|plan| plan.next_run == run)
        {
            self.next_plan = None;
        }
        SessionUpdate::default()
    }

    pub fn handle_backend(&mut self, event: BackendEvent, sample: &ClockSample) -> SessionUpdate {
        match event {
            BackendEvent::Started { run } => self.accept_started(run, sample),
            BackendEvent::State { run, state } => self.accept_state(run, state, sample),
            BackendEvent::Position { run, millis } => self.accept_position(run, millis, sample),
            BackendEvent::Duration { run, millis } => self.accept_duration(run, millis),
            BackendEvent::Buffering { run, percent } => self.accept_buffering(run, percent),
            BackendEvent::Ended { run } => self.accept_ended(run, sample),
            BackendEvent::Transitioned { old_run, new_run } => {
                self.accept_transitioned(old_run, new_run, sample)
            }
            BackendEvent::NextNeeded { run } => {
                if self
                    .current_run
                    .as_ref()
                    .is_some_and(|current| current.id == run)
                    && self.next_plan.is_none()
                {
                    let mut update = SessionUpdate::default();
                    self.plan_next(&mut update.effects);
                    update
                } else {
                    SessionUpdate::default()
                }
            }
            BackendEvent::NextPreparationFailed {
                current_run,
                next_run,
                error,
            } => {
                if self.next_plan.as_ref().is_some_and(|plan| {
                    plan.current_run == current_run && plan.next_run == next_run
                }) {
                    SessionUpdate {
                        effects: vec![SessionEffect::NonfatalError(error.message().to_string())],
                        ..SessionUpdate::default()
                    }
                } else {
                    SessionUpdate::default()
                }
            }
            BackendEvent::AudioApplied {
                volume,
                muted,
                output,
            } => {
                if self.settings.volume == volume
                    && self.settings.muted == muted
                    && self.settings.audio_output == output
                {
                    return SessionUpdate::default();
                }
                self.settings.volume = volume;
                self.settings.muted = muted;
                self.settings.audio_output = output;
                SessionUpdate::changed()
            }
            BackendEvent::Visualizer { run, levels } => {
                if self
                    .current_run
                    .as_ref()
                    .is_some_and(|current| current.id == run)
                {
                    SessionUpdate {
                        effects: vec![SessionEffect::Visualizer { run, levels }],
                        ..SessionUpdate::default()
                    }
                } else {
                    SessionUpdate::default()
                }
            }
            BackendEvent::Error { run, error } => {
                if !self
                    .current_run
                    .as_ref()
                    .is_some_and(|current| current.id == run)
                {
                    return SessionUpdate::default();
                }
                let mut update = SessionUpdate::changed();
                self.finish_current(RunEndReason::Failed, sample, &mut update.effects);
                self.last_error = Some(error.message().to_string());
                update
                    .effects
                    .push(SessionEffect::FatalError(error.message().to_string()));
                update
            }
        }
    }

    pub fn complete_auto_dj(
        &mut self,
        source_id: &SourceId,
        seed_occurrence: &OccurrenceId,
        batch: Batch,
        sample: &ClockSample,
    ) -> Result<Option<SessionUpdate>, SequenceError> {
        let key = AutoDjKey {
            source_id: source_id.clone(),
            seed_occurrence: seed_occurrence.clone(),
        };
        if self.auto_dj_in_flight.as_ref() != Some(&key) {
            return Ok(None);
        }
        self.auto_dj_in_flight = None;
        if !self.auto_dj_enabled
            || self.sequence.repeat_mode() == RepeatMode::One
            || self.sequence.source_id() != source_id
            || self.sequence.occurrence(seed_occurrence).is_none()
            || self.sequence.remaining_after_selected() >= self.auto_dj_refill_threshold
        {
            return Ok(None);
        }
        let continuation = self.auto_dj_waiting_for_continuation;
        self.auto_dj_waiting_for_continuation = false;
        self.sequence.trim_auto_dj_history(AUTO_DJ_HISTORY_LIMIT);
        let mut update = self.apply_batch(batch, Placement::End, sample)?;
        if continuation && self.current_run.is_none() && self.sequence.advance_manual().is_some() {
            self.begin_selected_run(&mut update.effects);
        }
        Ok(Some(update))
    }

    pub fn complete_auto_dj_candidates(
        &mut self,
        source_id: &SourceId,
        seed_occurrence: &OccurrenceId,
        candidates: Vec<Track>,
        requested_count: usize,
        shuffle_seed: u64,
        sample: &ClockSample,
    ) -> Result<Option<SessionUpdate>, SequenceError> {
        let mut seen = HashSet::new();
        let items = candidates
            .into_iter()
            .filter(|track| {
                !self.sequence.contains_track(&track.id) && seen.insert(track.id.clone())
            })
            .take(requested_count)
            .map(|track| BatchItem::new(track, Provenance::AutoDj))
            .collect::<Vec<_>>();
        if items.is_empty() {
            return Ok(self.auto_dj_unavailable(source_id, seed_occurrence, None));
        }
        self.complete_auto_dj(
            source_id,
            seed_occurrence,
            Batch::new(items).with_shuffle_intent(shuffle_seed, false),
            sample,
        )
    }

    pub fn auto_dj_unavailable(
        &mut self,
        source_id: &SourceId,
        seed_occurrence: &OccurrenceId,
        error: Option<String>,
    ) -> Option<SessionUpdate> {
        let key = AutoDjKey {
            source_id: source_id.clone(),
            seed_occurrence: seed_occurrence.clone(),
        };
        if self.auto_dj_in_flight.as_ref() != Some(&key) {
            return None;
        }
        self.auto_dj_in_flight = None;
        self.auto_dj_waiting_for_continuation = false;
        Some(SessionUpdate {
            effects: error
                .into_iter()
                .map(SessionEffect::NonfatalError)
                .collect(),
            ..SessionUpdate::default()
        })
    }

    fn apply_batch(
        &mut self,
        batch: Batch,
        placement: Placement,
        sample: &ClockSample,
    ) -> Result<SessionUpdate, SequenceError> {
        let replacing = matches!(placement, Placement::Replace { .. });
        let previous_selected = self
            .sequence
            .selected()
            .map(|entry| entry.occurrence.clone());
        let previous_had_run = self.current_run.is_some();
        let mut update = SessionUpdate::structural();
        if replacing {
            self.pending_replacement = None;
            self.pending_additive.clear();
            self.auto_dj_in_flight = None;
            if let Some(run) = self.current_run.as_ref() {
                update
                    .effects
                    .push(SessionEffect::Backend(BackendCommand::Stop { run: run.id }));
            }
            self.finish_current(RunEndReason::Replaced, sample, &mut update.effects);
        }
        self.sequence.apply_batch(batch, placement)?;
        let next_selected = self
            .sequence
            .selected()
            .map(|entry| entry.occurrence.clone());
        if replacing {
            self.begin_selected_run(&mut update.effects);
            if !previous_had_run && previous_selected.is_some() && next_selected.is_none() {
                update.effects.push(SessionEffect::CurrentMediaChanged);
            }
        } else if previous_selected != next_selected {
            update.effects.push(SessionEffect::CurrentMediaChanged);
            self.plan_next(&mut update.effects);
        } else {
            self.replan_next_if_changed(&mut update.effects);
        }
        self.maybe_request_auto_dj(&mut update.effects);
        Ok(update)
    }

    pub fn activate_context(
        &mut self,
        context_id: &str,
        track_id: &TrackId,
        source_rank: usize,
        sample: &ClockSample,
    ) -> Option<SessionUpdate> {
        let index = self
            .sequence
            .context_index(context_id, track_id, source_rank)?;
        let occurrence = self.sequence.entries().get(index)?.occurrence.clone();
        Some(self.activate_index(index, occurrence, sample))
    }

    fn activate(&mut self, occurrence: &OccurrenceId, sample: &ClockSample) -> SessionUpdate {
        let Some(index) = self.sequence.occurrence_index(occurrence) else {
            return SessionUpdate::default();
        };
        self.activate_index(index, occurrence.clone(), sample)
    }

    fn activate_index(
        &mut self,
        index: usize,
        occurrence: OccurrenceId,
        sample: &ClockSample,
    ) -> SessionUpdate {
        if self
            .current_run
            .as_ref()
            .is_some_and(|run| run.occurrence == occurrence)
        {
            return SessionUpdate::default();
        }
        self.pending_replacement = None;
        let mut update = SessionUpdate::changed();
        if let Some(run) = self.current_run.as_ref() {
            update
                .effects
                .push(SessionEffect::Backend(BackendCommand::Stop { run: run.id }));
        }
        self.finish_current(RunEndReason::ManualSkip, sample, &mut update.effects);
        if !self.sequence.activate_index(index) {
            return update;
        }
        self.begin_selected_run(&mut update.effects);
        update
    }

    fn remove(&mut self, occurrence: &OccurrenceId, sample: &ClockSample) -> SessionUpdate {
        let removing_current = self
            .sequence
            .selected()
            .is_some_and(|entry| &entry.occurrence == occurrence);
        let removing_current_run = removing_current && self.current_run.is_some();
        if self.sequence.occurrence(occurrence).is_none() {
            return SessionUpdate::default();
        }
        let mut update = SessionUpdate::structural();
        self.pending_replacement = None;
        if self
            .auto_dj_in_flight
            .as_ref()
            .is_some_and(|key| &key.seed_occurrence == occurrence)
        {
            self.auto_dj_in_flight = None;
        }
        if removing_current {
            if let Some(run) = self.current_run.as_ref() {
                update
                    .effects
                    .push(SessionEffect::Backend(BackendCommand::Stop { run: run.id }));
            }
            self.finish_current(RunEndReason::ManualSkip, sample, &mut update.effects);
        }
        self.sequence.remove(occurrence);
        if removing_current {
            self.begin_selected_run(&mut update.effects);
            if !removing_current_run && self.sequence.selected().is_none() {
                update.effects.push(SessionEffect::CurrentMediaChanged);
            }
        } else {
            self.replan_next_if_changed(&mut update.effects);
        }
        update
    }

    fn reorder(
        &mut self,
        occurrence: &OccurrenceId,
        target_index: usize,
        after: bool,
    ) -> SessionUpdate {
        let Some(old_index) = self
            .sequence
            .entries()
            .iter()
            .position(|entry| &entry.occurrence == occurrence)
        else {
            return SessionUpdate::default();
        };
        let mut absolute_index = target_index.saturating_add(usize::from(after));
        if old_index < absolute_index {
            absolute_index = absolute_index.saturating_sub(1);
        }
        if old_index == absolute_index {
            return SessionUpdate::default();
        }
        if !self.sequence.reorder(occurrence, absolute_index) {
            return SessionUpdate::default();
        }
        self.pending_replacement = None;
        let mut update = SessionUpdate::structural();
        self.replan_next_if_changed(&mut update.effects);
        update
    }

    fn move_after_current(&mut self, occurrence: &OccurrenceId) -> SessionUpdate {
        if !self.sequence.move_after_current(occurrence) {
            return SessionUpdate::default();
        }
        self.pending_replacement = None;
        let mut update = SessionUpdate::structural();
        self.replan_next_if_changed(&mut update.effects);
        update
    }

    fn clear_upcoming(&mut self) -> SessionUpdate {
        let clears_current = self.current_run.is_none() && self.sequence.selected().is_some();
        let changed = if self.current_run.is_some() {
            self.sequence.clear_upcoming()
        } else {
            self.sequence.clear()
        };
        self.pending_replacement = None;
        self.pending_additive.clear();
        self.auto_dj_in_flight = None;
        self.auto_dj_waiting_for_continuation = false;
        if !changed {
            return SessionUpdate::default();
        }
        self.next_plan = None;
        let mut update = SessionUpdate::structural();
        if clears_current {
            update.effects.push(SessionEffect::CurrentMediaChanged);
        }
        if let Some(run) = self.current_run.as_ref() {
            update
                .effects
                .push(SessionEffect::Backend(BackendCommand::PrepareNext {
                    current_run: run.id,
                    next: None,
                }));
        }
        update
    }

    fn play_pause(&mut self) -> SessionUpdate {
        let playing = self
            .current_run
            .as_ref()
            .is_some_and(|run| run.desired_playing);
        self.set_playing(!playing)
    }

    fn set_playing(&mut self, desired_playing: bool) -> SessionUpdate {
        let Some(run) = self.current_run.as_mut() else {
            if !desired_playing {
                return SessionUpdate::default();
            }
            let mut update = SessionUpdate::changed();
            self.begin_selected_run(&mut update.effects);
            return update;
        };
        let command = match run.status {
            TransportStatus::Resolving => {
                if run.desired_playing == desired_playing {
                    return SessionUpdate::default();
                }
                run.desired_playing = desired_playing;
                return SessionUpdate::changed();
            }
            TransportStatus::Paused => {
                if !desired_playing {
                    if !run.desired_playing {
                        return SessionUpdate::default();
                    }
                    run.desired_playing = false;
                    BackendCommand::Pause { run: run.id }
                } else {
                    run.desired_playing = true;
                    if let Some(stream) = run.resolved_stream.take() {
                        run.status = TransportStatus::Buffering;
                        let next = self.next_plan.as_ref().and_then(|plan| {
                            if plan.current_run != run.id {
                                return None;
                            }
                            let NextResolution::Ready(stream) = &plan.resolution else {
                                return None;
                            };
                            Some(PreparedNext::new(
                                plan.next_run,
                                stream.clone(),
                                plan.transition,
                            ))
                        });
                        return SessionUpdate {
                            effects: vec![
                                SessionEffect::Backend(BackendCommand::ConfigureAudio(
                                    self.settings.clone().into(),
                                )),
                                SessionEffect::Backend(BackendCommand::Start {
                                    run: run.id,
                                    current: stream,
                                    next,
                                    start_position_millis: self.sequence.progress_millis(),
                                }),
                            ],
                            view_changed: true,
                            structure_changed: false,
                        };
                    }
                    BackendCommand::Play { run: run.id }
                }
            }
            TransportStatus::Playing | TransportStatus::Buffering => {
                if run.desired_playing == desired_playing {
                    return SessionUpdate::default();
                }
                run.desired_playing = desired_playing;
                if desired_playing {
                    BackendCommand::Play { run: run.id }
                } else {
                    BackendCommand::Pause { run: run.id }
                }
            }
            TransportStatus::Stopped | TransportStatus::Failed => {
                return SessionUpdate::default();
            }
        };
        SessionUpdate {
            effects: vec![SessionEffect::Backend(command)],
            view_changed: true,
            ..SessionUpdate::default()
        }
    }

    fn stop(&mut self, sample: &ClockSample) -> SessionUpdate {
        let Some(run) = self.current_run.as_ref() else {
            if self.sequence.progress_millis() == 0 {
                return SessionUpdate::default();
            }
            self.sequence.set_progress_millis(0);
            return SessionUpdate {
                effects: vec![
                    self.progress_effect(),
                    SessionEffect::FlushPersistence {
                        source_id: self.sequence.source_id().clone(),
                    },
                ],
                view_changed: true,
                structure_changed: false,
            };
        };
        self.pending_replacement = None;
        let mut update = SessionUpdate::structural();
        update
            .effects
            .push(SessionEffect::Backend(BackendCommand::Stop { run: run.id }));
        self.finish_current(RunEndReason::Stopped, sample, &mut update.effects);
        self.sequence.set_progress_millis(0);
        update.effects.push(self.progress_effect());
        update.effects.push(SessionEffect::FlushPersistence {
            source_id: self.sequence.source_id().clone(),
        });
        update
    }

    pub(crate) fn shutdown(&mut self, sample: &ClockSample) -> SessionUpdate {
        self.pending_replacement = None;
        self.pending_additive.clear();
        self.auto_dj_in_flight = None;
        self.auto_dj_waiting_for_continuation = false;
        let mut update = SessionUpdate::changed();
        if let Some(run) = self.current_run.as_ref() {
            update
                .effects
                .push(SessionEffect::Backend(BackendCommand::Stop { run: run.id }));
        }
        self.finish_current(RunEndReason::Stopped, sample, &mut update.effects);
        update.effects.push(SessionEffect::FlushPersistence {
            source_id: self.sequence.source_id().clone(),
        });
        update
    }

    fn next(&mut self, sample: &ClockSample) -> SessionUpdate {
        let mut update = SessionUpdate::changed();
        let old = self.current_run.as_ref().map(|run| run.id);
        let reserved = self.next_plan.clone();
        self.pending_replacement = None;
        if let Some(run) = old {
            update
                .effects
                .push(SessionEffect::Backend(BackendCommand::Stop { run }));
        }
        self.finish_current(RunEndReason::ManualSkip, sample, &mut update.effects);
        let next_occurrence = self
            .sequence
            .advance_manual()
            .map(|entry| entry.occurrence.clone());
        let Some(next_occurrence) = next_occurrence else {
            self.auto_dj_waiting_for_continuation = true;
            self.maybe_request_auto_dj(&mut update.effects);
            return update;
        };
        self.next_plan = reserved.filter(|plan| plan.occurrence == next_occurrence);
        self.promote_or_begin(next_occurrence, true, &mut update.effects);
        update
    }

    fn previous(&mut self, sample: &ClockSample) -> SessionUpdate {
        if self.position_millis() > 10_000 {
            return self.seek(0);
        }
        if self.sequence.peek_previous().is_none() {
            return self.seek(0);
        }
        let mut update = SessionUpdate::changed();
        self.pending_replacement = None;
        if let Some(run) = self.current_run.as_ref() {
            update
                .effects
                .push(SessionEffect::Backend(BackendCommand::Stop { run: run.id }));
        }
        self.finish_current(RunEndReason::ManualSkip, sample, &mut update.effects);
        if self.sequence.previous().is_none() {
            return update;
        }
        self.begin_selected_run(&mut update.effects);
        update
    }

    fn seek(&mut self, position_millis: u64) -> SessionUpdate {
        let Some(run) = self.current_run.as_ref() else {
            self.sequence.set_progress_millis(position_millis);
            return SessionUpdate {
                effects: vec![self.progress_effect()],
                view_changed: true,
                structure_changed: false,
            };
        };
        SessionUpdate {
            effects: vec![
                SessionEffect::Backend(BackendCommand::Seek {
                    run: run.id,
                    position_millis,
                }),
                SessionEffect::PositionDiscontinuity(PositionDiscontinuity {
                    run: run.id,
                    position_millis,
                }),
            ],
            ..SessionUpdate::default()
        }
    }

    fn set_volume(&mut self, volume: f64) -> SessionUpdate {
        let volume = if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            1.0
        };
        if self.settings.volume == volume {
            return SessionUpdate::default();
        }
        self.settings.volume = volume;
        SessionUpdate {
            effects: vec![SessionEffect::Backend(BackendCommand::SetOutputVolume {
                volume,
                volume_scale: self.settings.volume_scale,
                muted: self.settings.muted,
            })],
            view_changed: true,
            structure_changed: false,
        }
    }

    fn set_muted(&mut self, muted: bool) -> SessionUpdate {
        if self.settings.muted == muted {
            return SessionUpdate::default();
        }
        self.settings.muted = muted;
        SessionUpdate {
            effects: vec![
                SessionEffect::Backend(BackendCommand::SetOutputVolume {
                    volume: self.settings.volume,
                    volume_scale: self.settings.volume_scale,
                    muted,
                }),
                SessionEffect::PersistOutputState {
                    volume: self.settings.volume,
                    muted,
                },
            ],
            view_changed: true,
            structure_changed: false,
        }
    }

    fn set_repeat(&mut self, repeat: RepeatMode) -> SessionUpdate {
        if self.sequence.repeat_mode() == repeat {
            return SessionUpdate::default();
        }
        self.sequence.set_repeat_mode(repeat);
        let mut update = SessionUpdate::changed();
        self.replan_next_if_changed(&mut update.effects);
        self.maybe_request_auto_dj(&mut update.effects);
        update
    }

    fn set_shuffle(&mut self, enabled: bool, seed: u64) -> SessionUpdate {
        if self.sequence.shuffle_enabled() == enabled {
            return SessionUpdate::default();
        }
        let revision = self.sequence.revision();
        self.sequence.set_shuffle_seed(enabled, seed);
        if self.sequence.revision() == revision {
            return SessionUpdate::default();
        }
        self.pending_replacement = None;
        let mut update = SessionUpdate::structural();
        self.replan_next_if_changed(&mut update.effects);
        update
    }

    fn set_auto_dj(&mut self, enabled: bool, refill_threshold: usize) -> SessionUpdate {
        let refill_threshold = refill_threshold.max(1);
        if self.auto_dj_enabled == enabled && self.auto_dj_refill_threshold == refill_threshold {
            return SessionUpdate::default();
        }
        self.auto_dj_enabled = enabled;
        self.auto_dj_refill_threshold = refill_threshold;
        if !enabled {
            self.auto_dj_in_flight = None;
            self.auto_dj_waiting_for_continuation = false;
        }
        let mut update = SessionUpdate::changed();
        self.maybe_request_auto_dj(&mut update.effects);
        update
    }

    fn update_settings(&mut self, mut settings: PlaybackSettings) -> SessionUpdate {
        settings.sanitize();
        if settings == self.settings {
            return SessionUpdate::default();
        }
        let stream_changed = settings.stream_quality != self.settings.stream_quality;
        let output_changed = settings.volume != self.settings.volume
            || settings.volume_scale != self.settings.volume_scale
            || settings.muted != self.settings.muted;
        let audio_configuration_changed = settings.replay_gain != self.settings.replay_gain
            || settings.audio_output != self.settings.audio_output
            || settings.equalizer != self.settings.equalizer
            || settings.audio_fade_on_status_change != self.settings.audio_fade_on_status_change;
        self.settings = settings.clone();
        let mut update = SessionUpdate::changed();
        if audio_configuration_changed {
            update
                .effects
                .push(SessionEffect::Backend(BackendCommand::ConfigureAudio(
                    settings.into(),
                )));
        } else if output_changed {
            update
                .effects
                .push(SessionEffect::Backend(BackendCommand::SetOutputVolume {
                    volume: settings.volume,
                    volume_scale: settings.volume_scale,
                    muted: settings.muted,
                }));
        }
        if stream_changed {
            self.replan_next(true, &mut update.effects);
        } else {
            self.replan_next_if_changed(&mut update.effects);
        }
        update
    }

    fn refresh_tracks(
        &mut self,
        source_session_epoch: SourceSessionEpoch,
        tracks: Vec<Track>,
    ) -> SessionUpdate {
        if self.source_session_epoch != source_session_epoch {
            return SessionUpdate::default();
        }
        let previous_current = self.sequence.selected().map(|entry| entry.track.clone());
        if !self.sequence.refresh_tracks(tracks) {
            return SessionUpdate::default();
        }
        let mut update = SessionUpdate::structural();
        if previous_current.as_ref() != self.sequence.selected().map(|entry| &entry.track) {
            update.effects.push(SessionEffect::CurrentMediaChanged);
        }
        update
    }

    fn stream_inputs_changed(&mut self) -> SessionUpdate {
        let mut update = SessionUpdate::default();
        self.replan_next(true, &mut update.effects);
        update
    }

    fn current_stream_resolved(&mut self, run: RunId, stream: ResolvedStream) -> SessionUpdate {
        let Some(current) = self
            .current_run
            .as_mut()
            .filter(|current| current.id == run)
        else {
            return SessionUpdate::default();
        };
        if !current.desired_playing {
            current.status = TransportStatus::Paused;
            current.resolved_stream = Some(stream);
            return SessionUpdate::changed();
        }
        current.status = TransportStatus::Buffering;
        let next = self.next_plan.as_ref().and_then(|plan| {
            if plan.current_run != run {
                return None;
            }
            let NextResolution::Ready(stream) = &plan.resolution else {
                return None;
            };
            Some(PreparedNext::new(
                plan.next_run,
                stream.clone(),
                plan.transition,
            ))
        });
        SessionUpdate {
            effects: vec![
                SessionEffect::Backend(BackendCommand::ConfigureAudio(
                    self.settings.clone().into(),
                )),
                SessionEffect::Backend(BackendCommand::Start {
                    run,
                    current: stream,
                    next,
                    start_position_millis: self.sequence.progress_millis(),
                }),
            ],
            view_changed: true,
            structure_changed: false,
        }
    }

    fn accept_started(&mut self, run: RunId, sample: &ClockSample) -> SessionUpdate {
        let Some(current) = self
            .current_run
            .as_ref()
            .filter(|current| current.id == run)
        else {
            return SessionUpdate::default();
        };
        if current.started_at_unix_seconds.is_some() {
            return SessionUpdate::default();
        }
        let mut update = SessionUpdate::changed();
        self.mark_started(sample, &mut update.effects);
        update
    }

    fn accept_state(
        &mut self,
        run: RunId,
        state: BackendState,
        sample: &ClockSample,
    ) -> SessionUpdate {
        let Some(current) = self
            .current_run
            .as_mut()
            .filter(|current| current.id == run)
        else {
            return SessionUpdate::default();
        };
        current.advance_clock(sample.monotonic_millis);
        let status = match state {
            BackendState::Stopped => TransportStatus::Stopped,
            BackendState::Buffering => TransportStatus::Buffering,
            BackendState::Paused => TransportStatus::Paused,
            BackendState::Playing => TransportStatus::Playing,
        };
        if current.status == status {
            return SessionUpdate::default();
        }
        current.status = status;
        let mut update = SessionUpdate::changed();
        let progress_reported = self.emit_progress_facts(&mut update.effects);
        if matches!(status, TransportStatus::Playing | TransportStatus::Paused)
            && !progress_reported
            && let Some(current) = self.current_run.as_ref()
            && current.started_at_unix_seconds.is_some()
        {
            update
                .effects
                .push(SessionEffect::SourceReport(self.source_report(
                    current,
                    SourceReportPhase::Progress,
                    false,
                )));
        }
        if status == TransportStatus::Paused && !progress_reported {
            update.effects.push(self.progress_effect());
        }
        if matches!(status, TransportStatus::Paused | TransportStatus::Stopped) {
            update.effects.push(SessionEffect::FlushPersistence {
                source_id: self.sequence.source_id().clone(),
            });
        }
        update
    }

    fn accept_position(&mut self, run: RunId, millis: u64, sample: &ClockSample) -> SessionUpdate {
        let Some(current) = self
            .current_run
            .as_mut()
            .filter(|current| current.id == run)
        else {
            return SessionUpdate::default();
        };
        current.advance_clock(sample.monotonic_millis);
        let playhead_millis = if current.duration_millis == 0 {
            millis
        } else {
            millis.min(current.duration_millis)
        };
        self.sequence.set_progress_millis(playhead_millis);
        let mut update = SessionUpdate::changed();
        self.emit_progress_facts(&mut update.effects);
        update
    }

    fn accept_duration(&mut self, run: RunId, millis: u64) -> SessionUpdate {
        let Some(current) = self
            .current_run
            .as_mut()
            .filter(|current| current.id == run)
        else {
            return SessionUpdate::default();
        };
        if millis == 0 || current.duration_millis == millis {
            return SessionUpdate::default();
        }
        current.duration_millis = millis;
        SessionUpdate::changed()
    }

    fn accept_buffering(&mut self, run: RunId, percent: u8) -> SessionUpdate {
        if !self
            .current_run
            .as_ref()
            .is_some_and(|current| current.id == run)
        {
            return SessionUpdate::default();
        }
        self.buffering_percent = Some(percent.min(100));
        SessionUpdate::changed()
    }

    fn accept_ended(&mut self, run: RunId, sample: &ClockSample) -> SessionUpdate {
        if !self
            .current_run
            .as_ref()
            .is_some_and(|current| current.id == run)
        {
            return SessionUpdate::default();
        }
        let desired_playing = self
            .current_run
            .as_ref()
            .is_some_and(|current| current.desired_playing);
        let mut update = SessionUpdate::changed();
        let reserved = self.next_plan.clone();
        self.finish_current(RunEndReason::Completed, sample, &mut update.effects);
        let next = self
            .sequence
            .advance_eos()
            .map(|entry| entry.occurrence.clone());
        if let Some(next) = next {
            self.next_plan = reserved.filter(|plan| plan.occurrence == next);
            self.promote_or_begin(next, desired_playing, &mut update.effects);
        } else {
            self.auto_dj_waiting_for_continuation = true;
            self.maybe_request_auto_dj(&mut update.effects);
        }
        update
    }

    fn accept_transitioned(
        &mut self,
        old_run: RunId,
        new_run: RunId,
        sample: &ClockSample,
    ) -> SessionUpdate {
        let current_run = self.current_run.as_ref().map(|current| current.id);
        if current_run == Some(new_run) {
            return SessionUpdate::default();
        }
        if current_run != Some(old_run) {
            return SessionUpdate {
                effects: vec![SessionEffect::Backend(BackendCommand::Stop {
                    run: new_run,
                })],
                ..SessionUpdate::default()
            };
        }
        let desired_playing = self
            .current_run
            .as_ref()
            .is_none_or(|current| current.desired_playing);
        if !self
            .next_plan
            .as_ref()
            .is_some_and(|plan| plan.current_run == old_run && plan.next_run == new_run)
        {
            let mut update = self.accept_ended(old_run, sample);
            update.effects.insert(
                0,
                SessionEffect::Backend(BackendCommand::Stop { run: new_run }),
            );
            return update;
        }
        let Some(occurrence) = self.next_plan.as_ref().map(|plan| plan.occurrence.clone()) else {
            return SessionUpdate::default();
        };
        let mut update = SessionUpdate::changed();
        self.finish_current(RunEndReason::Completed, sample, &mut update.effects);
        if !self.sequence.activate(&occurrence) {
            return update;
        }
        self.install_reserved_run(new_run, occurrence, desired_playing, &mut update.effects);
        self.mark_started(sample, &mut update.effects);
        if !desired_playing {
            update
                .effects
                .push(SessionEffect::Backend(BackendCommand::Pause {
                    run: new_run,
                }));
        }
        update
    }

    fn promote_or_begin(
        &mut self,
        occurrence: OccurrenceId,
        desired_playing: bool,
        effects: &mut Vec<SessionEffect>,
    ) {
        let plan = self
            .next_plan
            .take()
            .filter(|plan| plan.occurrence == occurrence);
        let Some(plan) = plan else {
            self.begin_selected_run(effects);
            if !desired_playing && let Some(current) = self.current_run.as_mut() {
                current.desired_playing = false;
            }
            return;
        };
        self.install_reserved_run(plan.next_run, occurrence, desired_playing, effects);
        match plan.resolution {
            NextResolution::Ready(stream) if desired_playing => {
                if let Some(current) = self.current_run.as_mut() {
                    current.status = TransportStatus::Buffering;
                }
                effects.push(SessionEffect::Backend(BackendCommand::Start {
                    run: plan.next_run,
                    current: stream,
                    next: None,
                    start_position_millis: 0,
                }));
            }
            NextResolution::Ready(stream) => {
                if let Some(current) = self.current_run.as_mut() {
                    current.status = TransportStatus::Paused;
                    current.resolved_stream = Some(stream);
                }
            }
            NextResolution::Resolving => {}
        }
    }

    fn install_reserved_run(
        &mut self,
        run: RunId,
        occurrence: OccurrenceId,
        desired_playing: bool,
        effects: &mut Vec<SessionEffect>,
    ) {
        let Some(entry) = self
            .sequence
            .selected()
            .filter(|entry| entry.occurrence == occurrence)
            .cloned()
        else {
            return;
        };
        let mut current = RunContext::resolving(
            run,
            self.play_id(run),
            self.sequence.source_id().clone(),
            &entry,
        );
        current.desired_playing = desired_playing;
        self.current_run = Some(current);
        effects.push(SessionEffect::CurrentMediaChanged);
        self.next_plan = None;
        self.buffering_percent = None;
        self.last_error = None;
        effects.push(self.state_effect());
        self.plan_next(effects);
    }

    fn begin_selected_run(&mut self, effects: &mut Vec<SessionEffect>) {
        let Some(entry) = self.sequence.selected().cloned() else {
            self.current_run = None;
            self.next_plan = None;
            return;
        };
        let run = self.next_run_id();
        self.current_run = Some(RunContext::resolving(
            run,
            self.play_id(run),
            self.sequence.source_id().clone(),
            &entry,
        ));
        effects.push(SessionEffect::CurrentMediaChanged);
        self.next_plan = None;
        self.buffering_percent = None;
        self.last_error = None;
        effects.push(self.resolve_effect(run, &entry));
        self.plan_next(effects);
        effects.push(self.state_effect());
    }

    fn plan_next(&mut self, effects: &mut Vec<SessionEffect>) {
        self.replan_next(false, effects);
    }

    fn replan_next_if_changed(&mut self, effects: &mut Vec<SessionEffect>) {
        self.replan_next(false, effects);
    }

    fn replan_next(&mut self, force: bool, effects: &mut Vec<SessionEffect>) {
        let Some(current) = self.current_run.as_ref() else {
            self.next_plan = None;
            return;
        };
        let current_run = current.id;
        let current_occurrence = current.occurrence.clone();
        let Some(current_entry) = self
            .sequence
            .selected()
            .filter(|entry| entry.occurrence == current_occurrence)
        else {
            self.next_plan = None;
            return;
        };
        let next = self.sequence.peek_next_eos().cloned();
        let Some(next) = next else {
            if self.next_plan.take().is_some() {
                effects.push(SessionEffect::Backend(BackendCommand::PrepareNext {
                    current_run,
                    next: None,
                }));
            }
            return;
        };
        let request = StreamRequest::new(next.track.id.clone(), self.settings.stream_quality);
        let transition = decided_transition(&self.settings, current_entry, &next);
        if !force
            && self.next_plan.as_ref().is_some_and(|plan| {
                plan.current_run == current_run
                    && plan.occurrence == next.occurrence
                    && plan.request == request
                    && plan.transition == transition
            })
        {
            return;
        }
        if self.next_plan.take().is_some() {
            effects.push(SessionEffect::Backend(BackendCommand::PrepareNext {
                current_run,
                next: None,
            }));
        }
        let next_run = self.next_run_id();
        self.next_plan = Some(NextPlan {
            current_run,
            next_run,
            occurrence: next.occurrence.clone(),
            request: request.clone(),
            transition,
            resolution: NextResolution::Resolving,
        });
        effects.push(SessionEffect::ResolveStream {
            run: next_run,
            source_id: self.sequence.source_id().clone(),
            occurrence: next.occurrence,
            request,
        });
    }

    fn resolve_effect(&self, run: RunId, entry: &SequenceEntry) -> SessionEffect {
        SessionEffect::ResolveStream {
            run,
            source_id: self.sequence.source_id().clone(),
            occurrence: entry.occurrence.clone(),
            request: StreamRequest::new(entry.track.id.clone(), self.settings.stream_quality),
        }
    }

    fn mark_started(&mut self, sample: &ClockSample, effects: &mut Vec<SessionEffect>) {
        let (run, track) = {
            let Some(current) = self.current_run.as_mut() else {
                return;
            };
            if current.started_at_unix_seconds.is_some() {
                return;
            }
            current.status = TransportStatus::Playing;
            current.started_at_unix_seconds = Some(sample.unix_seconds);
            current.local_period = Some(sample.local_period.clone());
            current.last_monotonic_millis = Some(sample.monotonic_millis);
            (current.id, current.track.clone())
        };
        effects.push(SessionEffect::Listening(ListeningFact::Started {
            run,
            started_at_unix_seconds: sample.unix_seconds,
            local_period: sample.local_period.clone(),
            track,
        }));
        if let Some(current) = self.current_run.as_ref() {
            effects.push(SessionEffect::SourceReport(self.source_report(
                current,
                SourceReportPhase::Started,
                false,
            )));
        }
        self.emit_progress_facts(effects);
    }

    fn emit_progress_facts(&mut self, effects: &mut Vec<SessionEffect>) -> bool {
        let playhead_millis = self.sequence.progress_millis();
        let (run, audible_millis, bucket_changed) = {
            let Some(current) = self.current_run.as_mut() else {
                return false;
            };
            if current.started_at_unix_seconds.is_none() {
                return false;
            }
            let bucket = playhead_millis / 10_000;
            let bucket_changed = current.last_progress_bucket != Some(bucket);
            if bucket_changed {
                current.last_progress_bucket = Some(bucket);
            }
            (current.id, current.audible_millis, bucket_changed)
        };
        effects.push(SessionEffect::Listening(ListeningFact::Progress {
            run,
            audible_millis,
            playhead_millis,
        }));
        self.qualify_current(effects);
        if bucket_changed {
            effects.push(self.progress_effect());
            if let Some(current) = self.current_run.as_ref() {
                effects.push(SessionEffect::SourceReport(self.source_report(
                    current,
                    SourceReportPhase::Progress,
                    false,
                )));
            }
        }
        bucket_changed
    }

    fn qualify_current(&mut self, effects: &mut Vec<SessionEffect>) {
        let activity = {
            let Some(current) = self.current_run.as_mut() else {
                return;
            };
            if !current.qualified
                && current.started_at_unix_seconds.is_some()
                && current.audible_millis
                    >= qualified_play_threshold_millis(current.duration_millis)
            {
                current.qualified = true;
                Some((
                    current.play_id.clone(),
                    current.id,
                    current.track.clone(),
                    current.local_period.clone(),
                    current.started_at_unix_seconds,
                ))
            } else {
                None
            }
        };
        let activity_qualified = activity.is_some();
        if let Some((play_id, run, track, Some(period), Some(started_at))) = activity {
            effects.push(SessionEffect::Activity(ListeningOutcome {
                play_id,
                run,
                source_id: track.source_id.clone(),
                track_id: track.track_id,
                local_period: period,
                qualified_plays: 1,
                skips: 0,
                last_played_at_unix_seconds: Some(started_at),
            }));
        }

        let external = {
            let Some(current) = self.current_run.as_mut() else {
                return;
            };
            let qualifies = external_scrobble_threshold_millis(current.duration_millis)
                .is_some_and(|threshold| current.audible_millis >= threshold);
            if !current.external_scrobble_emitted
                && current.started_at_unix_seconds.is_some()
                && qualifies
            {
                current.external_scrobble_emitted = true;
                Some((
                    current.play_id.clone(),
                    current.track.clone(),
                    current.started_at_unix_seconds,
                ))
            } else {
                None
            }
        };
        if let Some((play_id, track, Some(started_at))) = external {
            effects.push(SessionEffect::ExternalScrobble(crate::CompletedScrobble {
                play_id,
                track,
                started_at_unix_seconds: started_at,
            }));
        }

        if activity_qualified && let Some(current) = self.current_run.as_ref() {
            effects.push(SessionEffect::SourceReport(self.source_report(
                current,
                SourceReportPhase::QualifiedPlay,
                false,
            )));
        }
    }

    fn finish_current(
        &mut self,
        reason: RunEndReason,
        sample: &ClockSample,
        effects: &mut Vec<SessionEffect>,
    ) {
        let Some(current) = self.current_run.as_mut() else {
            self.next_plan = None;
            return;
        };
        current.advance_clock(sample.monotonic_millis);
        self.qualify_current(effects);
        let Some(current) = self.current_run.take() else {
            return;
        };
        effects.push(SessionEffect::CurrentMediaChanged);
        if current.started_at_unix_seconds.is_some() {
            effects.push(SessionEffect::Listening(ListeningFact::Ended {
                run: current.id,
                reason,
                audible_millis: current.audible_millis,
                playhead_millis: self.sequence.progress_millis(),
            }));
            effects.push(SessionEffect::SourceReport(self.source_report(
                &current,
                SourceReportPhase::Ended,
                reason == RunEndReason::Failed,
            )));
            if !current.qualified
                && manual_end_is_skip(
                    reason,
                    current.duration_millis,
                    current.audible_millis,
                    self.sequence.progress_millis(),
                )
                && let Some(period) = current.local_period.clone()
            {
                effects.push(SessionEffect::Activity(ListeningOutcome {
                    play_id: current.play_id,
                    run: current.id,
                    source_id: current.track.source_id.clone(),
                    track_id: current.track.track_id.clone(),
                    local_period: period,
                    qualified_plays: 0,
                    skips: 1,
                    last_played_at_unix_seconds: None,
                }));
            }
        }
        effects.push(self.progress_effect());
        self.next_plan = None;
        self.buffering_percent = None;
    }

    fn source_report(
        &self,
        current: &RunContext,
        phase: SourceReportPhase,
        failed: bool,
    ) -> SourceReportFact {
        SourceReportFact {
            run: current.id,
            source_id: current.track.source_id.clone(),
            track_id: current.track.track_id.clone(),
            phase,
            started_at_unix_seconds: current
                .started_at_unix_seconds
                .expect("source reports require a started Playback run"),
            position_millis: self.sequence.progress_millis(),
            paused: current.status == TransportStatus::Paused,
            muted: self.settings.muted,
            volume: self.settings.volume,
            shuffle: self.sequence.shuffle_enabled(),
            repeat_mode: self.sequence.repeat_mode(),
            failed,
        }
    }

    fn play_id(&self, run: RunId) -> String {
        format!("{}:{}", self.play_id_prefix, run.get())
    }

    fn progress_effect(&self) -> SessionEffect {
        SessionEffect::PersistProgress {
            source_id: self.sequence.source_id().clone(),
            revision: self.sequence.revision(),
            occurrence: self
                .sequence
                .selected()
                .map(|entry| entry.occurrence.clone()),
            progress_millis: self.sequence.progress_millis(),
        }
    }

    fn state_effect(&self) -> SessionEffect {
        SessionEffect::PersistState {
            source_id: self.sequence.source_id().clone(),
            revision: self.sequence.revision(),
            occurrence: self
                .sequence
                .selected()
                .map(|entry| entry.occurrence.clone()),
            progress_millis: self.sequence.progress_millis(),
        }
    }

    fn maybe_request_auto_dj(&mut self, effects: &mut Vec<SessionEffect>) {
        if !self.auto_dj_enabled
            || self.sequence.repeat_mode() == RepeatMode::One
            || self.sequence.remaining_after_selected() >= self.auto_dj_refill_threshold
            || self.auto_dj_in_flight.is_some()
        {
            return;
        }
        let Some(seed) = self.sequence.selected() else {
            return;
        };
        let key = AutoDjKey {
            source_id: self.sequence.source_id().clone(),
            seed_occurrence: seed.occurrence.clone(),
        };
        self.auto_dj_in_flight = Some(key.clone());
        effects.push(SessionEffect::RequestAutoDj(AutoDjRequest {
            source_id: key.source_id,
            seed_occurrence: key.seed_occurrence,
            seed_track_id: seed.track.id.clone(),
            requested_count: 5,
        }));
    }

    fn next_run_id(&mut self) -> RunId {
        let run = RunId::new(self.next_run_number);
        self.next_run_number = self.next_run_number.wrapping_add(1).max(1);
        run
    }
}

fn decided_transition(
    settings: &PlaybackSettings,
    current: &SequenceEntry,
    next: &SequenceEntry,
) -> NextTransition {
    match settings.transition_mode {
        PlaybackTransitionMode::Gapless => NextTransition::Gapless,
        PlaybackTransitionMode::Crossfade
            if settings.skip_same_album_crossfade
                && current.track.album_id == next.track.album_id =>
        {
            NextTransition::Gapless
        }
        PlaybackTransitionMode::Crossfade => NextTransition::Crossfade {
            duration_millis: u64::from(settings.crossfade_seconds) * 1_000,
        },
    }
}

#[cfg(test)]
mod tests {
    use library::{AlbumId, Track};

    use super::*;
    use crate::{BatchItem, Provenance, VolumeScale};

    #[test]
    fn current_media_changes_are_separate_from_position_and_control_updates() {
        let mut session = session(&[1, 2]);
        let started = session
            .handle_command(SessionCommand::Play, &sample(0))
            .expect("start");
        assert!(changes_current_media(&started));
        let run = session.current_run().expect("run");

        let position =
            session.handle_backend(BackendEvent::Position { run, millis: 500 }, &sample(1));
        assert!(!changes_current_media(&position));
        let volume = session
            .handle_command(SessionCommand::SetVolume(0.5), &sample(1))
            .expect("volume");
        assert!(!changes_current_media(&volume));

        let stopped = session
            .handle_command(SessionCommand::Stop, &sample(2))
            .expect("stop");
        assert!(changes_current_media(&stopped));
        let replayed = session
            .handle_command(SessionCommand::Play, &sample(3))
            .expect("replay");
        assert!(changes_current_media(&replayed));
        let next = session
            .handle_command(SessionCommand::Next, &sample(4))
            .expect("next");
        assert!(changes_current_media(&next));
    }

    #[test]
    fn volume_scale_change_uses_the_output_path_and_preserves_gain() {
        let mut session = session(&[1]);
        session
            .handle_command(SessionCommand::SetVolume(0.5), &sample(0))
            .expect("set perceptual volume");
        let expected_gain = VolumeScale::Perceptual.gain(0.5);
        let mut settings = session.settings().clone();
        settings.set_volume_scale_preserving_gain(VolumeScale::Linear);

        let update = session
            .handle_command(SessionCommand::UpdateSettings(settings), &sample(1))
            .expect("change volume scale");

        assert!(update.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::Backend(BackendCommand::SetOutputVolume {
                volume,
                volume_scale: VolumeScale::Linear,
                muted: false,
            }) if (*volume - expected_gain).abs() < 1e-12
        )));
        assert!(!update.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::Backend(BackendCommand::ConfigureAudio(_))
        )));
    }

    #[test]
    fn stopped_queue_and_track_replacements_signal_current_media_once() {
        let mut empty = empty_session();
        let addition = empty.reserve_materialization(Placement::End);
        let added = empty
            .apply_materialization(
                addition.id,
                &addition.source_id,
                batch(&[1]),
                Placement::End,
                &sample(0),
            )
            .expect("add to empty queue")
            .expect("accepted addition");
        assert_eq!(current_media_change_count(&added), 1);
        let cleared = empty
            .handle_command(SessionCommand::ClearUpcoming, &sample(1))
            .expect("clear stopped queue");
        assert_eq!(current_media_change_count(&cleared), 1);

        let mut session = session(&[1, 2]);
        let mut replacement = track(1);
        replacement.title = "Accepted replacement".to_string();
        let refreshed = session
            .handle_command(
                SessionCommand::RefreshTracks {
                    source_session_epoch: SourceSessionEpoch::new(1),
                    tracks: vec![replacement],
                },
                &sample(2),
            )
            .expect("refresh selected Track");
        assert_eq!(current_media_change_count(&refreshed), 1);
    }

    #[test]
    fn same_track_repeat_uses_a_fresh_run_and_transition_identity() {
        let mut session = session(&[1]);
        let start = sample(0);
        let update = session
            .handle_command(SessionCommand::PlayPause, &start)
            .expect("start command");
        let first = session.current_run().expect("first run");
        assert!(update.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::ResolveStream { run, .. } if *run == first
        )));
        session.stream_resolved(first, ResolvedStream::new("file:///track.flac"));
        session.handle_backend(BackendEvent::Started { run: first }, &sample(1));
        session.sequence.set_repeat_mode(RepeatMode::One);

        let ended = session.handle_backend(BackendEvent::Ended { run: first }, &sample(2));
        let second = session.current_run().expect("second run");
        assert_ne!(first, second);
        assert!(ended.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::ResolveStream { run, .. } if *run == second
        )));
    }

    #[test]
    fn next_preparation_failure_retains_playing_and_paused_fallbacks() {
        for pause_before_end in [false, true] {
            let (mut session, current_run, next_run) = session_with_resolved_next();
            let failed = session.handle_backend(
                BackendEvent::NextPreparationFailed {
                    current_run,
                    next_run,
                    error: crate::BackendFailure::new("next stream failed"),
                },
                &sample(2),
            );

            assert_eq!(session.current_run(), Some(current_run));
            assert!(session.desired_playing());
            assert!(matches!(
                failed.effects.as_slice(),
                [SessionEffect::NonfatalError(message)] if message == "next stream failed"
            ));
            assert!(!failed.effects.iter().any(|effect| matches!(
                effect,
                SessionEffect::FatalError(_)
                    | SessionEffect::Listening(ListeningFact::Ended { .. })
            )));
            if pause_before_end {
                session
                    .handle_command(SessionCommand::Pause, &sample(3))
                    .expect("pause before the failed handoff ends");
            }

            let ended =
                session.handle_backend(BackendEvent::Ended { run: current_run }, &sample(4));

            assert_eq!(session.current_run(), Some(next_run));
            assert_eq!(session.desired_playing(), !pause_before_end);
            let started_reserved = ended.effects.iter().any(|effect| {
                matches!(
                    effect,
                    SessionEffect::Backend(BackendCommand::Start {
                        run,
                        current,
                        start_position_millis: 0,
                        ..
                    }) if *run == next_run && current.uri() == "https://music.example/next.flac"
                )
            });
            assert_eq!(started_reserved, !pause_before_end);
            assert!(!ended.effects.iter().any(|effect| matches!(
                effect,
                SessionEffect::ResolveStream { run, .. } if *run == next_run
            )));

            if pause_before_end {
                let resumed = session
                    .handle_command(SessionCommand::Play, &sample(5))
                    .expect("resume the retained fallback");
                assert!(resumed.effects.iter().any(|effect| matches!(
                    effect,
                    SessionEffect::Backend(BackendCommand::Start { run, current, .. })
                        if *run == next_run
                            && current.uri() == "https://music.example/next.flac"
                )));
                assert!(!resumed.effects.iter().any(|effect| matches!(
                    effect,
                    SessionEffect::ResolveStream { run, .. } if *run == next_run
                )));
            }
        }
    }

    #[test]
    fn late_playing_state_cannot_resume_a_paused_transition() {
        let (mut session, current_run, next_run) = session_with_resolved_next();
        session
            .handle_command(SessionCommand::Pause, &sample(2))
            .expect("pause before the transition");
        session.handle_backend(
            BackendEvent::State {
                run: current_run,
                state: BackendState::Paused,
            },
            &sample(3),
        );

        session.handle_backend(
            BackendEvent::State {
                run: current_run,
                state: BackendState::Playing,
            },
            &sample(4),
        );
        let transitioned = session.handle_backend(
            BackendEvent::Transitioned {
                old_run: current_run,
                new_run: next_run,
            },
            &sample(5),
        );

        assert_eq!(session.current_run(), Some(next_run));
        assert!(!session.desired_playing());
        assert!(transitioned.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::Backend(BackendCommand::Pause { run }) if *run == next_run
        )));
    }

    #[test]
    fn late_paused_state_cannot_cancel_a_resumed_transition() {
        let (mut session, current_run, next_run) = session_with_resolved_next();
        session
            .handle_command(SessionCommand::Pause, &sample(2))
            .expect("pause before resuming");
        session
            .handle_command(SessionCommand::Play, &sample(3))
            .expect("resume before the transition");

        session.handle_backend(
            BackendEvent::State {
                run: current_run,
                state: BackendState::Paused,
            },
            &sample(4),
        );
        let transitioned = session.handle_backend(
            BackendEvent::Transitioned {
                old_run: current_run,
                new_run: next_run,
            },
            &sample(5),
        );

        assert_eq!(session.current_run(), Some(next_run));
        assert!(session.desired_playing());
        assert!(!transitioned.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::Backend(BackendCommand::Pause { run }) if *run == next_run
        )));
    }

    #[test]
    fn a_queue_change_wins_over_an_obsolete_prepared_transition() {
        let mut session = session(&[1, 2, 3]);
        let started = session
            .handle_command(SessionCommand::Play, &sample(0))
            .expect("start");
        let current_run = session.current_run().expect("current run");
        let obsolete_next_run = started
            .effects
            .iter()
            .find_map(|effect| match effect {
                SessionEffect::ResolveStream { run, .. } if *run != current_run => Some(*run),
                _ => None,
            })
            .expect("original reserved next run");
        session.stream_resolved(
            current_run,
            ResolvedStream::new("https://music.example/current.flac"),
        );
        session.handle_backend(BackendEvent::Started { run: current_run }, &sample(1));
        session.stream_resolved(
            obsolete_next_run,
            ResolvedStream::new("https://music.example/obsolete.flac"),
        );
        session
            .handle_command(SessionCommand::Pause, &sample(2))
            .expect("pause before the transition boundary");
        let replacement = session.sequence().entries()[2].occurrence.clone();
        let replanned = session
            .handle_command(SessionCommand::MoveAfterCurrent(replacement), &sample(3))
            .expect("replace the reserved next track");
        let replacement_run = replanned
            .effects
            .iter()
            .find_map(|effect| match effect {
                SessionEffect::ResolveStream { run, .. } => Some(*run),
                _ => None,
            })
            .expect("replacement reserved run");
        session.stream_resolved(
            replacement_run,
            ResolvedStream::new("https://music.example/replacement.flac"),
        );

        let transitioned = session.handle_backend(
            BackendEvent::Transitioned {
                old_run: current_run,
                new_run: obsolete_next_run,
            },
            &sample(4),
        );

        assert_eq!(session.current_run(), Some(replacement_run));
        assert!(!session.desired_playing());
        assert_eq!(
            session
                .sequence()
                .selected()
                .map(|entry| entry.track.id.clone()),
            Some(TrackId::fake(3))
        );
        assert!(transitioned.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::Backend(BackendCommand::Stop { run })
                if *run == obsolete_next_run
        )));
        assert!(!transitioned.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::Backend(BackendCommand::Start { run, .. })
                if *run == replacement_run
        )));
        assert!(!transitioned.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::Listening(ListeningFact::Started { run, .. })
                if *run == obsolete_next_run
        )));
    }

    #[test]
    fn context_activation_selects_the_exact_duplicate_occurrence() {
        let context_id = "route:tracks";
        let mut sequence = Sequence::new(SourceId::fake(1));
        sequence
            .apply_batch(
                Batch::new(vec![
                    BatchItem::new(
                        track(1),
                        Provenance::Context {
                            context_id: context_id.to_string(),
                            source_rank: 0,
                        },
                    ),
                    BatchItem::new(
                        track(1),
                        Provenance::Context {
                            context_id: context_id.to_string(),
                            source_rank: 1,
                        },
                    ),
                ]),
                Placement::Replace { anchor_index: 0 },
            )
            .expect("materialize context");
        let expected = sequence.entries()[1].occurrence.clone();
        let mut session = PlaybackSession::new(
            sequence,
            SourceSessionEpoch::new(1),
            "test",
            PlaybackSettings::default(),
            false,
            2,
        );
        session
            .handle_command(SessionCommand::PlayPause, &sample(0))
            .expect("start first occurrence");
        let first_run = session.current_run().expect("first run");
        session.stream_resolved(first_run, ResolvedStream::new("file:///first.flac"));
        session.handle_backend(BackendEvent::Started { run: first_run }, &sample(1));
        session.handle_backend(
            BackendEvent::Position {
                run: first_run,
                millis: 37_000,
            },
            &sample(2),
        );

        let update = session
            .activate_context(context_id, &TrackId::fake(1), 1, &sample(3))
            .expect("matching context occurrence");

        assert_eq!(
            session.sequence().selected().map(|entry| &entry.occurrence),
            Some(&expected)
        );
        assert!(update.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::ResolveStream { occurrence, .. } if occurrence == &expected
        )));
        assert_eq!(session.sequence().progress_millis(), 0);
        let second_run = session.current_run().expect("second run");
        let resolved =
            session.stream_resolved(second_run, ResolvedStream::new("file:///second.flac"));
        assert!(resolved.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::Backend(BackendCommand::Start {
                run,
                start_position_millis: 0,
                ..
            }) if *run == second_run
        )));
    }

    #[test]
    fn seek_does_not_increase_audible_time() {
        let mut session = session(&[1]);
        session
            .handle_command(SessionCommand::PlayPause, &sample(0))
            .expect("start");
        let run = session.current_run().expect("run");
        session.stream_resolved(run, ResolvedStream::new("file:///track.flac"));
        let started = session.handle_backend(BackendEvent::Started { run }, &sample(0));
        assert!(started.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::SourceReport(SourceReportFact {
                phase: SourceReportPhase::Started,
                started_at_unix_seconds: 1_700_000_000,
                ..
            })
        )));
        session.handle_backend(
            BackendEvent::Position {
                run,
                millis: 170_000,
            },
            &sample(4_000),
        );
        let ended = session
            .handle_command(SessionCommand::Next, &sample(4_000))
            .expect("next");
        assert!(ended.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::Activity(ListeningOutcome { skips: 1, .. })
        )));
        assert!(!ended.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::Activity(ListeningOutcome {
                qualified_plays: 1,
                ..
            })
        )));
    }

    #[test]
    fn final_clock_sample_qualifies_before_end_once() {
        let mut session = session(&[1]);
        session
            .handle_command(SessionCommand::PlayPause, &sample(0))
            .expect("start");
        let run = session.current_run().expect("run");
        session.stream_resolved(run, ResolvedStream::new("file:///track.flac"));
        session.handle_backend(BackendEvent::Started { run }, &sample(0));
        session.handle_backend(
            BackendEvent::Position {
                run,
                millis: 89_000,
            },
            &sample(89_000),
        );

        let ended = session.handle_backend(BackendEvent::Ended { run }, &sample(91_000));
        let qualified = ended
            .effects
            .iter()
            .position(|effect| {
                matches!(
                    effect,
                    SessionEffect::Activity(ListeningOutcome {
                        qualified_plays: 1,
                        ..
                    })
                )
            })
            .expect("qualified play");
        let finished = ended
            .effects
            .iter()
            .position(|effect| {
                matches!(
                    effect,
                    SessionEffect::Listening(ListeningFact::Ended { .. })
                )
            })
            .expect("ended fact");
        assert!(qualified < finished);
        assert_eq!(
            ended
                .effects
                .iter()
                .filter(|effect| matches!(
                    effect,
                    SessionEffect::Activity(ListeningOutcome {
                        qualified_plays: 1,
                        ..
                    })
                ))
                .count(),
            1
        );
    }

    #[test]
    fn pause_persists_exact_playhead_and_pause_resume_report_state() {
        let mut session = session(&[1]);
        session
            .handle_command(SessionCommand::PlayPause, &sample(0))
            .expect("start");
        let run = session.current_run().expect("run");
        session.stream_resolved(run, ResolvedStream::new("file:///track.flac"));
        session.handle_backend(BackendEvent::Started { run }, &sample(0));
        session.handle_backend(
            BackendEvent::Position {
                run,
                millis: 30_100,
            },
            &sample(30_100),
        );
        session.handle_backend(
            BackendEvent::Position {
                run,
                millis: 34_500,
            },
            &sample(34_500),
        );

        let paused = session.handle_backend(
            BackendEvent::State {
                run,
                state: BackendState::Paused,
            },
            &sample(34_500),
        );
        assert!(paused.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::PersistProgress {
                progress_millis: 34_500,
                ..
            }
        )));
        assert!(paused.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::SourceReport(SourceReportFact {
                phase: SourceReportPhase::Progress,
                paused: true,
                ..
            })
        )));

        let resumed = session.handle_backend(
            BackendEvent::State {
                run,
                state: BackendState::Playing,
            },
            &sample(35_000),
        );
        assert!(resumed.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::SourceReport(SourceReportFact {
                phase: SourceReportPhase::Progress,
                paused: false,
                ..
            })
        )));
    }

    #[test]
    fn accepted_seek_emits_a_position_discontinuity_for_the_current_run_only() {
        let mut session = session(&[1, 2]);
        let inactive = session
            .handle_command(SessionCommand::Seek(12_000), &sample(0))
            .expect("inactive seek");
        assert!(
            !inactive
                .effects
                .iter()
                .any(|effect| matches!(effect, SessionEffect::PositionDiscontinuity(_)))
        );

        session
            .handle_command(SessionCommand::PlayPause, &sample(0))
            .expect("start");
        let run = session.current_run().expect("current run");
        let accepted = session
            .handle_command(SessionCommand::Seek(42_000), &sample(0))
            .expect("active seek");

        assert!(accepted.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::PositionDiscontinuity(discontinuity)
                if discontinuity.run == run && discontinuity.position_millis == 42_000
        )));

        let changed_track = session
            .handle_command(SessionCommand::Next, &sample(0))
            .expect("next track");
        assert!(
            !changed_track
                .effects
                .iter()
                .any(|effect| matches!(effect, SessionEffect::PositionDiscontinuity(_)))
        );
    }

    #[test]
    fn additive_materializations_are_independent_but_replacement_supersedes() {
        let mut session = session(&[1]);
        let first = session.reserve_materialization(Placement::End).id;
        let second = session.reserve_materialization(Placement::End).id;
        assert!(
            session
                .apply_materialization(
                    first,
                    &SourceId::fake(1),
                    batch(&[2]),
                    Placement::End,
                    &sample(0),
                )
                .expect("first")
                .is_some()
        );
        assert!(
            session
                .apply_materialization(
                    second,
                    &SourceId::fake(1),
                    batch(&[3]),
                    Placement::End,
                    &sample(0),
                )
                .expect("second")
                .is_some()
        );

        let old_replace = session
            .reserve_materialization(Placement::Replace { anchor_index: 0 })
            .id;
        let new_replace = session
            .reserve_materialization(Placement::Replace { anchor_index: 0 })
            .id;
        assert!(
            session
                .apply_materialization(
                    old_replace,
                    &SourceId::fake(1),
                    batch(&[4]),
                    Placement::Replace { anchor_index: 0 },
                    &sample(0),
                )
                .expect("old")
                .is_none()
        );
        assert!(
            session
                .apply_materialization(
                    new_replace,
                    &SourceId::fake(1),
                    batch(&[5]),
                    Placement::Replace { anchor_index: 0 },
                    &sample(0),
                )
                .expect("new")
                .is_some()
        );
    }

    #[test]
    fn materialization_reservation_captures_live_exclusions_and_reports_failure() {
        let mut session = session(&[1, 1, 2]);
        let reservation = session.reserve_materialization(Placement::End);

        assert_eq!(reservation.current_track_id, Some(TrackId::fake(1)));
        assert_eq!(
            reservation.queued_track_ids,
            [TrackId::fake(1), TrackId::fake(2)]
        );

        let failed = session
            .fail_materialization(
                reservation.id,
                &reservation.source_id,
                Placement::End,
                "no matching radio tracks were found".to_string(),
            )
            .expect("active reservation");
        assert!(matches!(
            failed.effects.as_slice(),
            [SessionEffect::NonfatalError(message)]
                if message == "no matching radio tracks were found"
        ));
    }

    #[test]
    fn replacement_reservation_cancels_auto_dj_without_cancelling_later_additions() {
        let mut session = session(&[1]);
        let auto_dj = session
            .handle_command(
                SessionCommand::SetAutoDj {
                    enabled: true,
                    refill_threshold: 2,
                },
                &sample(0),
            )
            .expect("enable Auto DJ");
        let request = auto_dj.effects.into_iter().find_map(|effect| match effect {
            SessionEffect::RequestAutoDj(request) => Some(request),
            _ => None,
        });
        let request = request.expect("Auto DJ request");

        let replacement = session.reserve_materialization(Placement::Replace { anchor_index: 0 });
        assert!(
            session
                .complete_auto_dj(
                    &request.source_id,
                    &request.seed_occurrence,
                    batch(&[2]),
                    &sample(0),
                )
                .expect("old Auto DJ completion")
                .is_none()
        );

        let addition = session.reserve_materialization(Placement::End);
        assert!(
            session
                .apply_materialization(
                    addition.id,
                    &addition.source_id,
                    batch(&[3]),
                    Placement::End,
                    &sample(0),
                )
                .expect("addition")
                .is_some()
        );
        assert!(
            session
                .apply_materialization(
                    replacement.id,
                    &replacement.source_id,
                    batch(&[4]),
                    Placement::Replace { anchor_index: 0 },
                    &sample(0),
                )
                .expect("replacement")
                .is_some()
        );
    }

    #[test]
    fn auto_dj_filters_live_queue_and_candidate_duplicates_without_scanning_request_state() {
        let mut session = session(&[1]);
        let enabled = session
            .handle_command(
                SessionCommand::SetAutoDj {
                    enabled: true,
                    refill_threshold: 2,
                },
                &sample(0),
            )
            .expect("enable Auto DJ");
        let request = enabled.effects.into_iter().find_map(|effect| match effect {
            SessionEffect::RequestAutoDj(request) => Some(request),
            _ => None,
        });
        let request = request.expect("Auto DJ request");

        session
            .complete_auto_dj_candidates(
                &request.source_id,
                &request.seed_occurrence,
                [track(1), track(2), track(2), track(3)]
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                request.requested_count,
                42,
                &sample(0),
            )
            .expect("candidate completion")
            .expect("accepted candidates");

        assert_eq!(
            session
                .sequence()
                .entries()
                .iter()
                .map(|entry| entry.track.id.clone())
                .collect::<Vec<_>>(),
            [1, 2, 3].into_iter().map(TrackId::fake).collect::<Vec<_>>()
        );
    }

    #[test]
    fn clear_without_an_active_run_empties_the_sequence_and_cancels_pending_work() {
        let mut session = session(&[1, 2, 3]);
        let auto_dj = session
            .handle_command(
                SessionCommand::SetAutoDj {
                    enabled: true,
                    refill_threshold: 4,
                },
                &sample(0),
            )
            .expect("enable Auto DJ");
        let request = auto_dj
            .effects
            .into_iter()
            .find_map(|effect| match effect {
                SessionEffect::RequestAutoDj(request) => Some(request),
                _ => None,
            })
            .expect("Auto DJ request");
        let pending = session.reserve_materialization(Placement::End);

        let cleared = session
            .handle_command(SessionCommand::ClearUpcoming, &sample(0))
            .expect("clear");

        assert!(cleared.structure_changed);
        assert!(session.sequence().entries().is_empty());
        assert!(session.sequence().selected().is_none());
        assert!(
            session
                .apply_materialization(
                    pending.id,
                    &pending.source_id,
                    batch(&[2]),
                    Placement::End,
                    &sample(0),
                )
                .expect("obsolete addition")
                .is_none()
        );
        assert!(
            session
                .complete_auto_dj(
                    &request.source_id,
                    &request.seed_occurrence,
                    batch(&[4]),
                    &sample(0),
                )
                .expect("obsolete Auto DJ completion")
                .is_none()
        );
    }

    #[test]
    fn clear_after_stop_removes_the_former_current_occurrence() {
        let mut session = session(&[1, 2]);
        session
            .handle_command(SessionCommand::PlayPause, &sample(0))
            .expect("start");
        session
            .handle_command(SessionCommand::Stop, &sample(1))
            .expect("stop");
        assert!(session.current_run().is_none());

        let cleared = session
            .handle_command(SessionCommand::ClearUpcoming, &sample(2))
            .expect("clear");

        assert!(cleared.structure_changed);
        assert!(session.sequence().entries().is_empty());
    }

    #[test]
    fn clear_preserves_the_playing_occurrence_without_stopping_it() {
        let mut session = session(&[1, 2, 3]);
        let current = session
            .sequence()
            .selected()
            .map(|entry| entry.occurrence.clone())
            .expect("selected occurrence");
        session
            .handle_command(SessionCommand::PlayPause, &sample(0))
            .expect("start");
        let run = session.current_run().expect("run");
        session.stream_resolved(run, ResolvedStream::new("file:///track.flac"));
        session.handle_backend(BackendEvent::Started { run }, &sample(1));
        session.handle_backend(
            BackendEvent::State {
                run,
                state: BackendState::Playing,
            },
            &sample(1),
        );

        let cleared = session
            .handle_command(SessionCommand::ClearUpcoming, &sample(2))
            .expect("clear");

        assert!(cleared.structure_changed);
        assert_eq!(session.current_run(), Some(run));
        assert_eq!(session.status(), TransportStatus::Playing);
        assert_eq!(session.sequence().entries().len(), 1);
        assert_eq!(
            session.sequence().selected().map(|entry| &entry.occurrence),
            Some(&current)
        );
        assert!(
            !cleared.effects.iter().any(|effect| matches!(
                effect,
                SessionEffect::Backend(BackendCommand::Stop { .. })
            ))
        );
    }

    #[test]
    fn clear_preserves_the_paused_occurrence_for_resume() {
        let mut session = session(&[1, 2, 3]);
        let current = session
            .sequence()
            .selected()
            .map(|entry| entry.occurrence.clone())
            .expect("selected occurrence");
        session
            .handle_command(SessionCommand::PlayPause, &sample(0))
            .expect("start");
        let run = session.current_run().expect("run");
        session.stream_resolved(run, ResolvedStream::new("file:///track.flac"));
        session.handle_backend(BackendEvent::Started { run }, &sample(1));
        session.handle_backend(
            BackendEvent::State {
                run,
                state: BackendState::Paused,
            },
            &sample(2),
        );

        let cleared = session
            .handle_command(SessionCommand::ClearUpcoming, &sample(3))
            .expect("clear");

        assert!(cleared.structure_changed);
        assert_eq!(session.current_run(), Some(run));
        assert_eq!(session.status(), TransportStatus::Paused);
        assert_eq!(session.sequence().entries().len(), 1);
        assert_eq!(
            session.sequence().selected().map(|entry| &entry.occurrence),
            Some(&current)
        );
        assert!(
            !cleared.effects.iter().any(|effect| matches!(
                effect,
                SessionEffect::Backend(BackendCommand::Stop { .. })
            ))
        );
    }

    #[test]
    fn changing_shuffle_invalidates_a_pending_replacement() {
        let mut session = session(&[1, 2, 3]);
        let pending = session.reserve_materialization(Placement::Replace { anchor_index: 0 });
        session
            .handle_command(
                SessionCommand::SetShuffle {
                    enabled: true,
                    seed: 42,
                },
                &sample(0),
            )
            .expect("shuffle");

        assert!(
            session
                .apply_materialization(
                    pending.id,
                    &pending.source_id,
                    batch(&[4]),
                    Placement::Replace { anchor_index: 0 },
                    &sample(0),
                )
                .expect("obsolete replacement")
                .is_none()
        );
    }

    #[test]
    fn pause_while_resolving_does_not_start_audio_until_resumed() {
        let mut session = session(&[1]);
        session
            .handle_command(SessionCommand::PlayPause, &sample(0))
            .expect("start resolving");
        let run = session.current_run().expect("run");
        session
            .handle_command(SessionCommand::PlayPause, &sample(0))
            .expect("pause resolving");
        assert_eq!(session.status(), TransportStatus::Paused);

        let resolved = session.stream_resolved(run, ResolvedStream::new("file:///track.flac"));
        assert_eq!(session.status(), TransportStatus::Paused);
        assert!(
            !resolved.effects.iter().any(|effect| matches!(
                effect,
                SessionEffect::Backend(BackendCommand::Start { .. })
            ))
        );

        let resumed = session
            .handle_command(SessionCommand::PlayPause, &sample(0))
            .expect("resume");
        assert!(resumed.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::Backend(BackendCommand::Start { run: started, .. }) if *started == run
        )));
    }

    #[test]
    fn pause_can_cancel_resume_before_the_backend_state_arrives() {
        let mut session = session(&[1]);
        session
            .handle_command(SessionCommand::PlayPause, &sample(0))
            .expect("start");
        let run = session.current_run().expect("run");
        session.stream_resolved(run, ResolvedStream::new("file:///track.flac"));
        session.handle_backend(BackendEvent::Started { run }, &sample(0));

        let paused = session
            .handle_command(SessionCommand::PlayPause, &sample(1))
            .expect("pause");
        assert!(paused.view_changed);
        assert!(!session.desired_playing());
        session.handle_backend(
            BackendEvent::State {
                run,
                state: BackendState::Paused,
            },
            &sample(1),
        );
        let resumed = session
            .handle_command(SessionCommand::PlayPause, &sample(2))
            .expect("resume");
        assert!(resumed.view_changed);
        assert!(session.desired_playing());
        assert!(resumed.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::Backend(BackendCommand::Play { run: resumed }) if *resumed == run
        )));

        let cancelled = session
            .handle_command(SessionCommand::PlayPause, &sample(2))
            .expect("cancel resume");
        assert!(cancelled.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::Backend(BackendCommand::Pause { run: paused }) if *paused == run
        )));
    }

    #[test]
    fn resume_can_cancel_pause_before_the_backend_state_arrives() {
        let mut session = session(&[1]);
        session
            .handle_command(SessionCommand::PlayPause, &sample(0))
            .expect("start");
        let run = session.current_run().expect("run");
        session.stream_resolved(run, ResolvedStream::new("file:///track.flac"));
        session.handle_backend(BackendEvent::Started { run }, &sample(0));

        let paused = session
            .handle_command(SessionCommand::PlayPause, &sample(1))
            .expect("pause");
        assert!(paused.view_changed);
        assert!(!session.desired_playing());

        let resumed = session
            .handle_command(SessionCommand::PlayPause, &sample(1))
            .expect("cancel pause");
        assert!(resumed.view_changed);
        assert!(session.desired_playing());
        assert!(resumed.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::Backend(BackendCommand::Play { run: resumed }) if *resumed == run
        )));
    }

    #[test]
    fn stopped_seek_is_persisted_and_shutdown_keeps_the_live_playhead() {
        let mut session = session(&[1]);
        let pending = session.reserve_materialization(Placement::End);
        let stopped_seek = session
            .handle_command(SessionCommand::Seek(12_000), &sample(0))
            .expect("stopped seek");
        assert!(stopped_seek.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::PersistProgress {
                progress_millis: 12_000,
                ..
            }
        )));

        session
            .handle_command(SessionCommand::PlayPause, &sample(0))
            .expect("start");
        let run = session.current_run().expect("run");
        session.stream_resolved(run, ResolvedStream::new("file:///track.flac"));
        session.handle_backend(BackendEvent::Started { run }, &sample(0));
        session.handle_backend(
            BackendEvent::Position {
                run,
                millis: 37_000,
            },
            &sample(1_000),
        );

        let shutdown = session.shutdown(&sample(1_000));
        assert_eq!(session.sequence().progress_millis(), 37_000);
        assert!(
            shutdown
                .effects
                .iter()
                .any(|effect| matches!(effect, SessionEffect::FlushPersistence { .. }))
        );
        assert!(
            session
                .apply_materialization(
                    pending.id,
                    &pending.source_id,
                    batch(&[2]),
                    Placement::End,
                    &sample(1_000),
                )
                .expect("obsolete post-shutdown materialization")
                .is_none()
        );
    }

    #[test]
    fn stream_input_change_replans_next_without_stopping_the_current_run() {
        let mut session = session(&[1, 2]);
        session
            .handle_command(SessionCommand::PlayPause, &sample(0))
            .expect("start");
        let run = session.current_run().expect("run");
        session.stream_resolved(run, ResolvedStream::new("file:///track.flac"));

        let changed = session
            .handle_command(SessionCommand::StreamInputsChanged, &sample(0))
            .expect("replan stream inputs");

        assert_eq!(session.current_run(), Some(run));
        assert!(changed.effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::ResolveStream { run: next, .. } if *next != run
        )));
        assert!(
            !changed.effects.iter().any(|effect| matches!(
                effect,
                SessionEffect::Backend(BackendCommand::Stop { .. })
            ))
        );
    }

    #[test]
    fn accepted_track_refresh_replaces_every_occurrence_without_changing_current_identity() {
        let mut session = session(&[1, 1, 2]);
        assert_eq!(
            session.sequence().unique_track_ids(),
            vec![TrackId::fake(1), TrackId::fake(2)]
        );
        session
            .handle_command(SessionCommand::Play, &sample(0))
            .expect("start selected Track");
        let run = session.current_run().expect("current run");
        session.handle_backend(BackendEvent::Started { run }, &sample(1));
        session.handle_backend(
            BackendEvent::Position {
                run,
                millis: 42_000,
            },
            &sample(2),
        );
        let before = session.view().transport.current.expect("current media");

        let mut changed_track = track(1);
        changed_track.title = "Accepted replacement".to_string();
        changed_track.favorite = true;
        let ignored = session
            .handle_command(
                SessionCommand::RefreshTracks {
                    source_session_epoch: SourceSessionEpoch::new(2),
                    tracks: vec![changed_track.clone()],
                },
                &sample(3),
            )
            .expect("ignore stale refresh");
        assert!(!ignored.view_changed);

        let accepted = session
            .handle_command(
                SessionCommand::RefreshTracks {
                    source_session_epoch: SourceSessionEpoch::new(1),
                    tracks: vec![changed_track.clone()],
                },
                &sample(4),
            )
            .expect("apply accepted refresh");
        assert!(accepted.structure_changed);
        assert_eq!(session.current_run(), Some(run));
        assert_eq!(session.position_millis(), 42_000);
        assert!(Track::ptr_eq(
            &session.sequence().entries()[0].track,
            &changed_track
        ));
        assert!(Track::ptr_eq(
            &session.sequence().entries()[1].track,
            &changed_track
        ));
        let after = session.view().transport.current.expect("refreshed media");
        assert_eq!(before.id, after.id);
        assert_eq!(after.track.title, "Accepted replacement");
        assert!(after.track.favorite);
    }

    fn session(numbers: &[u32]) -> PlaybackSession {
        let mut sequence = Sequence::new(SourceId::fake(1));
        sequence
            .apply_batch(batch(numbers), Placement::Replace { anchor_index: 0 })
            .expect("seed sequence");
        PlaybackSession::new(
            sequence,
            SourceSessionEpoch::new(1),
            "test",
            PlaybackSettings::default(),
            false,
            2,
        )
    }

    fn session_with_resolved_next() -> (PlaybackSession, RunId, RunId) {
        let mut session = session(&[1, 2]);
        let started = session
            .handle_command(SessionCommand::Play, &sample(0))
            .expect("start");
        let current_run = session.current_run().expect("current run");
        let next_run = started
            .effects
            .iter()
            .find_map(|effect| match effect {
                SessionEffect::ResolveStream { run, .. } if *run != current_run => Some(*run),
                _ => None,
            })
            .expect("reserved next run");
        session.stream_resolved(
            current_run,
            ResolvedStream::new("https://music.example/current.flac"),
        );
        session.handle_backend(BackendEvent::Started { run: current_run }, &sample(1));
        session.stream_resolved(
            next_run,
            ResolvedStream::new("https://music.example/next.flac"),
        );
        (session, current_run, next_run)
    }

    fn empty_session() -> PlaybackSession {
        PlaybackSession::new(
            Sequence::new(SourceId::fake(1)),
            SourceSessionEpoch::new(1),
            "test",
            PlaybackSettings::default(),
            false,
            2,
        )
    }

    fn batch(numbers: &[u32]) -> Batch {
        Batch::new(
            numbers
                .iter()
                .map(|number| BatchItem::new(track(*number), Provenance::Manual))
                .collect(),
        )
    }

    fn track(number: u32) -> Track {
        Track::new(library::TrackData {
            id: TrackId::fake(number),
            album_id: Some(AlbumId::fake(1)),
            title: format!("Track {number}"),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            album_artwork: None,
            year: 2026,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 180,
            favorite: false,
            disc_number: 1,
            track_number: number as u16,
            image_ref: None,
            local_artwork: None,
            musicbrainz_recording_id: None,
            musicbrainz_release_track_id: None,
            source_path: None,
            cue: None,
            source_format: None,
            comment: None,
            skip_count: None,
            bpm: None,
            relations: library::TrackRelations::default(),
        })
    }

    fn sample(monotonic_millis: u64) -> ClockSample {
        ClockSample {
            monotonic_millis,
            unix_seconds: 1_700_000_000,
            local_period: "2026-07".to_string(),
        }
    }

    fn changes_current_media(update: &SessionUpdate) -> bool {
        current_media_change_count(update) != 0
    }

    fn current_media_change_count(update: &SessionUpdate) -> usize {
        update
            .effects
            .iter()
            .filter(|effect| matches!(effect, SessionEffect::CurrentMediaChanged))
            .count()
    }
}
