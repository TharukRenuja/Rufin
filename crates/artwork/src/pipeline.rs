use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;

use async_channel::{Receiver, Sender, unbounded};
use library::SourceId;
use tokio::runtime::Runtime;
use tracing::info;

use crate::cache::FilesystemCache;
use crate::decode::{decode_cached, decode_normalized, normalize_for_cache};
use crate::fetch::{FetchContext, FetchOutcome};
use crate::selection::Candidate;
use crate::{
    ArtworkBinding, ArtworkBindingIdentity, ArtworkError, ArtworkEvent, ArtworkKey,
    ArtworkProjection, ArtworkRequest, ArtworkRequestIdentity, ArtworkVisualIdentity, DecodedImage,
    ExternalPolicy, PrefetchOwner, PrefetchPriority, Readiness, RequestId, SourceImages,
};

pub(crate) const WORKERS: usize = 4;
const MAX_JOBS: usize = 256;
const MAX_ACTIVE_WITHOUT_DEMAND: usize = WORKERS - 1;
const MAX_ACTIVE_IDLE: usize = 1;
const MAX_BACKGROUND_JOBS: usize = 12;
const MAX_IDLE_JOBS: usize = 1;
const MAX_PREFETCH_INPUTS: usize = 4_096;
const MAX_DECODED_IMAGES: usize = 4_096;
const MAX_DECODED_BYTES: usize = 128 * 1024 * 1024;

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
    next_prefetch_owner: u64,
    external_epoch: u64,
    source_epochs: HashMap<SourceId, u64>,
    demand: VecDeque<JobKey>,
    viewport: VecDeque<JobKey>,
    background: VecDeque<JobKey>,
    idle: VecDeque<JobKey>,
    viewport_refill: VecDeque<PrefetchOwner>,
    background_refill: VecDeque<PrefetchOwner>,
    idle_refill: VecDeque<PrefetchOwner>,
    viewport_paused: bool,
    background_paused: bool,
    idle_paused: bool,
    active_jobs: usize,
    active_idle: usize,
    idle_completed: u64,
    idle_decoded_declines: u64,
    prefetch_batches: HashMap<PrefetchOwner, PrefetchBatch>,
    jobs: HashMap<JobKey, JobRecord>,
    projections: HashMap<RequestId, ProjectionRecord>,
    decoded: DecodedCache,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct JobKey(String);

struct JobRecord {
    source: SourceImages,
    request: ArtworkRequest,
    subscribers: HashSet<RequestId>,
    prefetch_owners: HashMap<PrefetchOwner, PrefetchPriority>,
    active: bool,
    source_epoch: u64,
    external_epoch: u64,
}

