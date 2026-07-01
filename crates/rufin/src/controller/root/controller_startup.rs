use super::local_library_stress::{self, LocalStressDelta, LocalStressSnapshot};
use super::*;
use library::AlbumIdentityCandidate;
use std::future::Future;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const SYNC_PROGRESS_MIN_INTERVAL: Duration = Duration::from_secs(2);
const SYNC_CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(250);
const RELEASE_TYPE_LOOKUP_LIMIT: usize = 500;
const SYNC_CANCELLED_ERROR: &str = "Sync cancelled.";
static RELEASE_TYPE_LOOKUPS_IN_FLIGHT: OnceLock<Mutex<HashSet<ServerId>>> = OnceLock::new();

pub(in crate::controller) fn check_sync_cancelled(
    cancellation: &CancellationToken,
) -> Result<(), String> {
    if cancellation.cancelled() {
        return Err(SYNC_CANCELLED_ERROR.to_string());
    }
    Ok(())
}

fn check_optional_sync_cancelled(cancellation: Option<&CancellationToken>) -> Result<(), String> {
    if let Some(cancellation) = cancellation {
        check_sync_cancelled(cancellation)?;
    }
    Ok(())
}

fn sync_result_was_cancelled(
    result: &Result<SyncJobOutcome, String>,
    cancellation: &CancellationToken,
) -> bool {
    cancellation.cancelled()
        || matches!(result, Err(error) if error.as_str() == SYNC_CANCELLED_ERROR)
}

fn sync_source_label(server: &ServerIdentity) -> String {
    let name = server.name.trim();
    if name.is_empty() {
        provider_display_name(&server.provider).to_string()
    } else {
        name.to_string()
    }
}

fn mark_sync_cancelled(store: &StoreHandle, server_id: &ServerId, generation: i64) {
    if let Err(error) = store.with_store(|store| store.cancel_sync(server_id, generation)) {
        warn!(%error, server_id = %server_id.as_str(), generation, "failed to mark sync cancelled");
    }
}

pub(in crate::controller) async fn await_provider<T, Fut>(
    cancellation: &CancellationToken,
    operation: Fut,
) -> Result<T, String>
where
    Fut: Future<Output = source::ProviderResult<T>>,
{
    tokio::pin!(operation);
    loop {
        check_sync_cancelled(cancellation)?;
        tokio::select! {
            result = &mut operation => return result.map_err(|error| error.to_string()),
            _ = tokio::time::sleep(SYNC_CANCEL_POLL_INTERVAL) => {}
        }
    }
}

async fn await_optional_provider<T, Fut>(
    cancellation: Option<&CancellationToken>,
    operation: Fut,
) -> Result<T, String>
where
    Fut: Future<Output = source::ProviderResult<T>>,
{
    match cancellation {
        Some(cancellation) => await_provider(cancellation, operation).await,
        None => operation.await.map_err(|error| error.to_string()),
    }
}

pub(in crate::controller) fn start_sync_thread(context: SyncContext, saved: SavedServer) {
    start_sync_thread_inner(context, saved, false, false, true);
}

pub(in crate::controller) fn start_background_sync_thread(
    context: SyncContext,
    saved: SavedServer,
) {
    if active_server_needs_sync(&context.store, &saved.server.id) {
        start_sync_thread_inner(context, saved, false, false, false);
    } else {
        start_cached_active_source_reconciliation_thread(context, saved);
    }
}

pub(in crate::controller) fn start_sync_thread_with_snapshots(
    context: SyncContext,
    saved: SavedServer,
) {
    start_sync_thread_inner(context, saved, true, false, true);
}

pub(in crate::controller) fn start_login_sync_thread(context: SyncContext, saved: SavedServer) {
    if sync_target_is_current(&context.store, &saved.server.id)
        && cached_library_exists(&context.store, &saved.server.id)
    {
        send_library_sync_status(
            &context.store,
            &context.events,
            &saved,
            "Cached library ready".to_string(),
            None,
            LibraryDelta::default(),
        );
        let _sent = context.events.send(ControllerEvent::LoginStatus(
            "Cached library ready".to_string(),
        ));
        return;
    }
    start_sync_thread_inner(context, saved, false, true, true);
}

pub(in crate::controller) fn start_sync_thread_inner(
    context: SyncContext,
    saved: SavedServer,
    force_snapshots: bool,
    completion_snapshot: bool,
    emit_progress: bool,
) {
    let server_id = saved.server.id.clone();
    let cached_current = sync_target_is_current(&context.store, &server_id)
        && cached_library_exists(&context.store, &server_id);
    let skip_sync_snapshots = !force_snapshots && !completion_snapshot && cached_current;
    let permit = match context.sync_in_flight.acquire(server_id.clone()) {
        Ok(Some(permit)) => permit,
        Ok(None) => {
            if emit_progress {
                let _sent = context.events.send(ControllerEvent::LoginStatus(
                    "Sync already running.".to_string(),
                ));
            }
            if force_snapshots {
                emit_runtime_snapshot(&context.store, &context.secrets, &context.events);
            }
            return;
        }
        Err(error) => {
            send_sync_error(
                &context.store,
                &context.events,
                &saved,
                error,
                skip_sync_snapshots,
            );
            return;
        }
    };
    let cancellation = permit.cancellation_token();

    let prefetch_initial_covers = initial_cover_cache_required(&context.store, &server_id);
    let generation = match context
        .store
        .with_store(|store| store.begin_sync(&server_id))
    {
        Ok(generation) => generation,
        Err(error) => {
            send_sync_error(
                &context.store,
                &context.events,
                &saved,
                error,
                skip_sync_snapshots,
            );
            return;
        }
    };
    if !skip_sync_snapshots && !completion_snapshot {
        emit_runtime_snapshot(&context.store, &context.secrets, &context.events);
    }

    thread::spawn(move || {
        let _permit = permit;
        let provider_name = sync_source_label(&saved.server);
        if emit_progress {
            let _sent = context.events.send(ControllerEvent::LoginStatus(format!(
                "Syncing {provider_name} library..."
            )));
        }
        let sync_result = if cancellation.cancelled() {
            Err(SYNC_CANCELLED_ERROR.to_string())
        } else {
            run_sync_job(
                &context,
                &saved,
                generation,
                prefetch_initial_covers,
                skip_sync_snapshots,
                emit_progress,
                &cancellation,
            )
        };
        if sync_result_was_cancelled(&sync_result, &cancellation) {
            mark_sync_cancelled(&context.store, &server_id, generation);
            return;
        }
        match sync_result {
            Ok(outcome) => {
                if !sync_target_is_current(&context.store, &server_id) {
                    return;
                }
                if cancellation.cancelled() {
                    mark_sync_cancelled(&context.store, &server_id, generation);
                    return;
                }
                if outcome.post_sync_work {
                    refresh_queue_refs(&context, &saved);
                    start_album_identity_lookup(&context, &saved);
                    covers::start_cover_prefetch(covers::ExternalCoverPrefetchRequest {
                        store: context.store.clone(),
                        runtime: Arc::clone(&context.runtime),
                        secrets: Arc::clone(&context.secrets),
                        events: context.events.clone(),
                        cover_in_flight: Arc::clone(&context.cover_in_flight),
                        external_cover_retry_generation: Arc::clone(
                            &context.external_cover_retry_generation,
                        ),
                        retry_generation: context
                            .external_cover_retry_generation
                            .load(Ordering::SeqCst),
                        external_cover_prefetch_in_flight: Arc::clone(
                            &context.external_cover_prefetch_in_flight,
                        ),
                        cover_slots: Arc::clone(&context.cover_slots),
                        saved: saved.clone(),
                    });
                }
                if skip_sync_snapshots {
                    send_library_sync_status(
                        &context.store,
                        &context.events,
                        &saved,
                        "Cached library ready".to_string(),
                        None,
                        outcome.delta,
                    );
                } else {
                    emit_sync_complete_snapshot(&context.store, &context.secrets, &context.events);
                }
            }
            Err(error) => {
                let _failed = context.store.with_store(|store| {
                    store.fail_sync(&saved.server.id, &error)?;
                    Ok(())
                });
                if !sync_target_is_current(&context.store, &server_id) {
                    return;
                }
                send_sync_error(
                    &context.store,
                    &context.events,
                    &saved,
                    error,
                    skip_sync_snapshots,
                );
            }
        }
    });
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::controller) struct SyncJobOutcome {
    pub(in crate::controller) delta: LibraryDelta,
    pub(in crate::controller) post_sync_work: bool,
}
impl SyncJobOutcome {
    fn unchanged() -> Self {
        Self {
            delta: LibraryDelta::default(),
            post_sync_work: false,
        }
    }

    fn changed(delta: LibraryDelta) -> Self {
        Self {
            delta,
            post_sync_work: true,
        }
    }
}

