use super::*;
use crate::sources::{active_source_instance_is_current, with_active_source_instance};
use std::time::Instant;

type ReconcileActiveSource = Arc<
    dyn Fn(
            &SyncContext,
            &SourceId,
            &Arc<ActiveSource>,
            i64,
            &CancellationToken,
        ) -> Result<LibraryDelta, String>
        + Send
        + Sync,
>;

impl AppController {
    pub fn refresh_source_freshness_watcher(&self) {
        refresh_source_freshness_watcher(
            self.sync_context(),
            Arc::clone(&self.source_freshness_watcher),
        );
    }
}

/// Watch only the selected source, not sources being synced manually
pub(in crate::controller) fn refresh_source_freshness_watcher(
    context: SyncContext,
    slot: Arc<Mutex<Option<Box<dyn crate::sources::FreshnessWatcher>>>>,
) {
    let target = context
        .store
        .with_store(|store| store.active_source())
        .ok()
        .flatten()
        .and_then(|saved| {
            selected_active_source(&context.active_source, &saved.source.id)
                .ok()
                .map(|active| (saved, active))
        });
    let Ok(mut current) = slot.lock() else {
        return;
    };
    let Some((saved, active)) = target else {
        *current = None;
        return;
    };
    if current
        .as_ref()
        .is_some_and(|watcher| Arc::ptr_eq(watcher.active(), &active))
    {
        return;
    }
    let start_watcher = active
        .freshness
        .as_ref()
        .map(|freshness| Arc::clone(&freshness.start_watcher));
    *current = start_watcher.and_then(|start| start(context, saved, active));
}

pub(in crate::controller) fn start_cached_active_source_reconciliation_thread(
    context: SyncContext,
    saved: SavedSource,
) {
    let cached_current = sync_target_is_current(&context.store, &saved.source.id)
        && cached_library_exists(&context.store, &saved.source.id);
    if !cached_current {
        return;
    }
    let Ok(active) = selected_active_source(&context.active_source, &saved.source.id) else {
        return;
    };
    let Some(reconcile_cached) = active
        .freshness
        .as_ref()
        .map(|freshness| Arc::clone(&freshness.reconcile_cached))
    else {
        return;
    };
    reconcile_cached(context, saved, active);
}

fn start_incremental_reconciliation_thread(
    context: SyncContext,
    saved: SavedSource,
    active: Arc<ActiveSource>,
    reconcile: ReconcileActiveSource,
) {
    let source_id = saved.source.id.clone();
    let Ok(Some(permit)) = context.sync_in_flight.acquire(source_id.clone()) else {
        return;
    };
    let cancellation = permit.cancellation_token();
    let generation = context
        .store
        .with_store(|store| store.sync_state(&source_id).map(|state| state.generation))
        .unwrap_or(0);
    thread::spawn(move || {
        let _permit = permit;
        let started = Instant::now();
        if !active_reconciliation_target_is_current(&context, &source_id, &active) {
            return;
        }
        let result = reconcile(&context, &source_id, &active, generation, &cancellation);
        if cancellation.cancelled() {
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
                    "failed active source reconciliation"
                );
                return;
            }
        };
        if !active_reconciliation_target_is_current(&context, &source_id, &active) {
            return;
        }
        info!(
            generation,
            source_id = %source_id.as_str(),
            total_elapsed_ms = started.elapsed().as_millis() as u64,
            library_changed = !delta.is_empty(),
            "completed active source reconciliation"
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
    });
}

pub(crate) fn full_ingest_cached_reconciliation() -> crate::sources::CachedReconciliation {
    Arc::new(|context, saved, _active| start_silent_sync_thread(context, saved))
}

pub(crate) fn recent_tracks_cached_reconciliation(
    source: crate::sources::RecentTracks,
    limit: usize,
) -> crate::sources::CachedReconciliation {
    let reconcile: ReconcileActiveSource = Arc::new(
        move |context, source_id, active, generation, cancellation| {
            context.runtime.block_on(reconcile_recent_tracks(
                &context.store,
                source_id,
                source.as_ref(),
                limit,
                generation,
                cancellation,
                &context.active_source,
                active,
            ))
        },
    );
    Arc::new(move |context, saved, active| {
        start_incremental_reconciliation_thread(context, saved, active, Arc::clone(&reconcile));
    })
}

pub(crate) fn recent_albums_cached_reconciliation(
    core: crate::sources::LibraryCore,
    source: crate::sources::RecentAlbums,
    limit: usize,
) -> crate::sources::CachedReconciliation {
    let reconcile: ReconcileActiveSource = Arc::new(
        move |context, source_id, active, generation, cancellation| {
            context.runtime.block_on(reconcile_recent_albums(
                &context.store,
                source_id,
                core.as_ref(),
                source.as_ref(),
                limit,
                generation,
                cancellation,
                &context.active_source,
                active,
            ))
        },
    );
    Arc::new(move |context, saved, active| {
        start_incremental_reconciliation_thread(context, saved, active, Arc::clone(&reconcile));
    })
}

