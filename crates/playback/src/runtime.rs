use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use library::{PlaybackCheckpoint, ResolvedStream, SourceId, Track};
use thiserror::Error;

use crate::{
    BackendEvent, BackendFailure, Batch, ClockSample, LoadedPlayRequest, MaterializationId,
    MaterializationReservation, Placement, PlaybackBackend, PlaybackNotice, PlaybackProjection,
    PlaybackSession, PlaybackSettings, QueuePage, QueuePageQuery, RunId, Sequence, SequenceError,
    SessionCommand, SessionEffect, SessionUpdate, SourceSessionEpoch, build_checkpoint,
};

const BACKEND_POLL_INTERVAL: Duration = Duration::from_millis(33);

#[derive(Debug, Error)]
pub enum PlaybackError {
    #[error("playback runtime state is unavailable")]
    Unavailable,
    #[error("the loaded music selection belongs to an inactive source session")]
    InactiveSourceSession,
    #[error("could not start the playback worker: {0}")]
    WorkerStart(String),
    #[error("the playback worker stopped unexpectedly")]
    WorkerStopped,
    #[error("could not stop the playback backend: {0}")]
    BackendShutdown(String),
    #[error(transparent)]
    Sequence(#[from] SequenceError),
}

pub type PlaybackResult<T> = Result<T, PlaybackError>;

#[derive(Debug, Default)]
pub struct PlaybackUpdate {
    pub checkpoint: Option<PlaybackCheckpoint>,
    pub projection: Option<PlaybackProjection>,
    pub effects: Vec<SessionEffect>,
    pub current_media_changed: bool,
}

impl PlaybackUpdate {
    fn is_empty(&self) -> bool {
        self.checkpoint.is_none()
            && self.projection.is_none()
            && self.effects.is_empty()
            && !self.current_media_changed
    }

    fn merge(&mut self, mut newer: Self) {
        if newer.checkpoint.is_some() {
            self.checkpoint = newer.checkpoint.take();
        }
        match (&mut self.projection, newer.projection.take()) {
            (Some(current), Some(mut next)) => {
                let mut notices = std::mem::take(&mut current.notices);
                notices.append(&mut next.notices);
                next.notices = notices;
                self.projection = Some(next);
            }
            (None, Some(next)) => self.projection = Some(next),
            _ => {}
        }
        self.effects.append(&mut newer.effects);
        self.current_media_changed |= newer.current_media_changed;
    }
}

/// Playback's serialized command edge and ordered output stream.
///
/// The session and backend are kept on one thread so a stream completion
/// cannot publish ahead of an earlier GTK command. Rufin consumes each
/// [`PlaybackUpdate`] in this order and applies persistence, Source, and UI
/// effects without creating a second playback-state owner.
#[derive(Clone)]
pub struct Playback {
    inner: Arc<PlaybackInner>,
}

struct PlaybackInner {
    commands: SyncSender<RuntimeCommand>,
    threads: Mutex<Option<(JoinHandle<()>, JoinHandle<()>)>>,
}

type Reply<T> = SyncSender<PlaybackResult<T>>;
type Clock = Arc<dyn Fn() -> ClockSample + Send + Sync>;

enum RuntimeCommand {
    Session {
        command: SessionCommand,
        reply: Reply<()>,
    },
    RefreshTracks {
        source_session_epoch: SourceSessionEpoch,
        tracks: Vec<Track>,
        reply: Reply<PlaybackProjection>,
    },
    AdmitLoaded {
        source_id: SourceId,
        source_session_epoch: SourceSessionEpoch,
        activation: Option<(String, library::TrackId, usize)>,
        placement: Placement,
        reply: Reply<Option<MaterializationReservation>>,
    },
    ReserveMaterialization {
        placement: Placement,
        reply: Reply<MaterializationReservation>,
    },
    CompleteMaterialization {
        id: MaterializationId,
        source_id: SourceId,
        batch: Batch,
        placement: Placement,
        reply: Reply<bool>,
    },
    FailMaterialization {
        id: MaterializationId,
        source_id: SourceId,
        placement: Placement,
        message: String,
        reply: Reply<bool>,
    },
    CancelMaterialization {
        id: MaterializationId,
        source_id: SourceId,
        placement: Placement,
        reply: Reply<bool>,
    },
    ResolveStream {
        run: RunId,
        stream: Result<ResolvedStream, String>,
        reply: Reply<()>,
    },
    CompleteAutoDj {
        source_id: SourceId,
        seed_occurrence: crate::OccurrenceId,
        candidates: Vec<Track>,
        requested_count: usize,
        shuffle_seed: u64,
        reply: Reply<bool>,
    },
    AutoDjUnavailable {
        source_id: SourceId,
        seed_occurrence: crate::OccurrenceId,
        error: Option<String>,
        reply: Reply<bool>,
    },
    QueuePage {
        query: QueuePageQuery,
        reply: Reply<QueuePage>,
    },
    QueuedTrackIds {
        reply: Reply<(SourceId, SourceSessionEpoch, Vec<library::TrackId>)>,
    },
    CurrentMedia {
        reply: Reply<Option<Arc<crate::CurrentMedia>>>,
    },
    Projection {
        reply: Reply<PlaybackProjection>,
    },
    Shutdown {
        reply: Reply<()>,
    },
}

#[expect(
    clippy::large_enum_variant,
    reason = "playback updates stay inline so the frequent output path does not allocate"
)]
enum PlaybackOutput {
    Update(PlaybackUpdate),
    Fence(SyncSender<()>),
    Shutdown,
}