pub(in crate::controller) fn send_library_sync_status(
    store: &StoreHandle,
    events: &Sender<ControllerEvent>,
    saved: &SavedServer,
    sync_status: String,
    last_error: Option<String>,
    delta: LibraryDelta,
) {
    let server_id = &saved.server.id;
    let counts = load_library_counts(store, server_id).unwrap_or_else(|error| {
        warn!(%error, "failed to load library counts for sync update");
        LibraryCounts::default()
    });
    let home = if delta.home_changed {
        load_home_update(store, saved)
            .map(Some)
            .unwrap_or_else(|error| {
                warn!(%error, "failed to load home sections for sync update");
                None
            })
    } else {
        None
    };
    let _sent = events.send(ControllerEvent::LibrarySyncStatus(Box::new(
        LibrarySyncStatus {
            server_id: server_id.clone(),
            sync_status,
            last_error,
            counts,
            home,
            delta,
        },
    )));
}

fn send_sync_error(
    store: &StoreHandle,
    events: &Sender<ControllerEvent>,
    saved: &SavedServer,
    error: String,
    status_only: bool,
) {
    if status_only {
        send_library_sync_status(
            store,
            events,
            saved,
            "Action failed".to_string(),
            Some(error),
            LibraryDelta::default(),
        );
    } else {
        let _sent = events.send(ControllerEvent::Error(error));
    }
}

fn emit_sync_complete_snapshot(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    events: &Sender<ControllerEvent>,
) {
    let _sent = events.send(ControllerEvent::LoginStatus(
        "Library sync complete".to_string(),
    ));
    match load_runtime_snapshot(store, secrets) {
        Ok(snapshot) => {
            let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
        }
        Err(error) => {
            let _sent = events.send(ControllerEvent::Error(error));
        }
    }
}

fn start_album_identity_lookup(context: &SyncContext, saved: &SavedServer) {
    if saved.server.provider == "fake" {
        return;
    }
    let settings = load_settings_from_store(&context.store);
    if !external_metadata::enabled(&settings) {
        return;
    }
    let server_id = saved.server.id.clone();
    if !mark_release_type_lookup_in_flight(&server_id) {
        return;
    }

    let store = context.store.clone();
    let events = context.events.clone();
    thread::spawn(move || {
        let result = run_album_identity_lookup(&store, &events, &server_id);
        clear_release_type_lookup_in_flight(&server_id);
        if let Err(error) = result {
            warn!(%error, server_id = %server_id.as_str(), "failed to enrich album identity");
        }
    });
}

fn mark_release_type_lookup_in_flight(server_id: &ServerId) -> bool {
    RELEASE_TYPE_LOOKUPS_IN_FLIGHT
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|mut running| running.insert(server_id.clone()))
        .unwrap_or(false)
}

fn clear_release_type_lookup_in_flight(server_id: &ServerId) {
    if let Ok(mut running) = RELEASE_TYPE_LOOKUPS_IN_FLIGHT
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
    {
        running.remove(server_id);
    }
}

fn run_album_identity_lookup(
    store: &StoreHandle,
    events: &Sender<ControllerEvent>,
    server_id: &ServerId,
) -> Result<(), String> {
    let candidates = store.with_store(|store| {
        store.load_album_identity_candidates(server_id, RELEASE_TYPE_LOOKUP_LIMIT)
    })?;
    if candidates.is_empty() {
        return Ok(());
    }

    info!(
        server_id = %server_id.as_str(),
        candidate_count = candidates.len(),
        "started album identity enrichment"
    );
    let mut updated = Vec::new();
    let mut misses = 0_usize;
    let mut errors = 0_usize;
    for candidate in &candidates {
        if !sync_target_is_current(store, server_id) {
            break;
        }
        match lookup_album_identity(candidate) {
            Ok(metadata) => {
                store.with_store(|store| {
                    store.update_album_identity_metadata(
                        server_id,
                        &candidate.album_id,
                        &metadata.release_types,
                        metadata.is_compilation,
                    )
                })?;
                updated.push(candidate.album_id.clone());
            }
            Err(error) if external_metadata::is_expected_release_type_lookup_miss(&error) => {
                misses += 1;
                store.with_store(|store| {
                    store.save_album_identity_miss(
                        server_id,
                        &candidate.album_id,
                        &candidate.identity_key,
                        &error,
                    )
                })?;
            }
            Err(error) => {
                errors += 1;
                warn!(
                    %error,
                    album_id = %candidate.album_id.as_str(),
                    "failed to look up album identity"
                );
            }
        }
    }
    if !updated.is_empty() {
        let _sent = events.send(ControllerEvent::LibraryDelta(Box::new(LibraryDelta {
            albums: EntityDelta {
                fields: updated.clone(),
                ..EntityDelta::default()
            },
            ..LibraryDelta::default()
        })));
    }
    info!(
        server_id = %server_id.as_str(),
        updated = updated.len(),
        misses,
        errors,
        "completed album identity enrichment"
    );
    Ok(())
}

fn lookup_album_identity(
    candidate: &AlbumIdentityCandidate,
) -> Result<external_metadata::AlbumReleaseMetadata, String> {
    external_metadata::fetch_album_release_metadata(
        candidate.musicbrainz_release_group_id.as_deref(),
        candidate.musicbrainz_album_id.as_deref(),
    )
}

pub(in crate::controller) fn refresh_queue_refs(context: &SyncContext, saved: &SavedServer) {
    let Some(original_snapshot) = context
        .queue
        .lock()
        .ok()
        .and_then(|queue| queue.as_ref().map(QueueEngine::snapshot))
    else {
        return;
    };
    snapshot_queue_refs(context, saved, original_snapshot);
}

pub(in crate::controller) fn snapshot_queue_refs(
    context: &SyncContext,
    saved: &SavedServer,
    original_snapshot: QueueSnapshot,
) {
    if original_snapshot.server_id != saved.server.id {
        return;
    }
    let mut normalized_entries = original_snapshot.entries.clone();
    let settings = load_settings_for_saved(&context.store, saved);
    if let Err(error) = queue_album_refs(
        &context.store,
        &saved.server,
        &settings,
        &mut normalized_entries,
    ) {
        warn!(%error, "failed to refresh queue image refs after sync");
        return;
    }
    if normalized_entries == original_snapshot.entries {
        return;
    }
    let mut queue = match context.queue.lock() {
        Ok(queue) => queue,
        Err(error) => {
            warn!(%error, "failed to lock queue after local sync");
            return;
        }
    };
    let Some(current_snapshot) = queue.as_ref().map(QueueEngine::snapshot) else {
        return;
    };
    if !queue_snapshot_entries_match(&original_snapshot, &current_snapshot) {
        return;
    }
    let mut refreshed_snapshot = current_snapshot.clone();
    for (entry, normalized_entry) in refreshed_snapshot
        .entries
        .iter_mut()
        .zip(normalized_entries)
    {
        entry.image_ref = normalized_entry.image_ref;
    }
    if refreshed_snapshot.entries == current_snapshot.entries {
        return;
    }
    *queue = Some(QueueEngine::restore(refreshed_snapshot.clone()));
    drop(queue);

    defer_queue_snapshot(
        context.store.clone(),
        context.events.clone(),
        Arc::clone(&context.queue_persist_generation),
        refreshed_snapshot.clone(),
    );
    sync_queue_snapshot(
        &context.queue,
        &context.playback_snapshot,
        &context.auto_dj_enabled,
    );
    let _sent = context
        .events
        .send(ControllerEvent::Queue(Box::new(Some(refreshed_snapshot))));
    let playback = context
        .playback_snapshot
        .lock()
        .map(|snapshot| snapshot.clone())
        .unwrap_or_default();
    let _sent = context
        .events
        .send(ControllerEvent::Playback(Box::new(playback)));
}

fn queue_snapshot_entries_match(left: &QueueSnapshot, right: &QueueSnapshot) -> bool {
    left.server_id == right.server_id
        && left.entries.len() == right.entries.len()
        && left
            .entries
            .iter()
            .zip(&right.entries)
            .all(|(left, right)| {
                left.id == right.id
                    && left.track_id == right.track_id
                    && left.album_id == right.album_id
            })
}

pub(in crate::controller) fn sync_target_is_current(
    store: &StoreHandle,
    server_id: &ServerId,
) -> bool {
    store
        .with_store(|store| {
            Ok(store
                .active_server()?
                .is_some_and(|saved| saved.server.id == *server_id))
        })
        .unwrap_or(false)
}