async fn reconcile_recent_tracks(
    store: &StoreHandle,
    source_id: &SourceId,
    source: &(dyn source::RecentTrackProvider + Send + Sync),
    limit: usize,
    generation: i64,
    cancellation: &CancellationToken,
    active_source: &ActiveSourceSlot,
    expected: &Arc<ActiveSource>,
) -> Result<LibraryDelta, String> {
    let mut collector = LibraryDeltaCollector::new();
    let tracks = await_source(cancellation, source.recent_tracks(limit)).await?;
    check_sync_cancelled(cancellation)?;
    with_active_source_instance(active_source, expected, || {
        if !tracks.is_empty() {
            let delta = store
                .with_store(|store| store.upsert_tracks_delta(source_id, &tracks, generation))?;
            collector.merge(delta);
        }
        let delta = collector.finish();
        if !delta.is_empty() {
            store.with_store(|store| store.refresh_library_counts(source_id))?;
        }
        Ok(delta)
    })
}

async fn reconcile_recent_albums(
    store: &StoreHandle,
    source_id: &SourceId,
    core: &(dyn MusicSource + Send + Sync),
    source: &(dyn source::RecentAlbumProvider + Send + Sync),
    limit: usize,
    generation: i64,
    cancellation: &CancellationToken,
    active_source: &ActiveSourceSlot,
    expected: &Arc<ActiveSource>,
) -> Result<LibraryDelta, String> {
    let newest_albums = await_source(cancellation, source.recent_albums(limit)).await?;
    check_sync_cancelled(cancellation)?;
    ensure_active_instance(active_source, expected)?;
    if newest_albums.is_empty() {
        return Ok(LibraryDelta::default());
    }

    let albums_needing_detail =
        store.with_store(|store| albums_needing_detail(store, source_id, &newest_albums))?;
    let mut detail_albums = Vec::new();
    let mut detail_tracks = Vec::new();
    for album_id in albums_needing_detail {
        let detail = await_source(cancellation, core.album_detail(&album_id)).await?;
        check_sync_cancelled(cancellation)?;
        ensure_active_instance(active_source, expected)?;
        detail_albums.push(detail.album);
        detail_tracks.extend(detail.tracks);
    }

    with_active_source_instance(active_source, expected, || {
        let mut collector = LibraryDeltaCollector::new();
        collector.merge(store.with_store(|store| {
            store.upsert_albums_delta(source_id, &newest_albums, generation)
        })?);
        if !detail_albums.is_empty() {
            collector.merge(store.with_store(|store| {
                store.upsert_albums_delta(source_id, &detail_albums, generation)
            })?);
        }
        if !detail_tracks.is_empty() {
            collector.merge(store.with_store(|store| {
                store.upsert_tracks_delta(source_id, &detail_tracks, generation)
            })?);
        }

        let delta = collector.finish();
        if !delta.is_empty() {
            store.with_store(|store| store.refresh_library_counts(source_id))?;
        }
        Ok(delta)
    })
}

fn active_reconciliation_target_is_current(
    context: &SyncContext,
    source_id: &SourceId,
    expected: &Arc<ActiveSource>,
) -> bool {
    sync_target_is_current(&context.store, source_id)
        && active_source_instance_is_current(&context.active_source, expected)
}

fn ensure_active_instance(
    slot: &ActiveSourceSlot,
    expected: &Arc<ActiveSource>,
) -> Result<(), String> {
    active_source_instance_is_current(slot, expected)
        .then_some(())
        .ok_or_else(|| "The selected source changed during reconciliation.".to_string())
}

fn albums_needing_detail(
    store: &Store,
    source_id: &SourceId,
    newest_albums: &[Album],
) -> StoreResult<Vec<AlbumId>> {
    let album_ids = newest_albums
        .iter()
        .map(|album| album.id.clone())
        .collect::<Vec<_>>();
    let cached_albums = store
        .load_albums_by_ids(source_id, &album_ids)?
        .into_iter()
        .map(|album| (album.id.clone(), album))
        .collect::<HashMap<_, _>>();

    Ok(newest_albums
        .iter()
        .filter(|album| {
            cached_albums
                .get(&album.id)
                .is_none_or(|cached| album_detail_needed(cached, album))
        })
        .map(|album| album.id.clone())
        .collect())
}

fn album_detail_needed(cached: &Album, newest: &Album) -> bool {
    cached.track_count != newest.track_count || cached.duration_seconds != newest.duration_seconds
}
