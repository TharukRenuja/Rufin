use super::*;
use std::time::{Duration, Instant};

const RETRY_INITIAL_DELAY: Duration = Duration::from_secs(5);
const RETRY_MAX_DELAY: Duration = Duration::from_secs(60);
const RETRY_POLL: Duration = Duration::from_millis(500);
const SUBSONIC_FRESHNESS_INTERVAL: Duration = Duration::from_secs(5 * 60);

type PendingJellyfinChange = Arc<Mutex<Option<JellyfinLibraryChange>>>;

pub(in crate::controller) struct RemoteLibraryWatcher {
    source_id: SourceId,
    cancellation: CancellationToken,
}

impl AppController {
    pub fn refresh_remote_library_watcher(&self) {
        refresh_remote_library_watcher(
            self.sync_context(),
            Arc::clone(&self.remote_library_watcher),
        );
    }
}

pub(in crate::controller) fn refresh_remote_library_watcher(
    context: SyncContext,
    slot: Arc<Mutex<Option<RemoteLibraryWatcher>>>,
) {
    let target = active_remote_freshness_target(&context);
    let Ok(mut current) = slot.lock() else {
        return;
    };
    let Some(saved) = target else {
        *current = None;
        return;
    };
    if current
        .as_ref()
        .is_some_and(|watcher| watcher.source_id == saved.source.id)
    {
        return;
    }
    *current = Some(RemoteLibraryWatcher::start(context, saved));
}

impl RemoteLibraryWatcher {
    fn start(context: SyncContext, saved: SavedSource) -> Self {
        let cancellation = CancellationToken::new();
        let thread_cancellation = cancellation.clone();
        let pending = Arc::new(Mutex::new(None));
        let pending_waiter = Arc::new(AtomicBool::new(false));
        let source_id = saved.source.id.clone();
        thread::spawn(move || {
            if saved.source.kind == "jellyfin" {
                watch_jellyfin_library(
                    context,
                    saved,
                    thread_cancellation,
                    pending,
                    pending_waiter,
                );
            } else {
                poll_subsonic_library(context, saved, thread_cancellation);
            }
        });
        Self {
            source_id,
            cancellation,
        }
    }
}