pub(in crate::controller) fn start_home_refresh_thread(
    context: HomeRefreshContext,
    saved: SavedServer,
    target: HomeRefreshTarget,
) {
    if saved.server.provider == "fake" {
        return;
    }

    let server_id = saved.server.id.clone();
    if sync_is_running(&context.sync_in_flight, &server_id) {
        return;
    }
    let permit = match context.home_refresh_in_flight.acquire(server_id) {
        Ok(Some(permit)) => permit,
        Ok(None) => return,
        Err(error) => {
            let _sent = context.events.send(ControllerEvent::Error(error));
            return;
        }
    };

    thread::spawn(move || {
        let result = match target {
            HomeRefreshTarget::Section(kind) => refresh_home_section_for_saved(
                &context.store,
                &context.runtime,
                &context.secrets,
                &saved,
                kind,
            ),
        }
        .and_then(|()| load_snapshot(&context.store).map(Box::new));
        drop(permit);
        match result {
            Ok(snapshot) => {
                let _sent = context
                    .events
                    .send(home_refresh_completed_event(target, snapshot));
            }
            Err(error) => {
                warn!(%error, "failed to refresh home sections");
            }
        }
    });
}
pub(in crate::controller) fn home_refresh_completed_event(
    target: HomeRefreshTarget,
    snapshot: Box<LibrarySnapshot>,
) -> ControllerEvent {
    ControllerEvent::HomeSectionsUpdated {
        snapshot,
        include_explore: matches!(target, HomeRefreshTarget::Section(HomeSectionKind::Explore)),
    }
}
pub(in crate::controller) fn start_explore_prefetch_thread(
    context: ExplorePrefetchContext,
    saved: SavedServer,
) {
    if saved.server.provider == "fake" {
        return;
    }

    let server_id = saved.server.id.clone();
    if sync_is_running(&context.sync_in_flight, &server_id) {
        return;
    }
    let permit = match context
        .explore_prefetch_in_flight
        .acquire(server_id.clone())
    {
        Ok(Some(permit)) => permit,
        Ok(None) => return,
        Err(error) => {
            let _sent = context.events.send(ControllerEvent::Error(error));
            return;
        }
    };

    thread::spawn(move || {
        let result = prefetch_home_section_for_saved(
            &context.store,
            &context.runtime,
            &context.secrets,
            &saved,
            HomeSectionKind::Explore,
        );
        drop(permit);
        match result {
            Ok(section) => {
                let _sent = context
                    .events
                    .send(ControllerEvent::HomeSectionPrefetched { server_id, section });
            }
            Err(error) => {
                warn!(%error, "failed to prefetch Explore section");
            }
        }
    });
}
pub(in crate::controller) fn start_home_promotion(
    store: StoreHandle,
    events: Sender<ControllerEvent>,
    server_id: ServerId,
    section: HomeSection,
) {
    thread::spawn(move || {
        let result = promote_prefetched_home_section(&store, &server_id, &section)
            .and_then(|()| load_snapshot(&store).map(Box::new));
        match result {
            Ok(snapshot) => {
                let _sent = events.send(ControllerEvent::HomeSectionsUpdated {
                    snapshot,
                    include_explore: false,
                });
            }
            Err(error) => {
                warn!(%error, "failed to promote prefetched home section");
            }
        }
    });
}

pub(in crate::controller) fn initial_cover_cache_required(
    store: &StoreHandle,
    server_id: &ServerId,
) -> bool {
    if server_id.as_str() == LOCAL_SOURCE_SERVER_ID {
        return local_initial_cover_cache_required(store, server_id);
    }
    provider_initial_cover_cache_required(store, server_id)
}

fn provider_initial_cover_cache_required(store: &StoreHandle, server_id: &ServerId) -> bool {
    store
        .with_store(|store| store.selected_provider_cover_cache_missing(server_id))
        .unwrap_or(true)
}

pub(in crate::controller) fn run_sync_job(
    context: &SyncContext,
    saved: &SavedServer,
    generation: i64,
    prefetch_initial_covers: bool,
    detect_unchanged: bool,
    emit_progress: bool,
    cancellation: &CancellationToken,
) -> Result<SyncJobOutcome, String> {
    check_sync_cancelled(cancellation)?;
    let events = emit_progress.then(|| context.events.clone());
    let mut progress = SyncProgressReporter::new(
        events,
        saved.server.name.clone(),
        provider_display_name(&saved.server.provider).to_string(),
    );
    let mut local_scan_progress = |scan| progress.local_scan_progress(scan);
    let provider = provider_for_saved_with_local_scan_progress(
        &context.store,
        &context.runtime,
        &context.secrets,
        saved,
        Some(&mut local_scan_progress),
    )?;
    check_sync_cancelled(cancellation)?;
    let outcome = sync_loaded_provider_generation(
        context,
        saved,
        generation,
        &provider,
        progress,
        detect_unchanged,
        cancellation,
    )?;
    if prefetch_initial_covers {
        check_sync_cancelled(cancellation)?;
        if emit_progress {
            let _sent = context.events.send(ControllerEvent::LoginStatus(
                "Caching library artwork...".to_string(),
            ));
        }
        covers::prefetch_initial_provider_cover_cache(covers::ProviderCoverPrefetchRequest {
            store: &context.store,
            runtime: &context.runtime,
            secrets: &context.secrets,
            events: &context.events,
            cover_in_flight: &context.cover_in_flight,
            external_cover_retry_generation: &context.external_cover_retry_generation,
            retry_generation: context
                .external_cover_retry_generation
                .load(Ordering::SeqCst),
            cover_slots: &context.cover_slots,
            saved,
            provider: provider.as_music_provider(),
            cancellation: Some(cancellation),
            emit_status: emit_progress,
        })?;
    }
    Ok(outcome)
}
fn sync_loaded_provider_generation(
    context: &SyncContext,
    saved: &SavedServer,
    generation: i64,
    provider: &LoadedProvider,
    progress: SyncProgressReporter,
    detect_unchanged: bool,
    cancellation: &CancellationToken,
) -> Result<SyncJobOutcome, String> {
    match provider {
        LoadedProvider::Local(local) => sync_local_provider_generation(
            context,
            &saved.server.id,
            local,
            generation,
            progress,
            cancellation,
        ),
        _ => context.runtime.block_on(sync_provider_generation(
            &context.store,
            &saved.server.id,
            provider.as_music_provider(),
            generation,
            progress,
            detect_unchanged,
            cancellation,
        )),
    }
}
fn sync_local_provider_generation(
    context: &SyncContext,
    server_id: &ServerId,
    provider: &LocalProvider,
    generation: i64,
    progress: SyncProgressReporter,
    cancellation: &CancellationToken,
) -> Result<SyncJobOutcome, String> {
    check_sync_cancelled(cancellation)?;
    let scan = provider.manifest_scan();
    info!(
        generation,
        tag_reads = scan.counters.tag_reads,
        unchanged_reused = scan.counters.unchanged_reused,
        deleted = scan.counters.deleted,
        artwork_changed = scan.counters.artwork_changed,
        filesystem_walk_elapsed_ms = scan.counters.filesystem_walk_elapsed_ms,
        manifest_compare_elapsed_ms = scan.counters.manifest_compare_elapsed_ms,
        "completed manifest-backed local scan"
    );
    context
        .runtime
        .block_on(sync_local_provider_store_generation(
            &context.store,
            server_id,
            provider,
            generation,
            progress,
            cancellation,
        ))
}
async fn sync_local_provider_store_generation(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &LocalProvider,
    generation: i64,
    progress: SyncProgressReporter,
    cancellation: &CancellationToken,
) -> Result<SyncJobOutcome, String> {
    sync_local_provider_store_generation_with_multiplier(
        store,
        server_id,
        provider,
        generation,
        progress,
        cancellation,
        local_library_stress::local_library_stress_multiplier(),
    )
    .await
}

async fn sync_local_provider_store_generation_with_multiplier(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &LocalProvider,
    generation: i64,
    mut progress: SyncProgressReporter,
    cancellation: &CancellationToken,
    stress_multiplier: usize,
) -> Result<SyncJobOutcome, String> {
    check_sync_cancelled(cancellation)?;
    let scan = provider.manifest_scan();
    let mut snapshot =
        collect_local_provider_snapshot(provider, &mut progress, cancellation).await?;
    let stress_delta = local_library_stress::apply_local_library_stress_multiplier(
        LocalStressSnapshot {
            store,
            server_id,
            scan,
            tracks: &mut snapshot.tracks,
            albums: &mut snapshot.albums,
            artists: &mut snapshot.artists,
            album_artists: &mut snapshot.album_artists,
            genres: &mut snapshot.genres,
            home_sections: &mut snapshot.home_sections,
        },
        stress_multiplier,
    )?;
    check_sync_cancelled(cancellation)?;
    let aggregate_dirty = local_aggregate_image_dirty(store, server_id, &snapshot)?;
    check_sync_cancelled(cancellation)?;
    if !scan.library_changed && aggregate_dirty.is_empty() && stress_delta.is_empty() {
        check_sync_cancelled(cancellation)?;
        let pruned_cover_entries =
            store.with_store(|store| store.complete_unchanged_local_sync(server_id, generation))?;
        prune_successful_sync_image_cache(store, server_id, pruned_cover_entries);
        info!(
            generation,
            "completed unchanged local sync without library row writes"
        );
        return Ok(SyncJobOutcome::unchanged());
    }
    info!(
        generation,
        changed_tracks = scan.changed_track_ids.len(),
        metadata_tracks = scan.metadata_track_ids.len(),
        artwork_tracks = scan.artwork_track_ids.len(),
        retained_tracks = scan.retained_track_ids.len(),
        deleted_tracks = scan.deleted_track_ids.len(),
        "started manifest-delta local sync"
    );
    progress.finalizing();
    let finalize_started = Instant::now();
    let delta = local_library_delta(
        provider.manifest_scan(),
        snapshot,
        aggregate_dirty,
        stress_delta.clone(),
    );
    let sync_delta = local_store_delta(&delta, scan.library_changed || !stress_delta.is_empty());
    check_sync_cancelled(cancellation)?;
    let pruned_cover_entries =
        store.with_store(|store| store.commit_local_library_delta(server_id, generation, delta))?;
    prune_successful_sync_image_cache(store, server_id, pruned_cover_entries);
    prune_disk_waveform_cache_entries(server_id, &sync_delta.tracks.deleted);
    progress.finished();
    info!(
        generation,
        finalize_elapsed_ms = finalize_started.elapsed().as_millis() as u64,
        total_elapsed_ms = progress.total_elapsed().as_millis() as u64,
        "completed manifest-delta local sync"
    );
    Ok(SyncJobOutcome::changed(sync_delta))
}

