use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;

use library::SourceId;
use tokio::runtime::Runtime;

use crate::cache::FilesystemCache;
use crate::decode::{decode_cached, decode_normalized, normalize_for_cache};
use crate::fetch::{FetchContext, FetchOutcome};
use crate::selection::Candidate;
use crate::{
    ArtworkBindingIdentity, ArtworkError, ArtworkEvent, ArtworkKey, ArtworkProjection,
    ArtworkRequest, ArtworkRequestIdentity, ArtworkVisualIdentity, CandidateSet, DecodedImage,
    ExternalPolicy, Readiness, RequestId, SourceImages,
};

const WORKERS: usize = 4;
const MAX_JOBS: usize = 256;
const MAX_DECODED_IMAGES: usize = 128;
const MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;

pub(crate) struct Pipeline {
    shared: Arc<Shared>,
}

struct Shared {
    runtime: Arc<Runtime>,
    cache: FilesystemCache,
    fetch: FetchContext,
    cache_commit: Mutex<()>,
    state: Mutex<State>,
    wake: Condvar,
    events: Sender<ArtworkEvent>,
}

#[derive(Default)]
struct State {
    next_request: u64,
    external_epoch: u64,
    source_epochs: HashMap<SourceId, u64>,
    visible: VecDeque<JobKey>,
    jobs: HashMap<JobKey, JobRecord>,
    projections: HashMap<RequestId, ProjectionRecord>,
    decoded: DecodedCache,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct JobKey(String);

struct JobRecord {
    source: SourceImages,
    request: ArtworkRequest,
    subscribers: Vec<RequestId>,
    active: bool,
    source_epoch: u64,
    external_epoch: u64,
}

#[derive(Clone)]
struct Work {
    key: JobKey,
    source: SourceImages,
    request: ArtworkRequest,
    source_epoch: u64,
    external_epoch: u64,
}

struct ProjectionRecord {
    projection: ArtworkProjection,
    source: SourceImages,
    request: ArtworkRequest,
}

#[derive(Default)]
struct DecodedCache {
    entries: VecDeque<DecodedEntry>,
    bytes: usize,
}

struct DecodedEntry {
    key: ArtworkKey,
    source_id: SourceId,
    image: Arc<DecodedImage>,
}

enum Resolution {
    Ready(Arc<DecodedImage>),
    Missing,
    Failed(Arc<str>),
}

impl Pipeline {
    pub(crate) fn new(
        cache_root: &Path,
        runtime: Arc<Runtime>,
    ) -> Result<(Self, Receiver<ArtworkEvent>), ArtworkError> {
        let cache = FilesystemCache::new(cache_root.to_path_buf())?;
        let fetch = FetchContext::new().map_err(ArtworkError::FetchSetup)?;
        let (events, receiver) = mpsc::channel();
        let shared = Arc::new(Shared {
            runtime,
            cache,
            fetch,
            cache_commit: Mutex::new(()),
            state: Mutex::new(State {
                next_request: 1,
                ..State::default()
            }),
            wake: Condvar::new(),
            events,
        });
        for index in 0..WORKERS {
            let worker = Arc::clone(&shared);
            thread::Builder::new()
                .name(format!("artwork-{index}"))
                .spawn(move || run_worker(worker))
                .map_err(ArtworkError::Cache)?;
        }
        Ok((Self { shared }, receiver))
    }

    pub(crate) fn request(
        &self,
        source: SourceImages,
        request: ArtworkRequest,
    ) -> Result<ArtworkProjection, ArtworkError> {
        let mut state = lock_state(&self.shared);
        let request_id = RequestId(state.next_request);
        state.next_request = state.next_request.wrapping_add(1).max(1);
        let key = self.shared.cache.artwork_key(&source.source_id, &request);
        if let Some(image) = state.decoded.get(&key) {
            let projection = ArtworkProjection {
                request_id,
                readiness: Readiness::Ready(image),
            };
            return Ok(projection);
        }
        let projection = ArtworkProjection {
            request_id,
            readiness: Readiness::Pending,
        };
        state.projections.insert(
            request_id,
            ProjectionRecord {
                projection: projection.clone(),
                source: source.clone(),
                request: request.clone(),
            },
        );
        if let Err(error) = enqueue(&mut state, source, request, Some(request_id)) {
            state.projections.remove(&request_id);
            return Err(error);
        }
        drop(state);
        self.shared.wake.notify_one();
        Ok(projection)
    }

    #[cfg(test)]
    pub(crate) fn projection(&self, request_id: RequestId) -> Option<ArtworkProjection> {
        lock_state(&self.shared)
            .projections
            .get(&request_id)
            .map(|record| record.projection.clone())
    }

