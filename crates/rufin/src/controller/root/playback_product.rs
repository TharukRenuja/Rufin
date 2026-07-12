use super::*;

use std::collections::{HashMap, HashSet};
use std::sync::Condvar;
use std::time::Instant;

use library::{ActivityOutcome, PlaybackCheckpointRecord};
#[cfg(test)]
use playback::BackendCommand;
use playback::{
    BackendEvent, Batch, CheckpointError, CheckpointHeader, ClockSample, ListeningFact,
    MaterializationId, MaterializationReservation, OccurrenceId, Placement, PlaybackBackend,
    PlaybackSession, PlaybackView, QueuePage, QueuePageQuery, RepeatMode, SessionCommand,
    SessionEffect, SessionUpdate, SourceReportFact, SourceReportPhase, decode_checkpoint,
    decode_legacy_queue_snapshot_with_tracks, encode_checkpoint,
};
#[cfg(not(test))]
use playback_gstreamer::GStreamerPlaybackBackend;
use scrobbling::Scrobbler;

const STORE_ACTIVITY_CAPACITY: usize = 64;
#[cfg(not(test))]
const SLOW_RICH_PRESENCE_ARTWORK_MS: u128 = 5_000;

#[derive(Clone, Debug, PartialEq)]
pub enum PlaybackNotice {
    RunStarted(playback::RunId),
    MediaChanged(playback::MediaChanged),
    PositionDiscontinuity(playback::PositionDiscontinuity),
    Visualizer {
        run: playback::RunId,
        levels: Vec<f64>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlaybackProjection {
    pub view: PlaybackView,
    pub queue_page: Option<QueuePage>,
    pub notices: Vec<PlaybackNotice>,
}

enum PlaybackWrite {
    Checkpoint(PlaybackCheckpointRecord),
    State {
        source_id: SourceId,
        revision: u64,
        occurrence: Option<String>,
        progress_millis: u64,
        repeat_mode: String,
        shuffle_enabled: bool,
    },
    Progress {
        source_id: SourceId,
        revision: u64,
        occurrence: String,
        progress_millis: u64,
    },
    OutputState {
        volume: f64,
        muted: bool,
    },
    Activity(ActivityOutcome),
}

enum PlaybackWriterStore {
    Disk(Store),
    #[cfg(test)]
    Memory(Arc<Mutex<Store>>),
}

struct PlaybackStoreFailure {
    message: String,
    contention: bool,
}

impl PlaybackStoreFailure {
    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            contention: false,
        }
    }
}

impl From<StoreError> for PlaybackStoreFailure {
    fn from(error: StoreError) -> Self {
        Self {
            contention: error.is_contention(),
            message: error.to_string(),
        }
    }
}

impl PlaybackWriterStore {
    fn apply(&self, write: &PlaybackWrite) -> Result<(), PlaybackStoreFailure> {
        match self {
            Self::Disk(store) => apply_store_write(store, write),
            #[cfg(test)]
            Self::Memory(store) => store
                .lock()
                .map_err(|_| PlaybackStoreFailure::unavailable("playback Store lock was poisoned"))
                .and_then(|store| apply_store_write(&store, write)),
        }
    }
}

struct PlaybackStoreWriter {
    mailbox: Arc<PlaybackStoreMailbox>,
}

struct PlaybackStoreMailbox {
    state: Mutex<PendingPlaybackWrites>,
    ready: Condvar,
    drained: Condvar,
}

#[derive(Default)]
struct PendingPlaybackWrites {
    durable: HashMap<SourceId, PlaybackWrite>,
    output_state: Option<PlaybackWrite>,
    activities: Vec<ActivityOutcome>,
    retry_requested: bool,
    generation: u64,
    completed_generation: u64,
    draining: bool,
    closing: bool,
}

struct PlaybackWriteBatch {
    durable: HashMap<SourceId, PlaybackWrite>,
    output_state: Option<PlaybackWrite>,
    activities: Vec<ActivityOutcome>,
    retry_requested: bool,
    generation: u64,
}

impl PlaybackStoreMailbox {
    fn new() -> Self {
        Self {
            state: Mutex::new(PendingPlaybackWrites::default()),
            ready: Condvar::new(),
            drained: Condvar::new(),
        }
    }

    fn enqueue(&self, write: PlaybackWrite) {
        let Ok(mut pending) = self.state.lock() else {
            warn!("playback Store mailbox is unavailable");
            return;
        };
        match write {
            PlaybackWrite::Activity(outcome) => {
                if !enqueue_activity(&mut pending.activities, outcome) {
                    warn!("dropping playback activity because the Store mailbox is full");
                    return;
                }
            }
            write @ PlaybackWrite::OutputState { .. } => {
                pending.output_state = Some(write);
            }
            write => {
                let Some(source_id) = durable_write_source(&write).cloned() else {
                    return;
                };
                let merged = match pending.durable.remove(&source_id) {
                    Some(older) => merge_durable_write(older, write),
                    None => write,
                };
                pending.durable.insert(source_id, merged);
            }
        }
        pending.generation += 1;
        self.ready.notify_one();
    }

    fn fence(&self) {
        let Ok(mut pending) = self.state.lock() else {
            warn!("playback Store mailbox is unavailable");
            return;
        };
        pending.retry_requested = true;
        pending.generation += 1;
        self.ready.notify_one();
    }

    fn drain(&self) {
        let Ok(mut pending) = self.state.lock() else {
            warn!("playback Store mailbox is unavailable");
            return;
        };
        pending.draining = true;
        pending.retry_requested = true;
        pending.generation += 1;
        let target = pending.generation;
        self.ready.notify_one();
        while pending.completed_generation < target || pending.draining {
            let Ok(next) = self.drained.wait(pending) else {
                warn!("playback Store mailbox is unavailable");
                return;
            };
            pending = next;
        }
    }

    fn close(&self) {
        let Ok(mut pending) = self.state.lock() else {
            return;
        };
        pending.closing = true;
        pending.retry_requested = true;
        pending.generation += 1;
        self.ready.notify_one();
    }

    fn next_batch(&self) -> Option<PlaybackWriteBatch> {
        let mut pending = self.state.lock().ok()?;
        while pending.completed_generation == pending.generation && !pending.closing {
            pending = self.ready.wait(pending).ok()?;
        }
        if pending.closing && pending.completed_generation == pending.generation {
            return None;
        }
        let batch = PlaybackWriteBatch {
            durable: std::mem::take(&mut pending.durable),
            output_state: pending.output_state.take(),
            activities: std::mem::take(&mut pending.activities),
            retry_requested: std::mem::take(&mut pending.retry_requested),
            generation: pending.generation,
        };
        Some(batch)
    }

    fn complete(&self, generation: u64, contention_pending: bool) {
        let Ok(mut pending) = self.state.lock() else {
            return;
        };
        pending.completed_generation = generation;
        if pending.draining && contention_pending {
            pending.retry_requested = true;
            pending.generation += 1;
            self.ready.notify_one();
        } else {
            pending.draining = false;
        }
        self.drained.notify_all();
    }
}

impl PlaybackStoreWriter {
    fn new(store: &StoreHandle, events: Sender<ControllerEvent>) -> Result<Self, String> {
        let target = match store {
            StoreHandle::Path {
                cache_database_path,
                write_gate,
                ..
            } => PlaybackWriterStore::Disk(
                Store::open_with_write_gate(cache_database_path, write_gate.clone())
                    .map_err(|error| error.to_string())?,
            ),
            #[cfg(test)]
            StoreHandle::Memory { store, .. } => PlaybackWriterStore::Memory(Arc::clone(store)),
        };
        let mailbox = Arc::new(PlaybackStoreMailbox::new());
        let worker_mailbox = Arc::clone(&mailbox);
        thread::Builder::new()
            .name("rufin-playback-store".to_string())
            .spawn({
                let settings_store = store.clone();
                move || run_playback_store_writer(target, settings_store, worker_mailbox, events)
            })
            .map_err(|error| error.to_string())?;
        Ok(Self { mailbox })
    }

    fn enqueue(&self, write: PlaybackWrite) {
        self.mailbox.enqueue(write);
    }

    fn fence(&self) {
        self.mailbox.fence();
    }

    fn drain(&self) {
        self.mailbox.drain();
    }
}

impl Drop for PlaybackStoreWriter {
    fn drop(&mut self) {
        self.mailbox.close();
    }
}

fn run_playback_store_writer(
    target: PlaybackWriterStore,
    settings_store: StoreHandle,
    mailbox: Arc<PlaybackStoreMailbox>,
    events: Sender<ControllerEvent>,
) {
    let mut failed = HashMap::new();
    let mut deferred_activities = Vec::new();
    let mut failed_output = None;
    let mut error_reported = false;
    while let Some(batch) = mailbox.next_batch() {
        let mut store_busy = false;
        if batch.retry_requested || !batch.durable.is_empty() {
            store_busy = apply_durable_store_writes(
                &target,
                batch.durable,
                &mut failed,
                &mut error_reported,
            );
        }
        if batch.retry_requested
            && let Some(write) = failed_output.take()
            && let Err(error) = apply_output_state(&settings_store, &write)
        {
            failed_output = Some(write);
            report_store_error(error, false, &mut error_reported);
        }
        if let Some(write) = batch.output_state {
            match apply_output_state(&settings_store, &write) {
                Ok(()) => failed_output = None,
                Err(error) => {
                    failed_output = Some(write);
                    report_store_error(error, false, &mut error_reported);
                }
            }
        }
        store_busy = apply_activity_store_writes(
            &target,
            batch.activities,
            &mut deferred_activities,
            &events,
            store_busy,
        );
        mailbox.complete(batch.generation, store_busy);
    }
}