fn local_store_delta(local: &LocalLibraryDelta, home_changed: bool) -> LibraryDelta {
    let mut delta = LibraryDelta::default();
    delta.tracks.added = local
        .changed_tracks
        .iter()
        .map(|track| track.id.clone())
        .collect();
    delta.tracks.fields = local
        .metadata_tracks
        .iter()
        .map(|track| track.id.clone())
        .collect();
    delta.tracks.cover_refs = local
        .artwork_tracks
        .iter()
        .map(|track| track.id.clone())
        .collect();
    delta.tracks.deleted = local.deleted_track_ids.clone();
    delta.albums.links = local
        .dirty_albums
        .iter()
        .map(|album| album.id.clone())
        .collect();
    delta.albums.cover_refs = delta.albums.links.clone();
    delta.artists.links = local
        .dirty_artists
        .iter()
        .map(|artist| artist.id.clone())
        .collect();
    delta.artists.cover_refs = delta.artists.links.clone();
    delta.album_artists.links = local
        .dirty_album_artists
        .iter()
        .map(|artist| artist.id.clone())
        .collect();
    delta.album_artists.cover_refs = delta.album_artists.links.clone();
    delta.genres.links = local
        .dirty_genres
        .iter()
        .map(|genre| genre.id.clone())
        .collect();
    delta.genres.cover_refs = delta.genres.links.clone();
    delta.home_changed = home_changed;
    delta
}
#[derive(Clone, Debug, Default)]
struct LocalProviderSnapshot {
    tracks: Vec<Track>,
    albums: Vec<Album>,
    artists: Vec<Artist>,
    album_artists: Vec<Artist>,
    genres: Vec<Genre>,
    home_sections: Vec<HomeSection>,
}
#[derive(Clone, Debug, Default)]
struct LocalAggregateDirty {
    track_ids: HashSet<TrackId>,
    album_ids: HashSet<AlbumId>,
    artist_ids: HashSet<ArtistId>,
    album_artist_ids: HashSet<ArtistId>,
    genre_names: HashSet<String>,
}
impl LocalAggregateDirty {
    fn is_empty(&self) -> bool {
        self.track_ids.is_empty()
            && self.album_ids.is_empty()
            && self.artist_ids.is_empty()
            && self.album_artist_ids.is_empty()
            && self.genre_names.is_empty()
    }
}
async fn collect_local_provider_snapshot(
    provider: &LocalProvider,
    progress: &mut SyncProgressReporter,
    cancellation: &CancellationToken,
) -> Result<LocalProviderSnapshot, String> {
    progress.collection_started(SyncCollection::Tracks);
    let tracks = load_match_tracks(provider, Some(cancellation)).await?;
    progress.collection_started(SyncCollection::Albums);
    let albums = load_all_local_albums(provider, cancellation).await?;
    progress.collection_started(SyncCollection::Artists);
    let artists = load_all_local_artists(provider, false, cancellation).await?;
    progress.collection_started(SyncCollection::AlbumArtists);
    let album_artists = load_all_local_artists(provider, true, cancellation).await?;
    progress.collection_started(SyncCollection::Genres);
    let genres = load_all_local_genres(provider, cancellation).await?;
    progress.collection_started(SyncCollection::HomeSections);
    let home_sections = await_provider(cancellation, provider.home_sections()).await?;
    Ok(LocalProviderSnapshot {
        tracks,
        albums,
        artists,
        album_artists,
        genres,
        home_sections,
    })
}
async fn load_all_local_albums(
    provider: &LocalProvider,
    cancellation: &CancellationToken,
) -> Result<Vec<Album>, String> {
    let mut albums = Vec::new();
    let mut offset = 0;
    loop {
        let page = await_provider(
            cancellation,
            provider.albums(PagedRequest::new(offset, PAGE_SIZE)),
        )
        .await?;
        let item_count = page.items.len();
        albums.extend(page.items);
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(albums);
        }
    }
}
async fn load_all_local_artists(
    provider: &LocalProvider,
    album_artist: bool,
    cancellation: &CancellationToken,
) -> Result<Vec<Artist>, String> {
    let mut artists = Vec::new();
    let mut offset = 0;
    loop {
        let page = if album_artist {
            await_provider(
                cancellation,
                provider.album_artists(PagedRequest::new(offset, PAGE_SIZE)),
            )
            .await
        } else {
            await_provider(
                cancellation,
                provider.artists(PagedRequest::new(offset, PAGE_SIZE)),
            )
            .await
        }?;
        let item_count = page.items.len();
        artists.extend(page.items);
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(artists);
        }
    }
}
async fn load_all_local_genres(
    provider: &LocalProvider,
    cancellation: &CancellationToken,
) -> Result<Vec<Genre>, String> {
    let mut genres = Vec::new();
    let mut offset = 0;
    loop {
        let page = await_provider(
            cancellation,
            provider.genres(PagedRequest::new(offset, PAGE_SIZE)),
        )
        .await?;
        let item_count = page.items.len();
        genres.extend(page.items);
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(genres);
        }
    }
}
fn local_aggregate_image_dirty(
    store: &StoreHandle,
    server_id: &ServerId,
    snapshot: &LocalProviderSnapshot,
) -> Result<LocalAggregateDirty, String> {
    let cached_track_refs = store.with_store(|store| store.load_raw_track_image_refs(server_id))?;
    let cached_album_refs = store.with_store(|store| store.load_raw_album_image_refs(server_id))?;
    let cached_artist_refs =
        store.with_store(|store| store.load_raw_artist_image_refs(server_id, false))?;
    let cached_album_artist_refs =
        store.with_store(|store| store.load_raw_artist_image_refs(server_id, true))?;
    let mut dirty = LocalAggregateDirty::default();
    for track in &snapshot.tracks {
        if cached_track_refs.get(&track.id) != Some(&track.image_ref) {
            dirty.track_ids.insert(track.id.clone());
        }
    }
    for album in &snapshot.albums {
        if cached_album_refs.get(&album.id) != Some(&album.image_ref) {
            dirty.album_ids.insert(album.id.clone());
        }
    }
    for artist in &snapshot.artists {
        if cached_artist_refs.get(&artist.id) != Some(&artist.image_ref) {
            dirty.artist_ids.insert(artist.id.clone());
        }
    }
    for artist in &snapshot.album_artists {
        if cached_album_artist_refs.get(&artist.id) != Some(&artist.image_ref) {
            dirty.album_artist_ids.insert(artist.id.clone());
        }
    }
    Ok(dirty)
}