    pub(crate) fn cancel(&self, request_id: RequestId) {
        let mut state = lock_state(&self.shared);
        if state.projections.remove(&request_id).is_none() {
            return;
        }
        let mut removable = Vec::new();
        for (key, record) in &mut state.jobs {
            record
                .subscribers
                .retain(|subscriber| *subscriber != request_id);
            if !record.active && record.subscribers.is_empty() {
                removable.push(key.clone());
            }
        }
        for key in &removable {
            state.jobs.remove(key);
        }
        state.visible.retain(|key| !removable.contains(key));
        drop(state);
    }

    pub(crate) fn cache_only_file(
        &self,
        source_id: &SourceId,
        request: &ArtworkRequest,
    ) -> Option<std::path::PathBuf> {
        for candidate in request.candidates.candidates() {
            if candidate.is_external() && !request.external.allow_cached {
                continue;
            }
            if let Some(entry) =
                self.shared
                    .cache
                    .ready_entry(source_id, candidate, request.fetch_size)
            {
                return Some(entry.path);
            }
        }
        None
    }

    pub(crate) fn binding_identity(
        &self,
        source: &SourceImages,
        request: &ArtworkRequest,
    ) -> ArtworkBindingIdentity {
        let state = lock_state(&self.shared);
        let source_epoch = source_epoch(&state, &source.source_id);
        let external_epoch = request
            .candidates
            .has_external()
            .then_some(state.external_epoch)
            .unwrap_or_default();
        let cached_external = request.candidates.has_external() && request.external.allow_cached;
        let visual = format!(
            "{}\0{}\0{}\0{}",
            source.source_id,
            request.candidates.stable_identity(),
            cached_external,
            source_epoch,
        );
        let request = job_key(source, request, source_epoch, external_epoch);
        ArtworkBindingIdentity {
            visual: ArtworkVisualIdentity::new(visual),
            request: ArtworkRequestIdentity::new(request.0),
        }
    }

    pub(crate) fn retry_external(&self) -> Result<(), ArtworkError> {
        let commit = lock_cache_commit(&self.shared);
        self.shared.cache.retry_external()?;
        let mut state = lock_state(&self.shared);
        state.external_epoch = state.external_epoch.wrapping_add(1);
        drop(state);
        drop(commit);
        self.shared.wake.notify_all();
        Ok(())
    }

