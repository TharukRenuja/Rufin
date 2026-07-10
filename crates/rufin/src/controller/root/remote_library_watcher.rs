use super::*;
use crate::sources::{
    CachedReconciliation, FreshnessOperations, FreshnessWatcher, TrackChanges,
    with_active_source_instance,
};
use std::time::{Duration, Instant};

const RETRY_INITIAL_DELAY: Duration = Duration::from_secs(5);
const RETRY_MAX_DELAY: Duration = Duration::from_secs(60);
const RETRY_POLL: Duration = Duration::from_millis(500);
type PendingTrackChange = Arc<Mutex<Option<TrackChange>>>;

pub(in crate::controller) struct RemoteLibraryWatcher {
    active: Arc<ActiveSource>,
    cancellation: CancellationToken,
}

pub(crate) fn event_freshness_operations(
    feed: TrackChanges,
    reconcile_cached: CachedReconciliation,
) -> FreshnessOperations {
    FreshnessOperations {
        available: Arc::new(|| true),
        reconcile_cached,
        start_watcher: Arc::new(move |context, saved, active| {
            Some(Box::new(RemoteLibraryWatcher::start_events(
                context,
                saved,
                active,
                Arc::clone(&feed),
            )))
        }),
    }
}

pub(crate) fn poll_freshness_operations(
    interval: Duration,
    reconcile_cached: CachedReconciliation,
) -> FreshnessOperations {
    FreshnessOperations {
        available: Arc::new(|| true),
        reconcile_cached,
        start_watcher: Arc::new(move |context, saved, active| {
            Some(Box::new(RemoteLibraryWatcher::start_poll(
                context, saved, active, interval,
            )))
        }),
    }
}

impl RemoteLibraryWatcher {
    fn start_events(
        context: SyncContext,
        saved: SavedSource,
        active: Arc<ActiveSource>,
        feed: TrackChanges,
    ) -> Self {
        let cancellation = CancellationToken::new();
        let thread_cancellation = cancellation.clone();
        let pending = Arc::new(Mutex::new(None));
        let pending_waiter = Arc::new(AtomicBool::new(false));
        let watcher_active = Arc::clone(&active);
        thread::spawn(move || {
            watch_library_events(
                context,
                saved,
                watcher_active,
                feed,
                thread_cancellation,
                pending,
                pending_waiter,
            );
        });
        Self {
            active,
            cancellation,
        }
    }

    fn start_poll(
        context: SyncContext,
        saved: SavedSource,
        active: Arc<ActiveSource>,
        interval: Duration,
    ) -> Self {
        let cancellation = CancellationToken::new();
        let thread_cancellation = cancellation.clone();
        let watcher_active = Arc::clone(&active);
        thread::spawn(move || {
            poll_library(
                context,
                saved,
                watcher_active,
                thread_cancellation,
                interval,
            );
        });
        Self {
            active,
            cancellation,
        }
    }
}

impl Drop for RemoteLibraryWatcher {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

impl FreshnessWatcher for RemoteLibraryWatcher {
    fn active(&self) -> &Arc<ActiveSource> {
        &self.active
    }
}

fn watch_library_events(
    context: SyncContext,
    saved: SavedSource,
    expected: Arc<ActiveSource>,
    feed: TrackChanges,
    cancellation: CancellationToken,
    pending: PendingTrackChange,
    pending_waiter: Arc<AtomicBool>,
) {
    let mut retry_delay = RETRY_INITIAL_DELAY;
    info!(
        source_id = %saved.source.id.as_str(),
        "started remote library event watcher"
    );
    while !cancellation.cancelled()
        && remote_freshness_target_is_active(&context, &saved, &expected)
    {
        let result = context
            .runtime
            .block_on(feed.listen(
                &mut |change| {
                    if !remote_freshness_target_is_active(&context, &saved, &expected) {
                        return false;
                    }
                    queue_library_change(
                        context.clone(),
                        saved.clone(),
                        Arc::clone(&expected),
                        Arc::clone(&feed),
                        change,
                        Arc::clone(&pending),
                        Arc::clone(&pending_waiter),
                        cancellation.clone(),
                    );
                    !cancellation.cancelled()
                },
                &|| {
                    cancellation.cancelled()
                        || !remote_freshness_target_is_active(&context, &saved, &expected)
                },
            ))
            .map_err(|error| error.to_string());
        if cancellation.cancelled()
            || !remote_freshness_target_is_active(&context, &saved, &expected)
        {
            break;
        }
        if let Err(error) = result {
            warn!(
                %error,
                source_id = %saved.source.id.as_str(),
                retry_delay_ms = retry_delay.as_millis() as u64,
                "remote library event watcher disconnected"
            );
        }
        if !sleep_until_retry(&cancellation, retry_delay) {
            break;
        }
        retry_delay = retry_delay.saturating_mul(2).min(RETRY_MAX_DELAY);
    }
    info!(
        source_id = %saved.source.id.as_str(),
        "stopped remote library event watcher"
    );
}

fn poll_library(
    context: SyncContext,
    saved: SavedSource,
    expected: Arc<ActiveSource>,
    cancellation: CancellationToken,
    interval: Duration,
) {
    info!(
        source_id = %saved.source.id.as_str(),
        source = %saved.source.kind,
        interval_ms = interval.as_millis() as u64,
        "started remote library freshness poller"
    );
    while sleep_until_retry(&cancellation, interval)
        && remote_freshness_target_is_active(&context, &saved, &expected)
    {
        start_background_sync_thread(context.clone(), saved.clone());
    }
    info!(
        source_id = %saved.source.id.as_str(),
        source = %saved.source.kind,
        "stopped remote library freshness poller"
    );
}

fn queue_library_change(
    context: SyncContext,
    saved: SavedSource,
    expected: Arc<ActiveSource>,
    feed: TrackChanges,
    change: TrackChange,
    pending: PendingTrackChange,
    pending_waiter: Arc<AtomicBool>,
    watcher_cancellation: CancellationToken,
) {
    let source_id = saved.source.id.clone();
    if !cached_library_exists(&context.store, &source_id) {
        merge_pending_change(&pending, change);
        start_pending_waiter(
            context,
            saved,
            expected,
            feed,
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
                expected,
                feed,
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
                "failed to queue library event reconciliation"
            );
            return;
        }
    };
    start_library_change_reconciliation_thread(
        context,
        saved,
        expected,
        feed,
        change,
        permit,
        pending,
        pending_waiter,
        watcher_cancellation,
    );
}