#[derive(Clone)]
struct Work {
    key: JobKey,
    priority: JobPriority,
    source: SourceImages,
    request: ArtworkRequest,
    source_epoch: u64,
    external_epoch: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum JobPriority {
    Demand,
    Viewport,
    Background,
    Idle,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum DecodedPriority {
    Warm,
    Foreground,
}

struct PrefetchBatch {
    priority: PrefetchPriority,
    pending: VecDeque<PrefetchInput>,
    jobs: HashSet<JobKey>,
    refill_queued: bool,
}

enum PrefetchAdmission {
    Enqueued,
    Backpressured,
    WarmCapacityExhausted,
}

#[derive(Clone)]
struct PrefetchInput {
    source: SourceImages,
    request: ArtworkRequest,
}

struct ProjectionRecord {
    projection: ArtworkProjection,
    source: SourceImages,
    job: JobKey,
}

#[derive(Default)]
struct DecodedCache {
    entries: HashMap<ArtworkKey, DecodedEntry>,
    families: HashMap<ArtworkVisualIdentity, BTreeMap<u32, HashSet<ArtworkKey>>>,
    eviction_order: BTreeSet<DecodedAccess>,
    bytes: usize,
    next_access: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
}

struct DecodedEntry {
    source_id: SourceId,
    family: ArtworkVisualIdentity,
    render_size: u32,
    image: Arc<DecodedImage>,
    priority: DecodedPriority,
    last_used: u64,
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
struct DecodedAccess {
    priority: DecodedPriority,
    last_used: u64,
    key: ArtworkKey,
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
        let (events, receiver) = unbounded();
        let shared = Arc::new(Shared {
            runtime,
            cache,
            fetch,
            cache_commit: Mutex::new(()),
            state: Mutex::new(State {
                next_request: 1,
                next_prefetch_owner: 1,
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
        if let Some(image) = decoded_for_request(
            &mut state,
            &self.shared.cache,
            &source,
            &request,
            DecodedPriority::Foreground,
        ) {
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
        let job = enqueue_demand(&mut state, source.clone(), request.clone(), request_id)?;
        state.projections.insert(
            request_id,
            ProjectionRecord {
                projection: projection.clone(),
                source,
                job,
            },
        );
        drop(state);
        self.shared.wake.notify_one();
        Ok(projection)
    }

    pub(crate) fn allocate_prefetch_owner(&self) -> PrefetchOwner {
        let mut state = lock_state(&self.shared);
        let owner = PrefetchOwner(state.next_prefetch_owner);
        state.next_prefetch_owner = state.next_prefetch_owner.wrapping_add(1).max(1);
        owner
    }

    pub(crate) fn replace_prefetch(
        &self,
        owner: PrefetchOwner,
        priority: PrefetchPriority,
        source: SourceImages,
        requests: Vec<ArtworkRequest>,
    ) {
        let mut state = lock_state(&self.shared);
        let mut desired = Vec::new();
        for request in coalesce_prefetch_requests(requests) {
            let decoded_priority = match priority {
                PrefetchPriority::Viewport => DecodedPriority::Foreground,
                PrefetchPriority::Background | PrefetchPriority::Idle => DecodedPriority::Warm,
            };
            if decoded_for_request(
                &mut state,
                &self.shared.cache,
                &source,
                &request,
                decoded_priority,
            )
            .is_some()
            {
                continue;
            }
            let source_epoch = source_epoch(&state, &source.source_id);
            let external_epoch = request
                .binding
                .has_external()
                .then_some(state.external_epoch)
                .unwrap_or_default();
            let key = job_key(&source, &request, source_epoch, external_epoch);
            desired.push((key, request));
        }

        remove_prefetch_refill_owner(&mut state, owner);
        let previous_jobs = state
            .prefetch_batches
            .remove(&owner)
            .map(|batch| batch.jobs)
            .unwrap_or_default();
        state.prefetch_batches.insert(
            owner,
            PrefetchBatch {
                priority,
                pending: VecDeque::new(),
                jobs: HashSet::new(),
                refill_queued: false,
            },
        );

        let mut affected = HashSet::new();
        for key in previous_jobs {
            if state
                .jobs
                .get_mut(&key)
                .is_some_and(|record| record.prefetch_owners.remove(&owner).is_some())
            {
                affected.insert(key);
            }
        }
        for (key, request) in desired {
            if let Some(changed) = attach_prefetch_owner(&mut state, &key, owner, priority) {
                if changed {
                    affected.insert(key);
                }
                continue;
            }
            match enqueue_prefetch_owner(&mut state, &source, &request, owner, priority) {
                PrefetchAdmission::Enqueued => {}
                PrefetchAdmission::Backpressured => {
                    backlog_prefetch(&mut state, owner, source.clone(), request);
                }
                PrefetchAdmission::WarmCapacityExhausted => {
                    backlog_prefetch(&mut state, owner, source.clone(), request);
                }
            }
        }
        for key in affected {
            reschedule_or_remove(&mut state, &key, false);
        }
        refill_prefetch(&mut state, &self.shared.cache);
        if priority == PrefetchPriority::Idle {
            log_memory_snapshot(&state, "idle_prefetch_replaced");
        }
        drop(state);
        self.shared.wake.notify_all();
    }

    pub(crate) fn clear_prefetch(&self, owner: PrefetchOwner) {
        let mut state = lock_state(&self.shared);
        remove_prefetch_refill_owner(&mut state, owner);
        let jobs = state
            .prefetch_batches
            .remove(&owner)
            .map(|batch| batch.jobs)
            .unwrap_or_default();
        for key in jobs {
            if let Some(record) = state.jobs.get_mut(&key) {
                record.prefetch_owners.remove(&owner);
            }
            reschedule_or_remove(&mut state, &key, false);
        }
        refill_prefetch(&mut state, &self.shared.cache);
        drop(state);
        self.shared.wake.notify_all();
    }

    pub(crate) fn set_prefetch_paused(&self, priority: PrefetchPriority, paused: bool) {
        let mut state = lock_state(&self.shared);
        match priority {
            PrefetchPriority::Viewport => state.viewport_paused = paused,
            PrefetchPriority::Background => state.background_paused = paused,
            PrefetchPriority::Idle => state.idle_paused = paused,
        }
        drop(state);
        self.shared.wake.notify_all();
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
        let Some(projection) = state.projections.remove(&request_id) else {
            return;
        };
        if let Some(record) = state.jobs.get_mut(&projection.job) {
            record.subscribers.remove(&request_id);
        }
        reschedule_or_remove(&mut state, &projection.job, false);
        refill_prefetch(&mut state, &self.shared.cache);
        drop(state);
        self.shared.wake.notify_all();
    }

    pub(crate) fn cache_only_file(
        &self,
        source_id: &SourceId,
        request: &ArtworkRequest,
    ) -> Option<std::path::PathBuf> {
        for candidate in request.binding.candidates() {
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
        binding_identity_from_state(&state, source, request)
    }

    pub(crate) fn binding_identity_and_ready(
        &self,
        source: &SourceImages,
        request: &ArtworkRequest,
    ) -> (ArtworkBindingIdentity, Option<Arc<DecodedImage>>) {
        let mut state = lock_state(&self.shared);
        binding_identity_and_ready_from_state(&mut state, &self.shared.cache, source, request)
    }

    pub(crate) fn retry_external(&self) -> Result<(), ArtworkError> {
        let commit = lock_cache_commit(&self.shared);
        self.shared.cache.retry_external()?;
        let mut state = lock_state(&self.shared);
        state.external_epoch = state.external_epoch.wrapping_add(1);
        reconcile_inactive_external_jobs(&mut state);
        refill_prefetch(&mut state, &self.shared.cache);
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
            .collect::<HashSet<_>>();
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
            .filter(|(_, record)| record.source.source_id == *source_id && !record.active)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in &removable {
            remove_job(&mut state, key);
        }
        for batch in state.prefetch_batches.values_mut() {
            batch
                .pending
                .retain(|input| input.source.source_id != *source_id);
        }
        refill_prefetch(&mut state, &self.shared.cache);
        drop(state);
        drop(commit);
        for request_id in invalidated {
            send_event(&self.shared, ArtworkEvent::Invalidated(request_id));
        }
        Ok(())
    }

    pub(crate) fn resolve_public_album_url(
        &self,
        candidates: &ArtworkBinding,
        size: u32,
        external: &ExternalPolicy,
    ) -> Result<Option<String>, String> {
        self.shared.fetch.public_url(candidates, size, external)
    }
}

fn binding_identity_from_state(
    state: &State,
    source: &SourceImages,
    request: &ArtworkRequest,
) -> ArtworkBindingIdentity {
    let source_epoch = source_epoch(state, &source.source_id);
    let external_epoch = request
        .binding
        .has_external()
        .then_some(state.external_epoch)
        .unwrap_or_default();
    let visual = artwork_visual_identity(source, request, source_epoch);
    let request = job_key(source, request, source_epoch, external_epoch);
    ArtworkBindingIdentity {
        visual,
        request: ArtworkRequestIdentity::new(request.0),
    }
}

fn binding_identity_and_ready_from_state(
    state: &mut State,
    cache: &FilesystemCache,
    source: &SourceImages,
    request: &ArtworkRequest,
) -> (ArtworkBindingIdentity, Option<Arc<DecodedImage>>) {
    let identity = binding_identity_from_state(state, source, request);
    let exact = cache.artwork_key(&source.source_id, request);
    let ready = state.decoded.get_for_request(
        &exact,
        &identity.visual,
        request.render_size,
        DecodedPriority::Foreground,
    );
    (identity, ready)
}

fn artwork_visual_identity(
    source: &SourceImages,
    request: &ArtworkRequest,
    source_epoch: u64,
) -> ArtworkVisualIdentity {
    let cached_external = request.binding.has_external() && request.external.allow_cached;
    ArtworkVisualIdentity::new(format!(
        "{}\0{}\0{}\0{}",
        source.source_id,
        request.binding.stable_identity(),
        cached_external,
        source_epoch,
    ))
}

fn decoded_for_request(
    state: &mut State,
    cache: &FilesystemCache,
    source: &SourceImages,
    request: &ArtworkRequest,
    priority: DecodedPriority,
) -> Option<Arc<DecodedImage>> {
    let source_epoch = source_epoch(state, &source.source_id);
    let family = artwork_visual_identity(source, request, source_epoch);
    let exact = cache.artwork_key(&source.source_id, request);
    state
        .decoded
        .get_for_request(&exact, &family, request.render_size, priority)
}

impl DecodedCache {
    fn get(&mut self, key: &ArtworkKey, priority: DecodedPriority) -> Option<Arc<DecodedImage>> {
        let last_used = self.next_access();
        let (previous_access, priority, image) = {
            let entry = self.entries.get_mut(key)?;
            let previous_access = DecodedAccess {
                priority: entry.priority,
                last_used: entry.last_used,
                key: key.clone(),
            };
            entry.last_used = last_used;
            if priority == DecodedPriority::Foreground {
                entry.priority = DecodedPriority::Foreground;
            }
            (previous_access, entry.priority, Arc::clone(&entry.image))
        };
        self.eviction_order.remove(&previous_access);
        self.eviction_order.insert(DecodedAccess {
            priority,
            last_used,
            key: key.clone(),
        });
        Some(image)
    }

    fn get_for_request(
        &mut self,
        exact_key: &ArtworkKey,
        family: &ArtworkVisualIdentity,
        render_size: u32,
        priority: DecodedPriority,
    ) -> Option<Arc<DecodedImage>> {
        let result = if let Some(image) = self.get(exact_key, priority) {
            Some(image)
        } else {
            self.families
                .get(family)
                .and_then(|sizes| {
                    sizes
                        .range(render_size..)
                        .find_map(|(_, keys)| keys.iter().next().cloned())
                })
                .and_then(|reusable| self.get(&reusable, priority))
        };
        if result.is_some() {
            self.hits = self.hits.saturating_add(1);
        } else {
            self.misses = self.misses.saturating_add(1);
        }
        result
    }

    fn insert(
        &mut self,
        key: ArtworkKey,
        source_id: SourceId,
        family: ArtworkVisualIdentity,
        render_size: u32,
        image: Arc<DecodedImage>,
        priority: DecodedPriority,
    ) -> bool {
        self.insert_with_limits(
            key,
            source_id,
            family,
            render_size,
            image,
            priority,
            MAX_DECODED_IMAGES,
            MAX_DECODED_BYTES,
        )
    }

    fn insert_with_limits(
        &mut self,
        key: ArtworkKey,
        source_id: SourceId,
        family: ArtworkVisualIdentity,
        render_size: u32,
        image: Arc<DecodedImage>,
        priority: DecodedPriority,
        max_images: usize,
        max_bytes: usize,
    ) -> bool {
        let replaced_bytes = self
            .entries
            .get(&key)
            .map_or(0, |entry| entry.image.rgba().len());
        let next_images = self.entries.len() + usize::from(!self.entries.contains_key(&key));
        let next_bytes = self
            .bytes
            .saturating_sub(replaced_bytes)
            .saturating_add(image.rgba().len());
        if priority == DecodedPriority::Warm && (next_images > max_images || next_bytes > max_bytes)
        {
            return false;
        }
        self.remove_entry(&key);
        let last_used = self.next_access();
        self.bytes = self.bytes.saturating_add(image.rgba().len());
        self.families
            .entry(family.clone())
            .or_default()
            .entry(render_size)
            .or_default()
            .insert(key.clone());
        self.entries.insert(
            key.clone(),
            DecodedEntry {
                source_id,
                family,
                render_size,
                image,
                priority,
                last_used,
            },
        );
        self.eviction_order.insert(DecodedAccess {
            priority,
            last_used,
            key,
        });
        self.evict_to_limits(max_images, max_bytes);
        true
    }

    fn has_warm_capacity(&self, reserved_images: usize, reserved_bytes: usize) -> bool {
        self.entries
            .len()
            .saturating_add(reserved_images)
            .saturating_add(1)
            <= MAX_DECODED_IMAGES
            && self.bytes.saturating_add(reserved_bytes) <= MAX_DECODED_BYTES
    }

    fn invalidate_source(&mut self, source_id: &SourceId) {
        let stale = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.source_id == *source_id)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in stale {
            self.remove_entry(&key);
        }
    }

    fn remove_entry(&mut self, key: &ArtworkKey) -> Option<DecodedEntry> {
        let removed = self.entries.remove(key)?;
        self.eviction_order.remove(&DecodedAccess {
            priority: removed.priority,
            last_used: removed.last_used,
            key: key.clone(),
        });
        self.bytes = self.bytes.saturating_sub(removed.image.rgba().len());
        let remove_family = self.families.get_mut(&removed.family).is_some_and(|sizes| {
            if let Some(keys) = sizes.get_mut(&removed.render_size) {
                keys.remove(key);
                if keys.is_empty() {
                    sizes.remove(&removed.render_size);
                }
            }
            sizes.is_empty()
        });
        if remove_family {
            self.families.remove(&removed.family);
        }
        Some(removed)
    }

    fn next_access(&mut self) -> u64 {
        self.next_access = self.next_access.wrapping_add(1).max(1);
        self.next_access
    }

    fn evict_to_limits(&mut self, max_images: usize, max_bytes: usize) {
        while self.entries.len() > max_images || self.bytes > max_bytes {
            let Some(access) = self.eviction_order.first().cloned() else {
                break;
            };
            if self.remove_entry(&access.key).is_some() {
                self.evictions = self.evictions.saturating_add(1);
            }
        }
    }
}

impl From<PrefetchPriority> for JobPriority {
    fn from(priority: PrefetchPriority) -> Self {
        match priority {
            PrefetchPriority::Viewport => Self::Viewport,
            PrefetchPriority::Background => Self::Background,
            PrefetchPriority::Idle => Self::Idle,
        }
    }
}

fn reconcile_inactive_external_jobs(state: &mut State) {
    let stale = state
        .jobs
        .iter()
        .filter(|(_, record)| {
            !record.active
                && record.request.binding.has_external()
                && record.external_epoch != state.external_epoch
        })
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    let stale = stale
        .iter()
        .filter_map(|key| remove_job(state, key))
        .collect::<Vec<_>>();

    for record in stale {
        for request_id in record.subscribers.iter().copied() {
            let job = enqueue_demand(
                state,
                record.source.clone(),
                record.request.clone(),
                request_id,
            )
            .expect("removing stale artwork jobs reserves their replacement capacity");
            if let Some(projection) = state.projections.get_mut(&request_id) {
                projection.projection.readiness = Readiness::Pending;
                projection.job = job;
            }
        }
        for (owner, priority) in record.prefetch_owners {
            restore_prefetch_owner(state, &record.source, &record.request, owner, priority);
        }
    }
}

fn restore_prefetch_owner(
    state: &mut State,
    source: &SourceImages,
    request: &ArtworkRequest,
    owner: PrefetchOwner,
    priority: PrefetchPriority,
) {
    match enqueue_prefetch_owner(state, source, request, owner, priority) {
        PrefetchAdmission::Enqueued => {}
        PrefetchAdmission::Backpressured => {
            backlog_prefetch(state, owner, source.clone(), request.clone());
        }
        PrefetchAdmission::WarmCapacityExhausted => {
            backlog_prefetch(state, owner, source.clone(), request.clone());
        }
    }
}

fn enqueue_demand(
    state: &mut State,
    source: SourceImages,
    request: ArtworkRequest,
    subscriber: RequestId,
) -> Result<JobKey, ArtworkError> {
    let source_epoch = source_epoch(state, &source.source_id);
    let external_epoch = request
        .binding
        .has_external()
        .then_some(state.external_epoch)
        .unwrap_or_default();
    let key = job_key(&source, &request, source_epoch, external_epoch);
    if let Some(record) = state.jobs.get_mut(&key) {
        record.subscribers.insert(subscriber);
        let active = record.active;
        if !active {
            queue(state, key.clone(), JobPriority::Demand, true);
        }
        return Ok(key);
    }
    if !make_room(state, JobPriority::Demand) {
        return Err(ArtworkError::Busy);
    }
    let subscribers = HashSet::from([subscriber]);
    state.jobs.insert(
        key.clone(),
        JobRecord {
            source,
            request,
            subscribers,
            prefetch_owners: HashMap::new(),
            active: false,
            source_epoch,
            external_epoch,
        },
    );
    queue(state, key.clone(), JobPriority::Demand, false);
    Ok(key)
}

fn enqueue_prefetch_owner(
    state: &mut State,
    source: &SourceImages,
    request: &ArtworkRequest,
    owner: PrefetchOwner,
    priority: PrefetchPriority,
) -> PrefetchAdmission {
    if !state.prefetch_batches.contains_key(&owner) {
        return PrefetchAdmission::Backpressured;
    }
    let source_epoch = source_epoch(state, &source.source_id);
    let external_epoch = request
        .binding
        .has_external()
        .then_some(state.external_epoch)
        .unwrap_or_default();
    let key = job_key(source, request, source_epoch, external_epoch);
    if state.jobs.contains_key(&key) {
        let active = state.jobs.get(&key).is_some_and(|record| record.active);
        let changed = attach_prefetch_owner(state, &key, owner, priority)
            .expect("prefetch batch exists while attaching an owner");
        if !active && changed {
            reschedule_or_remove(state, &key, false);
        }
        return PrefetchAdmission::Enqueued;
    }
    if priority != PrefetchPriority::Viewport {
        let (background_jobs, idle_jobs, reserved_images, reserved_bytes) =
            prefetch_reservation(state);
        let lane_full = match priority {
            PrefetchPriority::Background => background_jobs >= MAX_BACKGROUND_JOBS,
            PrefetchPriority::Idle => idle_jobs >= MAX_IDLE_JOBS,
            PrefetchPriority::Viewport => false,
        };
        if lane_full {
            return PrefetchAdmission::Backpressured;
        }
        if priority == PrefetchPriority::Background {
            let request_bytes = estimated_decoded_bytes(request.render_size);
            if !state.decoded.has_warm_capacity(
                reserved_images,
                reserved_bytes.saturating_add(request_bytes),
            ) {
                return PrefetchAdmission::WarmCapacityExhausted;
            }
        }
    }
    if !make_room(state, priority.into()) {
        return PrefetchAdmission::Backpressured;
    }
    state.jobs.insert(
        key.clone(),
        JobRecord {
            source: source.clone(),
            request: request.clone(),
            subscribers: HashSet::new(),
            prefetch_owners: HashMap::new(),
            active: false,
            source_epoch,
            external_epoch,
        },
    );
    attach_prefetch_owner(state, &key, owner, priority)
        .expect("prefetch batch exists for a newly inserted job");
    queue(state, key.clone(), priority.into(), false);
    PrefetchAdmission::Enqueued
}

fn attach_prefetch_owner(
    state: &mut State,
    key: &JobKey,
    owner: PrefetchOwner,
    priority: PrefetchPriority,
) -> Option<bool> {
    if !state.prefetch_batches.contains_key(&owner) {
        return None;
    }
    let changed = state
        .jobs
        .get_mut(key)?
        .prefetch_owners
        .insert(owner, priority)
        != Some(priority);
    state
        .prefetch_batches
        .get_mut(&owner)
        .expect("checked prefetch batch")
        .jobs
        .insert(key.clone());
    Some(changed)
}

fn prefetch_reservation(state: &State) -> (usize, usize, usize, usize) {
    state.jobs.values().fold(
        (0usize, 0usize, 0usize, 0usize),
        |(background, idle, images, bytes), record| match desired_priority(record) {
            Some(JobPriority::Background) => (
                background.saturating_add(1),
                idle,
                images.saturating_add(1),
                bytes.saturating_add(estimated_decoded_bytes(record.request.render_size)),
            ),
            Some(JobPriority::Idle) => (background, idle.saturating_add(1), images, bytes),
            _ => (background, idle, images, bytes),
        },
    )
}

fn estimated_decoded_bytes(render_size: u32) -> usize {
    let size = render_size.max(1) as usize;
    size.saturating_mul(size).saturating_mul(4)
}

fn coalesce_prefetch_requests(requests: Vec<ArtworkRequest>) -> Vec<ArtworkRequest> {
    let mut positions = HashMap::<String, usize>::new();
    let mut coalesced = Vec::<ArtworkRequest>::new();
    for request in requests {
        let identity = prefetch_request_identity(&request);
        if let Some(position) = positions.get(&identity).copied() {
            let existing = &mut coalesced[position];
            existing.fetch_size = existing.fetch_size.max(request.fetch_size);
            existing.render_size = existing.render_size.max(request.render_size);
            continue;
        }
        if coalesced.len() == MAX_PREFETCH_INPUTS {
            continue;
        }
        positions.insert(identity, coalesced.len());
        coalesced.push(request);
    }
    coalesced
}

fn prefetch_request_identity(request: &ArtworkRequest) -> String {
    format!(
        "{}\0{}\0{}\0{}\0{:x}",
        request.binding.stable_identity(),
        request.external.allow_cached,
        request.external.allow_network,
        request.external.allow_musicbrainz,
        md5::compute(request.external.lastfm_api_key.as_bytes())
    )
}

fn backlog_prefetch(
    state: &mut State,
    owner: PrefetchOwner,
    source: SourceImages,
    request: ArtworkRequest,
) {
    if let Some(batch) = state.prefetch_batches.get_mut(&owner) {
        batch.pending.push_back(PrefetchInput { source, request });
        queue_prefetch_refill_owner(state, owner, false);
    }
}

fn refill_prefetch(state: &mut State, cache: &FilesystemCache) {
    loop {
        let pending = take_pending_prefetch(state, PrefetchPriority::Viewport)
            .or_else(|| take_pending_prefetch(state, PrefetchPriority::Background))
            .or_else(|| take_pending_prefetch(state, PrefetchPriority::Idle));
        let Some((owner, priority, input)) = pending else {
            return;
        };
        let decoded_priority = match priority {
            PrefetchPriority::Viewport => DecodedPriority::Foreground,
            PrefetchPriority::Background | PrefetchPriority::Idle => DecodedPriority::Warm,
        };
        if decoded_for_request(
            state,
            cache,
            &input.source,
            &input.request,
            decoded_priority,
        )
        .is_some()
        {
            continue;
        }
        match enqueue_prefetch_owner(state, &input.source, &input.request, owner, priority) {
            PrefetchAdmission::Enqueued => {}
            PrefetchAdmission::Backpressured => {
                if let Some(batch) = state.prefetch_batches.get_mut(&owner) {
                    batch.pending.push_front(input);
                    queue_prefetch_refill_owner(state, owner, false);
                }
                return;
            }
            PrefetchAdmission::WarmCapacityExhausted => {
                if let Some(batch) = state.prefetch_batches.get_mut(&owner) {
                    batch.pending.push_front(input);
                    queue_prefetch_refill_owner(state, owner, false);
                }
                return;
            }
        }
    }
}

fn take_pending_prefetch(
    state: &mut State,
    priority: PrefetchPriority,
) -> Option<(PrefetchOwner, PrefetchPriority, PrefetchInput)> {
    loop {
        let owner = refill_queue(state, priority).pop_front()?;
        let Some(batch) = state.prefetch_batches.get_mut(&owner) else {
            continue;
        };
        if batch.priority != priority || !batch.refill_queued {
            continue;
        }
        batch.refill_queued = false;
        let Some(input) = batch.pending.pop_front() else {
            continue;
        };
        if !batch.pending.is_empty() {
            queue_prefetch_refill_owner(state, owner, false);
        }
        return Some((owner, priority, input));
    }
}

fn queue_prefetch_refill_owner(state: &mut State, owner: PrefetchOwner, front: bool) {
    let Some(priority) = state.prefetch_batches.get_mut(&owner).and_then(|batch| {
        if batch.pending.is_empty() || batch.refill_queued {
            None
        } else {
            batch.refill_queued = true;
            Some(batch.priority)
        }
    }) else {
        return;
    };
    let queue = refill_queue(state, priority);
    if front {
        queue.push_front(owner);
    } else {
        queue.push_back(owner);
    }
}

fn remove_prefetch_refill_owner(state: &mut State, owner: PrefetchOwner) {
    state.viewport_refill.retain(|queued| *queued != owner);
    state.background_refill.retain(|queued| *queued != owner);
    state.idle_refill.retain(|queued| *queued != owner);
}

fn refill_queue(state: &mut State, priority: PrefetchPriority) -> &mut VecDeque<PrefetchOwner> {
    match priority {
        PrefetchPriority::Viewport => &mut state.viewport_refill,
        PrefetchPriority::Background => &mut state.background_refill,
        PrefetchPriority::Idle => &mut state.idle_refill,
    }
}

fn desired_priority(record: &JobRecord) -> Option<JobPriority> {
    if !record.subscribers.is_empty() {
        Some(JobPriority::Demand)
    } else if record
        .prefetch_owners
        .values()
        .any(|priority| *priority == PrefetchPriority::Viewport)
    {
        Some(JobPriority::Viewport)
    } else if record
        .prefetch_owners
        .values()
        .any(|priority| *priority == PrefetchPriority::Background)
    {
        Some(JobPriority::Background)
    } else if !record.prefetch_owners.is_empty() {
        Some(JobPriority::Idle)
    } else {
        None
    }
}

fn reschedule_or_remove(state: &mut State, key: &JobKey, demand_front: bool) {
    let Some((active, priority)) = state
        .jobs
        .get(key)
        .map(|record| (record.active, desired_priority(record)))
    else {
        remove_queued(state, key);
        return;
    };
    if active {
        remove_queued(state, key);
        return;
    }
    match priority {
        Some(priority) => queue(
            state,
            key.clone(),
            priority,
            demand_front && priority == JobPriority::Demand,
        ),
        None => {
            remove_job(state, key);
        }
    }
}

fn remove_job(state: &mut State, key: &JobKey) -> Option<JobRecord> {
    remove_queued(state, key);
    let record = state.jobs.remove(key)?;
    for owner in record.prefetch_owners.keys() {
        if let Some(batch) = state.prefetch_batches.get_mut(owner) {
            batch.jobs.remove(key);
        }
    }
    Some(record)
}

fn queue(state: &mut State, key: JobKey, priority: JobPriority, front: bool) {
    remove_queued(state, &key);
    let queue = match priority {
        JobPriority::Demand => &mut state.demand,
        JobPriority::Viewport => &mut state.viewport,
        JobPriority::Background => &mut state.background,
        JobPriority::Idle => &mut state.idle,
    };
    if front {
        queue.push_front(key);
    } else {
        queue.push_back(key);
    }
}

fn remove_queued(state: &mut State, key: &JobKey) {
    state.demand.retain(|queued| queued != key);
    state.viewport.retain(|queued| queued != key);
    state.background.retain(|queued| queued != key);
    state.idle.retain(|queued| queued != key);
}

fn make_room(state: &mut State, priority: JobPriority) -> bool {
    while state.jobs.len() >= MAX_JOBS {
        let evicted = match priority {
            JobPriority::Demand => {
                evict_queued_prefetch(state, JobPriority::Idle)
                    || evict_queued_prefetch(state, JobPriority::Background)
                    || evict_queued_prefetch(state, JobPriority::Viewport)
            }
            JobPriority::Viewport => {
                evict_queued_prefetch(state, JobPriority::Idle)
                    || evict_queued_prefetch(state, JobPriority::Background)
            }
            JobPriority::Background => evict_queued_prefetch(state, JobPriority::Idle),
            JobPriority::Idle => false,
        };
        if !evicted {
            return false;
        }
    }
    true
}

fn evict_queued_prefetch(state: &mut State, priority: JobPriority) -> bool {
    let queue = match priority {
        JobPriority::Demand => return false,
        JobPriority::Viewport => &mut state.viewport,
        JobPriority::Background => &mut state.background,
        JobPriority::Idle => &mut state.idle,
    };
    while let Some(key) = queue.pop_back() {
        let removable = state
            .jobs
            .get(&key)
            .is_some_and(|record| !record.active && record.subscribers.is_empty());
        if removable {
            let record = remove_job(state, &key).expect("queued prefetch job");
            for owner in record.prefetch_owners.keys().copied().collect::<Vec<_>>() {
                backlog_prefetch(state, owner, record.source.clone(), record.request.clone());
            }
            return true;
        }
    }
    false
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
        request.binding.stable_identity(),
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
        let queued = state
            .demand
            .pop_front()
            .map(|key| (key, JobPriority::Demand))
            .or_else(|| {
                (!state.viewport_paused && state.active_jobs < MAX_ACTIVE_WITHOUT_DEMAND)
                    .then(|| state.viewport.pop_front())
                    .flatten()
                    .map(|key| (key, JobPriority::Viewport))
            })
            .or_else(|| {
                (!state.background_paused && state.active_jobs < MAX_ACTIVE_WITHOUT_DEMAND)
                    .then(|| state.background.pop_front())
                    .flatten()
                    .map(|key| (key, JobPriority::Background))
            })
            .or_else(|| {
                (!state.idle_paused
                    && state.active_jobs < MAX_ACTIVE_WITHOUT_DEMAND
                    && state.active_idle < MAX_ACTIVE_IDLE)
                    .then(|| state.idle.pop_front())
                    .flatten()
                    .map(|key| (key, JobPriority::Idle))
            });
        if let Some((key, priority)) = queued {
            let eligible = state
                .jobs
                .get(&key)
                .is_some_and(|record| !record.active && desired_priority(record) == Some(priority));
            if !eligible {
                reschedule_or_remove(&mut state, &key, false);
                continue;
            }
            let record = state.jobs.get_mut(&key).expect("eligible artwork job");
            record.active = true;
            let work = Work {
                key,
                priority,
                source: record.source.clone(),
                request: record.request.clone(),
                source_epoch: record.source_epoch,
                external_epoch: record.external_epoch,
            };
            state.active_jobs += 1;
            if priority == JobPriority::Idle {
                state.active_idle += 1;
            }
            return work;
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
    for candidate in work.request.binding.candidates() {
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
        && (!work.request.binding.has_external() || state.external_epoch == work.external_epoch)
}

fn finish(shared: &Shared, work: Work, resolution: Resolution) {
    let mut state = lock_state(shared);
    let record = remove_job(&mut state, &work.key);
    state.active_jobs = state.active_jobs.saturating_sub(1);
    if work.priority == JobPriority::Idle {
        state.active_idle = state.active_idle.saturating_sub(1);
    }
    let Some(record) = record else {
        drop(state);
        shared.wake.notify_all();
        return;
    };
    if source_epoch(&state, &work.source.source_id) != work.source_epoch {
        refill_prefetch(&mut state, &shared.cache);
        drop(state);
        shared.wake.notify_all();
        return;
    }
    if work.request.binding.has_external() && state.external_epoch != work.external_epoch {
        for request_id in record.subscribers {
            if let Ok(job) = enqueue_demand(
                &mut state,
                record.source.clone(),
                record.request.clone(),
                request_id,
            ) && let Some(projection) = state.projections.get_mut(&request_id)
            {
                projection.projection.readiness = Readiness::Pending;
                projection.job = job;
            }
        }
        for (owner, priority) in record.prefetch_owners {
            restore_prefetch_owner(&mut state, &record.source, &record.request, owner, priority);
        }
        refill_prefetch(&mut state, &shared.cache);
        drop(state);
        shared.wake.notify_all();
        return;
    }
    let idle_owners = record
        .prefetch_owners
        .iter()
        .filter_map(|(owner, priority)| (*priority == PrefetchPriority::Idle).then_some(*owner))
        .collect::<Vec<_>>();
    if work.priority == JobPriority::Idle {
        state.idle_completed = state.idle_completed.saturating_add(1);
    }
    let mut log_idle_decline = false;
    if let Resolution::Ready(image) = &resolution {
        let family = artwork_visual_identity(&work.source, &work.request, work.source_epoch);
        let priority = if !record.subscribers.is_empty()
            || record
                .prefetch_owners
                .values()
                .any(|priority| *priority == PrefetchPriority::Viewport)
        {
            DecodedPriority::Foreground
        } else {
            DecodedPriority::Warm
        };
        let admitted = state.decoded.insert(
            image.key().clone(),
            work.source.source_id.clone(),
            family,
            work.request.render_size,
            Arc::clone(image),
            priority,
        );
        if work.priority == JobPriority::Idle && !admitted {
            state.idle_decoded_declines = state.idle_decoded_declines.saturating_add(1);
            log_idle_decline = state.idle_decoded_declines.is_power_of_two();
        }
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
    refill_prefetch(&mut state, &shared.cache);
    if log_idle_decline {
        log_memory_snapshot(&state, "idle_decoded_admission_declined");
    }
    if idle_owners.iter().any(|owner| {
        state.prefetch_batches.get(owner).is_some_and(|batch| {
            batch.priority == PrefetchPriority::Idle
                && batch.pending.is_empty()
                && batch.jobs.is_empty()
        })
    }) {
        log_memory_snapshot(&state, "idle_prefetch_completed");
    }
    drop(state);
    send_events(shared, events);
    shared.wake.notify_all();
}

fn log_memory_snapshot(state: &State, stage: &str) {
    if std::env::var_os("RUFIN_MEMORY_DEBUG").is_none() {
        return;
    }
    let idle_pending = state
        .prefetch_batches
        .values()
        .filter(|batch| batch.priority == PrefetchPriority::Idle)
        .map(|batch| batch.pending.len().saturating_add(batch.jobs.len()))
        .sum::<usize>();
    info!(
        stage,
        decoded_entries = state.decoded.entries.len(),
        decoded_bytes = state.decoded.bytes,
        decoded_hits = state.decoded.hits,
        decoded_misses = state.decoded.misses,
        decoded_evictions = state.decoded.evictions,
        idle_completed = state.idle_completed,
        idle_decoded_declines = state.idle_decoded_declines,
        idle_pending,
        active_jobs = state.active_jobs,
        queued_jobs = state.jobs.len(),
        "artwork memory snapshot"
    );
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
    let _ = shared.events.try_send(event);
}

fn send_events(shared: &Shared, events: Vec<ArtworkEvent>) {
    for event in events {
        send_event(shared, event);
    }
}

#[cfg(test)]
mod decoded_cache_tests {
    use super::*;
    use crate::decode::decoded_image_for_test;
    use library::ImageRef;

    fn key(value: &str) -> ArtworkKey {
        ArtworkKey(value.to_string())
    }

    fn image(key: &ArtworkKey, bytes: usize) -> Arc<DecodedImage> {
        Arc::new(decoded_image_for_test(key.clone(), bytes))
    }

    fn family(value: &str) -> ArtworkVisualIdentity {
        ArtworkVisualIdentity::new(value.to_string())
    }

    #[test]
    fn decoded_cache_hits_stay_bounded_and_protect_the_recent_entry() {
        let source_id = SourceId::new("source");
        let first = key("first");
        let second = key("second");
        let third = key("third");
        let mut cache = DecodedCache::default();

        cache.insert_with_limits(
            first.clone(),
            source_id.clone(),
            family("first"),
            96,
            image(&first, 4),
            DecodedPriority::Foreground,
            2,
            usize::MAX,
        );
        cache.insert_with_limits(
            second.clone(),
            source_id.clone(),
            family("second"),
            96,
            image(&second, 4),
            DecodedPriority::Foreground,
            2,
            usize::MAX,
        );
        for _ in 0..10_000 {
            assert!(cache.get(&first, DecodedPriority::Foreground).is_some());
        }
        assert_eq!(cache.eviction_order.len(), cache.entries.len());
        cache.insert_with_limits(
            third.clone(),
            source_id,
            family("third"),
            96,
            image(&third, 4),
            DecodedPriority::Foreground,
            2,
            usize::MAX,
        );

        assert!(cache.entries.contains_key(&first));
        assert!(!cache.entries.contains_key(&second));
        assert!(cache.entries.contains_key(&third));
    }

    #[test]
    fn decoded_cache_evicts_to_the_byte_ceiling_even_below_the_item_limit() {
        let source_id = SourceId::new("source");
        let first = key("first");
        let second = key("second");
        let mut cache = DecodedCache::default();

        cache.insert_with_limits(
            first.clone(),
            source_id.clone(),
            family("first"),
            96,
            image(&first, 8),
            DecodedPriority::Foreground,
            10,
            12,
        );
        cache.insert_with_limits(
            second.clone(),
            source_id,
            family("second"),
            96,
            image(&second, 8),
            DecodedPriority::Foreground,
            10,
            12,
        );

        assert_eq!(cache.bytes, 8);
        assert!(!cache.entries.contains_key(&first));
        assert!(cache.entries.contains_key(&second));
    }

    #[test]
    fn warm_insert_at_capacity_keeps_the_existing_warm_prefix() {
        let source_id = SourceId::new("source");
        let first = key("first");
        let second = key("second");
        let mut cache = DecodedCache::default();

        assert!(cache.insert_with_limits(
            first.clone(),
            source_id.clone(),
            family("first"),
            96,
            image(&first, 8),
            DecodedPriority::Warm,
            10,
            12,
        ));
        assert!(!cache.insert_with_limits(
            second.clone(),
            source_id,
            family("second"),
            96,
            image(&second, 8),
            DecodedPriority::Warm,
            10,
            12,
        ));

        assert_eq!(cache.bytes, 8);
        assert!(cache.entries.contains_key(&first));
        assert!(!cache.entries.contains_key(&second));
    }

    #[test]
    fn idle_prefetch_continues_at_decoded_capacity_but_background_waits() {
        let source_id = SourceId::new("source-capacity-lanes");
        let source = SourceImages::cache_only(source_id.clone());
        let mut state = State::default();
        for index in 0..MAX_DECODED_IMAGES {
            let entry_key = key(&format!("resident-{index}"));
            assert!(state.decoded.insert_with_limits(
                entry_key.clone(),
                source_id.clone(),
                family(&format!("resident-{index}")),
                1,
                image(&entry_key, 4),
                DecodedPriority::Warm,
                MAX_DECODED_IMAGES,
                MAX_DECODED_BYTES,
            ));
        }

        let idle_owner = PrefetchOwner(1);
        state.prefetch_batches.insert(
            idle_owner,
            PrefetchBatch {
                priority: PrefetchPriority::Idle,
                pending: VecDeque::new(),
                jobs: HashSet::new(),
                refill_queued: false,
            },
        );
        let idle_request = ArtworkRequest::new(
            ArtworkBinding::from_native(Some(&ImageRef::new("idle", None))),
            96,
            96,
        );
        assert!(matches!(
            enqueue_prefetch_owner(
                &mut state,
                &source,
                &idle_request,
                idle_owner,
                PrefetchPriority::Idle,
            ),
            PrefetchAdmission::Enqueued
        ));

        let background_owner = PrefetchOwner(2);
        state.prefetch_batches.insert(
            background_owner,
            PrefetchBatch {
                priority: PrefetchPriority::Background,
                pending: VecDeque::new(),
                jobs: HashSet::new(),
                refill_queued: false,
            },
        );
        let background_request = ArtworkRequest::new(
            ArtworkBinding::from_native(Some(&ImageRef::new("background", None))),
            96,
            96,
        );
        assert!(matches!(
            enqueue_prefetch_owner(
                &mut state,
                &source,
                &background_request,
                background_owner,
                PrefetchPriority::Background,
            ),
            PrefetchAdmission::WarmCapacityExhausted
        ));
    }

    #[test]
    fn foreground_insert_evicts_the_least_recent_warm_before_visible_artwork() {
        let source_id = SourceId::new("source");
        let visible = key("visible");
        let warm_recently_used = key("warm-recently-used");
        let warm_lru = key("warm-lru");
        let next_visible = key("next-visible");
        let mut cache = DecodedCache::default();

        cache.insert_with_limits(
            visible.clone(),
            source_id.clone(),
            family("visible"),
            96,
            image(&visible, 4),
            DecodedPriority::Foreground,
            3,
            usize::MAX,
        );
        cache.insert_with_limits(
            warm_recently_used.clone(),
            source_id.clone(),
            family("warm-recently-used"),
            96,
            image(&warm_recently_used, 4),
            DecodedPriority::Warm,
            3,
            usize::MAX,
        );
        cache.insert_with_limits(
            warm_lru.clone(),
            source_id.clone(),
            family("warm-lru"),
            96,
            image(&warm_lru, 4),
            DecodedPriority::Warm,
            3,
            usize::MAX,
        );
        assert!(
            cache
                .get(&warm_recently_used, DecodedPriority::Warm)
                .is_some()
        );
        cache.insert_with_limits(
            next_visible.clone(),
            source_id,
            family("next-visible"),
            96,
            image(&next_visible, 4),
            DecodedPriority::Foreground,
            3,
            usize::MAX,
        );

        assert!(cache.entries.contains_key(&visible));
        assert!(cache.entries.contains_key(&warm_recently_used));
        assert!(!cache.entries.contains_key(&warm_lru));
        assert!(cache.entries.contains_key(&next_visible));
    }

    #[test]
    fn foreground_hit_promotes_warmed_artwork_before_later_warm_entries() {
        let source_id = SourceId::new("source");
        let promoted = key("promoted");
        let newer_warm = key("newer-warm");
        let next_visible = key("next-visible");
        let mut cache = DecodedCache::default();

        cache.insert_with_limits(
            promoted.clone(),
            source_id.clone(),
            family("promoted"),
            96,
            image(&promoted, 4),
            DecodedPriority::Warm,
            2,
            usize::MAX,
        );
        assert!(cache.get(&promoted, DecodedPriority::Foreground).is_some());
        cache.insert_with_limits(
            newer_warm.clone(),
            source_id.clone(),
            family("newer-warm"),
            96,
            image(&newer_warm, 4),
            DecodedPriority::Warm,
            2,
            usize::MAX,
        );
        cache.insert_with_limits(
            next_visible.clone(),
            source_id,
            family("next-visible"),
            96,
            image(&next_visible, 4),
            DecodedPriority::Foreground,
            2,
            usize::MAX,
        );

        assert!(cache.entries.contains_key(&promoted));
        assert!(!cache.entries.contains_key(&newer_warm));
        assert!(cache.entries.contains_key(&next_visible));
    }

    #[test]
    fn decoded_cache_source_invalidation_preserves_other_sources_and_accounting() {
        let first_source = SourceId::new("first-source");
        let second_source = SourceId::new("second-source");
        let first = key("first");
        let second = key("second");
        let mut cache = DecodedCache::default();

        cache.insert_with_limits(
            first.clone(),
            first_source.clone(),
            family("first"),
            96,
            image(&first, 8),
            DecodedPriority::Foreground,
            10,
            usize::MAX,
        );
        cache.insert_with_limits(
            second.clone(),
            second_source,
            family("second"),
            96,
            image(&second, 12),
            DecodedPriority::Foreground,
            10,
            usize::MAX,
        );
        cache.invalidate_source(&first_source);

        assert_eq!(cache.bytes, 12);
        assert!(!cache.entries.contains_key(&first));
        assert!(cache.entries.contains_key(&second));
        assert_eq!(cache.eviction_order.len(), 1);
    }
}

#[cfg(test)]
mod prefetch_coalescing_tests {
    use super::*;
    use library::ImageRef;

    fn request(identity: &str, size: u32) -> ArtworkRequest {
        ArtworkRequest::new(
            ArtworkBinding::from_native(Some(&ImageRef::new(identity, None))),
            size,
            size,
        )
    }

    #[test]
    fn duplicate_prefix_does_not_consume_the_unique_prefetch_budget() {
        let repeated = request("shared-album-cover", 48);
        let larger_repeat = request("shared-album-cover", 256);
        let later_unique = request("later-artist-cover", 96);
        let mut requests = vec![repeated; MAX_PREFETCH_INPUTS];
        requests.push(later_unique.clone());
        requests.push(larger_repeat);

        let coalesced = coalesce_prefetch_requests(requests);

        assert_eq!(coalesced.len(), 2);
        assert_eq!(coalesced[0].fetch_size, 256);
        assert_eq!(coalesced[0].render_size, 256);
        assert_eq!(coalesced[1], later_unique);
    }
}