/// Playback's one live queue, transport session, and physical backend.
///
/// Callers send typed operations through this handle. Playback owns its clock,
/// backend polling cadence, and ordered output worker; callers cannot mutate
/// the session or drive the backend through a second path.
impl Playback {
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        sequence: Sequence,
        source_session_epoch: SourceSessionEpoch,
        play_id_prefix: impl Into<Arc<str>>,
        settings: PlaybackSettings,
        auto_dj_enabled: bool,
        auto_dj_refill_threshold: usize,
        backend: Box<dyn PlaybackBackend>,
        clock: Clock,
        consume: impl FnMut(PlaybackUpdate) + Send + 'static,
    ) -> PlaybackResult<(Self, PlaybackProjection)> {
        let runtime = PlaybackRuntime::new(
            sequence,
            source_session_epoch,
            play_id_prefix,
            settings,
            auto_dj_enabled,
            auto_dj_refill_threshold,
            backend,
        );
        let initial_projection = runtime.initial_projection();
        let (commands, command_receiver) = sync_channel(0);
        let (outputs, output_receiver) = sync_channel(0);
        let output_thread = thread::Builder::new()
            .name("rufin-playback-output".to_string())
            .spawn(move || run_playback_outputs(output_receiver, consume))
            .map_err(|error| PlaybackError::WorkerStart(error.to_string()))?;
        let actor_thread = match thread::Builder::new()
            .name("rufin-playback".to_string())
            .spawn(move || run_playback(runtime, command_receiver, outputs, clock))
        {
            Ok(thread) => thread,
            Err(error) => {
                let _ = output_thread.join();
                return Err(PlaybackError::WorkerStart(error.to_string()));
            }
        };
        Ok((
            Self {
                inner: Arc::new(PlaybackInner {
                    commands,
                    threads: Mutex::new(Some((actor_thread, output_thread))),
                }),
            },
            initial_projection,
        ))
    }

    pub fn command(&self, command: SessionCommand) -> PlaybackResult<()> {
        self.request(|reply| RuntimeCommand::Session { command, reply })
    }

    /// Replaces queued Track values and returns the coherent current
    /// projection without publishing it through the ordinary output stream.
    ///
    /// Rufin uses this during a same-source Library replacement so GTK receives
    /// the new Library and matching Playback projection in one Source event.
    pub fn refresh_tracks(
        &self,
        source_session_epoch: SourceSessionEpoch,
        tracks: Vec<Track>,
    ) -> PlaybackResult<PlaybackProjection> {
        self.request(|reply| RuntimeCommand::RefreshTracks {
            source_session_epoch,
            tracks,
            reply,
        })
    }

    pub fn admit_loaded(
        &self,
        request: &LoadedPlayRequest,
    ) -> PlaybackResult<Option<MaterializationReservation>> {
        self.request(|reply| RuntimeCommand::AdmitLoaded {
            source_id: request.source_id.clone(),
            source_session_epoch: request.source_session_epoch,
            activation: request.activation_context(),
            placement: request.placement(),
            reply,
        })
    }

    pub fn reserve_materialization(
        &self,
        placement: Placement,
    ) -> PlaybackResult<MaterializationReservation> {
        self.request(|reply| RuntimeCommand::ReserveMaterialization { placement, reply })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn complete_materialization(
        &self,
        id: MaterializationId,
        source_id: SourceId,
        batch: Batch,
        placement: Placement,
    ) -> PlaybackResult<bool> {
        self.request(|reply| RuntimeCommand::CompleteMaterialization {
            id,
            source_id,
            batch,
            placement,
            reply,
        })
    }

    pub fn fail_materialization(
        &self,
        id: MaterializationId,
        source_id: SourceId,
        placement: Placement,
        message: String,
    ) -> PlaybackResult<bool> {
        self.request(|reply| RuntimeCommand::FailMaterialization {
            id,
            source_id,
            placement,
            message,
            reply,
        })
    }

    pub fn cancel_materialization(
        &self,
        id: MaterializationId,
        source_id: SourceId,
        placement: Placement,
    ) -> PlaybackResult<bool> {
        self.request(|reply| RuntimeCommand::CancelMaterialization {
            id,
            source_id,
            placement,
            reply,
        })
    }

    pub fn resolve_stream(
        &self,
        run: RunId,
        stream: Result<ResolvedStream, String>,
    ) -> PlaybackResult<()> {
        self.request(|reply| RuntimeCommand::ResolveStream { run, stream, reply })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn complete_auto_dj_candidates(
        &self,
        source_id: SourceId,
        seed_occurrence: crate::OccurrenceId,
        candidates: Vec<Track>,
        requested_count: usize,
        shuffle_seed: u64,
    ) -> PlaybackResult<bool> {
        self.request(|reply| RuntimeCommand::CompleteAutoDj {
            source_id,
            seed_occurrence,
            candidates,
            requested_count,
            shuffle_seed,
            reply,
        })
    }

    pub fn auto_dj_unavailable(
        &self,
        source_id: SourceId,
        seed_occurrence: crate::OccurrenceId,
        error: Option<String>,
    ) -> PlaybackResult<bool> {
        self.request(|reply| RuntimeCommand::AutoDjUnavailable {
            source_id,
            seed_occurrence,
            error,
            reply,
        })
    }

    pub fn queue_page(&self, query: QueuePageQuery) -> PlaybackResult<QueuePage> {
        self.request(|reply| RuntimeCommand::QueuePage { query, reply })
    }

    pub fn queued_track_ids(
        &self,
    ) -> PlaybackResult<(SourceId, SourceSessionEpoch, Vec<library::TrackId>)> {
        self.request(|reply| RuntimeCommand::QueuedTrackIds { reply })
    }

    pub fn current_media(&self) -> PlaybackResult<Option<Arc<crate::CurrentMedia>>> {
        self.request(|reply| RuntimeCommand::CurrentMedia { reply })
    }

    pub fn projection(&self) -> PlaybackResult<PlaybackProjection> {
        self.request(|reply| RuntimeCommand::Projection { reply })
    }

    pub fn shutdown(&self) -> PlaybackResult<()> {
        let result = self.request(|reply| RuntimeCommand::Shutdown { reply });
        let joined = self.inner.join_threads();
        result.and(joined)
    }

    fn request<T>(&self, command: impl FnOnce(Reply<T>) -> RuntimeCommand) -> PlaybackResult<T> {
        let (reply, response) = sync_channel(0);
        self.inner
            .commands
            .send(command(reply))
            .map_err(|_| PlaybackError::Unavailable)?;
        response.recv().map_err(|_| PlaybackError::WorkerStopped)?
    }
}

impl PlaybackInner {
    fn join_threads(&self) -> PlaybackResult<()> {
        let Some((actor, output)) = self
            .threads
            .lock()
            .map_err(|_| PlaybackError::Unavailable)?
            .take()
        else {
            return Ok(());
        };
        actor.join().map_err(|_| PlaybackError::WorkerStopped)?;
        output.join().map_err(|_| PlaybackError::WorkerStopped)?;
        Ok(())
    }
}

fn run_playback(
    mut runtime: PlaybackRuntime,
    commands: Receiver<RuntimeCommand>,
    outputs: SyncSender<PlaybackOutput>,
    clock: Clock,
) {
    loop {
        match commands.recv_timeout(BACKEND_POLL_INTERVAL) {
            Ok(command) => {
                if !apply_runtime_command(&mut runtime, command, &outputs, &clock) {
                    break;
                }
                let sample = clock();
                if runtime
                    .poll(&sample)
                    .and_then(|update| publish_update(&outputs, update))
                    .is_err()
                {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                let sample = clock();
                if runtime
                    .poll(&sample)
                    .and_then(|update| publish_update(&outputs, update))
                    .is_err()
                {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                let sample = clock();
                if let Ok(update) = runtime.shutdown(&sample) {
                    let _ = publish_update(&outputs, update);
                }
                let _ = outputs.send(PlaybackOutput::Shutdown);
                break;
            }
        }
    }
}

fn apply_runtime_command(
    runtime: &mut PlaybackRuntime,
    command: RuntimeCommand,
    outputs: &SyncSender<PlaybackOutput>,
    clock: &Clock,
) -> bool {
    let sample = clock();
    match command {
        RuntimeCommand::Session { command, reply } => {
            reply_update(runtime.command(command, &sample), outputs, reply);
        }
        RuntimeCommand::RefreshTracks {
            source_session_epoch,
            tracks,
            reply,
        } => {
            let value = runtime
                .command(
                    SessionCommand::RefreshTracks {
                        source_session_epoch,
                        tracks,
                    },
                    &sample,
                )
                .and_then(|mut update| {
                    update.projection.take();
                    update.current_media_changed = false;
                    publish_update(outputs, update)?;
                    Ok(PlaybackProjection {
                        view: runtime.session.view(),
                        queue_page: None,
                        notices: Vec::new(),
                    })
                });
            let _ = reply.send(value);
        }
        RuntimeCommand::AdmitLoaded {
            source_id,
            source_session_epoch,
            activation,
            placement,
            reply,
        } => {
            let value = runtime
                .admit_loaded(
                    &source_id,
                    source_session_epoch,
                    activation,
                    placement,
                    &sample,
                )
                .and_then(|(reservation, update)| {
                    publish_optional_update(outputs, update)?;
                    Ok(reservation)
                });
            let _ = reply.send(value);
        }
        RuntimeCommand::ReserveMaterialization { placement, reply } => {
            let _ = reply.send(runtime.reserve_materialization(placement));
        }
        RuntimeCommand::CompleteMaterialization {
            id,
            source_id,
            batch,
            placement,
            reply,
        } => {
            let value = runtime
                .complete_materialization(id, &source_id, batch, placement, &sample)
                .and_then(|update| publish_optional_update(outputs, update));
            let _ = reply.send(value);
        }
        RuntimeCommand::FailMaterialization {
            id,
            source_id,
            placement,
            message,
            reply,
        } => {
            let value = runtime
                .fail_materialization(id, &source_id, placement, message, &sample)
                .and_then(|update| publish_optional_update(outputs, update));
            let _ = reply.send(value);
        }
        RuntimeCommand::CancelMaterialization {
            id,
            source_id,
            placement,
            reply,
        } => {
            let _ = reply.send(runtime.cancel_materialization(id, &source_id, placement));
        }
        RuntimeCommand::ResolveStream { run, stream, reply } => {
            reply_update(runtime.resolve_stream(run, stream, &sample), outputs, reply);
        }
        RuntimeCommand::CompleteAutoDj {
            source_id,
            seed_occurrence,
            candidates,
            requested_count,
            shuffle_seed,
            reply,
        } => {
            let value = runtime
                .complete_auto_dj_candidates(
                    &source_id,
                    &seed_occurrence,
                    candidates,
                    requested_count,
                    shuffle_seed,
                    &sample,
                )
                .and_then(|update| publish_optional_update(outputs, update));
            let _ = reply.send(value);
        }
        RuntimeCommand::AutoDjUnavailable {
            source_id,
            seed_occurrence,
            error,
            reply,
        } => {
            let value = runtime
                .auto_dj_unavailable(&source_id, &seed_occurrence, error, &sample)
                .and_then(|update| publish_optional_update(outputs, update));
            let _ = reply.send(value);
        }
        RuntimeCommand::QueuePage { query, reply } => {
            let _ = reply.send(runtime.queue_page(query));
        }
        RuntimeCommand::QueuedTrackIds { reply } => {
            let _ = reply.send(runtime.queued_track_ids());
        }
        RuntimeCommand::CurrentMedia { reply } => {
            let _ = reply.send(runtime.current_media());
        }
        RuntimeCommand::Projection { reply } => {
            let _ = reply.send(Ok(runtime.initial_projection()));
        }
        RuntimeCommand::Shutdown { reply } => {
            let mut value = runtime
                .shutdown(&sample)
                .and_then(|update| publish_update(outputs, update));
            if outputs.send(PlaybackOutput::Shutdown).is_err() && value.is_ok() {
                value = Err(PlaybackError::Unavailable);
            }
            let _ = reply.send(value);
            return false;
        }
    }
    true
}

fn reply_update(
    value: PlaybackResult<PlaybackUpdate>,
    outputs: &SyncSender<PlaybackOutput>,
    reply: Reply<()>,
) {
    let _ = reply.send(value.and_then(|update| publish_update(outputs, update)));
}

fn publish_optional_update(
    outputs: &SyncSender<PlaybackOutput>,
    update: Option<PlaybackUpdate>,
) -> PlaybackResult<bool> {
    let Some(update) = update else {
        return Ok(false);
    };
    publish_update(outputs, update)?;
    Ok(true)
}

fn publish_update(
    outputs: &SyncSender<PlaybackOutput>,
    update: PlaybackUpdate,
) -> PlaybackResult<()> {
    if update.is_empty() {
        return Ok(());
    }
    let flushes_persistence = update
        .effects
        .iter()
        .any(|effect| matches!(effect, SessionEffect::FlushPersistence { .. }));
    outputs
        .send(PlaybackOutput::Update(update))
        .map_err(|_| PlaybackError::Unavailable)?;
    if flushes_persistence {
        fence_outputs(outputs)?;
    }
    Ok(())
}

fn fence_outputs(outputs: &SyncSender<PlaybackOutput>) -> PlaybackResult<()> {
    let (fence, crossed) = sync_channel(0);
    outputs
        .send(PlaybackOutput::Fence(fence))
        .map_err(|_| PlaybackError::Unavailable)?;
    crossed.recv().map_err(|_| PlaybackError::WorkerStopped)
}

fn run_playback_outputs(
    outputs: Receiver<PlaybackOutput>,
    mut consume: impl FnMut(PlaybackUpdate),
) {
    while let Ok(output) = outputs.recv() {
        match output {
            PlaybackOutput::Update(update) => consume(update),
            PlaybackOutput::Fence(crossed) => {
                let _ = crossed.send(());
            }
            PlaybackOutput::Shutdown => break,
        }
    }
}

struct PlaybackRuntime {
    session: PlaybackSession,
    backend: Box<dyn PlaybackBackend>,
}

impl PlaybackRuntime {
    fn new(
        sequence: Sequence,
        source_session_epoch: SourceSessionEpoch,
        play_id_prefix: impl Into<Arc<str>>,
        settings: PlaybackSettings,
        auto_dj_enabled: bool,
        auto_dj_refill_threshold: usize,
        backend: Box<dyn PlaybackBackend>,
    ) -> Self {
        Self {
            session: PlaybackSession::new(
                sequence,
                source_session_epoch,
                play_id_prefix,
                settings,
                auto_dj_enabled,
                auto_dj_refill_threshold,
            ),
            backend,
        }
    }

    fn initial_projection(&self) -> PlaybackProjection {
        PlaybackProjection {
            view: self.session.view(),
            queue_page: Some(self.session.sequence().current_page()),
            notices: Vec::new(),
        }
    }

    fn command(
        &mut self,
        command: SessionCommand,
        sample: &ClockSample,
    ) -> PlaybackResult<PlaybackUpdate> {
        let update = self.session.handle_command(command, sample)?;
        self.finish(update, sample)
    }

    fn admit_loaded(
        &mut self,
        source_id: &SourceId,
        source_session_epoch: SourceSessionEpoch,
        activation: Option<(String, library::TrackId, usize)>,
        placement: Placement,
        sample: &ClockSample,
    ) -> PlaybackResult<(Option<MaterializationReservation>, Option<PlaybackUpdate>)> {
        if self.session.sequence().source_id() != source_id
            || self.session.source_session_epoch() != source_session_epoch
        {
            return Err(PlaybackError::InactiveSourceSession);
        }
        if let Some((context_id, track_id, source_rank)) = activation
            && let Some(update) =
                self.session
                    .activate_context(&context_id, &track_id, source_rank, sample)
        {
            return Ok((None, Some(self.finish(update, sample)?)));
        }
        Ok((Some(self.session.reserve_materialization(placement)), None))
    }

    fn reserve_materialization(
        &mut self,
        placement: Placement,
    ) -> PlaybackResult<MaterializationReservation> {
        Ok(self.session.reserve_materialization(placement))
    }

    fn complete_materialization(
        &mut self,
        id: MaterializationId,
        source_id: &SourceId,
        batch: Batch,
        placement: Placement,
        sample: &ClockSample,
    ) -> PlaybackResult<Option<PlaybackUpdate>> {
        let update = self
            .session
            .apply_materialization(id, source_id, batch, placement, sample)?
            .map(|update| self.finish(update, sample))
            .transpose()?;
        Ok(update)
    }

    fn fail_materialization(
        &mut self,
        id: MaterializationId,
        source_id: &SourceId,
        placement: Placement,
        message: String,
        sample: &ClockSample,
    ) -> PlaybackResult<Option<PlaybackUpdate>> {
        self.session
            .fail_materialization(id, source_id, placement, message)
            .map(|update| self.finish(update, sample))
            .transpose()
    }

    fn cancel_materialization(
        &mut self,
        id: MaterializationId,
        source_id: &SourceId,
        placement: Placement,
    ) -> PlaybackResult<bool> {
        Ok(self
            .session
            .cancel_materialization(id, source_id, placement))
    }

    fn resolve_stream(
        &mut self,
        run: RunId,
        result: Result<ResolvedStream, String>,
        sample: &ClockSample,
    ) -> PlaybackResult<PlaybackUpdate> {
        let update = match result {
            Ok(stream) => self.session.stream_resolved(run, stream),
            Err(error) => self.session.stream_failed(run, error, sample),
        };
        self.finish(update, sample)
    }

    fn complete_auto_dj_candidates(
        &mut self,
        source_id: &SourceId,
        seed_occurrence: &crate::OccurrenceId,
        candidates: Vec<Track>,
        requested_count: usize,
        shuffle_seed: u64,
        sample: &ClockSample,
    ) -> PlaybackResult<Option<PlaybackUpdate>> {
        let update = self
            .session
            .complete_auto_dj_candidates(
                source_id,
                seed_occurrence,
                candidates,
                requested_count,
                shuffle_seed,
                sample,
            )?
            .map(|update| self.finish(update, sample))
            .transpose()?;
        Ok(update)
    }

    fn auto_dj_unavailable(
        &mut self,
        source_id: &SourceId,
        seed_occurrence: &crate::OccurrenceId,
        error: Option<String>,
        sample: &ClockSample,
    ) -> PlaybackResult<Option<PlaybackUpdate>> {
        let update = self
            .session
            .auto_dj_unavailable(source_id, seed_occurrence, error)
            .map(|update| self.finish(update, sample))
            .transpose()?;
        Ok(update)
    }

    fn poll(&mut self, sample: &ClockSample) -> PlaybackResult<PlaybackUpdate> {
        let events = self.backend.drain_events();
        let mut output = PlaybackUpdate::default();
        for event in events {
            let update = self.session.handle_backend(event, sample);
            output.merge(self.finish(update, sample)?);
        }
        Ok(output)
    }

    fn queue_page(&self, query: QueuePageQuery) -> PlaybackResult<QueuePage> {
        Ok(self.session.sequence().page(query))
    }

    fn queued_track_ids(
        &self,
    ) -> PlaybackResult<(SourceId, SourceSessionEpoch, Vec<library::TrackId>)> {
        Ok((
            self.session.sequence().source_id().clone(),
            self.session.source_session_epoch(),
            self.session.sequence().unique_track_ids(),
        ))
    }

    fn current_media(&self) -> PlaybackResult<Option<std::sync::Arc<crate::CurrentMedia>>> {
        Ok(self.session.view().transport.current)
    }

    fn shutdown(&mut self, sample: &ClockSample) -> PlaybackResult<PlaybackUpdate> {
        let session_update = self.session.shutdown(sample);
        let update = self.finish(session_update, sample)?;
        self.backend
            .shutdown()
            .map_err(|error| PlaybackError::BackendShutdown(error.to_string()))?;
        Ok(update)
    }

    fn finish(
        &mut self,
        update: SessionUpdate,
        sample: &ClockSample,
    ) -> PlaybackResult<PlaybackUpdate> {
        let mut output = self.commit(update);
        let mut backend_failures = Vec::new();
        for effect in std::mem::take(&mut output.effects) {
            match effect {
                SessionEffect::Backend(command) => {
                    let run = command.run();
                    if let Err(error) = self.backend.send(command) {
                        backend_failures.push((run, error.to_string()));
                    }
                }
                effect => output.effects.push(effect),
            }
        }
        for (run, error) in backend_failures {
            if let Some(run) = run {
                let failed = self.session.handle_backend(
                    BackendEvent::Error {
                        run,
                        error: BackendFailure::new(error),
                    },
                    sample,
                );
                output.merge(self.commit(failed));
            } else {
                output.effects.push(SessionEffect::NonfatalError(error));
            }
        }
        Ok(output)
    }

    fn commit(&self, update: SessionUpdate) -> PlaybackUpdate {
        let checkpoint = update
            .structure_changed
            .then(|| build_checkpoint(self.session.sequence()));
        let mut notices = Vec::new();
        let mut effects = Vec::new();
        let mut current_media_changed = false;
        for effect in update.effects {
            match &effect {
                SessionEffect::Listening(crate::ListeningFact::Started { run, .. }) => {
                    notices.push(PlaybackNotice::RunStarted(*run));
                    effects.push(effect);
                }
                SessionEffect::PositionDiscontinuity(discontinuity) => {
                    notices.push(PlaybackNotice::PositionDiscontinuity(*discontinuity));
                }
                SessionEffect::Visualizer { run, levels } => {
                    notices.push(PlaybackNotice::Visualizer {
                        run: *run,
                        levels: levels.clone(),
                    });
                }
                SessionEffect::CurrentMediaChanged => {
                    current_media_changed = true;
                }
                _ => effects.push(effect),
            }
        }
        let projection = (update.view_changed || !notices.is_empty()).then(|| PlaybackProjection {
            view: self.session.view(),
            queue_page: update
                .structure_changed
                .then(|| self.session.sequence().current_page()),
            notices,
        });
        PlaybackUpdate {
            checkpoint,
            projection,
            effects,
            current_media_changed,
        }
    }
}

#[cfg(test)]
mod tests {
    use library::{AlbumId, TrackId};

    use super::*;
    use crate::{BackendCommand, BackendError, QueuePlacement};

    #[derive(Default)]
    struct AcceptingBackend;

    impl PlaybackBackend for AcceptingBackend {
        fn send(&mut self, _command: BackendCommand) -> Result<(), BackendError> {
            Ok(())
        }

        fn drain_events(&mut self) -> Vec<BackendEvent> {
            Vec::new()
        }
    }

    #[test]
    fn runtime_collapses_current_media_changes_without_marking_position_ticks() {
        let source_id = SourceId::fake(1);
        let mut sequence = Sequence::new(source_id);
        sequence
            .apply_batch(
                crate::Batch::new(vec![
                    crate::BatchItem::new(track(1), crate::Provenance::Manual),
                    crate::BatchItem::new(track(2), crate::Provenance::Manual),
                ]),
                Placement::Replace { anchor_index: 0 },
            )
            .expect("seed queue");
        let mut runtime = PlaybackRuntime::new(
            sequence,
            SourceSessionEpoch::new(1),
            "test",
            PlaybackSettings::default(),
            false,
            2,
            Box::<AcceptingBackend>::default(),
        );

        let started = runtime
            .command(SessionCommand::Play, &sample(0))
            .expect("start");
        assert!(started.current_media_changed);
        let run = runtime.session.current_run().expect("current run");

        let position = runtime
            .session
            .handle_backend(BackendEvent::Position { run, millis: 500 }, &sample(1));
        let position = runtime.finish(position, &sample(1)).expect("position");
        assert!(!position.current_media_changed);

        let next = runtime
            .command(SessionCommand::Next, &sample(2))
            .expect("next");
        assert!(next.current_media_changed);
    }

    #[test]
    fn loaded_selection_cannot_cross_a_source_session() {
        let source_id = SourceId::fake(1);
        let mut runtime = runtime(source_id.clone());
        let request = LoadedPlayRequest::now(
            source_id.clone(),
            SourceSessionEpoch::new(2),
            vec![track(1)].into(),
            0,
        );
        let error = runtime
            .admit_loaded(
                &source_id,
                request.source_session_epoch,
                request.activation_context(),
                request.placement(),
                &sample(1),
            )
            .expect_err("stale source session");

        assert!(matches!(error, PlaybackError::InactiveSourceSession));
        assert_eq!(
            runtime
                .queue_page(QueuePageQuery::current())
                .expect("queue")
                .total,
            0
        );
    }

    #[test]
    fn exact_loaded_context_activation_bypasses_materialization() {
        let source_id = SourceId::fake(1);
        let mut runtime = runtime(source_id.clone());
        let tracks: Arc<[Track]> = vec![track(1), track(1)].into();
        let initial = LoadedPlayRequest::context(
            source_id.clone(),
            SourceSessionEpoch::new(1),
            tracks.clone(),
            0,
            QueuePlacement::Now,
            "tracks",
            false,
        )
        .expect("initial context request");
        let (reservation, update) = runtime
            .admit_loaded(
                &source_id,
                initial.source_session_epoch,
                initial.activation_context(),
                initial.placement(),
                &sample(1),
            )
            .expect("admit initial context");
        assert!(update.is_none());
        let reservation = reservation.expect("initial context must materialize");
        let reservation_source = reservation.source_id.clone();
        let (batch, placement) = initial.materialize_batch(7).expect("initial batch");
        runtime
            .complete_materialization(
                reservation.id,
                &reservation_source,
                batch,
                placement,
                &sample(1),
            )
            .expect("complete initial context")
            .expect("initial context update");
        let before = runtime
            .queue_page(QueuePageQuery::current())
            .expect("initial queue");
        let expected = before.rows[1].entry.occurrence.clone();

        let activate = LoadedPlayRequest::context(
            source_id.clone(),
            SourceSessionEpoch::new(1),
            tracks,
            1,
            QueuePlacement::Now,
            "tracks",
            false,
        )
        .expect("activation request");
        let (reservation, update) = runtime
            .admit_loaded(
                &source_id,
                activate.source_session_epoch,
                activate.activation_context(),
                activate.placement(),
                &sample(2),
            )
            .expect("activate context occurrence");
        let update = update.expect("exact activation update");
        let after = runtime
            .queue_page(QueuePageQuery::current())
            .expect("updated queue");

        assert!(reservation.is_none());
        assert!(update.checkpoint.is_none());
        assert_eq!(after.rows.len(), before.rows.len());
        assert_eq!(after.current_absolute_index, Some(1));
        assert_eq!(
            after.rows.get(1).map(|row| &row.entry.occurrence),
            Some(&expected)
        );
    }

    #[test]
    fn shuffled_context_starts_a_new_queue() {
        let source_id = SourceId::fake(1);
        let tracks: Arc<[Track]> = vec![track(1), track(2)].into();
        let request = LoadedPlayRequest::context(
            source_id,
            SourceSessionEpoch::new(1),
            tracks,
            0,
            QueuePlacement::Now,
            "album:1",
            true,
        )
        .expect("shuffled context request");

        assert!(request.activation_context().is_none());
    }

    fn runtime(source_id: SourceId) -> PlaybackRuntime {
        PlaybackRuntime::new(
            Sequence::new(source_id),
            SourceSessionEpoch::new(1),
            "test",
            PlaybackSettings::default(),
            false,
            2,
            Box::<AcceptingBackend>::default(),
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
}