fn local_library_delta(
    scan: &LocalManifestScan,
    snapshot: LocalProviderSnapshot,
    aggregate_dirty: LocalAggregateDirty,
    stress_delta: LocalStressDelta,
) -> LocalLibraryDelta {
    let mut changed_track_ids = scan
        .changed_track_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    changed_track_ids.extend(stress_delta.changed_track_ids);
    let mut metadata_track_ids = scan
        .metadata_track_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    metadata_track_ids.extend(stress_delta.metadata_track_ids);
    let mut artwork_track_ids = scan
        .artwork_track_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    artwork_track_ids.extend(stress_delta.artwork_track_ids);
    artwork_track_ids.extend(aggregate_dirty.track_ids.into_iter().filter(|track_id| {
        !changed_track_ids.contains(track_id) && !metadata_track_ids.contains(track_id)
    }));
    let mut dirty_album_ids = scan.dirty_album_ids.iter().cloned().collect::<HashSet<_>>();
    dirty_album_ids.extend(aggregate_dirty.album_ids);
    dirty_album_ids.extend(stress_delta.dirty_album_ids);
    let mut dirty_artist_ids = scan
        .dirty_artist_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    dirty_artist_ids.extend(aggregate_dirty.artist_ids);
    dirty_artist_ids.extend(stress_delta.dirty_artist_ids);
    let mut dirty_album_artist_ids = scan
        .dirty_album_artist_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    dirty_album_artist_ids.extend(aggregate_dirty.album_artist_ids);
    dirty_album_artist_ids.extend(stress_delta.dirty_album_artist_ids);
    let mut dirty_genre_names = scan
        .dirty_genre_names
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    dirty_genre_names.extend(aggregate_dirty.genre_names);
    dirty_genre_names.extend(stress_delta.dirty_genre_names);
    let mut deleted_track_ids = scan
        .deleted_track_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    deleted_track_ids.extend(stress_delta.deleted_track_ids);
    let mut deleted_track_ids = deleted_track_ids.into_iter().collect::<Vec<_>>();
    deleted_track_ids.sort();
    LocalLibraryDelta {
        changed_tracks: snapshot
            .tracks
            .iter()
            .filter(|track| changed_track_ids.contains(&track.id))
            .cloned()
            .collect(),
        metadata_tracks: snapshot
            .tracks
            .iter()
            .filter(|track| metadata_track_ids.contains(&track.id))
            .cloned()
            .collect(),
        artwork_tracks: snapshot
            .tracks
            .iter()
            .filter(|track| artwork_track_ids.contains(&track.id))
            .cloned()
            .collect(),
        deleted_track_ids,
        current_track_ids: snapshot
            .tracks
            .iter()
            .map(|track| track.id.clone())
            .collect(),
        current_album_ids: snapshot
            .albums
            .iter()
            .map(|album| album.id.clone())
            .collect(),
        current_artist_ids: snapshot
            .artists
            .iter()
            .map(|artist| artist.id.clone())
            .collect(),
        current_album_artist_ids: snapshot
            .album_artists
            .iter()
            .map(|artist| artist.id.clone())
            .collect(),
        current_genre_ids: snapshot
            .genres
            .iter()
            .map(|genre| genre.id.clone())
            .collect(),
        dirty_albums: snapshot
            .albums
            .into_iter()
            .filter(|album| dirty_album_ids.contains(&album.id))
            .collect(),
        dirty_artists: snapshot
            .artists
            .into_iter()
            .filter(|artist| dirty_artist_ids.contains(&artist.id))
            .collect(),
        dirty_album_artists: snapshot
            .album_artists
            .into_iter()
            .filter(|artist| dirty_album_artist_ids.contains(&artist.id))
            .collect(),
        dirty_genres: snapshot
            .genres
            .into_iter()
            .filter(|genre| dirty_genre_names.contains(&genre.name))
            .collect(),
        home_sections: snapshot.home_sections,
        manifest_entries: scan.entries.clone(),
        cue_track_sources: scan.cue_track_sources.clone(),
    }
}
pub(in crate::controller) fn refresh_home_section_for_saved(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
    kind: HomeSectionKind,
) -> Result<(), String> {
    let provider = provider_for_saved(store, runtime, secrets, saved)?;
    runtime.block_on(refresh_home_section(
        store,
        &saved.server.id,
        provider.as_music_provider(),
        kind,
    ))
}
pub(in crate::controller) fn prefetch_home_section_for_saved(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
    kind: HomeSectionKind,
) -> Result<HomeSection, String> {
    let provider = provider_for_saved(store, runtime, secrets, saved)?;
    let mut section = runtime.block_on(prefetch_home_section(
        store,
        &saved.server.id,
        provider.as_music_provider(),
        kind,
    ))?;
    home_image_refs(store, saved, &mut section)?;
    Ok(section)
}
#[cfg(test)]
#[instrument(skip(store, provider), fields(server_id = %server_id.as_str()))]
pub(in crate::controller) async fn sync_provider(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
) -> Result<(), String> {
    let generation = store.with_store(|store| store.begin_sync(server_id))?;
    let cancellation = CancellationToken::new();
    sync_provider_generation(
        store,
        server_id,
        provider,
        generation,
        SyncProgressReporter::silent(provider),
        false,
        &cancellation,
    )
    .await
    .map(|_| ())
}
#[cfg(test)]
pub(in crate::controller) async fn sync_provider_with_events(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
    events: Sender<ControllerEvent>,
) -> Result<(), String> {
    let generation = store.with_store(|store| store.begin_sync(server_id))?;
    let cancellation = CancellationToken::new();
    sync_provider_generation(
        store,
        server_id,
        provider,
        generation,
        SyncProgressReporter::for_provider(provider, Some(events)),
        false,
        &cancellation,
    )
    .await
    .map(|_| ())
}
#[cfg(test)]
pub(in crate::controller) async fn sync_provider_outcome(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
) -> Result<SyncJobOutcome, String> {
    let generation = store.with_store(|store| store.begin_sync(server_id))?;
    let cancellation = CancellationToken::new();
    sync_provider_generation(
        store,
        server_id,
        provider,
        generation,
        SyncProgressReporter::silent(provider),
        true,
        &cancellation,
    )
    .await
}
#[cfg(test)]
pub(in crate::controller) async fn sync_provider_outcome_with_cancellation(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
    cancellation: &CancellationToken,
) -> Result<SyncJobOutcome, String> {
    let generation = store.with_store(|store| store.begin_sync(server_id))?;
    sync_provider_generation(
        store,
        server_id,
        provider,
        generation,
        SyncProgressReporter::silent(provider),
        true,
        cancellation,
    )
    .await
}
#[cfg(test)]
pub(in crate::controller) async fn sync_local_provider_with_events(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &LocalProvider,
    events: Sender<ControllerEvent>,
) -> Result<(), String> {
    let generation = store.with_store(|store| store.begin_sync(server_id))?;
    let cancellation = CancellationToken::new();
    sync_local_provider_store_generation(
        store,
        server_id,
        provider,
        generation,
        SyncProgressReporter::for_provider(provider, Some(events)),
        &cancellation,
    )
    .await
    .map(|_| ())
}
#[cfg(test)]
pub(in crate::controller) async fn sync_local_provider_outcome(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &LocalProvider,
) -> Result<SyncJobOutcome, String> {
    let generation = store.with_store(|store| store.begin_sync(server_id))?;
    let cancellation = CancellationToken::new();
    sync_local_provider_store_generation(
        store,
        server_id,
        provider,
        generation,
        SyncProgressReporter::silent(provider),
        &cancellation,
    )
    .await
}
#[cfg(test)]
pub(in crate::controller) async fn sync_local_provider_outcome_with_stress_multiplier(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &LocalProvider,
    stress_multiplier: usize,
) -> Result<SyncJobOutcome, String> {
    let generation = store.with_store(|store| store.begin_sync(server_id))?;
    let cancellation = CancellationToken::new();
    sync_local_provider_store_generation_with_multiplier(
        store,
        server_id,
        provider,
        generation,
        SyncProgressReporter::silent(provider),
        &cancellation,
        stress_multiplier,
    )
    .await
}