impl Drop for RemoteLibraryWatcher {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

fn watch_jellyfin_library(
    context: SyncContext,
    saved: SavedSource,
    cancellation: CancellationToken,
    pending: PendingJellyfinChange,
    pending_waiter: Arc<AtomicBool>,
) {
    let mut retry_delay = RETRY_INITIAL_DELAY;
    info!(
        source_id = %saved.source.id.as_str(),
        "started Jellyfin library watcher"
    );
    while !cancellation.cancelled() && remote_freshness_target_is_active(&context, &saved) {
        let result = source_for_saved(&context.store, &context.runtime, &context.secrets, &saved)
            .and_then(|source| match source {
                LoadedSource::Jellyfin(source) => context
                    .runtime
                    .block_on(source.listen_library_changes(
                        |change| {
                            if !remote_freshness_target_is_active(&context, &saved) {
                                return false;
                            }
                            queue_jellyfin_library_change(
                                context.clone(),
                                saved.clone(),
                                change,
                                Arc::clone(&pending),
                                Arc::clone(&pending_waiter),
                                cancellation.clone(),
                            );
                            !cancellation.cancelled()
                        },
                        || {
                            cancellation.cancelled()
                                || !remote_freshness_target_is_active(&context, &saved)
                        },
                    ))
                    .map_err(|error| error.to_string()),
                _ => Ok(()),
            });
        if cancellation.cancelled() || !remote_freshness_target_is_active(&context, &saved) {
            break;
        }
        if let Err(error) = result {
            warn!(
                %error,
                source_id = %saved.source.id.as_str(),
                retry_delay_ms = retry_delay.as_millis() as u64,
                "Jellyfin library watcher disconnected"
            );
        }
        if !sleep_until_retry(&cancellation, retry_delay) {
            break;
        }
        retry_delay = retry_delay.saturating_mul(2).min(RETRY_MAX_DELAY);
    }
    info!(
        source_id = %saved.source.id.as_str(),
        "stopped Jellyfin library watcher"
    );
}

fn poll_subsonic_library(
    context: SyncContext,
    saved: SavedSource,
    cancellation: CancellationToken,
) {
    info!(
        source_id = %saved.source.id.as_str(),
        source = %saved.source.kind,
        interval_ms = SUBSONIC_FRESHNESS_INTERVAL.as_millis() as u64,
        "started remote library freshness poller"
    );
    while sleep_until_retry(&cancellation, SUBSONIC_FRESHNESS_INTERVAL)
        && remote_freshness_target_is_active(&context, &saved)
    {
        start_background_sync_thread(context.clone(), saved.clone());
    }
    info!(
        source_id = %saved.source.id.as_str(),
        source = %saved.source.kind,
        "stopped remote library freshness poller"
    );
}

fn queue_jellyfin_library_change(
    context: SyncContext,
    saved: SavedSource,
    change: JellyfinLibraryChange,
    pending: PendingJellyfinChange,
    pending_waiter: Arc<AtomicBool>,
    watcher_cancellation: CancellationToken,
) {
    let source_id = saved.source.id.clone();
    if !cached_library_exists(&context.store, &source_id) {
        merge_pending_change(&pending, change);
        start_pending_waiter(
            context,
            saved,
            pending,
            pending_waiter,
            watcher_cancellation,
        );
        return;
    }
    let permit = match context.sync_in_flight.acquire(source_id.clone()) {
        Ok(Some(permit)) => permit,
        Ok(None) => {
            merge_pending_change(&pending, change);
            start_pending_waiter(
                context,
                saved,
                pending,
                pending_waiter,
                watcher_cancellation,
            );
            return;
        }
        Err(error) => {
            warn!(
                %error,
                source_id = %source_id.as_str(),
                "failed to queue Jellyfin library event reconciliation"
            );
            return;
        }
    };
    start_jellyfin_library_change_reconciliation_thread(
        context,
        saved,
        change,
        permit,
        pending,
        pending_waiter,
        watcher_cancellation,
    );
}

fn start_jellyfin_library_change_reconciliation_thread(
    context: SyncContext,
    saved: SavedSource,
    change: JellyfinLibraryChange,
    permit: InFlightPermit<SourceId>,
    pending: PendingJellyfinChange,
    pending_waiter: Arc<AtomicBool>,
    watcher_cancellation: CancellationToken,
) {
    let source_id = saved.source.id.clone();
    let cancellation = permit.cancellation_token();
    let generation = context
        .store
        .with_store(|store| store.sync_state(&source_id).map(|state| state.generation))
        .unwrap_or(0);
    thread::spawn(move || {
        let permit = permit;
        let started = Instant::now();
        let items_added = change.items_added.len();
        let items_updated = change.items_updated.len();
        let items_removed = change.items_removed.len();
        let result = source_for_saved(&context.store, &context.runtime, &context.secrets, &saved)
            .and_then(|source| match source {
                LoadedSource::Jellyfin(source) => {
                    context.runtime.block_on(reconcile_jellyfin_library_change(
                        &context.store,
                        &source_id,
                        &source,
                        change,
                        generation,
                        &cancellation,
                    ))
                }
                _ => Ok(LibraryDelta::default()),
            });
        if cancellation.cancelled() || watcher_cancellation.cancelled() {
            return;
        }
        let delta = match result {
            Ok(delta) => delta,
            Err(error) => {
                warn!(
                    %error,
                    generation,
                    source_id = %source_id.as_str(),
                    total_elapsed_ms = started.elapsed().as_millis() as u64,
                    "failed Jellyfin library event reconciliation"
                );
                return;
            }
        };
        if !sync_target_is_current(&context.store, &source_id) {
            return;
        }
        info!(
            generation,
            source_id = %source_id.as_str(),
            items_added,
            items_updated,
            items_removed,
            total_elapsed_ms = started.elapsed().as_millis() as u64,
            library_changed = !delta.is_empty(),
            "completed Jellyfin library event reconciliation"
        );
        if !delta.is_empty() {
            send_library_sync_status(
                &context.store,
                &context.events,
                &saved,
                "Cached library ready".to_string(),
                None,
                delta,
            );
        }
        drop(permit);
        if let Some(change) = take_pending_change(&pending) {
            queue_jellyfin_library_change(
                context,
                saved,
                change,
                pending,
                pending_waiter,
                watcher_cancellation,
            );
        }
    });
}

fn start_pending_waiter(
    context: SyncContext,
    saved: SavedSource,
    pending: PendingJellyfinChange,
    pending_waiter: Arc<AtomicBool>,
    watcher_cancellation: CancellationToken,
) {
    if pending_waiter.swap(true, Ordering::AcqRel) {
        return;
    }
    thread::spawn(move || {
        loop {
            if watcher_cancellation.cancelled()
                || !remote_freshness_target_is_active(&context, &saved)
                || !pending_has_change(&pending)
            {
                pending_waiter.store(false, Ordering::Release);
                return;
            }
            let Some(change) = take_pending_change(&pending) else {
                pending_waiter.store(false, Ordering::Release);
                return;
            };
            if !cached_library_exists(&context.store, &saved.source.id) {
                merge_pending_change(&pending, change);
                thread::sleep(RETRY_POLL);
                continue;
            }
            let source_id = saved.source.id.clone();
            match context.sync_in_flight.acquire(source_id) {
                Ok(Some(permit)) => {
                    pending_waiter.store(false, Ordering::Release);
                    start_jellyfin_library_change_reconciliation_thread(
                        context,
                        saved,
                        change,
                        permit,
                        pending,
                        pending_waiter,
                        watcher_cancellation,
                    );
                    return;
                }
                Ok(None) => {
                    merge_pending_change(&pending, change);
                    thread::sleep(RETRY_POLL);
                }
                Err(error) => {
                    warn!(
                        %error,
                        source_id = %saved.source.id.as_str(),
                        "failed to retry Jellyfin library event reconciliation"
                    );
                    merge_pending_change(&pending, change);
                    thread::sleep(RETRY_POLL);
                }
            }
        }
    });
}

fn merge_pending_change(pending: &PendingJellyfinChange, change: JellyfinLibraryChange) {
    let Ok(mut pending) = pending.lock() else {
        return;
    };
    if let Some(existing) = pending.as_mut() {
        existing.merge(change);
    } else {
        *pending = Some(change);
    }
}

fn take_pending_change(pending: &PendingJellyfinChange) -> Option<JellyfinLibraryChange> {
    pending.lock().ok().and_then(|mut pending| pending.take())
}

fn pending_has_change(pending: &PendingJellyfinChange) -> bool {
    pending.lock().ok().is_some_and(|pending| pending.is_some())
}

async fn reconcile_jellyfin_library_change(
    store: &StoreHandle,
    source_id: &SourceId,
    source: &source_jellyfin::JellyfinSource,
    change: JellyfinLibraryChange,
    generation: i64,
    cancellation: &CancellationToken,
) -> Result<LibraryDelta, String> {
    let mut collector = LibraryDeltaCollector::new();
    let fetch_ids = change.fetch_item_ids();
    if !fetch_ids.is_empty() {
        let tracks = await_source(cancellation, source.tracks_by_raw_item_ids(&fetch_ids)).await?;
        check_sync_cancelled(cancellation)?;
        if !tracks.is_empty() {
            collector.merge(
                store.with_store(|store| {
                    store.upsert_tracks_delta(source_id, &tracks, generation)
                })?,
            );
        }
    }

    let removed_track_ids = change.removed_track_ids();
    if !removed_track_ids.is_empty() {
        collector.merge(
            store.with_store(|store| store.delete_tracks_delta(source_id, &removed_track_ids))?,
        );
    }

    let delta = collector.finish();
    if !delta.is_empty() {
        store.with_store(|store| store.refresh_library_counts(source_id))?;
        prune_disk_waveform_cache_entries(source_id, &delta.tracks.deleted);
    }
    Ok(delta)
}

fn sleep_until_retry(cancellation: &CancellationToken, delay: Duration) -> bool {
    let mut slept = Duration::ZERO;
    while slept < delay {
        if cancellation.cancelled() {
            return false;
        }
        let step = (delay - slept).min(RETRY_POLL);
        thread::sleep(step);
        slept += step;
    }
    !cancellation.cancelled()
}

fn active_remote_freshness_target(context: &SyncContext) -> Option<SavedSource> {
    let saved = context
        .store
        .with_store(|store| store.active_source())
        .ok()
        .flatten()?;
    if !active_source_reconciliation_supported(&saved)
        || saved_server_needs_auth(&context.secrets, &saved)
        || !sync_target_is_current(&context.store, &saved.source.id)
    {
        return None;
    }
    Some(saved)
}

fn remote_freshness_target_is_active(context: &SyncContext, saved: &SavedSource) -> bool {
    active_remote_freshness_target(context)
        .as_ref()
        .is_some_and(|active| active.source.id == saved.source.id)
}