fn enqueue_activity(pending: &mut Vec<ActivityOutcome>, outcome: ActivityOutcome) -> bool {
    if let Some(existing) = pending.iter_mut().find(|existing| {
        existing.source_id == outcome.source_id
            && existing.period == outcome.period
            && existing.track_id == outcome.track_id
    }) {
        existing.qualified_plays += outcome.qualified_plays;
        existing.skips += outcome.skips;
        existing.last_played_at = existing.last_played_at.max(outcome.last_played_at);
        return true;
    }
    if pending.len() == STORE_ACTIVITY_CAPACITY {
        return false;
    }
    pending.push(outcome);
    true
}

fn apply_durable_store_writes(
    target: &PlaybackWriterStore,
    writes: HashMap<SourceId, PlaybackWrite>,
    failed: &mut HashMap<SourceId, PlaybackWrite>,
    error_reported: &mut bool,
) -> bool {
    let mut pending = std::mem::take(failed);
    for (source_id, write) in writes {
        let merged = match pending.remove(&source_id) {
            Some(previous) => merge_durable_write(previous, write),
            None => write,
        };
        pending.insert(source_id, merged);
    }

    let mut store_busy = false;
    for (source_id, write) in pending {
        if store_busy {
            failed.insert(source_id, write);
            continue;
        }
        match target.apply(&write) {
            Ok(()) => {}
            Err(error) => {
                store_busy = error.contention;
                failed.insert(source_id, write);
                report_store_error(error.message, error.contention, error_reported);
            }
        }
    }
    if failed.is_empty() {
        *error_reported = false;
    }
    store_busy
}

fn apply_activity_store_writes(
    target: &PlaybackWriterStore,
    activities: Vec<ActivityOutcome>,
    deferred: &mut Vec<ActivityOutcome>,
    events: &Sender<ControllerEvent>,
    store_busy: bool,
) -> bool {
    for outcome in activities {
        if !enqueue_activity(deferred, outcome) {
            warn!("dropping playback activity because the Store mailbox is full");
        }
    }
    if store_busy {
        return true;
    }

    let pending = std::mem::take(deferred);
    let mut store_busy = false;
    for outcome in pending {
        if store_busy {
            let _retained = enqueue_activity(deferred, outcome);
            continue;
        }
        match target.apply(&PlaybackWrite::Activity(outcome.clone())) {
            Ok(()) => emit_activity_delta(events, &outcome),
            Err(error) if error.contention => {
                store_busy = true;
                let _retained = enqueue_activity(deferred, outcome);
                debug!(
                    error = %error.message,
                    "deferred playback activity while Store is busy"
                );
            }
            Err(error) => warn!(error = %error.message, "failed to record playback activity"),
        }
    }
    store_busy
}

fn durable_write_source(write: &PlaybackWrite) -> Option<&SourceId> {
    match write {
        PlaybackWrite::Checkpoint(record) => Some(&record.source_id),
        PlaybackWrite::State { source_id, .. } | PlaybackWrite::Progress { source_id, .. } => {
            Some(source_id)
        }
        PlaybackWrite::OutputState { .. } | PlaybackWrite::Activity(_) => None,
    }
}

fn durable_write_revision(write: &PlaybackWrite) -> Option<u64> {
    match write {
        PlaybackWrite::Checkpoint(record) => Some(record.revision),
        PlaybackWrite::State { revision, .. } | PlaybackWrite::Progress { revision, .. } => {
            Some(*revision)
        }
        PlaybackWrite::OutputState { .. } | PlaybackWrite::Activity(_) => None,
    }
}

fn merge_durable_write(older: PlaybackWrite, newer: PlaybackWrite) -> PlaybackWrite {
    if durable_write_revision(&newer) < durable_write_revision(&older) {
        return older;
    }
    match (older, newer) {
        (
            PlaybackWrite::Checkpoint(mut record),
            PlaybackWrite::State {
                revision,
                occurrence,
                progress_millis,
                repeat_mode,
                shuffle_enabled,
                ..
            },
        ) if record.revision == revision => {
            record.selected_occurrence_id = occurrence;
            record.progress_millis = progress_millis;
            record.repeat_mode = repeat_mode;
            record.shuffle_enabled = shuffle_enabled;
            PlaybackWrite::Checkpoint(record)
        }
        (
            PlaybackWrite::Checkpoint(mut record),
            PlaybackWrite::Progress {
                revision,
                occurrence,
                progress_millis,
                ..
            },
        ) if record.revision == revision
            && record.selected_occurrence_id.as_deref() == Some(occurrence.as_str()) =>
        {
            record.progress_millis = progress_millis;
            PlaybackWrite::Checkpoint(record)
        }
        (
            PlaybackWrite::State {
                source_id,
                revision,
                occurrence,
                progress_millis: _,
                repeat_mode,
                shuffle_enabled,
            },
            PlaybackWrite::Progress {
                revision: progress_revision,
                occurrence: progress_occurrence,
                progress_millis,
                ..
            },
        ) if revision == progress_revision
            && occurrence.as_deref() == Some(progress_occurrence.as_str()) =>
        {
            PlaybackWrite::State {
                source_id,
                revision,
                occurrence,
                progress_millis,
                repeat_mode,
                shuffle_enabled,
            }
        }
        (_, newer) => newer,
    }
}

fn report_store_error(error: String, contention: bool, error_reported: &mut bool) {
    if contention {
        debug!(%error, "deferred playback persistence while Store is busy");
    } else if !*error_reported {
        *error_reported = true;
        warn!(%error, "failed to persist playback state; latest state retained for retry");
    }
}

fn emit_activity_delta(events: &Sender<ControllerEvent>, outcome: &ActivityOutcome) {
    let mut delta = LibraryDelta::default();
    if outcome.qualified_plays != 0 {
        delta.tracks.stats.push(outcome.track_id.clone());
    }
    if outcome.skips != 0 {
        delta.tracks.skip_stats.push(outcome.track_id.clone());
    }
    if !delta.is_empty() {
        let _ = events.send(ControllerEvent::LibraryDelta(Box::new(delta)));
    }
}

fn apply_output_state(store: &StoreHandle, write: &PlaybackWrite) -> Result<(), String> {
    let PlaybackWrite::OutputState { volume, muted } = write else {
        return Ok(());
    };
    let volume = *volume;
    let muted = *muted;
    store.update_settings(move |settings| {
        settings.playback.volume = volume;
        settings.playback.muted = muted;
        Ok(())
    })
}

fn apply_store_write(store: &Store, write: &PlaybackWrite) -> Result<(), PlaybackStoreFailure> {
    match write {
        PlaybackWrite::Checkpoint(record) => store
            .save_playback_checkpoint(record)
            .map_err(PlaybackStoreFailure::from),
        PlaybackWrite::State {
            source_id,
            revision,
            occurrence,
            progress_millis,
            repeat_mode,
            shuffle_enabled,
        } => store
            .save_playback_state(
                source_id,
                *revision,
                occurrence.as_deref(),
                *progress_millis,
                repeat_mode,
                *shuffle_enabled,
            )
            .map(|_| ())
            .map_err(PlaybackStoreFailure::from),
        PlaybackWrite::Progress {
            source_id,
            revision,
            occurrence,
            progress_millis,
        } => store
            .save_playback_progress(source_id, *revision, occurrence, *progress_millis)
            .map(|_| ())
            .map_err(PlaybackStoreFailure::from),
        PlaybackWrite::Activity(outcome) => store
            .record_activity_outcome(outcome)
            .map_err(PlaybackStoreFailure::from),
        PlaybackWrite::OutputState { .. } => Err(PlaybackStoreFailure::unavailable(
            "playback output settings require the settings store",
        )),
    }
}

#[cfg(not(test))]
fn composed_rich_presence(artwork: ::artwork::Artwork) -> rich_presence::Presence {
    let (presence, requests) = rich_presence::Presence::new();
    let executor = thread::Builder::new()
        .name("rufin-rich-presence-artwork".to_string())
        .spawn(move || {
            while let Some(request) = requests.recv() {
                let queued_ms = request.queued_for().as_millis();
                let lookup_started = Instant::now();
                let candidates = ::artwork::CandidateSet::album_facts(
                    request.artist(),
                    request.album(),
                    request.musicbrainz_release_group_id(),
                    request.musicbrainz_album_id(),
                );
                let result = artwork.resolve_public_album_url(
                    &candidates,
                    250,
                    &::artwork::ExternalPolicy::new(false, true, request.lastfm_api_key())
                        .with_musicbrainz(request.allow_musicbrainz()),
                );
                let lookup_ms = lookup_started.elapsed().as_millis();
                let total_ms = queued_ms.saturating_add(lookup_ms);
                if total_ms >= SLOW_RICH_PRESENCE_ARTWORK_MS {
                    warn!(
                        queued_ms,
                        lookup_ms, total_ms, "slow rich-presence artwork resolution"
                    );
                }
                request.complete(result);
            }
        });
    if let Err(error) = executor {
        warn!(%error, "failed to start rich-presence artwork executor");
    }
    presence
}