fn start_library_change_reconciliation_thread(
    context: SyncContext,
    saved: SavedSource,
    expected: Arc<ActiveSource>,
    feed: TrackChanges,
    change: TrackChange,
    permit: InFlightPermit<SourceId>,
    pending: PendingTrackChange,
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
        let items_changed = change.fetch_native_ids.len();
        let items_removed = change.removed_native_ids.len();
        let result = context.runtime.block_on(reconcile_library_change(
            &context.store,
            &source_id,
            &context.active_source,
            &expected,
            feed.as_ref(),
            change,
            generation,
            &cancellation,
        ));
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
                    "failed library event reconciliation"
                );
                return;
            }
        };
        if !sync_target_is_current(&context.store, &source_id)
            || !remote_freshness_target_is_active(&context, &saved, &expected)
        {
            return;
        }
        info!(
            generation,
            source_id = %source_id.as_str(),
            items_changed,
            items_removed,
            total_elapsed_ms = started.elapsed().as_millis() as u64,
            library_changed = !delta.is_empty(),
            "completed library event reconciliation"
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
            queue_library_change(
                context,
                saved,
                expected,
                feed,
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
    expected: Arc<ActiveSource>,
    feed: TrackChanges,
    pending: PendingTrackChange,
    pending_waiter: Arc<AtomicBool>,
    watcher_cancellation: CancellationToken,
) {
    if pending_waiter.swap(true, Ordering::AcqRel) {
        return;
    }
    thread::spawn(move || {
        loop {
            if watcher_cancellation.cancelled()
                || !remote_freshness_target_is_active(&context, &saved, &expected)
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
                    start_library_change_reconciliation_thread(
                        context,
                        saved,
                        expected,
                        feed,
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
                        "failed to retry library event reconciliation"
                    );
                    merge_pending_change(&pending, change);
                    thread::sleep(RETRY_POLL);
                }
            }
        }
    });
}

fn merge_pending_change(pending: &PendingTrackChange, change: TrackChange) {
    let Ok(mut pending) = pending.lock() else {
        return;
    };
    if let Some(existing) = pending.as_mut() {
        existing.merge(change);
    } else {
        *pending = Some(change);
    }
}

fn take_pending_change(pending: &PendingTrackChange) -> Option<TrackChange> {
    pending.lock().ok().and_then(|mut pending| pending.take())
}

fn pending_has_change(pending: &PendingTrackChange) -> bool {
    pending.lock().ok().is_some_and(|pending| pending.is_some())
}

async fn reconcile_library_change(
    store: &StoreHandle,
    source_id: &SourceId,
    active_source: &ActiveSourceSlot,
    expected: &Arc<ActiveSource>,
    feed: &(dyn source::TrackChangeFeed + Send + Sync),
    change: TrackChange,
    generation: i64,
    cancellation: &CancellationToken,
) -> Result<LibraryDelta, String> {
    let tracks = await_source(cancellation, feed.changed_tracks(&change.fetch_native_ids)).await?;
    check_sync_cancelled(cancellation)?;
    let removed_track_ids = change
        .removed_native_ids
        .iter()
        .map(|native_id| feed.track_id_from_native(native_id))
        .collect::<Vec<_>>();
    let delta =
        with_active_source_instance(active_source, expected, || {
            let mut collector = LibraryDeltaCollector::new();
            if !tracks.is_empty() {
                collector.merge(store.with_store(|store| {
                    store.upsert_tracks_delta(source_id, &tracks, generation)
                })?);
            }

            if !removed_track_ids.is_empty() {
                collector.merge(store.with_store(|store| {
                    store.delete_tracks_delta(source_id, &removed_track_ids, generation)
                })?);
            }

            let delta = collector.finish();
            if !delta.is_empty() {
                store.with_store(|store| store.refresh_library_counts(source_id))?;
            }
            Ok(delta)
        })?;
    if !delta.is_empty() {
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

fn remote_freshness_target_is_active(
    context: &SyncContext,
    saved: &SavedSource,
    expected: &Arc<ActiveSource>,
) -> bool {
    sync_target_is_current(&context.store, &saved.source.id)
        && selected_active_source(&context.active_source, &saved.source.id)
            .is_ok_and(|active| Arc::ptr_eq(&active, expected))
}