#[instrument(skip(store, provider, progress), fields(server_id = %server_id.as_str(), generation))]
async fn sync_provider_generation(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
    generation: i64,
    mut progress: SyncProgressReporter,
    _detect_unchanged: bool,
    cancellation: &CancellationToken,
) -> Result<SyncJobOutcome, String> {
    check_sync_cancelled(cancellation)?;
    let mut collector = LibraryDeltaCollector::new();
    info!(generation, "started provider cache sync");
    sync_album_pages(
        store,
        server_id,
        provider,
        generation,
        &mut progress,
        &mut collector,
        cancellation,
    )
    .await?;
    check_sync_cancelled(cancellation)?;
    sync_track_pages(
        store,
        server_id,
        provider,
        generation,
        &mut progress,
        &mut collector,
        cancellation,
    )
    .await?;
    check_sync_cancelled(cancellation)?;
    progress.collection_started(SyncCollection::MusicFolders);
    let folders_changed =
        sync_music_folders(store, server_id, provider, generation, cancellation).await?;
    if folders_changed {
        collector.merge(LibraryDelta {
            folders_changed: true,
            ..LibraryDelta::default()
        });
    }
    check_sync_cancelled(cancellation)?;
    progress.collection_started(SyncCollection::Artists);
    sync_artist_pages(
        store,
        server_id,
        provider,
        generation,
        false,
        &mut collector,
        cancellation,
    )
    .await?;
    check_sync_cancelled(cancellation)?;
    progress.collection_started(SyncCollection::AlbumArtists);
    sync_artist_pages(
        store,
        server_id,
        provider,
        generation,
        true,
        &mut collector,
        cancellation,
    )
    .await?;
    check_sync_cancelled(cancellation)?;
    progress.collection_started(SyncCollection::Genres);
    sync_genre_pages(
        store,
        server_id,
        provider,
        generation,
        &mut collector,
        cancellation,
    )
    .await?;
    check_sync_cancelled(cancellation)?;
    progress.collection_started(SyncCollection::Playlists);
    sync_playlist_pages(
        store,
        server_id,
        provider,
        generation,
        &mut collector,
        cancellation,
    )
    .await?;
    check_sync_cancelled(cancellation)?;
    progress.collection_started(SyncCollection::HomeSections);
    collector
        .merge(sync_home_sections(store, server_id, provider, generation, cancellation).await?);
    progress.finalizing();
    let finalize_started = Instant::now();
    check_sync_cancelled(cancellation)?;
    store.with_store(|store| store.refresh_library_counts(server_id))?;
    check_sync_cancelled(cancellation)?;
    let completion = store.with_store(|store| store.complete_sync_delta(server_id, generation))?;
    collector.merge(completion.delta);
    prune_successful_sync_image_cache(store, server_id, completion.pruned_cover_entries);
    let finalize_elapsed = finalize_started.elapsed();
    progress.finished();
    match refresh_local_track_matches(store, server_id, Some(generation), Some(cancellation)).await
    {
        Ok(_) => {}
        Err(error) if error == SYNC_CANCELLED_ERROR => return Err(error),
        Err(error) => warn!(%error, "failed to refresh local track matches"),
    }
    let delta = collector.finish();
    prune_disk_waveform_cache_entries(server_id, &delta.tracks.deleted);
    let library_changed = !delta.is_empty();
    info!(
        generation,
        finalize_elapsed_ms = finalize_elapsed.as_millis() as u64,
        total_elapsed_ms = progress.total_elapsed().as_millis() as u64,
        library_changed,
        "completed provider cache sync"
    );
    if delta.is_empty() {
        Ok(SyncJobOutcome::unchanged())
    } else {
        Ok(SyncJobOutcome::changed(delta))
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::controller) enum SyncCollection {
    Albums,
    Tracks,
    MusicFolders,
    Artists,
    AlbumArtists,
    Genres,
    Playlists,
    HomeSections,
}
impl SyncCollection {
    fn label(self) -> &'static str {
        match self {
            Self::Albums => "albums",
            Self::Tracks => "tracks",
            Self::MusicFolders => "music folders",
            Self::Artists => "artists",
            Self::AlbumArtists => "album artists",
            Self::Genres => "genres",
            Self::Playlists => "playlists",
            Self::HomeSections => "home sections",
        }
    }
}
pub(in crate::controller) struct SyncPageProgress {
    pub(in crate::controller) collection: SyncCollection,
    pub(in crate::controller) page_number: usize,
    pub(in crate::controller) fetched: usize,
    pub(in crate::controller) written: usize,
    pub(in crate::controller) total: Option<usize>,
    pub(in crate::controller) finished: bool,
    pub(in crate::controller) fetch_elapsed: Duration,
    pub(in crate::controller) write_elapsed: Duration,
}
pub(in crate::controller) struct SyncProgressReporter {
    events: Option<Sender<ControllerEvent>>,
    source_name: String,
    provider_kind: String,
    started_at: Instant,
    last_status_at: Option<Instant>,
    min_interval: Duration,
    cache_headline_sent: bool,
}
impl SyncProgressReporter {
    pub(in crate::controller) fn new(
        events: Option<Sender<ControllerEvent>>,
        source_name: String,
        provider_kind: String,
    ) -> Self {
        Self {
            events,
            source_name,
            provider_kind,
            started_at: Instant::now(),
            last_status_at: None,
            min_interval: SYNC_PROGRESS_MIN_INTERVAL,
            cache_headline_sent: false,
        }
    }

    #[cfg(test)]
    fn for_provider(
        provider: &(impl MusicProvider + ?Sized),
        events: Option<Sender<ControllerEvent>>,
    ) -> Self {
        let server = &provider.identity().server;
        Self::new(
            events,
            server.name.clone(),
            provider_display_name(&server.provider).to_string(),
        )
    }

    #[cfg(test)]
    fn silent(provider: &(impl MusicProvider + ?Sized)) -> Self {
        Self::for_provider(provider, None)
    }

    fn total_elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    fn collection_started(&mut self, collection: SyncCollection) {
        if !self.cache_headline_sent {
            self.cache_headline_sent = true;
            self.emit_status(
                true,
                "Caching library... This may take some time.".to_string(),
            );
            return;
        }
        self.emit_status(
            true,
            format!(
                "Fetching {} for {} ({})",
                collection.label(),
                self.source_label(),
                elapsed_label(self.total_elapsed())
            ),
        );
    }

    fn page_fetching(
        &mut self,
        collection: SyncCollection,
        page_number: usize,
        fetched: usize,
        total: Option<usize>,
    ) {
        let count = progress_count_label(fetched, total);
        self.emit_status(
            false,
            format!(
                "Fetching {} page {page_number} for {}, {count} fetched ({})",
                collection.label(),
                self.source_label(),
                elapsed_label(self.total_elapsed())
            ),
        );
    }

    fn local_scan_progress(&mut self, progress: LocalScanProgress) {
        if !self.cache_headline_sent {
            self.cache_headline_sent = true;
            self.emit_status(
                true,
                "Caching local library... This may take some time.".to_string(),
            );
            return;
        }
        let count = match progress.stage {
            LocalScanStage::Walking => format!(
                "{} audio files found, {} entries checked",
                formatted_count(progress.audio_candidates.min(usize::MAX as u64) as usize),
                formatted_count(progress.directory_entries_visited.min(usize::MAX as u64) as usize)
            ),
            LocalScanStage::ReadingTags => format!(
                "{} tracks processed",
                progress_count_label(progress.processed_tracks, progress.total_tracks)
            ),
            LocalScanStage::BuildingLibrary => format!(
                "{} tracks ready",
                progress_count_label(progress.processed_tracks, progress.total_tracks)
            ),
        };
        let action = match progress.stage {
            LocalScanStage::Walking => "Scanning folders",
            LocalScanStage::ReadingTags => "Reading track metadata",
            LocalScanStage::BuildingLibrary => "Preparing local cache",
        };
        let force = progress.stage != LocalScanStage::Walking
            || progress.audio_candidates == 0
            || progress.audio_candidates.is_multiple_of(100);
        self.emit_status(
            force,
            format!(
                "{action} for {}, {count} ({})",
                self.source_label(),
                elapsed_label(self.total_elapsed())
            ),
        );
    }

    pub(in crate::controller) fn page_written(&mut self, progress: SyncPageProgress) {
        let page = page_label(progress.page_number, progress.total);
        self.emit_status(
            progress.finished,
            format!(
                "Cached {} {page} for {}, {} cached ({})",
                progress.collection.label(),
                self.source_label(),
                formatted_count(progress.written),
                elapsed_label(self.total_elapsed())
            ),
        );
        info!(
            collection = progress.collection.label(),
            page = progress.page_number,
            fetched = progress.fetched,
            written = progress.written,
            total = progress.total,
            finished = progress.finished,
            fetch_elapsed_ms = progress.fetch_elapsed.as_millis() as u64,
            write_elapsed_ms = progress.write_elapsed.as_millis() as u64,
            total_elapsed_ms = self.total_elapsed().as_millis() as u64,
            "synced library cache page"
        );
    }

    fn finalizing(&mut self) {
        self.emit_status(
            true,
            format!(
                "Finalizing cache for {} ({})",
                self.source_label(),
                elapsed_label(self.total_elapsed())
            ),
        );
    }

    fn finished(&mut self) {
        self.emit_status(
            true,
            format!(
                "Library cache ready for {} in {}",
                self.source_label(),
                elapsed_label(self.total_elapsed())
            ),
        );
    }

    fn source_label(&self) -> String {
        let source_name = self.source_name.trim();
        if source_name.is_empty() {
            return self.provider_kind.clone();
        }
        source_name.to_string()
    }