#[cfg(test)]
fn composed_rich_presence() -> rich_presence::Presence {
    let (presence, requests) = rich_presence::Presence::new();
    drop(requests);
    presence
}

pub(in crate::controller) struct PlaybackProduct {
    session: Mutex<PlaybackSession>,
    backend: Mutex<Box<dyn PlaybackBackend>>,
    store: StoreHandle,
    store_writer: PlaybackStoreWriter,
    materializer: BoundedRunner,
    stream_resolver: BoundedRunner,
    runtime: Arc<Runtime>,
    active_source: ActiveSourceSlot,
    reporter: Mutex<Option<(playback::RunId, Arc<ActiveSource>)>>,
    source_reports: SourceReportWorker,
    scrobbler: Mutex<Scrobbler>,
    rich_presence: rich_presence::Presence,
    private_mode: Mutex<bool>,
    events: Sender<ControllerEvent>,
    monotonic_origin: Instant,
}

impl PlaybackProduct {
    #[cfg(not(test))]
    pub(in crate::controller) fn production(
        store: StoreHandle,
        runtime: Arc<Runtime>,
        active_source: ActiveSourceSlot,
        artwork: ::artwork::Artwork,
        events: Sender<ControllerEvent>,
        sequence: playback::Sequence,
        settings: &StoredSettings,
    ) -> Result<Arc<Self>, String> {
        let backend = GStreamerPlaybackBackend::new().map_err(|error| error.to_string())?;
        let rich_presence = composed_rich_presence(artwork);
        Self::new(
            store,
            runtime,
            active_source,
            events,
            sequence,
            settings,
            Box::new(backend),
            rich_presence,
        )
    }

    fn new(
        store: StoreHandle,
        runtime: Arc<Runtime>,
        active_source: ActiveSourceSlot,
        events: Sender<ControllerEvent>,
        sequence: playback::Sequence,
        settings: &StoredSettings,
        backend: Box<dyn PlaybackBackend>,
        rich_presence: rich_presence::Presence,
    ) -> Result<Arc<Self>, String> {
        let store_writer = PlaybackStoreWriter::new(&store, events.clone())?;
        let materializer =
            BoundedRunner::new("Playback materialization", "rufin-playback-materialize", 4)?;
        let stream_resolver =
            BoundedRunner::new("Playback stream resolution", "rufin-playback-resolve", 4)?;
        let source_reports = SourceReportWorker::new(Arc::clone(&runtime))?;
        let scrobbler = Scrobbler::new(settings.scrobbling_runtime_settings())?;
        let session = PlaybackSession::new(
            sequence,
            settings.playback.clone(),
            settings.auto_dj_enabled,
            usize::from(settings.auto_dj_refill_threshold),
        );
        rich_presence.update(
            settings.rich_presence.clone(),
            !settings.private_mode,
            &settings.lastfm_api_key,
            Some(&session.view()),
        );
        Ok(Arc::new(Self {
            session: Mutex::new(session),
            backend: Mutex::new(backend),
            store,
            store_writer,
            materializer,
            stream_resolver,
            runtime,
            active_source,
            reporter: Mutex::new(None),
            source_reports,
            scrobbler: Mutex::new(scrobbler),
            rich_presence,
            private_mode: Mutex::new(settings.private_mode),
            events,
            monotonic_origin: Instant::now(),
        }))
    }

    #[cfg(test)]
    pub(in crate::controller) fn memory(
        store: StoreHandle,
        runtime: Arc<Runtime>,
        active_source: ActiveSourceSlot,
        events: Sender<ControllerEvent>,
        sequence: playback::Sequence,
        settings: &StoredSettings,
        backend: Box<dyn PlaybackBackend>,
    ) -> Result<Arc<Self>, String> {
        let rich_presence = composed_rich_presence();
        Self::new(
            store,
            runtime,
            active_source,
            events,
            sequence,
            settings,
            backend,
            rich_presence,
        )
    }

    pub(in crate::controller) fn initial_projection(&self) -> Result<PlaybackProjection, String> {
        self.session
            .lock()
            .map(|session| PlaybackProjection {
                view: session.view(),
                queue_page: Some(session.sequence().current_page()),
                notices: Vec::new(),
            })
            .map_err(|_| "playback session lock was poisoned".to_string())
    }

    pub(in crate::controller) fn command(
        self: &Arc<Self>,
        command: SessionCommand,
    ) -> Result<(), String> {
        let sample = self.clock_sample();
        let committed = {
            let mut session = self
                .session
                .lock()
                .map_err(|_| "playback session lock was poisoned".to_string())?;
            let update = session
                .handle_command(command, &sample)
                .map_err(|error| error.to_string())?;
            commit_update(&session, update)?
        };
        self.apply(committed);
        Ok(())
    }

    pub(in crate::controller) fn reserve_materialization(
        &self,
        placement: Placement,
    ) -> Result<MaterializationReservation, String> {
        self.session
            .lock()
            .map(|mut session| session.reserve_materialization(placement))
            .map_err(|_| "playback session lock was poisoned".to_string())
    }

    pub(in crate::controller) fn apply_materialization(
        self: &Arc<Self>,
        id: MaterializationId,
        source_id: SourceId,
        batch: Batch,
        placement: Placement,
    ) -> Result<bool, String> {
        let sample = self.clock_sample();
        let committed = {
            let mut session = self
                .session
                .lock()
                .map_err(|_| "playback session lock was poisoned".to_string())?;
            let Some(update) = session
                .apply_materialization(id, &source_id, batch, placement, &sample)
                .map_err(|error| error.to_string())?
            else {
                return Ok(false);
            };
            commit_update(&session, update)?
        };
        self.apply(committed);
        Ok(true)
    }

    pub(in crate::controller) fn fail_materialization(
        &self,
        id: MaterializationId,
        source_id: &SourceId,
        placement: Placement,
    ) -> bool {
        self.session
            .lock()
            .is_ok_and(|mut session| session.fail_materialization(id, source_id, placement))
    }

    pub(in crate::controller) fn switch_source(
        self: &Arc<Self>,
        sequence: playback::Sequence,
    ) -> Result<PlaybackProjection, String> {
        let sample = self.clock_sample();
        let mut committed = {
            let mut session = self
                .session
                .lock()
                .map_err(|_| "playback session lock was poisoned".to_string())?;
            let update = session.switch_source(sequence, &sample);
            commit_update(&session, update)?
        };
        let projection = committed
            .projection
            .take()
            .ok_or_else(|| "source switch produced no playback projection".to_string())?;
        self.rich_presence.observe(Some(&projection.view), false);
        self.apply(committed);
        Ok(projection)
    }

    pub(in crate::controller) fn poll(self: &Arc<Self>) {
        let events = self
            .backend
            .lock()
            .map(|mut backend| backend.drain_events())
            .unwrap_or_default();
        for event in events {
            self.backend_event(event);
        }
    }

    pub(in crate::controller) fn request_page(&self, query: QueuePageQuery) {
        if let Ok(session) = self.session.lock() {
            let _ = self
                .events
                .send(ControllerEvent::QueuePage(session.sequence().page(query)));
        }
    }

    pub(in crate::controller) fn update_runtime_settings(
        self: &Arc<Self>,
        settings: &StoredSettings,
    ) -> Result<(), String> {
        if let Ok(mut private_mode) = self.private_mode.lock() {
            *private_mode = settings.private_mode;
        }
        self.scrobbler
            .lock()
            .map_err(|_| "scrobbling lock was poisoned".to_string())?
            .update_settings(settings.scrobbling_runtime_settings());
        let view = self
            .session
            .lock()
            .map_err(|_| "playback session lock was poisoned".to_string())?
            .view();
        self.rich_presence.update(
            settings.rich_presence.clone(),
            !settings.private_mode,
            &settings.lastfm_api_key,
            Some(&view),
        );
        self.command(SessionCommand::SetAutoDj {
            enabled: settings.auto_dj_enabled,
            refill_threshold: usize::from(settings.auto_dj_refill_threshold),
        })?;
        self.command(SessionCommand::UpdateSettings(settings.playback.clone()))
    }

    pub(in crate::controller) fn submit_materialization(
        &self,
        job: impl FnOnce() + Send + 'static,
    ) -> Result<(), String> {
        self.materializer.submit(job)
    }

    pub(in crate::controller) fn shutdown_and_drain(self: &Arc<Self>) {
        let _ = self.command(SessionCommand::Shutdown);
        self.store_writer.drain();
    }

    #[cfg(test)]
    pub(in crate::controller) fn sequence_snapshot(&self) -> Option<playback::Sequence> {
        self.session
            .lock()
            .ok()
            .map(|session| session.sequence().clone())
    }

    pub(in crate::controller) fn current_entry(
        &self,
    ) -> Option<(SourceId, playback::SequenceEntry, u64)> {
        self.session.lock().ok().and_then(|session| {
            Some((
                session.sequence().source_id().clone(),
                session.sequence().selected()?.clone(),
                session.position_millis(),
            ))
        })
    }