    pub(crate) fn invalidate_source(&self, source_id: &SourceId) -> Result<(), ArtworkError> {
        let commit = lock_cache_commit(&self.shared);
        self.shared.cache.invalidate_source(source_id)?;
        let mut state = lock_state(&self.shared);
        *state.source_epochs.entry(source_id.clone()).or_default() = state
            .source_epochs
            .get(source_id)
            .copied()
            .unwrap_or_default()
            .wrapping_add(1);
        state.decoded.invalidate_source(source_id);
        let invalidated = state
            .projections
            .iter()
            .filter(|(_, record)| record.source.source_id == *source_id)
            .map(|(request_id, _)| *request_id)
            .collect::<Vec<_>>();
        for request_id in &invalidated {
            state.projections.remove(request_id);
        }
        for record in state.jobs.values_mut() {
            if record.source.source_id == *source_id {
                record
                    .subscribers
                    .retain(|request_id| !invalidated.contains(request_id));
            }
        }
        let removable = state
            .jobs
            .iter()
            .filter(|(_, record)| {
                record.source.source_id == *source_id
                    && !record.active
                    && record.subscribers.is_empty()
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in &removable {
            state.jobs.remove(key);
        }
        state.visible.retain(|key| !removable.contains(key));
        drop(state);
        drop(commit);
        for request_id in invalidated {
            send_event(&self.shared, ArtworkEvent::Invalidated(request_id));
        }
        Ok(())
    }

    pub(crate) fn resolve_public_album_url(
        &self,
        candidates: &CandidateSet,
        size: u32,
        external: &ExternalPolicy,
    ) -> Result<Option<String>, String> {
        self.shared.fetch.public_url(candidates, size, external)
    }
}

impl DecodedCache {
    fn get(&mut self, key: &ArtworkKey) -> Option<Arc<DecodedImage>> {
        let index = self.entries.iter().position(|entry| &entry.key == key)?;
        let entry = self.entries.remove(index)?;
        let image = Arc::clone(&entry.image);
        self.entries.push_back(entry);
        Some(image)
    }

    fn insert(&mut self, key: ArtworkKey, source_id: SourceId, image: Arc<DecodedImage>) {
        if let Some(index) = self.entries.iter().position(|entry| entry.key == key)
            && let Some(previous) = self.entries.remove(index)
        {
            self.bytes = self.bytes.saturating_sub(previous.image.rgba().len());
        }
        self.bytes = self.bytes.saturating_add(image.rgba().len());
        self.entries.push_back(DecodedEntry {
            key,
            source_id,
            image,
        });
        while self.entries.len() > MAX_DECODED_IMAGES || self.bytes > MAX_DECODED_BYTES {
            let Some(removed) = self.entries.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(removed.image.rgba().len());
        }
    }

    fn invalidate_source(&mut self, source_id: &SourceId) {
        self.entries.retain(|entry| {
            if entry.source_id == *source_id {
                self.bytes = self.bytes.saturating_sub(entry.image.rgba().len());
                false
            } else {
                true
            }
        });
    }
}

fn enqueue(
    state: &mut State,
    source: SourceImages,
    request: ArtworkRequest,
    subscriber: Option<RequestId>,
) -> Result<(), ArtworkError> {
    let source_epoch = source_epoch(state, &source.source_id);
    let external_epoch = request
        .candidates
        .has_external()
        .then_some(state.external_epoch)
        .unwrap_or_default();
    let key = job_key(&source, &request, source_epoch, external_epoch);
    if let Some(record) = state.jobs.get_mut(&key) {
        if let Some(subscriber) = subscriber
            && !record.subscribers.contains(&subscriber)
        {
            record.subscribers.push(subscriber);
        }
        return Ok(());
    }
    if state.jobs.len() >= MAX_JOBS {
        return Err(ArtworkError::Busy);
    }
    let subscribers = subscriber.into_iter().collect();
    state.jobs.insert(
        key.clone(),
        JobRecord {
            source,
            request,
            subscribers,
            active: false,
            source_epoch,
            external_epoch,
        },
    );
    state.visible.push_back(key);
    Ok(())
}

fn job_key(
    source: &SourceImages,
    request: &ArtworkRequest,
    source_epoch: u64,
    external_epoch: u64,
) -> JobKey {
    let identity = format!(
        "{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{:x}",
        source.source_id,
        request.candidates.stable_identity(),
        request.fetch_size,
        request.render_size,
        request.external.allow_cached,
        request.external.allow_network,
        request.external.allow_musicbrainz,
        source.provider.is_some(),
        source_epoch,
        md5::compute(request.external.lastfm_api_key.as_bytes())
    );
    JobKey(format!(
        "{:x}-{external_epoch}",
        md5::compute(identity.as_bytes())
    ))
}

fn source_epoch(state: &State, source_id: &SourceId) -> u64 {
    state
        .source_epochs
        .get(source_id)
        .copied()
        .unwrap_or_default()
}

fn run_worker(shared: Arc<Shared>) {
    loop {
        let work = next_work(&shared);
        let resolution = resolve(&shared, &work);
        finish(&shared, work, resolution);
    }
}

fn next_work(shared: &Shared) -> Work {
    let mut state = lock_state(shared);
    loop {
        let key = state.visible.pop_front();
        if let Some(key) = key
            && let Some(record) = state.jobs.get_mut(&key)
            && !record.active
        {
            record.active = true;
            return Work {
                key,
                source: record.source.clone(),
                request: record.request.clone(),
                source_epoch: record.source_epoch,
                external_epoch: record.external_epoch,
            };
        }
        state = shared
            .wake
            .wait(state)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
}

fn resolve(shared: &Shared, work: &Work) -> Resolution {
    let artwork_key = shared
        .cache
        .artwork_key(&work.source.source_id, &work.request);
    let mut failures = Vec::new();
    for candidate in work.request.candidates.candidates() {
        let external = candidate.is_external();
        let may_read_cache = !external || work.request.external.allow_cached;
        if may_read_cache {
            if let Some(entry) =
                shared
                    .cache
                    .ready_entry(&work.source.source_id, candidate, work.request.fetch_size)
            {
                match decode_cached(
                    entry.path.clone(),
                    artwork_key.clone(),
                    work.request.render_size,
                ) {
                    Ok(image) => return Resolution::Ready(Arc::new(image)),
                    Err(error) => {
                        shared.cache.remove_ready(&entry.path);
                        failures.push(error.to_string());
                    }
                }
            }
            if shared
                .cache
                .is_missing(&work.source.source_id, candidate, work.request.fetch_size)
            {
                continue;
            }
        }
        if external && !work.request.external.allow_network {
            continue;
        }
        if work.source.provider.is_none() {
            continue;
        }
        match shared.fetch.fetch(
            &shared.runtime,
            &work.source,
            candidate,
            work.request.fetch_size,
            &work.request.external,
        ) {
            Ok(FetchOutcome::Ready(bytes)) => {
                let normalized = match normalize_for_cache(bytes, work.request.fetch_size) {
                    Ok(normalized) => normalized,
                    Err(error) => {
                        failures.push(error.to_string());
                        continue;
                    }
                };
                match write_ready_if_current(shared, work, candidate, normalized.bytes()) {
                    Ok(Some(path)) => {
                        match decode_normalized(
                            normalized,
                            path.clone(),
                            artwork_key.clone(),
                            work.request.render_size,
                        ) {
                            Ok(image) => return Resolution::Ready(Arc::new(image)),
                            Err(error) => {
                                shared.cache.remove_ready(&path);
                                failures.push(error.to_string());
                            }
                        }
                    }
                    Ok(None) => return Resolution::Missing,
                    Err(error) => failures.push(error.to_string()),
                }
            }
            Ok(FetchOutcome::Missing) => match mark_missing_if_current(shared, work, candidate) {
                Ok(true) => {}
                Ok(false) => return Resolution::Missing,
                Err(error) => failures.push(error.to_string()),
            },
            Err(error) => failures.push(error),
        }
    }
    if failures.is_empty() {
        Resolution::Missing
    } else {
        Resolution::Failed(failures.join("; ").into())
    }
}

fn write_ready_if_current(
    shared: &Shared,
    work: &Work,
    candidate: &Candidate,
    bytes: &[u8],
) -> std::io::Result<Option<std::path::PathBuf>> {
    let _commit = lock_cache_commit(shared);
    {
        let state = lock_state(shared);
        if !work_is_current(&state, work) {
            return Ok(None);
        }
    }
    shared
        .cache
        .write_ready(
            &work.source.source_id,
            candidate,
            work.request.fetch_size,
            bytes,
        )
        .map(Some)
}

fn mark_missing_if_current(
    shared: &Shared,
    work: &Work,
    candidate: &Candidate,
) -> std::io::Result<bool> {
    let _commit = lock_cache_commit(shared);
    {
        let state = lock_state(shared);
        if !work_is_current(&state, work) {
            return Ok(false);
        }
    }
    shared
        .cache
        .mark_missing(&work.source.source_id, candidate, work.request.fetch_size)?;
    Ok(true)
}

fn work_is_current(state: &State, work: &Work) -> bool {
    source_epoch(state, &work.source.source_id) == work.source_epoch
        && (!work.request.candidates.has_external() || state.external_epoch == work.external_epoch)
}

fn finish(shared: &Shared, work: Work, resolution: Resolution) {
    let mut state = lock_state(shared);
    let Some(record) = state.jobs.remove(&work.key) else {
        return;
    };
    if source_epoch(&state, &work.source.source_id) != work.source_epoch {
        return;
    }
    if work.request.candidates.has_external() && state.external_epoch != work.external_epoch {
        let subscribers = record.subscribers;
        for request_id in subscribers {
            if let Some(projection) = state.projections.get_mut(&request_id) {
                projection.projection.readiness = Readiness::Pending;
                let source = projection.source.clone();
                let request = projection.request.clone();
                let _ = enqueue(&mut state, source, request, Some(request_id));
            }
        }
        drop(state);
        shared.wake.notify_all();
        return;
    }
    if let Resolution::Ready(image) = &resolution {
        state.decoded.insert(
            image.key().clone(),
            work.source.source_id.clone(),
            Arc::clone(image),
        );
    }
    let mut events = Vec::new();
    let mut finished = Vec::new();
    for request_id in record.subscribers {
        let Some(projection) = state.projections.get_mut(&request_id) else {
            continue;
        };
        projection.projection.readiness = match &resolution {
            Resolution::Ready(image) => Readiness::Ready(Arc::clone(image)),
            Resolution::Missing => Readiness::Missing,
            Resolution::Failed(error) => Readiness::Failed(Arc::clone(error)),
        };
        events.push(ArtworkEvent::Changed(projection.projection.clone()));
        finished.push(request_id);
    }
    for request_id in &finished {
        state.projections.remove(request_id);
    }
    drop(state);
    send_events(shared, events);
}

fn lock_state(shared: &Shared) -> MutexGuard<'_, State> {
    shared
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock_cache_commit(shared: &Shared) -> MutexGuard<'_, ()> {
    shared
        .cache_commit
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn send_event(shared: &Shared, event: ArtworkEvent) {
    let _ = shared.events.send(event);
}

fn send_events(shared: &Shared, events: Vec<ArtworkEvent>) {
    for event in events {
        send_event(shared, event);
    }
}
