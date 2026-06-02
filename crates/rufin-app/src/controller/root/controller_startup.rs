use super::*;
use std::future::Future;
use std::time::{Duration, Instant};

const SYNC_PROGRESS_MIN_INTERVAL: Duration = Duration::from_secs(2);

pub(in crate::controller) fn start_sync_thread(context: SyncContext, saved: SavedServer) {
    let server_id = saved.server.id.clone();
    let permit = match context.sync_in_flight.acquire(server_id.clone()) {
        Ok(Some(permit)) => permit,
        Ok(None) => {
            let _sent = context.events.send(ControllerEvent::LoginStatus(
                "Sync already running.".to_string(),
            ));
            return;
        }
        Err(error) => {
            let _sent = context.events.send(ControllerEvent::Error(error));
            return;
        }
    };

    let prefetch_initial_covers = initial_cover_cache_required(&context.store, &server_id);
    let generation = match context
        .store
        .with_store(|store| store.begin_sync(&server_id))
    {
        Ok(generation) => generation,
        Err(error) => {
            let _sent = context.events.send(ControllerEvent::Error(error));
            return;
        }
    };
    emit_snapshot(&context.store, &context.events);

    thread::spawn(move || {
        let provider_name = provider_display_name(&saved.server.provider);
        let _sent = context.events.send(ControllerEvent::LoginStatus(format!(
            "Syncing {provider_name} library..."
        )));
        let sync_result = run_sync_job(&context, &saved, generation, prefetch_initial_covers);
        drop(permit);
        match sync_result {
            Ok(()) => {
                if !sync_target_is_current(&context.store, &server_id) {
                    return;
                }
                refresh_local_queue_image_refs_after_sync(&context, &saved);
                covers::start_external_metadata_cover_prefetch_thread(
                    covers::ExternalCoverPrefetchRequest {
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
                    },
                );
                let _sent = context.events.send(ControllerEvent::LoginStatus(
                    "Library sync complete".to_string(),
                ));
                match load_snapshot(&context.store) {
                    Ok(snapshot) => {
                        let _sent = context
                            .events
                            .send(ControllerEvent::Snapshot(Box::new(snapshot)));
                    }
                    Err(error) => {
                        let _sent = context.events.send(ControllerEvent::Error(error));
                    }
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
                let _sent = context.events.send(ControllerEvent::Error(error));
            }
        }
    });
}

pub(in crate::controller) fn refresh_local_queue_image_refs_after_sync(
    context: &SyncContext,
    saved: &SavedServer,
) {
    let Some(original_snapshot) = context
        .queue
        .lock()
        .ok()
        .and_then(|queue| queue.as_ref().map(QueueEngine::snapshot))
    else {
        return;
    };
    refresh_local_queue_image_refs_from_snapshot(context, saved, original_snapshot);
}

pub(in crate::controller) fn refresh_local_queue_image_refs_from_snapshot(
    context: &SyncContext,
    saved: &SavedServer,
    original_snapshot: QueueSnapshot,
) {
    if saved.server.provider != LOCAL_PROVIDER_ID {
        return;
    }
    if original_snapshot.server_id != saved.server.id {
        return;
    }
    let mut normalized_entries = original_snapshot.entries.clone();
    if let Err(error) = normalize_local_queue_image_refs_from_albums(
        &context.store,
        &saved.server,
        &mut normalized_entries,
    ) {
        warn!(%error, "failed to refresh local queue image refs after sync");
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

    persist_queue_snapshot_deferred_from_handles(
        context.store.clone(),
        context.events.clone(),
        Arc::clone(&context.queue_persist_generation),
        refreshed_snapshot.clone(),
    );
    sync_playback_snapshot_from_queue_handles(
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
pub(in crate::controller) fn start_playlist_refresh_thread(
    context: PlaylistRefreshContext,
    saved: SavedServer,
) {
    if saved.server.provider == "fake" || saved.server.provider == LOCAL_PROVIDER_ID {
        return;
    }

    let server_id = saved.server.id.clone();
    if sync_is_running(&context.sync_in_flight, &server_id) {
        return;
    }
    let permit = match context.playlist_refresh_in_flight.acquire(server_id) {
        Ok(Some(permit)) => permit,
        Ok(None) => return,
        Err(error) => {
            let _sent = context.events.send(ControllerEvent::Error(error));
            return;
        }
    };

    thread::spawn(move || {
        let result =
            refresh_playlists_for_saved(&context.store, &context.runtime, &context.secrets, &saved)
                .and_then(|()| load_snapshot(&context.store).map(Box::new));
        drop(permit);
        match result {
            Ok(snapshot) => {
                let _sent = context.events.send(ControllerEvent::Snapshot(snapshot));
            }
            Err(error) => {
                warn!(%error, "failed to refresh playlists");
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
pub(in crate::controller) fn start_prefetched_home_section_promotion_thread(
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

    store
        .with_store(|store| {
            let albums = store.load_albums(server_id, 0, 1)?;
            let tracks = store.load_tracks(server_id, 0, 1)?;
            Ok(albums.total == 0 && tracks.total == 0)
        })
        .unwrap_or(true)
}

pub(in crate::controller) fn run_sync_job(
    context: &SyncContext,
    saved: &SavedServer,
    generation: i64,
    prefetch_initial_covers: bool,
) -> Result<(), String> {
    let provider = provider_for_saved(&context.store, &context.runtime, &context.secrets, saved)?;
    let progress = SyncProgressReporter::new(
        Some(context.events.clone()),
        saved.server.name.clone(),
        provider_display_name(&saved.server.provider).to_string(),
    );
    sync_loaded_provider_generation(context, saved, generation, &provider, progress)?;
    if prefetch_initial_covers {
        let _sent = context.events.send(ControllerEvent::LoginStatus(
            "Caching library artwork...".to_string(),
        ));
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
        })?;
    }
    Ok(())
}
fn sync_loaded_provider_generation(
    context: &SyncContext,
    saved: &SavedServer,
    generation: i64,
    provider: &LoadedProvider,
    progress: SyncProgressReporter,
) -> Result<(), String> {
    match provider {
        LoadedProvider::Local(local) => {
            sync_local_provider_generation(context, &saved.server.id, local, generation, progress)
        }
        _ => context.runtime.block_on(sync_provider_generation(
            &context.store,
            &saved.server.id,
            provider.as_music_provider(),
            generation,
            progress,
        )),
    }
}
fn sync_local_provider_generation(
    context: &SyncContext,
    server_id: &ServerId,
    provider: &LocalProvider,
    generation: i64,
    progress: SyncProgressReporter,
) -> Result<(), String> {
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
        ))
}
async fn sync_local_provider_store_generation(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &LocalProvider,
    generation: i64,
    mut progress: SyncProgressReporter,
) -> Result<(), String> {
    let scan = provider.manifest_scan();
    let snapshot = collect_local_provider_snapshot(provider, &mut progress).await?;
    let aggregate_dirty = local_aggregate_image_dirty(store, server_id, &snapshot)?;
    if !scan.library_changed && aggregate_dirty.is_empty() {
        let pruned_cover_entries =
            store.with_store(|store| store.complete_unchanged_local_sync(server_id, generation))?;
        prune_successful_sync_image_cache(store, server_id, pruned_cover_entries);
        info!(
            generation,
            "completed unchanged local sync without library row writes"
        );
        return Ok(());
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
    let delta = local_library_delta(provider.manifest_scan(), snapshot, aggregate_dirty);
    let pruned_cover_entries =
        store.with_store(|store| store.commit_local_library_delta(server_id, generation, delta))?;
    prune_successful_sync_image_cache(store, server_id, pruned_cover_entries);
    progress.finished();
    info!(
        generation,
        finalize_elapsed_ms = finalize_started.elapsed().as_millis() as u64,
        total_elapsed_ms = progress.total_elapsed().as_millis() as u64,
        "completed manifest-delta local sync"
    );
    Ok(())
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
    album_ids: HashSet<AlbumId>,
    artist_ids: HashSet<ArtistId>,
    album_artist_ids: HashSet<ArtistId>,
    genre_names: HashSet<String>,
}
impl LocalAggregateDirty {
    fn is_empty(&self) -> bool {
        self.album_ids.is_empty()
            && self.artist_ids.is_empty()
            && self.album_artist_ids.is_empty()
            && self.genre_names.is_empty()
    }
}
async fn collect_local_provider_snapshot(
    provider: &LocalProvider,
    progress: &mut SyncProgressReporter,
) -> Result<LocalProviderSnapshot, String> {
    progress.collection_started(SyncCollection::Tracks);
    let tracks = load_all_local_tracks_for_matching(provider).await?;
    progress.collection_started(SyncCollection::Albums);
    let albums = load_all_local_albums(provider).await?;
    progress.collection_started(SyncCollection::Artists);
    let artists = load_all_local_artists(provider, false).await?;
    progress.collection_started(SyncCollection::AlbumArtists);
    let album_artists = load_all_local_artists(provider, true).await?;
    progress.collection_started(SyncCollection::Genres);
    let genres = load_all_local_genres(provider).await?;
    progress.collection_started(SyncCollection::HomeSections);
    let home_sections = provider
        .home_sections()
        .await
        .map_err(|error| error.to_string())?;
    Ok(LocalProviderSnapshot {
        tracks,
        albums,
        artists,
        album_artists,
        genres,
        home_sections,
    })
}
async fn load_all_local_albums(provider: &LocalProvider) -> Result<Vec<Album>, String> {
    let mut albums = Vec::new();
    let mut offset = 0;
    loop {
        let page = provider
            .albums(PagedRequest::new(offset, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
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
) -> Result<Vec<Artist>, String> {
    let mut artists = Vec::new();
    let mut offset = 0;
    loop {
        let page = if album_artist {
            provider
                .album_artists(PagedRequest::new(offset, PAGE_SIZE))
                .await
        } else {
            provider.artists(PagedRequest::new(offset, PAGE_SIZE)).await
        }
        .map_err(|error| error.to_string())?;
        let item_count = page.items.len();
        artists.extend(page.items);
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(artists);
        }
    }
}
async fn load_all_local_genres(provider: &LocalProvider) -> Result<Vec<Genre>, String> {
    let mut genres = Vec::new();
    let mut offset = 0;
    loop {
        let page = provider
            .genres(PagedRequest::new(offset, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
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
    let cached_albums = load_all_cached_albums(store, server_id)?;
    let cached_artists = load_all_cached_artists(store, server_id, false)?;
    let cached_album_artists = load_all_cached_artists(store, server_id, true)?;
    let cached_album_refs = cached_albums
        .into_iter()
        .map(|album| (album.id, album.image_ref))
        .collect::<HashMap<_, _>>();
    let cached_artist_refs = cached_artists
        .into_iter()
        .map(|artist| (artist.id, artist.image_ref))
        .collect::<HashMap<_, _>>();
    let cached_album_artist_refs = cached_album_artists
        .into_iter()
        .map(|artist| (artist.id, artist.image_ref))
        .collect::<HashMap<_, _>>();
    let mut dirty = LocalAggregateDirty::default();
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
) -> LocalLibraryDelta {
    let changed_track_ids = scan
        .changed_track_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let metadata_track_ids = scan
        .metadata_track_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let artwork_track_ids = scan
        .artwork_track_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let mut dirty_album_ids = scan.dirty_album_ids.iter().cloned().collect::<HashSet<_>>();
    dirty_album_ids.extend(aggregate_dirty.album_ids);
    let mut dirty_artist_ids = scan
        .dirty_artist_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    dirty_artist_ids.extend(aggregate_dirty.artist_ids);
    let mut dirty_album_artist_ids = scan
        .dirty_album_artist_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    dirty_album_artist_ids.extend(aggregate_dirty.album_artist_ids);
    let mut dirty_genre_names = scan
        .dirty_genre_names
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    dirty_genre_names.extend(aggregate_dirty.genre_names);
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
        deleted_track_ids: scan.deleted_track_ids.clone(),
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
    }
}
fn load_all_cached_albums(store: &StoreHandle, server_id: &ServerId) -> Result<Vec<Album>, String> {
    let mut albums = Vec::new();
    let mut offset = 0;
    loop {
        let page = store.with_store(|store| store.load_albums(server_id, offset, PAGE_SIZE))?;
        let item_count = page.items.len();
        albums.extend(page.items);
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(albums);
        }
    }
}
fn load_all_cached_artists(
    store: &StoreHandle,
    server_id: &ServerId,
    album_artist: bool,
) -> Result<Vec<Artist>, String> {
    let mut artists = Vec::new();
    let mut offset = 0;
    loop {
        let page = store
            .with_store(|store| store.load_artists(server_id, album_artist, offset, PAGE_SIZE))?;
        let item_count = page.items.len();
        artists.extend(page.items);
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(artists);
        }
    }
}
pub(in crate::controller) fn refresh_playlists_for_saved(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
) -> Result<(), String> {
    let provider = provider_for_saved(store, runtime, secrets, saved)?;
    runtime.block_on(refresh_playlist_pages(
        store,
        &saved.server.id,
        provider.as_music_provider(),
    ))
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
    normalize_home_section_image_refs_for_saved(store, saved, &mut section)?;
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
    sync_provider_generation(
        store,
        server_id,
        provider,
        generation,
        SyncProgressReporter::silent(provider),
    )
    .await
}
#[cfg(test)]
pub(in crate::controller) async fn sync_provider_with_events(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
    events: Sender<ControllerEvent>,
) -> Result<(), String> {
    let generation = store.with_store(|store| store.begin_sync(server_id))?;
    sync_provider_generation(
        store,
        server_id,
        provider,
        generation,
        SyncProgressReporter::for_provider(provider, Some(events)),
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
    sync_local_provider_store_generation(
        store,
        server_id,
        provider,
        generation,
        SyncProgressReporter::for_provider(provider, Some(events)),
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
) -> Result<(), String> {
    info!(generation, "started provider cache sync");
    sync_album_pages(store, server_id, provider, generation, &mut progress).await?;
    sync_track_pages(store, server_id, provider, generation, &mut progress).await?;
    progress.collection_started(SyncCollection::MusicFolders);
    sync_music_folders(store, server_id, provider, generation).await?;
    progress.collection_started(SyncCollection::Artists);
    sync_artist_pages(store, server_id, provider, generation, false).await?;
    progress.collection_started(SyncCollection::AlbumArtists);
    sync_artist_pages(store, server_id, provider, generation, true).await?;
    progress.collection_started(SyncCollection::Genres);
    sync_genre_pages(store, server_id, provider, generation).await?;
    progress.collection_started(SyncCollection::Playlists);
    sync_playlist_pages(store, server_id, provider, generation).await?;
    progress.collection_started(SyncCollection::HomeSections);
    sync_home_sections(store, server_id, provider, generation).await?;
    progress.finalizing();
    let finalize_started = Instant::now();
    store.with_store(|store| store.refresh_library_counts(server_id))?;
    let pruned_cover_entries =
        store.with_store(|store| store.complete_sync(server_id, generation))?;
    prune_successful_sync_image_cache(store, server_id, pruned_cover_entries);
    let finalize_elapsed = finalize_started.elapsed();
    progress.finished();
    if let Err(error) = refresh_local_track_matches(store, server_id, Some(generation)).await {
        warn!(%error, "failed to refresh local track matches");
    }
    info!(
        generation,
        finalize_elapsed_ms = finalize_elapsed.as_millis() as u64,
        total_elapsed_ms = progress.total_elapsed().as_millis() as u64,
        "completed provider cache sync"
    );
    Ok(())
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
        self.emit_status(
            true,
            format!(
                "Caching library... This may take some time. Fetching {} for {} ({})",
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
                "Caching library... This may take some time. Fetching {} page {page_number} for {}, {count} fetched ({})",
                collection.label(),
                self.source_label(),
                elapsed_label(self.total_elapsed())
            ),
        );
    }

    pub(in crate::controller) fn page_written(&mut self, progress: SyncPageProgress) {
        let fetched = progress_count_label(progress.fetched, progress.total);
        let page = page_label(progress.page_number, progress.total);
        self.emit_status(
            progress.finished,
            format!(
                "Caching library... This may take some time. Cached {} {page} for {}, {fetched} fetched, {} cached ({})",
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
                "Caching library... This may take some time. Finalizing cache for {} ({})",
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
        if self.source_name.trim().is_empty() || self.source_name == self.provider_kind {
            return self.provider_kind.clone();
        }
        format!("{} ({})", self.source_name, self.provider_kind)
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
    page: Fut,
) -> rufin_provider::ProviderResult<rufin_provider::PagedResponse<T>>
where
    Fut: Future<Output = rufin_provider::ProviderResult<rufin_provider::PagedResponse<T>>>,
{
    progress.page_fetching(collection, page_number, fetched, total);
    tokio::pin!(page);
    loop {
        tokio::select! {
            result = &mut page => return result,
            _ = tokio::time::sleep(progress.min_interval) => {
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
) -> Result<(), String> {
    progress.collection_started(SyncCollection::Albums);
    let mut offset = 0;
    let mut page_number = 0;
    loop {
        page_number += 1;
        let fetch_started = Instant::now();
        let page = fetch_page_with_progress(
            progress,
            SyncCollection::Albums,
            page_number,
            offset,
            None,
            provider.albums(PagedRequest::new(offset, PAGE_SIZE)),
        )
        .await
        .map_err(|error| error.to_string())?;
        let fetch_elapsed = fetch_started.elapsed();
        let write_started = Instant::now();
        store.with_store(|store| store.upsert_albums(server_id, &page.items, generation))?;
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
) -> Result<(), String> {
    progress.collection_started(SyncCollection::Tracks);
    let mut offset = 0;
    let mut page_number = 0;
    loop {
        page_number += 1;
        let fetch_started = Instant::now();
        let page = fetch_page_with_progress(
            progress,
            SyncCollection::Tracks,
            page_number,
            offset,
            None,
            provider.tracks(PagedRequest::new(offset, PAGE_SIZE)),
        )
        .await
        .map_err(|error| error.to_string())?;
        let fetch_elapsed = fetch_started.elapsed();
        let write_started = Instant::now();
        store.with_store(|store| store.upsert_tracks(server_id, &page.items, generation))?;
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
) -> Result<(), String> {
    if !provider.capabilities().music_folders {
        return Ok(());
    }
    let folders = provider
        .music_folders()
        .await
        .map_err(|error| error.to_string())?;
    store.with_store(|store| store.upsert_music_folders(server_id, &folders, generation))?;
    for folder in folders {
        let mut offset = 0;
        loop {
            let page = provider
                .tracks_in_music_folder(&folder.id, PagedRequest::new(offset, PAGE_SIZE))
                .await
                .map_err(|error| error.to_string())?;
            store.with_store(|store| store.upsert_tracks(server_id, &page.items, generation))?;
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
    Ok(())
}
pub(in crate::controller) async fn refresh_local_track_matches(
    store: &StoreHandle,
    server_id: &ServerId,
    manifest_generation: Option<i64>,
) -> Result<usize, String> {
    let Some(access) = store.with_store(|store| store.server_local_access(server_id))? else {
        return Ok(0);
    };
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
    let remote_tracks =
        store.with_store(|store| store.load_tracks_for_local_matching(server_id))?;
    if remote_tracks.is_empty() {
        store.with_store(|store| store.replace_track_local_matches(server_id, &[]))?;
        return Ok(0);
    }
    let root = PathBuf::from(&access.root_path);
    let manifest_cache = store.with_store(|store| store.load_local_manifest(server_id))?;
    let local_identity =
        LocalProvider::identity_for_root(&root).map_err(|error| error.to_string())?;
    let local_provider =
        LocalProvider::from_roots_with_manifest_cache(vec![root], local_identity, manifest_cache)
            .map_err(|error| error.to_string())?;
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
    let local_tracks = load_all_local_tracks_for_matching(&local_provider).await?;
    let matches = conservative_local_matches(&remote_tracks, &local_tracks);
    let count = matches.len();
    store.with_store(|store| {
        store.replace_track_local_matches(server_id, &matches)?;
        store.replace_local_manifest(
            server_id,
            manifest_generation.unwrap_or_default(),
            &local_provider.manifest_scan().entries,
        )
    })?;
    debug!(server_id = %server_id, count, "refreshed local track matches");
    Ok(count)
}
async fn load_all_local_tracks_for_matching(
    provider: &LocalProvider,
) -> Result<Vec<Track>, String> {
    let mut tracks = Vec::new();
    let mut offset = 0;
    loop {
        let page = provider
            .tracks(PagedRequest::new(offset, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
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

    let remote_tracks =
        store.with_store(|store| store.load_tracks_for_local_matching(&server.id))?;
    let metadata_matches = store.with_store(|store| store.track_local_match_paths(&server.id))?;
    let metadata_by_track = metadata_matches
        .into_iter()
        .collect::<HashMap<TrackId, String>>();

    let sample_track = remote_tracks
        .iter()
        .find(|track| {
            track
                .local_path
                .as_deref()
                .is_some_and(|path| !path.trim().is_empty())
                && metadata_by_track.contains_key(&track.id)
        })
        .or_else(|| {
            remote_tracks.iter().find(|track| {
                track
                    .local_path
                    .as_deref()
                    .is_some_and(|path| !path.trim().is_empty())
            })
        });
    let sample_server_path = sample_track.and_then(|track| track.local_path.clone());
    let sample_local_path = sample_track.and_then(|track| {
        metadata_by_track.get(&track.id).cloned().or_else(|| {
            track
                .local_path
                .as_deref()
                .and_then(|raw| potential_local_path_text(raw, access))
        })
    });

    let mut effective_matches = HashSet::<TrackId>::new();
    let mut direct_match_count = 0;
    let mut prefix_match_count = 0;
    for track in &remote_tracks {
        let Some(raw) = track.local_path.as_deref() else {
            continue;
        };
        if map_server_path_to_local(raw, access).is_some() {
            prefix_match_count += 1;
            effective_matches.insert(track.id.clone());
        } else if Path::new(raw).is_absolute() {
            direct_match_count += 1;
            effective_matches.insert(track.id.clone());
        }
    }

    let metadata_match_count = metadata_by_track.len();
    for track_id in metadata_by_track.into_keys() {
        effective_matches.insert(track_id);
    }

    let total_track_count = remote_tracks.len();
    let unmatched_count = total_track_count.saturating_sub(effective_matches.len());
    Ok(LocalAccessStatus {
        sample_server_path,
        sample_local_path,
        direct_match_count,
        prefix_match_count,
        metadata_match_count,
        unmatched_count,
        total_track_count,
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
) -> Result<(), String> {
    let mut offset = 0;
    loop {
        let page = if album_artist {
            provider
                .album_artists(PagedRequest::new(offset, PAGE_SIZE))
                .await
        } else {
            provider.artists(PagedRequest::new(offset, PAGE_SIZE)).await
        }
        .map_err(|error| error.to_string())?;
        store.with_store(|store| {
            store.upsert_artists(server_id, &page.items, album_artist, generation)
        })?;
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
) -> Result<(), String> {
    let mut offset = 0;
    loop {
        let page = provider
            .genres(PagedRequest::new(offset, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        store.with_store(|store| store.upsert_genres(server_id, &page.items, generation))?;
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
) -> Result<(), String> {
    let mut offset = 0;
    loop {
        let page = provider
            .playlists(PagedRequest::new(offset, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
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
            return Ok(());
        }
    }
}
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
) -> Result<(), String> {
    let sections = provider
        .home_sections()
        .await
        .map_err(|error| error.to_string())?;
    cache_home_sections(store, server_id, &sections, generation)
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