    pub(in crate::controller) fn upcoming_tracks(
        &self,
        limit: usize,
    ) -> Option<(SourceId, Vec<Track>)> {
        self.session.lock().ok().map(|session| {
            (
                session.sequence().source_id().clone(),
                session
                    .sequence()
                    .upcoming(limit)
                    .into_iter()
                    .map(|entry| entry.track.clone())
                    .collect(),
            )
        })
    }

    pub(in crate::controller) fn activate_context_occurrence(
        self: &Arc<Self>,
        context_id: &str,
        track_id: &TrackId,
        source_rank: usize,
    ) -> Result<bool, String> {
        let sample = self.clock_sample();
        let committed = {
            let mut session = self
                .session
                .lock()
                .map_err(|_| "playback session lock was poisoned".to_string())?;
            let Some(update) = session.activate_context(context_id, track_id, source_rank, &sample)
            else {
                return Ok(false);
            };
            commit_update(&session, update)?
        };
        self.apply(committed);
        Ok(true)
    }

    pub(in crate::controller) fn queued_track_ids(
        &self,
        source_id: &SourceId,
    ) -> Option<Vec<TrackId>> {
        self.session.lock().ok().and_then(|session| {
            (session.sequence().source_id() == source_id).then(|| {
                session
                    .sequence()
                    .entries()
                    .iter()
                    .map(|entry| entry.track.id.clone())
                    .collect()
            })
        })
    }

    fn backend_event(self: &Arc<Self>, event: BackendEvent) {
        let sample = self.clock_sample();
        let committed = self.session.lock().ok().and_then(|mut session| {
            let update = session.handle_backend(event, &sample);
            commit_update(&session, update).ok()
        });
        if let Some(committed) = committed {
            self.apply(committed);
        }
    }

    fn stream_result(
        self: &Arc<Self>,
        run: playback::RunId,
        result: Result<StreamDescriptor, String>,
    ) {
        let sample = self.clock_sample();
        let committed = self.session.lock().ok().and_then(|mut session| {
            let update = match result {
                Ok(stream) => session.stream_resolved(run, stream.into()),
                Err(error) => session.stream_failed(run, error, &sample),
            };
            commit_update(&session, update).ok()
        });
        if let Some(committed) = committed {
            self.apply(committed);
        }
    }

    fn apply(self: &Arc<Self>, committed: CommittedUpdate) {
        let mut notices = Vec::new();
        let mut backend_failures = Vec::new();
        if let Some(record) = committed.checkpoint.clone() {
            self.store_writer.enqueue(PlaybackWrite::Checkpoint(record));
        }
        for effect in committed.effects {
            match effect {
                SessionEffect::ResolveStream {
                    run,
                    source_id,
                    request,
                    ..
                } => {
                    let product = Arc::clone(self);
                    let store = self.store.clone();
                    let runtime = Arc::clone(&self.runtime);
                    let active_source = Arc::clone(&self.active_source);
                    if let Err(error) = self.stream_resolver.submit(move || {
                        let result = resolve_stream_request(
                            &store,
                            &runtime,
                            &active_source,
                            &source_id,
                            &request,
                        );
                        product.stream_result(run, result);
                    }) {
                        self.stream_result(run, Err(error));
                    }
                }
                SessionEffect::Backend(command) => {
                    let run = command.run();
                    if let Err(error) = self
                        .backend
                        .lock()
                        .map_err(|_| "playback backend lock was poisoned".to_string())
                        .and_then(|mut backend| {
                            backend.send(command).map_err(|error| error.to_string())
                        })
                    {
                        backend_failures.push((run, error));
                    }
                }
                SessionEffect::PersistState {
                    source_id,
                    revision,
                    occurrence,
                    progress_millis,
                    repeat_mode,
                    shuffle_enabled,
                } => self.store_writer.enqueue(PlaybackWrite::State {
                    source_id,
                    revision,
                    occurrence: occurrence.map(|occurrence| occurrence.to_string()),
                    progress_millis,
                    repeat_mode: repeat_mode_text(repeat_mode).to_string(),
                    shuffle_enabled,
                }),
                SessionEffect::PersistOutputState { volume, muted } => self
                    .store_writer
                    .enqueue(PlaybackWrite::OutputState { volume, muted }),
                SessionEffect::PersistProgress {
                    source_id,
                    revision,
                    occurrence: Some(occurrence),
                    progress_millis,
                } => self.store_writer.enqueue(PlaybackWrite::Progress {
                    source_id,
                    revision,
                    occurrence: occurrence.to_string(),
                    progress_millis,
                }),
                SessionEffect::PersistProgress {
                    occurrence: None, ..
                } => {}
                SessionEffect::FlushPersistence { .. } => self.store_writer.fence(),
                SessionEffect::Listening(fact) => {
                    if let ListeningFact::Started { run, track, .. } = &fact
                        && let Ok(active) =
                            selected_active_source(&self.active_source, &track.source_id)
                        && let Ok(mut reporter) = self.reporter.lock()
                    {
                        *reporter = Some((*run, active));
                    }
                    let delivery_enabled = self.private_mode.lock().is_ok_and(|private| !*private);
                    if let Ok(mut scrobbler) = self.scrobbler.lock() {
                        let _ = scrobbler.observe_with_delivery(&fact, delivery_enabled);
                    }
                    if let ListeningFact::Started { run, .. } = fact {
                        notices.push(PlaybackNotice::RunStarted(run));
                    }
                }
                SessionEffect::Activity(outcome) => {
                    self.store_writer
                        .enqueue(PlaybackWrite::Activity(ActivityOutcome {
                            source_id: outcome.source_id,
                            period: outcome.local_period,
                            track_id: outcome.track_id,
                            qualified_plays: outcome.qualified_plays,
                            skips: outcome.skips,
                            last_played_at: outcome.last_played_at_unix_seconds,
                        }));
                }
                SessionEffect::SourceReport(report) => {
                    let ended_run =
                        (report.phase == SourceReportPhase::Ended).then_some(report.run);
                    self.report_source(report);
                    if let Some(run) = ended_run
                        && let Ok(mut reporter) = self.reporter.lock()
                        && reporter
                            .as_ref()
                            .is_some_and(|(active_run, _)| *active_run == run)
                    {
                        *reporter = None;
                    }
                }
                SessionEffect::MediaChanged(media) => {
                    notices.push(PlaybackNotice::MediaChanged(media));
                }
                SessionEffect::PositionDiscontinuity(discontinuity) => {
                    notices.push(PlaybackNotice::PositionDiscontinuity(discontinuity));
                }
                SessionEffect::RequestAutoDj(request) => {
                    self.request_auto_dj(request);
                }
                SessionEffect::Visualizer { run, levels } => {
                    notices.push(PlaybackNotice::Visualizer { run, levels });
                }
                SessionEffect::NonfatalError(error) => debug!(%error, "playback nonfatal effect"),
                SessionEffect::FatalError(error) => {
                    let _ = self.events.send(ControllerEvent::Error(error));
                }
            }
        }
        if let Some(mut projection) = committed.projection {
            projection.notices = notices;
            let position_discontinuity = projection.notices.iter().any(|notice| {
                matches!(
                    notice,
                    PlaybackNotice::PositionDiscontinuity(discontinuity)
                        if projection.view.transport.run == Some(discontinuity.run)
                )
            });
            self.rich_presence
                .observe(Some(&projection.view), position_discontinuity);
            let _ = self
                .events
                .send(ControllerEvent::PlaybackProduct(Box::new(projection)));
        } else if !notices.is_empty()
            && let Ok(session) = self.session.lock()
        {
            let view = session.view();
            let position_discontinuity = notices.iter().any(|notice| {
                matches!(
                    notice,
                    PlaybackNotice::PositionDiscontinuity(discontinuity)
                        if view.transport.run == Some(discontinuity.run)
                )
            });
            self.rich_presence
                .observe(Some(&view), position_discontinuity);
            let _ = self.events.send(ControllerEvent::PlaybackProduct(Box::new(
                PlaybackProjection {
                    view,
                    queue_page: None,
                    notices,
                },
            )));
        }
        for (run, error) in backend_failures {
            if let Some(run) = run {
                self.backend_event(BackendEvent::Error {
                    run,
                    error: playback::BackendFailure::new(error),
                });
            } else {
                let _ = self.events.send(ControllerEvent::Error(error));
            }
        }
    }

    fn report_source(&self, fact: SourceReportFact) {
        let reporter = self
            .reporter
            .lock()
            .ok()
            .and_then(|reporter| {
                reporter
                    .as_ref()
                    .filter(|(run, _)| *run == fact.run)
                    .map(|(_, active)| Arc::clone(active))
            })
            .and_then(|active| active.reporter.clone());
        let Some(reporter) = reporter else {
            return;
        };
        let report = sources::PlaybackReport {
            kind: match fact.phase {
                SourceReportPhase::Started => sources::PlaybackReportKind::Started,
                SourceReportPhase::Progress => sources::PlaybackReportKind::Progress,
                SourceReportPhase::QualifiedPlay => sources::PlaybackReportKind::QualifiedPlay,
                SourceReportPhase::Ended => sources::PlaybackReportKind::Stopped,
            },
            track_id: fact.track_id,
            position_seconds: (fact.position_millis / 1_000).min(u64::from(u32::MAX)) as u32,
            paused: fact.paused,
            muted: fact.muted,
            volume_percent: (fact.volume.clamp(0.0, 1.0) * 100.0).round() as u8,
            shuffle: fact.shuffle,
            repeat_one: fact.repeat_mode == RepeatMode::One,
            repeat_all: fact.repeat_mode == RepeatMode::All,
            failed: fact.failed,
        };
        if let Err(error) = self
            .source_reports
            .submit(fact.run, fact.phase, reporter, report)
        {
            warn!(%error, run = %fact.run, phase = ?fact.phase, "dropped source playback report");
        }
    }