    fn emit_status(&mut self, force: bool, status: String) {
        let now = Instant::now();
        let due = self
            .last_status_at
            .is_none_or(|last| now.duration_since(last) >= self.min_interval);
        if !force && !due {
            return;
        }
        self.last_status_at = Some(now);
        if let Some(events) = &self.events {
            let _sent = events.send(ControllerEvent::LoginStatus(status));
        }
    }
}
fn known_sync_total(total: usize) -> Option<usize> {
    (total > 0).then_some(total)
}
async fn fetch_page_with_progress<T, Fut>(
    progress: &mut SyncProgressReporter,
    collection: SyncCollection,
    page_number: usize,
    fetched: usize,
    total: Option<usize>,
    cancellation: &CancellationToken,
    page: Fut,
) -> Result<source::PagedResponse<T>, String>
where
    Fut: Future<Output = source::ProviderResult<source::PagedResponse<T>>>,
{
    progress.page_fetching(collection, page_number, fetched, total);
    tokio::pin!(page);
    loop {
        check_sync_cancelled(cancellation)?;
        tokio::select! {
            result = &mut page => return result.map_err(|error| error.to_string()),
            _ = tokio::time::sleep(SYNC_CANCEL_POLL_INTERVAL) => {
                check_sync_cancelled(cancellation)?;
                progress.page_fetching(collection, page_number, fetched, total);
            }
        }
    }
}
fn page_label(page_number: usize, total: Option<usize>) -> String {
    match total {
        Some(total) => {
            let page_total = total.div_ceil(PAGE_SIZE).max(1);
            format!("page {page_number}/{page_total}")
        }
        None => format!("page {page_number}"),
    }
}
fn progress_count_label(count: usize, total: Option<usize>) -> String {
    match total {
        Some(total) => format!("{}/{}", formatted_count(count), formatted_count(total)),
        None => formatted_count(count),
    }
}
fn formatted_count(count: usize) -> String {
    let raw = count.to_string();
    let mut output = String::new();
    for (index, character) in raw.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            output.push(',');
        }
        output.push(character);
    }
    output.chars().rev().collect()
}
fn elapsed_label(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        return format!("{seconds}s elapsed");
    }
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes}m {seconds:02}s elapsed")
}
async fn sync_album_pages(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
    generation: i64,
    progress: &mut SyncProgressReporter,
    collector: &mut LibraryDeltaCollector,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    progress.collection_started(SyncCollection::Albums);
    let mut offset = 0;
    let mut page_number = 0;
    loop {
        check_sync_cancelled(cancellation)?;
        page_number += 1;
        let fetch_started = Instant::now();
        let page = fetch_page_with_progress(
            progress,
            SyncCollection::Albums,
            page_number,
            offset,
            None,
            cancellation,
            provider.albums(PagedRequest::new(offset, PAGE_SIZE)),
        )
        .await?;
        check_sync_cancelled(cancellation)?;
        let fetch_elapsed = fetch_started.elapsed();
        let write_started = Instant::now();
        collector.merge(
            store.with_store(|store| {
                store.upsert_albums_delta(server_id, &page.items, generation)
            })?,
        );
        let write_elapsed = write_started.elapsed();
        let item_count = page.items.len();
        offset += item_count;
        let finished = sync_page_finished(item_count, page.total, offset);
        progress.page_written(SyncPageProgress {
            collection: SyncCollection::Albums,
            page_number,
            fetched: offset,
            written: offset,
            total: known_sync_total(page.total),
            finished,
            fetch_elapsed,
            write_elapsed,
        });
        if finished {
            return Ok(());
        }
    }
}
async fn sync_track_pages(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
    generation: i64,
    progress: &mut SyncProgressReporter,
    collector: &mut LibraryDeltaCollector,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    progress.collection_started(SyncCollection::Tracks);
    let mut offset = 0;
    let mut page_number = 0;
    loop {
        check_sync_cancelled(cancellation)?;
        page_number += 1;
        let fetch_started = Instant::now();
        let page = fetch_page_with_progress(
            progress,
            SyncCollection::Tracks,
            page_number,
            offset,
            None,
            cancellation,
            provider.tracks(PagedRequest::new(offset, PAGE_SIZE)),
        )
        .await?;
        check_sync_cancelled(cancellation)?;
        let fetch_elapsed = fetch_started.elapsed();
        let write_started = Instant::now();
        collector.merge(
            store.with_store(|store| {
                store.upsert_tracks_delta(server_id, &page.items, generation)
            })?,
        );
        let write_elapsed = write_started.elapsed();
        let item_count = page.items.len();
        offset += item_count;
        let finished = sync_page_finished(item_count, page.total, offset);
        progress.page_written(SyncPageProgress {
            collection: SyncCollection::Tracks,
            page_number,
            fetched: offset,
            written: offset,
            total: known_sync_total(page.total),
            finished,
            fetch_elapsed,
            write_elapsed,
        });
        if finished {
            return Ok(());
        }
    }
}
async fn sync_music_folders(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
    generation: i64,
    cancellation: &CancellationToken,
) -> Result<bool, String> {
    if !provider.capabilities().music_folders {
        return Ok(false);
    }
    check_sync_cancelled(cancellation)?;
    let before = store.with_store(|store| store.list_music_folders(server_id))?;
    let folders = await_provider(cancellation, provider.music_folders()).await?;
    check_sync_cancelled(cancellation)?;
    let changed = before != folders;
    check_sync_cancelled(cancellation)?;
    store.with_store(|store| store.upsert_music_folders(server_id, &folders, generation))?;
    for folder in folders {
        let mut offset = 0;
        loop {
            check_sync_cancelled(cancellation)?;
            let page = await_provider(
                cancellation,
                provider.tracks_in_music_folder(&folder.id, PagedRequest::new(offset, PAGE_SIZE)),
            )
            .await?;
            check_sync_cancelled(cancellation)?;
            store.with_store(|store| store.upsert_tracks(server_id, &page.items, generation))?;
            check_sync_cancelled(cancellation)?;
            store.with_store(|store| {
                store.upsert_track_music_folder_memberships(
                    server_id,
                    &folder.id,
                    &page.items,
                    generation,
                )
            })?;
            let item_count = page.items.len();
            offset += item_count;
            if sync_page_finished(item_count, page.total, offset) {
                break;
            }
        }
    }
    Ok(changed)
}
pub(in crate::controller) async fn refresh_local_track_matches(
    store: &StoreHandle,
    server_id: &ServerId,
    manifest_generation: Option<i64>,
    cancellation: Option<&CancellationToken>,
) -> Result<usize, String> {
    check_optional_sync_cancelled(cancellation)?;
    let Some(access) = store.with_store(|store| store.server_local_access(server_id))? else {
        return Ok(0);
    };
    check_optional_sync_cancelled(cancellation)?;
    let saved = store
        .with_store(|store| {
            store.list_servers().map(|servers| {
                servers
                    .into_iter()
                    .find(|saved| saved.server.id == *server_id)
            })
        })?
        .ok_or_else(|| "The server is no longer saved.".to_string())?;
    if saved.server.provider == "local" {
        return Ok(0);
    }
    check_optional_sync_cancelled(cancellation)?;
    let remote_tracks =
        store.with_store(|store| store.load_tracks_for_local_matching(server_id))?;
    if remote_tracks.is_empty() {
        check_optional_sync_cancelled(cancellation)?;
        store.with_store(|store| store.replace_track_local_matches(server_id, &[]))?;
        return Ok(0);
    }
    check_optional_sync_cancelled(cancellation)?;
    let root = PathBuf::from(&access.root_path);
    let manifest_cache = store.with_store(|store| store.load_local_manifest(server_id))?;
    let local_identity =
        LocalProvider::identity_for_root(&root).map_err(|error| error.to_string())?;
    check_optional_sync_cancelled(cancellation)?;
    let local_provider =
        LocalProvider::from_roots_with_manifest_cache(vec![root], local_identity, manifest_cache)
            .map_err(|error| error.to_string())?;
    check_optional_sync_cancelled(cancellation)?;
    let scan = local_provider.manifest_scan();
    info!(
        server_id = %server_id,
        tag_reads = scan.counters.tag_reads,
        unchanged_reused = scan.counters.unchanged_reused,
        deleted = scan.counters.deleted,
        filesystem_walk_elapsed_ms = scan.counters.filesystem_walk_elapsed_ms,
        manifest_compare_elapsed_ms = scan.counters.manifest_compare_elapsed_ms,
        "completed manifest-backed local-access scan"
    );
    let local_tracks = load_match_tracks(&local_provider, cancellation).await?;
    check_optional_sync_cancelled(cancellation)?;
    let matches = conservative_local_matches(&remote_tracks, &local_tracks);
    let count = matches.len();
    check_optional_sync_cancelled(cancellation)?;
    store.with_store(|store| store.replace_track_local_matches(server_id, &matches))?;
    check_optional_sync_cancelled(cancellation)?;
    store.with_store(|store| {
        store.replace_local_manifest(
            server_id,
            manifest_generation.unwrap_or_default(),
            &local_provider.manifest_scan().entries,
        )
    })?;
    debug!(server_id = %server_id, count, "refreshed local track matches");
    Ok(count)
}
async fn load_match_tracks(
    provider: &LocalProvider,
    cancellation: Option<&CancellationToken>,
) -> Result<Vec<Track>, String> {
    let mut tracks = Vec::new();
    let mut offset = 0;
    loop {
        check_optional_sync_cancelled(cancellation)?;
        let page = await_optional_provider(
            cancellation,
            provider.tracks(PagedRequest::new(offset, PAGE_SIZE)),
        )
        .await?;
        check_optional_sync_cancelled(cancellation)?;
        let item_count = page.items.len();
        tracks.extend(page.items);
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(tracks);
        }
    }
}
pub(in crate::controller) fn local_access_status_for_server(
    store: &StoreHandle,
    server: &ServerIdentity,
    access: Option<&ServerLocalAccess>,
) -> Result<LocalAccessStatus, String> {
    let Some(access) = access else {
        return Ok(LocalAccessStatus::default());
    };
    if server.provider == "local" {
        return Ok(LocalAccessStatus::default());
    }

    let facts = store.with_store(|store| store.local_access_status_facts(access))?;
    let sample_local_path = facts.sample_metadata_path.clone().or_else(|| {
        facts
            .sample_server_path
            .as_deref()
            .and_then(|raw| potential_local_path_text(raw, access))
    });
    Ok(LocalAccessStatus {
        sample_server_path: facts.sample_server_path,
        sample_local_path,
        direct_match_count: facts.direct_match_count,
        prefix_match_count: facts.prefix_match_count,
        metadata_match_count: facts.metadata_match_count,
        unmatched_count: facts.unmatched_count,
        total_track_count: facts.total_track_count,
    })
}
pub(in crate::controller) fn potential_local_path_text(
    raw: &str,
    access: &ServerLocalAccess,
) -> Option<String> {
    if raw.trim().is_empty() {
        return None;
    }
    if let Some(mapped) = map_server_path_to_local(raw, access) {
        return Some(mapped.to_string_lossy().into_owned());
    }
    let direct = Path::new(raw);
    if direct.is_absolute() {
        return Some(direct.to_string_lossy().into_owned());
    }
    None
}
#[derive(Hash, Eq, PartialEq)]
pub(in crate::controller) struct LocalMatchKey {
    title: String,
    album: String,
    artist: String,
    disc_number: u16,
    track_number: u16,
}
pub(in crate::controller) fn conservative_local_matches(
    remote_tracks: &[Track],
    local_tracks: &[Track],
) -> Vec<(TrackId, String, String)> {
    let mut index = HashMap::<LocalMatchKey, Vec<&Track>>::new();
    for track in local_tracks {
        if track.local_path.is_none() {
            continue;
        }
        index.entry(local_match_key(track)).or_default().push(track);
    }

    let mut matches = Vec::new();
    for remote in remote_tracks {
        let Some(candidates) = index.get(&local_match_key(remote)) else {
            continue;
        };
        let matched = candidates
            .iter()
            .copied()
            .filter(|candidate| {
                durations_close(remote.duration_seconds, candidate.duration_seconds)
            })
            .collect::<Vec<_>>();
        if matched.len() != 1 {
            continue;
        }
        let Some(local_path) = matched[0].local_path.clone() else {
            continue;
        };
        matches.push((remote.id.clone(), local_path, "metadata".to_string()));
    }
    matches
}
pub(in crate::controller) fn local_match_key(track: &Track) -> LocalMatchKey {
    LocalMatchKey {
        title: normalize_match_text(&track.title),
        album: normalize_match_text(&track.album),
        artist: normalize_match_text(&track.artist),
        disc_number: track.disc_number,
        track_number: track.track_number,
    }
}
pub(in crate::controller) fn durations_close(left: u32, right: u32) -> bool {
    left == 0 || right == 0 || left.abs_diff(right) <= 3
}
pub(in crate::controller) fn normalize_match_text(value: &str) -> String {
    let mut normalized = String::new();
    for character in value.chars() {
        if character.is_alphanumeric() {
            normalized.extend(character.to_lowercase());
        } else {
            normalized.push(' ');
        }
    }
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}
async fn sync_artist_pages(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
    generation: i64,
    album_artist: bool,
    collector: &mut LibraryDeltaCollector,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    let mut offset = 0;
    loop {
        check_sync_cancelled(cancellation)?;
        let page = if album_artist {
            await_provider(
                cancellation,
                provider.album_artists(PagedRequest::new(offset, PAGE_SIZE)),
            )
            .await
        } else {
            await_provider(
                cancellation,
                provider.artists(PagedRequest::new(offset, PAGE_SIZE)),
            )
            .await
        }?;
        check_sync_cancelled(cancellation)?;
        collector.merge(store.with_store(|store| {
            store.upsert_artists_delta(server_id, &page.items, album_artist, generation)
        })?);
        let item_count = page.items.len();
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(());
        }
    }
}
async fn sync_genre_pages(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
    generation: i64,
    collector: &mut LibraryDeltaCollector,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    let mut offset = 0;
    loop {
        check_sync_cancelled(cancellation)?;
        let page = await_provider(
            cancellation,
            provider.genres(PagedRequest::new(offset, PAGE_SIZE)),
        )
        .await?;
        check_sync_cancelled(cancellation)?;
        collector.merge(
            store.with_store(|store| {
                store.upsert_genres_delta(server_id, &page.items, generation)
            })?,
        );
        let item_count = page.items.len();
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(());
        }
    }
}
async fn sync_playlist_pages(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
    generation: i64,
    collector: &mut LibraryDeltaCollector,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    let mut offset = 0;
    loop {
        check_sync_cancelled(cancellation)?;
        let page = await_provider(
            cancellation,
            provider.playlists(PagedRequest::new(offset, PAGE_SIZE)),
        )
        .await?;
        check_sync_cancelled(cancellation)?;
        collector.merge(store.with_store(|store| {
            store.upsert_playlists_delta(server_id, &page.items, generation)
        })?);
        for playlist in &page.items {
            check_sync_cancelled(cancellation)?;
            let detail =
                await_provider(cancellation, provider.playlist_detail(&playlist.id)).await?;
            check_sync_cancelled(cancellation)?;
            collector.merge(store.with_store(|store| {
                store.upsert_tracks_delta(server_id, &detail.tracks, generation)
            })?);
            check_sync_cancelled(cancellation)?;
            collector.merge(store.with_store(|store| {
                store.upsert_playlist_entries_delta(
                    server_id,
                    &detail.playlist.id,
                    &detail.entries,
                    generation,
                )
            })?);
        }
        let item_count = page.items.len();
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(());
        }
    }
}
#[cfg(test)]
pub(in crate::controller) async fn refresh_playlist_pages(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
) -> Result<(), String> {
    let generation =
        store.with_store(|store| store.sync_state(server_id).map(|state| state.generation))?;
    let mut playlist_ids = Vec::new();
    let mut offset = 0;
    loop {
        let page = provider
            .playlists(PagedRequest::new(offset, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        for playlist in &page.items {
            playlist_ids.push(playlist.id.clone());
        }
        store.with_store(|store| store.upsert_playlists(server_id, &page.items, generation))?;
        for playlist in &page.items {
            let detail = provider
                .playlist_detail(&playlist.id)
                .await
                .map_err(|error| error.to_string())?;
            store.with_store(|store| {
                store.upsert_tracks(server_id, &detail.tracks, generation)?;
                store.upsert_playlist_entries(
                    server_id,
                    &detail.playlist.id,
                    &detail.entries,
                    generation,
                )?;
                Ok(())
            })?;
        }
        let item_count = page.items.len();
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            store.with_store(|store| store.prune_playlists_except(server_id, &playlist_ids))?;
            return Ok(());
        }
    }
}
async fn sync_home_sections(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
    generation: i64,
    cancellation: &CancellationToken,
) -> Result<LibraryDelta, String> {
    check_sync_cancelled(cancellation)?;
    let sections = await_provider(cancellation, provider.home_sections()).await?;
    check_sync_cancelled(cancellation)?;
    store.with_store(|store| store.upsert_home_sections_delta(server_id, &sections, generation))
}
#[cfg(test)]
pub(in crate::controller) async fn refresh_home_sections(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
) -> Result<(), String> {
    let generation =
        store.with_store(|store| store.sync_state(server_id).map(|state| state.generation))?;
    let sections = provider
        .home_sections()
        .await
        .map_err(|error| error.to_string())?;
    cache_home_sections(store, server_id, &sections, generation)
}
#[cfg(test)]
pub(in crate::controller) async fn refresh_home_sections_without_explore(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
) -> Result<(), String> {
    for kind in home_refresh_section_kinds()
        .into_iter()
        .filter(|kind| *kind != HomeSectionKind::Explore)
    {
        refresh_home_section(store, server_id, provider, kind).await?;
    }
    Ok(())
}
pub(in crate::controller) async fn refresh_home_section(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
    kind: HomeSectionKind,
) -> Result<(), String> {
    let generation =
        store.with_store(|store| store.sync_state(server_id).map(|state| state.generation))?;
    let section = provider
        .home_section(kind)
        .await
        .map_err(|error| error.to_string())?;
    cache_home_section(store, server_id, &section, generation)
}
pub(in crate::controller) async fn prefetch_home_section(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
    kind: HomeSectionKind,
) -> Result<HomeSection, String> {
    let generation =
        store.with_store(|store| store.sync_state(server_id).map(|state| state.generation))?;
    let section = provider
        .home_section(kind)
        .await
        .map_err(|error| error.to_string())?;
    cache_home_section_items(store, server_id, &section, generation)?;
    store
        .with_store(|store| store.upsert_home_section_prefetch(server_id, &section, generation))?;
    Ok(section)
}
