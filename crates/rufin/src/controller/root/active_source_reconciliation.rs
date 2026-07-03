use super::*;
use std::time::Instant;

pub(in crate::controller) fn start_cached_active_source_reconciliation_thread(
    context: SyncContext,
    saved: SavedServer,
) {
    let cached_current = sync_target_is_current(&context.store, &saved.server.id)
        && cached_library_exists(&context.store, &saved.server.id);
    if !cached_current {
        return;
    }
    if saved.server.provider == LOCAL_SOURCE_ID {
        if !load_settings_from_store(&context.store)
            .sources
            .local_folders
            .is_empty()
        {
            start_silent_sync_thread(context, saved);
        }
        return;
    }
    if active_source_reconciliation_supported(&saved) {
        start_remote_active_source_reconciliation_thread(context, saved);
    }
}

pub(in crate::controller) fn active_source_reconciliation_supported(saved: &SavedServer) -> bool {
    matches!(
        saved.server.provider.as_str(),
        "jellyfin" | "navidrome" | "subsonic" | "opensubsonic"
    )
}

fn start_remote_active_source_reconciliation_thread(context: SyncContext, saved: SavedServer) {
    let server_id = saved.server.id.clone();
    let Ok(Some(permit)) = context.sync_in_flight.acquire(server_id.clone()) else {
        return;
    };
    let cancellation = permit.cancellation_token();
    let generation = context
        .store
        .with_store(|store| store.sync_state(&server_id).map(|state| state.generation))
        .unwrap_or(0);
    thread::spawn(move || {
        let _permit = permit;
        let started = Instant::now();
        let result = source_for_saved(&context.store, &context.runtime, &context.secrets, &saved)
            .and_then(|source| {
                context.runtime.block_on(reconcile_active_source(
                    &context.store,
                    &server_id,
                    &source,
                    generation,
                    &cancellation,
                ))
            });
        if cancellation.cancelled() {
            return;
        }
        let delta = match result {
            Ok(delta) => delta,
            Err(error) => {
                warn!(
                    %error,
                    generation,
                    server_id = %server_id.as_str(),
                    total_elapsed_ms = started.elapsed().as_millis() as u64,
                    "failed active source reconciliation"
                );
                return;
            }
        };
        if !sync_target_is_current(&context.store, &server_id) {
            return;
        }
        info!(
            generation,
            server_id = %server_id.as_str(),
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

pub(in crate::controller) async fn reconcile_active_source(
    store: &StoreHandle,
    server_id: &ServerId,
    source: &LoadedSource,
    generation: i64,
    cancellation: &CancellationToken,
) -> Result<LibraryDelta, String> {
    match source {
        LoadedSource::Jellyfin(source) => {
            reconcile_jellyfin_active_source(store, server_id, source, generation, cancellation)
                .await
        }
        LoadedSource::Subsonic(source) => {
            reconcile_subsonic_active_source(store, server_id, source, generation, cancellation)
                .await
        }
        LoadedSource::Local(_) => Ok(LibraryDelta::default()),
    }
}

async fn reconcile_jellyfin_active_source(
    store: &StoreHandle,
    server_id: &ServerId,
    source: &source_jellyfin::JellyfinSource,
    generation: i64,
    cancellation: &CancellationToken,
) -> Result<LibraryDelta, String> {
    let mut collector = LibraryDeltaCollector::new();
    let tracks = await_source(cancellation, source.recently_added_tracks(50)).await?;
    check_sync_cancelled(cancellation)?;
    if !tracks.is_empty() {
        let delta =
            store.with_store(|store| store.upsert_tracks_delta(server_id, &tracks, generation))?;
        collector.merge(delta);
    }
    let delta = collector.finish();
    if !delta.is_empty() {
        store.with_store(|store| store.refresh_library_counts(server_id))?;
    }
    Ok(delta)
}

async fn reconcile_subsonic_active_source(
    store: &StoreHandle,
    server_id: &ServerId,
    source: &source_subsonic::SubsonicSource,
    generation: i64,
    cancellation: &CancellationToken,
) -> Result<LibraryDelta, String> {
    const NEWEST_ALBUM_LIMIT: usize = 20;

    let newest_albums =
        await_source(cancellation, source.newest_albums(NEWEST_ALBUM_LIMIT)).await?;
    check_sync_cancelled(cancellation)?;
    if newest_albums.is_empty() {
        return Ok(LibraryDelta::default());
    }

    let albums_needing_detail = store
        .with_store(|store| subsonic_albums_needing_detail(store, server_id, &newest_albums))?;
    let mut detail_albums = Vec::new();
    let mut detail_tracks = Vec::new();
    for album_id in albums_needing_detail {
        let detail = await_source(cancellation, source.album_detail(&album_id)).await?;
        check_sync_cancelled(cancellation)?;
        detail_albums.push(detail.album);
        detail_tracks.extend(detail.tracks);
    }

    let mut collector = LibraryDeltaCollector::new();
    collector
        .merge(store.with_store(|store| {
            store.upsert_albums_delta(server_id, &newest_albums, generation)
        })?);
    if !detail_albums.is_empty() {
        collector.merge(store.with_store(|store| {
            store.upsert_albums_delta(server_id, &detail_albums, generation)
        })?);
    }
    if !detail_tracks.is_empty() {
        collector.merge(store.with_store(|store| {
            store.upsert_tracks_delta(server_id, &detail_tracks, generation)
        })?);
    }

    let delta = collector.finish();
    if !delta.is_empty() {
        store.with_store(|store| store.refresh_library_counts(server_id))?;
    }
    Ok(delta)
}

fn subsonic_albums_needing_detail(
    store: &Store,
    server_id: &ServerId,
    newest_albums: &[Album],
) -> StoreResult<Vec<AlbumId>> {
    let album_ids = newest_albums
        .iter()
        .map(|album| album.id.clone())
        .collect::<Vec<_>>();
    let cached_albums = store
        .load_albums_by_ids(server_id, &album_ids)?
        .into_iter()
        .map(|album| (album.id.clone(), album))
        .collect::<HashMap<_, _>>();

    Ok(newest_albums
        .iter()
        .filter(|album| {
            cached_albums
                .get(&album.id)
                .is_none_or(|cached| subsonic_album_detail_needed(cached, album))
        })
        .map(|album| album.id.clone())
        .collect())
}

fn subsonic_album_detail_needed(cached: &Album, newest: &Album) -> bool {
    cached.track_count != newest.track_count || cached.duration_seconds != newest.duration_seconds
}