    fn request_auto_dj(self: &Arc<Self>, request: playback::AutoDjRequest) {
        let product = Arc::clone(self);
        let failed_request = request.clone();
        if let Err(error) = self.materializer.submit(move || {
            let result = product.generate_auto_dj_candidates(&request);
            product.complete_auto_dj(request, result);
        }) {
            self.complete_auto_dj(failed_request, Err(error));
        }
    }

    fn generate_auto_dj_candidates(
        &self,
        request: &playback::AutoDjRequest,
    ) -> Result<Vec<Track>, String> {
        let active = selected_active_source(&self.active_source, &request.source_id)?;
        let saved = self
            .store
            .with_store(|store| store.stored_source(&request.source_id))?
            .ok_or_else(|| "The active source is no longer configured.".to_string())?;
        let settings = self.store.load_settings();
        let generated = (active.auto_dj.generated)(
            &self.store,
            &self.runtime,
            &saved,
            &settings,
            GeneratedTrackSeed::Track(request.seed_track_id.clone()),
            AUTO_DJ_PROVIDER_CANDIDATE_LIMIT,
        );
        let genre_name = self
            .store
            .with_store(|store| store.load_track(&request.source_id, &request.seed_track_id))?
            .and_then(|track| {
                track
                    .genres
                    .into_iter()
                    .find(|genre| !genre.trim().is_empty())
            });
        let fallback = (active.auto_dj.fallback)(
            &self.store,
            &self.runtime,
            &saved,
            &settings,
            genre_name,
            AUTO_DJ_PROVIDER_CANDIDATE_LIMIT,
            &request.seed_track_id,
        );

        let mut errors = Vec::new();
        let mut pool = Vec::new();
        match generated {
            Ok(tracks) => pool.extend(tracks),
            Err(error) => errors.push(error),
        }
        match fallback {
            Ok(tracks) => pool.extend(tracks),
            Err(error) => errors.push(error),
        }
        let mut seen = HashSet::new();
        pool.retain(|track| seen.insert(track.id.clone()));
        shuffle_tracks(&mut pool, random_seed());
        if pool.is_empty() && !errors.is_empty() {
            return Err(errors.join("; "));
        }
        Ok(pool)
    }

    fn complete_auto_dj(
        self: &Arc<Self>,
        request: playback::AutoDjRequest,
        result: Result<Vec<Track>, String>,
    ) {
        let sample = self.clock_sample();
        let committed = self.session.lock().ok().and_then(|mut session| {
            let update = match result {
                Ok(candidates) => session
                    .complete_auto_dj_candidates(
                        &request.source_id,
                        &request.seed_occurrence,
                        candidates,
                        request.requested_count,
                        shuffle_seed(),
                        &sample,
                    )
                    .ok()
                    .flatten(),
                Err(error) => session.auto_dj_unavailable(
                    &request.source_id,
                    &request.seed_occurrence,
                    Some(error),
                ),
            }?;
            commit_update(&session, update).ok()
        });
        if let Some(committed) = committed {
            self.apply(committed);
        }
    }

    fn clock_sample(&self) -> ClockSample {
        let now = SystemTime::now();
        let unix_seconds = now
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
            .unwrap_or_default();
        ClockSample {
            monotonic_millis: self
                .monotonic_origin
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
            unix_seconds,
            local_period: local_calendar_period(now),
        }
    }
}

fn random_seed() -> u64 {
    let mut bytes = [0_u8; 8];
    if getrandom::fill(&mut bytes).is_ok() {
        return u64::from_le_bytes(bytes);
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

fn shuffle_tracks(tracks: &mut [Track], mut state: u64) {
    for index in (1..tracks.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let target = (state as usize) % (index + 1);
        tracks.swap(index, target);
    }
}

impl AppController {
    pub(in crate::controller) fn playback_product(&self) -> Result<Arc<PlaybackProduct>, String> {
        self.playback_product
            .read()
            .map_err(|_| "playback product lock was poisoned".to_string())?
            .clone()
            .ok_or_else(|| "No playback source is active.".to_string())
    }

    pub(in crate::controller) fn playback_product_if_present(
        &self,
    ) -> Option<Arc<PlaybackProduct>> {
        self.playback_product
            .read()
            .ok()
            .and_then(|product| product.clone())
    }

    pub(in crate::controller) fn submit_playback_materialization(
        &self,
        job: impl FnOnce() + Send + 'static,
    ) -> Result<(), String> {
        self.playback_product()?.submit_materialization(job)
    }

    pub(in crate::controller) fn activate_playback_source(
        &self,
        source_id: &SourceId,
    ) -> Result<PlaybackProjection, String> {
        let sequence = match restore_playback_sequence(&self.store, source_id) {
            Ok(sequence) => sequence,
            Err(PlaybackRestoreError::Corrupt(error)) => {
                let _ = self.events.send(ControllerEvent::Error(format!(
                    "Saved playback could not be restored: {error}"
                )));
                playback::Sequence::new(source_id.clone())
            }
            Err(PlaybackRestoreError::Unavailable(error)) => {
                return Err(format!("Saved playback could not be restored: {error}"));
            }
        };
        let settings = self.load_settings_with_scrobbling_secrets();
        self.store
            .with_store(|store| store.stored_source(source_id))?
            .ok_or_else(|| format!("Saved source {} was not found.", source_id.as_str()))?;
        let existing = self.playback_product_if_present();
        let (product, switched_projection) = if let Some(product) = existing {
            let projection = product.switch_source(sequence)?;
            product.update_runtime_settings(&settings)?;
            (product, Some(projection))
        } else {
            #[cfg(not(test))]
            let product = PlaybackProduct::production(
                self.store.clone(),
                Arc::clone(&self.runtime),
                Arc::clone(&self.active_source),
                self.artwork.clone(),
                self.events.clone(),
                sequence,
                &settings,
            )?;
            #[cfg(test)]
            let product = PlaybackProduct::memory(
                self.store.clone(),
                Arc::clone(&self.runtime),
                Arc::clone(&self.active_source),
                self.events.clone(),
                sequence,
                &settings,
                Box::new(RecordingBackend::default()),
            )?;
            *self
                .playback_product
                .write()
                .map_err(|_| "playback product lock was poisoned".to_string())? =
                Some(Arc::clone(&product));
            (product, None)
        };
        switched_projection.map_or_else(|| product.initial_projection(), Ok)
    }

    pub(in crate::controller) fn clear_playback_product(&self) {
        let product = self
            .playback_product
            .write()
            .ok()
            .and_then(|mut product| product.take());
        if let Some(product) = product {
            product.shutdown_and_drain();
        }
    }

    pub fn shutdown_playback(&self) {
        if let Some(product) = self.playback_product_if_present() {
            product.shutdown_and_drain();
        }
    }

    pub(in crate::controller) fn send_session_command(&self, command: SessionCommand) {
        let result = self
            .playback_product()
            .and_then(|product| product.command(command));
        if let Err(error) = result {
            let _ = self.events.send(ControllerEvent::Error(error));
        }
    }

    pub(in crate::controller) fn current_playback_entry(
        &self,
    ) -> Option<(SourceId, playback::SequenceEntry, u64)> {
        self.playback_product_if_present()
            .and_then(|product| product.current_entry())
    }

    pub fn request_queue_page(&self, query: QueuePageQuery) -> bool {
        if let Some(product) = self.playback_product_if_present() {
            product.request_page(query);
            true
        } else {
            false
        }
    }
}

struct CommittedUpdate {
    effects: Vec<SessionEffect>,
    checkpoint: Option<PlaybackCheckpointRecord>,
    projection: Option<PlaybackProjection>,
}

fn commit_update(
    session: &PlaybackSession,
    update: SessionUpdate,
) -> Result<CommittedUpdate, String> {
    let checkpoint = update
        .structure_changed
        .then(|| encode_store_checkpoint(session.sequence()))
        .transpose()?;
    let projection = update.view_changed.then(|| PlaybackProjection {
        view: session.view(),
        queue_page: update
            .structure_changed
            .then(|| session.sequence().current_page()),
        notices: Vec::new(),
    });
    Ok(CommittedUpdate {
        effects: update.effects,
        checkpoint,
        projection,
    })
}

fn encode_store_checkpoint(
    sequence: &playback::Sequence,
) -> Result<PlaybackCheckpointRecord, String> {
    let checkpoint = encode_checkpoint(sequence).map_err(|error| error.to_string())?;
    Ok(PlaybackCheckpointRecord {
        source_id: checkpoint.header.source_id,
        revision: checkpoint.header.revision,
        selected_occurrence_id: checkpoint
            .header
            .selected_occurrence
            .map(|occurrence| occurrence.to_string()),
        progress_millis: checkpoint.header.progress_millis,
        repeat_mode: repeat_mode_text(checkpoint.header.repeat_mode).to_string(),
        shuffle_enabled: checkpoint.header.shuffle_enabled,
        payload: checkpoint.payload,
    })
}

pub(in crate::controller) fn restore_playback_sequence(
    store: &StoreHandle,
    source_id: &SourceId,
) -> Result<playback::Sequence, PlaybackRestoreError> {
    let Some(record) = store
        .with_store(|store| store.load_playback_checkpoint(source_id))
        .map_err(PlaybackRestoreError::Unavailable)?
    else {
        return Ok(playback::Sequence::new(source_id.clone()));
    };
    let current = parse_repeat_mode(&record.repeat_mode).and_then(|repeat_mode| {
        decode_checkpoint(&playback::CheckpointRecord {
            header: CheckpointHeader {
                source_id: record.source_id.clone(),
                revision: record.revision,
                selected_occurrence: record
                    .selected_occurrence_id
                    .as_deref()
                    .map(OccurrenceId::from),
                progress_millis: record.progress_millis,
                repeat_mode,
                shuffle_enabled: record.shuffle_enabled,
            },
            payload: record.payload.clone(),
        })
        .map_err(|error| error.to_string())
    });
    let current_error = match current {
        Ok(mut sequence) => {
            hydrate_restored_tracks(store, &record.source_id, &mut sequence)?;
            return Ok(sequence);
        }
        Err(error) => error,
    };
    let legacy = store.with_store_result(
        |store| {
            decode_legacy_queue_snapshot_with_tracks(&record.payload, |track_id| {
                store
                    .load_track(&record.source_id, track_id)
                    .map_err(|error| error.to_string())
            })
            .map_err(|error| match error {
                CheckpointError::LegacyTrackLookup(error) => LegacyRestoreError::Store(error),
                error => LegacyRestoreError::Corrupt(error.to_string()),
            })
        },
        |error| LegacyRestoreError::Store(error.to_string()),
        |error| error,
        || LegacyRestoreError::Store("store lock was poisoned".to_string()),
    );
    match legacy {
        Ok(mut sequence) => {
            hydrate_restored_tracks(store, &record.source_id, &mut sequence)?;
            let rewritten =
                encode_store_checkpoint(&sequence).map_err(PlaybackRestoreError::Unavailable)?;
            store
                .with_store(|store| store.save_playback_checkpoint(&rewritten))
                .map_err(PlaybackRestoreError::Unavailable)?;
            Ok(sequence)
        }
        Err(LegacyRestoreError::Store(error)) => Err(PlaybackRestoreError::Unavailable(error)),
        Err(LegacyRestoreError::Corrupt(legacy_error)) => {
            store
                .with_store(|store| {
                    store
                        .delete_playback_checkpoint(source_id)
                        .map(|_deleted| ())
                })
                .map_err(PlaybackRestoreError::Unavailable)?;
            Err(PlaybackRestoreError::Corrupt(format!(
                "{current_error}; legacy playback checkpoint is invalid: {legacy_error}"
            )))
        }
    }
}

fn hydrate_restored_tracks(
    store: &StoreHandle,
    source_id: &SourceId,
    sequence: &mut playback::Sequence,
) -> Result<(), PlaybackRestoreError> {
    let mut seen = HashSet::new();
    let track_ids = sequence
        .entries()
        .iter()
        .map(|entry| entry.track.id.clone())
        .filter(|track_id| seen.insert(track_id.clone()))
        .collect::<Vec<_>>();
    let tracks = store
        .with_store(|store| store.load_tracks_by_ids(source_id, &track_ids))
        .map_err(PlaybackRestoreError::Unavailable)?;
    sequence.hydrate_tracks(tracks);
    Ok(())
}

#[derive(Debug)]
pub(in crate::controller) enum PlaybackRestoreError {
    Corrupt(String),
    Unavailable(String),
}

enum LegacyRestoreError {
    Store(String),
    Corrupt(String),
}

fn parse_repeat_mode(value: &str) -> Result<RepeatMode, String> {
    match value {
        "Off" => Ok(RepeatMode::Off),
        "One" => Ok(RepeatMode::One),
        "All" => Ok(RepeatMode::All),
        _ => Err(format!("saved repeat mode is invalid: {value}")),
    }
}

fn repeat_mode_text(value: RepeatMode) -> &'static str {
    match value {
        RepeatMode::Off => "Off",
        RepeatMode::One => "One",
        RepeatMode::All => "All",
    }
}

fn local_calendar_period(now: SystemTime) -> String {
    let _ = now;
    gtk::glib::DateTime::now_local()
        .or_else(|_| gtk::glib::DateTime::now_utc())
        .ok()
        .and_then(|date| date.format("%Y-%m").ok())
        .map(|period| period.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
#[derive(Default)]
pub(in crate::controller) struct RecordingBackend {
    commands: Vec<BackendCommand>,
    events: Vec<BackendEvent>,
}

#[cfg(test)]
impl PlaybackBackend for RecordingBackend {
    fn send(&mut self, command: BackendCommand) -> Result<(), playback::BackendError> {
        self.commands.push(command);
        Ok(())
    }

    fn drain_events(&mut self) -> Vec<BackendEvent> {
        self.events.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::TryRecvError;

    struct StartFailingBackend;

    impl PlaybackBackend for StartFailingBackend {
        fn send(&mut self, command: BackendCommand) -> Result<(), playback::BackendError> {
            if matches!(command, BackendCommand::Start { .. }) {
                return Err(playback::BackendError::Backend(
                    "start was rejected".to_string(),
                ));
            }
            Ok(())
        }

        fn drain_events(&mut self) -> Vec<BackendEvent> {
            Vec::new()
        }
    }

    #[test]
    fn backend_start_failure_transitions_the_run_and_emits_one_diagnostic() {
        let store = StoreHandle::open_memory().expect("open store");
        let mut sequence = playback::Sequence::new(SourceId::new("source:backend-failure"));
        sequence
            .apply_batch(
                Batch::new(vec![playback::BatchItem::new(
                    restored_track(),
                    playback::Provenance::Manual,
                )]),
                Placement::Replace { anchor_index: 0 },
            )
            .expect("seed sequence");
        let (events, receiver) = channel();
        let product = PlaybackProduct::memory(
            store,
            Arc::new(Runtime::new().expect("runtime")),
            Arc::new(RwLock::new(None)),
            events,
            sequence,
            &StoredSettings::default(),
            Box::new(StartFailingBackend),
        )
        .expect("playback product");
        let committed = {
            let mut session = product.session.lock().expect("session");
            let sample = product.clock_sample();
            session
                .handle_command(SessionCommand::Play, &sample)
                .expect("start command");
            let run = session.current_run().expect("run");
            let update = session.stream_resolved(run, playback::PreparedStream::new("file:///a"));
            commit_update(&session, update).expect("commit start")
        };

        product.apply(committed);

        assert_eq!(
            product.session.lock().expect("session").status(),
            playback::TransportStatus::Failed
        );
        let mut errors = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if let ControllerEvent::Error(error) = event {
                errors.push(error);
            }
        }
        assert_eq!(errors, ["playback backend failed: start was rejected"]);
    }

    #[test]
    fn drain_waits_until_contended_store_work_finishes() {
        let mailbox = Arc::new(PlaybackStoreMailbox::new());
        let (finished, receiver) = channel();
        let drainer = Arc::clone(&mailbox);
        let handle = thread::spawn(move || {
            drainer.drain();
            let _ = finished.send(());
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if mailbox.state.lock().expect("inspect drain").draining {
                break;
            }
            assert!(Instant::now() < deadline, "drain did not reach the worker");
            thread::yield_now();
        }

        let first = mailbox.next_batch().expect("first drain batch");
        mailbox.complete(first.generation, true);
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));

        let retry = mailbox.next_batch().expect("contention retry batch");
        mailbox.complete(retry.generation, false);
        receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("drain should finish after contention clears");
        handle.join().expect("join drain thread");
    }

    #[test]
    fn saturated_writer_keeps_callers_nonblocking_and_coalesces_exact_facts() {
        let store = StoreHandle::open_memory().expect("open store");
        let source = saved_source();
        let occurrence = "occurrence:writer";
        let track_id = TrackId::new("track:writer-activity");
        store
            .with_store(|store| {
                store.save_source(&source)?;
                store.save_playback_checkpoint(&PlaybackCheckpointRecord {
                    source_id: source.source_id.clone(),
                    revision: 7,
                    selected_occurrence_id: Some(occurrence.to_string()),
                    progress_millis: 0,
                    repeat_mode: "Off".to_string(),
                    shuffle_enabled: false,
                    payload: "opaque".to_string(),
                })
            })
            .expect("seed checkpoint");
        let (events, receiver) = channel();
        let writer = Arc::new(PlaybackStoreWriter::new(&store, events).expect("start writer"));
        let StoreHandle::Memory { store: memory, .. } = &store else {
            return;
        };
        let memory = Arc::clone(memory);
        let guard = memory.lock().expect("hold Store writer");
        writer.enqueue(PlaybackWrite::State {
            source_id: source.source_id.clone(),
            revision: 7,
            occurrence: Some(occurrence.to_string()),
            progress_millis: 1,
            repeat_mode: "Off".to_string(),
            shuffle_enabled: false,
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let pending = writer.mailbox.state.lock().expect("inspect mailbox");
            if pending.durable.is_empty() && pending.completed_generation < pending.generation {
                break;
            }
            assert!(Instant::now() < deadline, "Store writer did not start");
            drop(pending);
            thread::yield_now();
        }

        let producer = Arc::clone(&writer);
        let producer_source = source.source_id.clone();
        let producer_occurrence = occurrence.to_string();
        let producer_track = track_id.clone();
        let (producer_done, producer_wait) = channel();
        thread::spawn(move || {
            for index in 1..=STORE_ACTIVITY_CAPACITY * 4 {
                producer.enqueue(PlaybackWrite::Progress {
                    source_id: producer_source.clone(),
                    revision: 7,
                    occurrence: producer_occurrence.clone(),
                    progress_millis: index as u64 * 1_000,
                });
                producer.enqueue(PlaybackWrite::Activity(ActivityOutcome {
                    source_id: producer_source.clone(),
                    period: "2026-07".to_string(),
                    track_id: producer_track.clone(),
                    qualified_plays: 1,
                    skips: 0,
                    last_played_at: Some(1_783_850_400 + index as i64),
                }));
                producer.enqueue(PlaybackWrite::OutputState {
                    volume: index as f64 / (STORE_ACTIVITY_CAPACITY * 4) as f64,
                    muted: index % 2 == 0,
                });
            }
            producer.fence();
            let _ = producer_done.send(());
        });
        producer_wait
            .recv_timeout(Duration::from_secs(1))
            .expect("enqueue and fence must not wait for Store I/O");
        drop(guard);

        writer.drain();

        let saved = store
            .with_store(|store| store.load_playback_checkpoint(&source.source_id))
            .expect("load checkpoint")
            .expect("checkpoint");
        assert_eq!(
            saved.progress_millis,
            (STORE_ACTIVITY_CAPACITY * 4) as u64 * 1_000
        );
        let activity = store
            .with_store(|store| store.track_activity_summary(&source.source_id, &track_id))
            .expect("load activity");
        assert_eq!(
            activity.qualified_plays,
            (STORE_ACTIVITY_CAPACITY * 4) as u32
        );
        assert_eq!(activity.skips, 0);
        let settings = store.load_settings();
        assert_eq!(settings.playback.volume, 1.0);
        assert!(settings.playback.muted);
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)),
            Ok(ControllerEvent::LibraryDelta(delta))
                if delta.tracks.stats == [track_id]
                    && delta.tracks.skip_stats.is_empty()
        ));
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn durable_write_retries_on_a_later_fact_without_emitting_ui_errors() {
        let store = StoreHandle::open_memory().expect("open store");
        let source = saved_source();
        let (events, receiver) = channel();
        let writer = PlaybackStoreWriter::new(&store, events).expect("start writer");
        writer.enqueue(PlaybackWrite::Checkpoint(test_checkpoint(
            &source.source_id,
            1,
            "first",
        )));
        writer.drain();
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
        store
            .with_store(|store| store.save_source(&source))
            .expect("save source");
        writer.enqueue(PlaybackWrite::Checkpoint(test_checkpoint(
            &source.source_id,
            2,
            "second",
        )));
        writer.drain();

        let saved = store
            .with_store(|store| store.load_playback_checkpoint(&source.source_id))
            .expect("load checkpoint")
            .expect("checkpoint");
        assert_eq!(saved.revision, 2);
        assert_eq!(saved.payload, "second");
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn durable_failure_retains_each_sources_latest_checkpoint_and_state() {
        let store = StoreHandle::open_memory().expect("open store");
        let first = saved_source();
        let mut second = first.clone();
        second.source_id = SourceId::new("jellyfin:server:second");
        second.name = "Second Server".to_string();
        let (events, receiver) = channel();
        let writer = PlaybackStoreWriter::new(&store, events).expect("start writer");

        writer.enqueue(PlaybackWrite::Checkpoint(test_checkpoint(
            &first.source_id,
            1,
            "obsolete",
        )));
        writer.drain();
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
        writer.enqueue(PlaybackWrite::Checkpoint(test_checkpoint(
            &first.source_id,
            2,
            "latest",
        )));
        writer.enqueue(PlaybackWrite::State {
            source_id: first.source_id.clone(),
            revision: 2,
            occurrence: Some("current".to_string()),
            progress_millis: 21_000,
            repeat_mode: "One".to_string(),
            shuffle_enabled: true,
        });
        writer.enqueue(PlaybackWrite::Progress {
            source_id: first.source_id.clone(),
            revision: 2,
            occurrence: "current".to_string(),
            progress_millis: 42_000,
        });
        writer.enqueue(PlaybackWrite::Checkpoint(test_checkpoint(
            &second.source_id,
            3,
            "second",
        )));
        writer.drain();
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));

        store
            .with_store(|store| {
                store.save_source(&first)?;
                store.save_source(&second)
            })
            .expect("save sources");
        writer.drain();

        let first_record = store
            .with_store(|store| store.load_playback_checkpoint(&first.source_id))
            .expect("load first checkpoint")
            .expect("first checkpoint");
        assert_eq!(first_record.revision, 2);
        assert_eq!(first_record.payload, "latest");
        assert_eq!(
            first_record.selected_occurrence_id.as_deref(),
            Some("current")
        );
        assert_eq!(first_record.progress_millis, 42_000);
        assert_eq!(first_record.repeat_mode, "One");
        assert!(first_record.shuffle_enabled);
        let second_record = store
            .with_store(|store| store.load_playback_checkpoint(&second.source_id))
            .expect("load second checkpoint")
            .expect("second checkpoint");
        assert_eq!(second_record.revision, 3);
        assert_eq!(second_record.payload, "second");
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn non_contention_activity_failure_is_nonfatal_and_is_not_retried() {
        let store = StoreHandle::open_memory().expect("open store");
        let source = saved_source();
        let track_id = TrackId::new("track:activity-failure");
        let (events, receiver) = channel();
        let writer = PlaybackStoreWriter::new(&store, events).expect("start writer");
        writer.enqueue(PlaybackWrite::Activity(ActivityOutcome {
            source_id: source.source_id.clone(),
            period: "2026-07".to_string(),
            track_id: track_id.clone(),
            qualified_plays: 1,
            skips: 0,
            last_played_at: Some(1_783_850_400),
        }));
        writer.drain();
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));

        store
            .with_store(|store| store.save_source(&source))
            .expect("save source");
        writer.drain();

        let activity = store
            .with_store(|store| store.track_activity_summary(&source.source_id, &track_id))
            .expect("load activity");
        assert_eq!(activity.qualified_plays, 0);
        assert_eq!(activity.skips, 0);
    }

    #[test]
    fn recorded_activity_invalidates_play_and_skip_projections() {
        let store = StoreHandle::open_memory().expect("open store");
        let source = saved_source();
        store
            .with_store(|store| store.save_source(&source))
            .expect("save source");
        let track_id = TrackId::new("track:activity");
        let (events, receiver) = channel();
        let writer = PlaybackStoreWriter::new(&store, events).expect("start writer");

        writer.enqueue(PlaybackWrite::Activity(ActivityOutcome {
            source_id: source.source_id,
            period: "2026-07".to_string(),
            track_id: track_id.clone(),
            qualified_plays: 1,
            skips: 1,
            last_played_at: Some(1_783_850_400),
        }));
        writer.drain();

        let delta = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("activity invalidation");
        assert!(matches!(
            delta,
            ControllerEvent::LibraryDelta(delta)
                if delta.tracks.stats == [track_id.clone()]
                    && delta.tracks.skip_stats == [track_id]
        ));
    }

    #[test]
    fn process_shutdown_drains_live_progress_before_reopen() {
        let store = StoreHandle::open_memory().expect("open store");
        let source = saved_source();
        let track = restored_track();
        let mut sequence = playback::Sequence::new(source.source_id.clone());
        sequence
            .apply_batch(
                Batch::new(vec![playback::BatchItem::new(
                    track.clone(),
                    playback::Provenance::Manual,
                )]),
                Placement::Replace { anchor_index: 0 },
            )
            .expect("seed sequence");
        let checkpoint = encode_store_checkpoint(&sequence).expect("encode checkpoint");
        store
            .with_store(|store| {
                store.save_source(&source)?;
                store.set_active_source(&source.source_id)?;
                store.save_playback_checkpoint(&checkpoint)
            })
            .expect("seed playback");
        let (controller, _events) = controller_from_store_for_test(store.clone());
        let product = controller.playback_product().expect("playback product");
        let run = {
            let sample = product.clock_sample();
            let mut session = product.session.lock().expect("playback session");
            session
                .handle_command(SessionCommand::PlayPause, &sample)
                .expect("start run");
            session.current_run().expect("current run")
        };
        product.backend_event(BackendEvent::Position {
            run,
            millis: 37_000,
        });

        controller.shutdown_playback();
        drop(product);
        drop(controller);

        let (reopened, _events) = controller_from_store_for_test(store);
        let restored = reopened
            .playback_product()
            .expect("reopened playback product")
            .sequence_snapshot()
            .expect("restored sequence");
        assert_eq!(restored.progress_millis(), 37_000);
        assert_eq!(
            restored.selected().map(|entry| &entry.track.id),
            Some(&track.id)
        );
    }

    #[test]
    fn activation_hydration_and_progress_survive_reopen() {
        let store = StoreHandle::open_memory().expect("open store");
        let source = saved_source();
        let album = remote_album_with_image_ref(provider_cover_ref());
        let mut canonical = library_track(
            1,
            album.artist_id.clone(),
            album.id.clone(),
            &album.artist,
            &["Canonical Genre"],
        );
        canonical.id = TrackId::new("jellyfin:track:hydrated");
        canonical.title = "Canonical title".to_string();
        canonical.album = album.title.clone();
        seed_cached_library(
            &store,
            &source,
            std::slice::from_ref(&album),
            std::slice::from_ref(&canonical),
            &[],
        );

        let mut stale = canonical.clone();
        stale.title = "Stale title".to_string();
        let mut sequence = playback::Sequence::new(source.source_id.clone());
        sequence
            .apply_batch(
                Batch::new(vec![playback::BatchItem::new(
                    stale,
                    playback::Provenance::Manual,
                )]),
                Placement::Replace { anchor_index: 0 },
            )
            .expect("seed stale sequence");
        let checkpoint = encode_store_checkpoint(&sequence).expect("encode checkpoint");
        store
            .with_store(|store| store.save_playback_checkpoint(&checkpoint))
            .expect("seed playback");

        let (controller, _events) = controller_from_store_for_test(store.clone());
        let product = controller.playback_product().expect("playback product");
        let hydrated = product.sequence_snapshot().expect("hydrated sequence");
        assert_eq!(hydrated.entries()[0].track.title, canonical.title);
        let run = {
            let sample = product.clock_sample();
            let mut session = product.session.lock().expect("playback session");
            session
                .handle_command(SessionCommand::PlayPause, &sample)
                .expect("start run");
            session.current_run().expect("current run")
        };
        product.backend_event(BackendEvent::Position {
            run,
            millis: 37_000,
        });
        controller.shutdown_playback();
        drop(product);
        drop(controller);

        let (reopened, _events) = controller_from_store_for_test(store);
        let restored = reopened
            .playback_product()
            .expect("reopened playback product")
            .sequence_snapshot()
            .expect("restored sequence");
        assert_eq!(restored.progress_millis(), 37_000);
        assert_eq!(restored.entries()[0].track.title, canonical.title);
    }

    #[test]
    fn legacy_checkpoint_hydrates_the_canonical_store_track_and_rewrites_once() {
        let store = StoreHandle::open_memory().expect("open store");
        let source = saved_source();
        let album = remote_album_with_image_ref(provider_cover_ref());
        let mut canonical = library_track(
            1,
            album.artist_id.clone(),
            album.id.clone(),
            &album.artist,
            &["Canonical Genre"],
        );
        canonical.id = TrackId::new("jellyfin:track:legacy");
        canonical.title = "Canonical title".to_string();
        canonical.album = album.title.clone();
        canonical.image_ref = album.image_ref.clone();
        seed_cached_library(
            &store,
            &source,
            std::slice::from_ref(&album),
            std::slice::from_ref(&canonical),
            &[],
        );
        let expected = store
            .with_store(|store| store.load_track(&source.source_id, &canonical.id))
            .expect("load canonical track")
            .expect("canonical track");
        let occurrence = "queue-entry:legacy";
        let payload = serde_json::json!({
            "server_id": source.source_id.as_str(),
            "entries": [{
                "id": occurrence,
                "track_id": canonical.id.as_str(),
                "title": "Stale title",
                "artist": "Stale artist",
                "artist_id": null,
                "album": "Stale album",
                "year": 0,
                "duration_seconds": 1,
                "favorite": false,
                "image_ref": null,
                "local_path": null,
                "source_format": null,
                "origin": { "Manual": {} }
            }],
            "current_index": 0,
            "repeat_mode": "Off",
            "shuffle": { "enabled": false, "seed": 0 },
            "shuffle_order": [],
            "progress_seconds": 0
        })
        .to_string();
        store
            .with_store(|store| {
                store.save_playback_checkpoint(&PlaybackCheckpointRecord {
                    source_id: source.source_id.clone(),
                    revision: 0,
                    selected_occurrence_id: Some(occurrence.to_string()),
                    progress_millis: 0,
                    repeat_mode: "Off".to_string(),
                    shuffle_enabled: false,
                    payload,
                })
            })
            .expect("save legacy checkpoint");

        let restored = restore_playback_sequence(&store, &source.source_id).expect("restore");

        assert_eq!(restored.entries()[0].track, expected);
        assert_eq!(
            restored.entries()[0].provenance,
            playback::Provenance::Manual
        );
        assert_eq!(restored.revision(), 1);
        let rewritten = store
            .with_store(|store| store.load_playback_checkpoint(&source.source_id))
            .expect("load rewritten checkpoint")
            .expect("rewritten checkpoint");
        assert_eq!(rewritten.revision, 1);
        let restored_again =
            restore_playback_sequence(&store, &source.source_id).expect("restore rewritten");
        assert_eq!(restored_again.entries(), restored.entries());
        assert_eq!(restored_again.revision(), 1);
    }

    #[test]
    fn corrupt_checkpoint_is_removed_and_the_next_queue_survives_reopen() {
        let store = StoreHandle::open_memory().expect("open store");
        let source = saved_source();
        store
            .with_store(|store| {
                store.save_source(&source)?;
                store.set_active_source(&source.source_id)?;
                store.save_playback_checkpoint(&PlaybackCheckpointRecord {
                    source_id: source.source_id.clone(),
                    revision: 99,
                    selected_occurrence_id: None,
                    progress_millis: 0,
                    repeat_mode: "Off".to_string(),
                    shuffle_enabled: false,
                    payload: "not json".to_string(),
                })
            })
            .expect("seed corrupt checkpoint");

        let (controller, events) = controller_from_store_for_test(store.clone());

        let error = events
            .recv_timeout(Duration::from_secs(1))
            .expect("restore error");
        assert!(matches!(
            error,
            ControllerEvent::Error(message)
                if message.starts_with("Saved playback could not be restored:")
        ));
        assert!(matches!(
            events.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        let sequence = controller
            .playback_product()
            .expect("playback product")
            .sequence_snapshot()
            .expect("sequence");
        assert_eq!(sequence.source_id(), &source.source_id);
        assert!(sequence.entries().is_empty());
        assert!(
            store
                .with_store(|store| store.load_playback_checkpoint(&source.source_id))
                .expect("load deleted checkpoint")
                .is_none()
        );

        let track = restored_track();
        controller.play_tracks_now(vec![track.clone()]);
        controller.shutdown_playback();
        drop(controller);

        let (reopened, _events) = controller_from_store_for_test(store);
        let restored = reopened
            .playback_product()
            .expect("reopened playback product")
            .sequence_snapshot()
            .expect("restored sequence");
        assert_eq!(restored.entries().len(), 1);
        assert_eq!(restored.entries()[0].track.id, track.id);
    }

    #[test]
    fn unavailable_restore_does_not_replace_the_live_session() {
        let store = StoreHandle::open_memory().expect("open store");
        let source = saved_source();
        let track = restored_track();
        let mut sequence = playback::Sequence::new(source.source_id.clone());
        sequence
            .apply_batch(
                Batch::new(vec![playback::BatchItem::new(
                    track.clone(),
                    playback::Provenance::Manual,
                )]),
                Placement::Replace { anchor_index: 0 },
            )
            .expect("seed sequence");
        let checkpoint = encode_store_checkpoint(&sequence).expect("encode checkpoint");
        store
            .with_store(|store| {
                store.save_source(&source)?;
                store.set_active_source(&source.source_id)?;
                store.save_playback_checkpoint(&checkpoint)
            })
            .expect("seed playback");
        let (controller, _events) = controller_from_store_for_test(store.clone());
        let live = controller
            .playback_product()
            .expect("live playback product");
        let StoreHandle::Memory { store: memory, .. } = &store else {
            panic!("expected memory Store");
        };
        let memory = Arc::clone(memory);
        let _ = thread::spawn(move || {
            let _guard = memory.lock().expect("lock Store before poisoning");
            panic!("poison Store lock");
        })
        .join();

        controller
            .activate_playback_source(&source.source_id)
            .expect_err("unavailable restore should fail");
        let still_live = controller.playback_product().expect("unchanged product");
        assert!(Arc::ptr_eq(&live, &still_live));
        assert_eq!(
            still_live
                .sequence_snapshot()
                .expect("live sequence")
                .selected()
                .map(|entry| &entry.track.id),
            Some(&track.id)
        );
    }

    fn test_checkpoint(
        source_id: &SourceId,
        revision: u64,
        payload: &str,
    ) -> PlaybackCheckpointRecord {
        PlaybackCheckpointRecord {
            source_id: source_id.clone(),
            revision,
            selected_occurrence_id: None,
            progress_millis: 0,
            repeat_mode: "Off".to_string(),
            shuffle_enabled: false,
            payload: payload.to_string(),
        }
    }
}
