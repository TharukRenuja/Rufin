use super::*;
use std::time::Instant;

pub(in crate::controller) fn promote_prefetched_home_section(
    store: &StoreHandle,
    server_id: &ServerId,
    section: &HomeSection,
) -> Result<(), String> {
    let generation =
        store.with_store(|store| store.sync_state(server_id).map(|state| state.generation))?;
    cache_home_section(store, server_id, section, generation)?;
    store.with_store(|store| store.clear_home_section_prefetch(server_id, section.kind))?;
    Ok(())
}
#[cfg(test)]
pub(in crate::controller) fn cache_home_sections(
    store: &StoreHandle,
    server_id: &ServerId,
    sections: &[HomeSection],
    generation: i64,
) -> Result<(), String> {
    let sections = source_scoped_home_sections(server_id, sections);
    for section in &sections {
        cache_home_section_items(store, server_id, section, generation)?;
    }
    store.with_store(|store| store.upsert_home_sections(server_id, &sections, generation))?;
    Ok(())
}
pub(in crate::controller) fn cache_home_section(
    store: &StoreHandle,
    server_id: &ServerId,
    section: &HomeSection,
    generation: i64,
) -> Result<(), String> {
    let section = source_scoped_home_section(server_id, section);
    cache_home_section_items(store, server_id, &section, generation)?;
    store.with_store(|store| store.upsert_home_section(server_id, &section, generation))?;
    Ok(())
}
pub(in crate::controller) fn cache_home_section_items(
    store: &StoreHandle,
    server_id: &ServerId,
    section: &HomeSection,
    generation: i64,
) -> Result<(), String> {
    if !section.albums.is_empty() {
        store.with_store(|store| store.upsert_albums(server_id, &section.albums, generation))?;
    }
    if !section.tracks.is_empty() {
        store.with_store(|store| store.upsert_tracks(server_id, &section.tracks, generation))?;
    }
    Ok(())
}
#[cfg(test)]
fn source_scoped_home_sections(server_id: &ServerId, sections: &[HomeSection]) -> Vec<HomeSection> {
    sections
        .iter()
        .map(|section| source_scoped_home_section(server_id, section))
        .collect()
}
fn source_scoped_home_section(server_id: &ServerId, section: &HomeSection) -> HomeSection {
    if server_id.as_str() != LOCAL_SOURCE_SERVER_ID {
        return section.clone();
    }
    let mut section = section.clone();
    section.albums.retain(|album| is_local_album_id(&album.id));
    section.tracks.retain(|track| is_local_track_id(&track.id));
    let saved = local_source_saved();
    scrub_home_refs(&saved, &mut section);
    section
}
pub(in crate::controller) fn sync_page_finished(
    item_count: usize,
    total: usize,
    offset: usize,
) -> bool {
    item_count == 0 || (total > 0 && offset >= total) || (total == 0 && item_count < PAGE_SIZE)
}
#[cfg(test)]
pub(in crate::controller) fn home_refresh_section_kinds() -> [HomeSectionKind; 5] {
    [
        HomeSectionKind::Explore,
        HomeSectionKind::MostPlayed,
        HomeSectionKind::NewlyAdded,
        HomeSectionKind::RecentlyPlayed,
        HomeSectionKind::RecentlyReleased,
    ]
}
pub(in crate::controller) fn load_snapshot(store: &StoreHandle) -> Result<LibrarySnapshot, String> {
    let source_settings = load_settings_from_store(store);
    let saved_servers = store.with_store(|store| store.list_servers())?;
    let remote_saved_servers = saved_servers
        .iter()
        .filter(|saved| saved.server.provider != LOCAL_PROVIDER_ID)
        .cloned()
        .collect::<Vec<_>>();
    let servers = remote_saved_servers
        .iter()
        .map(|saved| saved.server.clone())
        .collect::<Vec<_>>();
    let server_local_access = remote_saved_servers
        .iter()
        .map(|saved| {
            let access = store.with_store(|store| store.server_local_access(&saved.server.id))?;
            let status = local_access_status_for_server(store, &saved.server, access.as_ref())?;
            let sync_state = store
                .with_store(|store| store.sync_state(&saved.server.id))
                .ok();
            let sync_status = sync_state
                .as_ref()
                .map(sync_status_text)
                .unwrap_or_else(|| "Cached library ready".to_string());
            let cached_album_count = store
                .with_store(|store| {
                    store
                        .load_albums(&saved.server.id, 0, 1)
                        .map(|page| page.total)
                })
                .unwrap_or_default();
            let cached_track_count = store
                .with_store(|store| {
                    store
                        .load_tracks(&saved.server.id, 0, 1)
                        .map(|page| page.total)
                })
                .unwrap_or_default();
            let selected_music_folder_name = store
                .with_store(|store| {
                    let selected = store.selected_music_folder_id(&saved.server.id)?;
                    let folders = store.list_music_folders(&saved.server.id)?;
                    Ok(selected.and_then(|selected| {
                        folders
                            .into_iter()
                            .find(|folder| folder.id == selected)
                            .map(|folder| folder.name)
                    }))
                })
                .unwrap_or_default();
            Ok(ServerLocalAccessSnapshot {
                server_id: saved.server.id.clone(),
                access,
                status,
                selected_music_folder_name,
                username: Some(saved.username.clone()),
                trust_invalid_cert: saved.trust_invalid_cert,
                sync_status,
                cached_album_count,
                cached_track_count,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let Some(reconciled_source) =
        reconcile_snapshot_source(store, &source_settings, &remote_saved_servers)?
    else {
        let mut snapshot = LibrarySnapshot::first_run();
        snapshot.servers = servers;
        snapshot.local_folders = source_settings.sources.local_folders.clone();
        snapshot.server_local_access = server_local_access;
        return Ok(snapshot);
    };
    let SnapshotSourceReconciliation {
        selected_source,
        saved,
    } = reconciled_source;
    let (local_access, local_access_status) = if saved.server.provider != LOCAL_PROVIDER_ID
        && let Some(summary) = server_local_access
            .iter()
            .find(|summary| summary.server_id == saved.server.id)
    {
        (summary.access.clone(), summary.status.clone())
    } else {
        let local_access = store.with_store(|store| store.server_local_access(&saved.server.id))?;
        let local_access_status =
            local_access_status_for_server(store, &saved.server, local_access.as_ref())?;
        (local_access, local_access_status)
    };
    let music_folders = store.with_store(|store| store.list_music_folders(&saved.server.id))?;
    let selected_music_folder_id =
        store.with_store(|store| store.selected_music_folder_id(&saved.server.id))?;
    let metadata_settings = load_settings_for_saved(store, &saved);
    let sync_state = store
        .with_store(|store| store.sync_state(&saved.server.id))
        .ok();
    store.with_store(|store| store.repair_artwork_projections(&saved.server.id))?;
    store.with_store(|store| store.ensure_collection_cover_refs(&saved.server.id))?;
    let mut home_sections = store.with_store(|store| store.load_home_sections(&saved.server.id))?;
    let mut prefetched_explore = store.with_store(|store| {
        store.load_home_section_prefetch(&saved.server.id, HomeSectionKind::Explore)
    })?;
    let album_page =
        store.with_store(|store| store.load_albums(&saved.server.id, 0, SNAPSHOT_GRID_LIMIT))?;
    let track_page =
        store.with_store(|store| store.load_tracks(&saved.server.id, 0, SNAPSHOT_TRACK_LIMIT))?;
    let cached_album_count = album_page.total;
    let cached_track_count = track_page.total;
    let mut albums = album_page.items;
    let mut tracks = track_page.items;
    let artist_page = store
        .with_store(|store| store.load_artists(&saved.server.id, false, 0, SNAPSHOT_GRID_LIMIT))?;
    let album_artist_page = store
        .with_store(|store| store.load_artists(&saved.server.id, true, 0, SNAPSHOT_GRID_LIMIT))?;
    let genre_page =
        store.with_store(|store| store.load_genres(&saved.server.id, 0, SNAPSHOT_GRID_LIMIT))?;
    let playlist_page =
        store.with_store(|store| store.load_playlists(&saved.server.id, 0, SNAPSHOT_GRID_LIMIT))?;
    let cached_artist_count = artist_page.total;
    let cached_album_artist_count = album_artist_page.total;
    let cached_genre_count = genre_page.total;
    let cached_playlist_count = playlist_page.total;
    let mut artists = artist_page.items;
    let mut album_artists = album_artist_page.items;
    let mut genres = genre_page.items;
    let mut playlists = playlist_page.items;
    let playlist_ids = playlists
        .iter()
        .map(|playlist| playlist.id.clone())
        .collect::<Vec<_>>();
    let playlist_entry_keys = store.with_store(|store| {
        store.playlist_entry_keys_for_playlists(&saved.server.id, &playlist_ids)
    })?;
    let mut favorites = store.with_store(|store| store.load_favorite_tracks(&saved.server.id))?;
    scrub_snapshot_image_refs(
        &saved,
        &mut home_sections,
        prefetched_explore.as_mut(),
        &mut albums,
        &mut tracks,
        &mut artists,
        &mut album_artists,
        &mut genres,
        &mut playlists,
        &mut favorites,
        external_metadata::enabled(&metadata_settings),
    );
    cover_art_policy::bind_home_sections(&mut home_sections, &metadata_settings);
    if let Some(section) = &mut prefetched_explore {
        cover_art_policy::bind_home_section(section, &metadata_settings);
    }
    cover_art_policy::bind_albums(&mut albums, &metadata_settings);
    cover_art_policy::bind_tracks(&mut tracks, &metadata_settings);
    cover_art_policy::bind_artists(&mut artists, &metadata_settings);
    cover_art_policy::bind_artists(&mut album_artists, &metadata_settings);
    cover_art_policy::bind_playlists(&mut playlists, &metadata_settings);
    cover_art_policy::bind_tracks(&mut favorites, &metadata_settings);
    scrub_snapshot_image_refs(
        &saved,
        &mut home_sections,
        prefetched_explore.as_mut(),
        &mut albums,
        &mut tracks,
        &mut artists,
        &mut album_artists,
        &mut genres,
        &mut playlists,
        &mut favorites,
        external_metadata::enabled(&metadata_settings),
    );
    album_track_refs(store, &saved, &mut albums)?;
    track_album_refs(store, &saved, &mut tracks, &albums)?;
    for section in &mut home_sections {
        home_local_refs(store, &saved, section)?;
    }
    if let Some(section) = &mut prefetched_explore {
        home_local_refs(store, &saved, section)?;
    }
    track_album_refs(store, &saved, &mut favorites, &albums)?;
    let status = sync_state
        .as_ref()
        .map(sync_status_text)
        .unwrap_or_else(|| "Cached library ready".to_string());
    let last_error = sync_state.and_then(|state| state.last_error);

    Ok(LibrarySnapshot {
        server: Some(saved.server),
        servers,
        selected_source: Some(selected_source),
        local_folders: source_settings.sources.local_folders,
        server_local_access,
        local_access,
        local_access_status,
        music_folders,
        selected_music_folder_id,
        username: Some(saved.username),
        first_run: false,
        sync_status: status,
        last_error,
        cached_album_count,
        cached_track_count,
        cached_artist_count,
        cached_album_artist_count,
        cached_genre_count,
        cached_playlist_count,
        home_sections,
        prefetched_explore,
        albums,
        tracks,
        artists,
        album_artists,
        genres,
        playlists,
        playlist_entry_keys,
        favorites,
        search: SearchResults::default(),
    })
}

pub(in crate::controller) fn load_runtime_snapshot(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
) -> Result<LibrarySnapshot, String> {
    let mut snapshot = load_snapshot(store)?;
    if active_server_needs_auth(&snapshot, secrets) {
        snapshot.first_run = true;
        snapshot.sync_status = "Connect once more to continue using this server.".to_string();
        snapshot.last_error = None;
    }
    Ok(snapshot)
}

fn active_server_needs_auth(snapshot: &LibrarySnapshot, secrets: &Arc<dyn SecretStore>) -> bool {
    let Some(server) = snapshot.server.as_ref() else {
        return false;
    };
    if server.provider == LOCAL_PROVIDER_ID || server.provider == "fake" {
        return false;
    }
    !config_token_available(secrets, &server.id)
}
pub(crate) fn grouped_cover_refs_for_items(albums: &[Album], tracks: &[Track]) -> Vec<ImageRef> {
    let mut image_refs = Vec::new();
    for album in albums {
        push_unique_cover_ref(&mut image_refs, album.image_ref.as_ref());
    }
    for track in tracks {
        push_unique_cover_ref(&mut image_refs, track.image_ref.as_ref());
    }
    image_refs
}
pub(crate) fn track_cover_refs_for_items(tracks: &[Track]) -> Vec<ImageRef> {
    let mut image_refs = Vec::new();
    for track in tracks {
        push_unique_cover_ref(&mut image_refs, track.image_ref.as_ref());
    }
    image_refs
}
pub(in crate::controller) fn normalize_artist_detail_image_refs(
    detail: &mut CachedArtistDetail,
    settings: &AppSettings,
) {
    cover_art_policy::bind_artist(&mut detail.artist, settings);
    cover_art_policy::bind_albums(&mut detail.albums, settings);
    cover_art_policy::bind_albums(&mut detail.appears_on, settings);
    cover_art_policy::bind_tracks(&mut detail.tracks, settings);
    cover_art_policy::bind_artist(&mut detail.artist, settings);
}
pub(in crate::controller) fn track_album_refs(
    store: &StoreHandle,
    saved: &SavedServer,
    tracks: &mut [Track],
    albums: &[Album],
) -> Result<(), String> {
    if tracks.is_empty() {
        return Ok(());
    }
    let settings = load_settings_for_saved(store, saved);
    track_album_refs_with_settings(store, saved, &settings, tracks, albums)
}

pub(in crate::controller) fn track_album_refs_with_settings(
    store: &StoreHandle,
    saved: &SavedServer,
    settings: &AppSettings,
    tracks: &mut [Track],
    albums: &[Album],
) -> Result<(), String> {
    if tracks.is_empty() {
        return Ok(());
    }
    let mut image_refs = albums
        .iter()
        .filter_map(|album| {
            let mut image_ref = album.image_ref.clone();
            scrub_source_image_ref(saved, &mut image_ref);
            image_ref.map(|image_ref| (album.id.clone(), image_ref))
        })
        .collect::<HashMap<_, _>>();
    let missing_album_ids = tracks
        .iter()
        .map(|track| track.album_id.clone())
        .filter(|album_id| !image_refs.contains_key(album_id))
        .fold(Vec::<AlbumId>::new(), |mut ids, album_id| {
            if !ids.iter().any(|existing| existing == &album_id) {
                ids.push(album_id);
            }
            ids
        });
    if !missing_album_ids.is_empty() {
        let mut loaded = store.with_store(|store| {
            store.load_album_image_refs(&saved.server.id, &missing_album_ids)
        })?;
        loaded.retain(|_, image_ref| source_image_ref_allowed(saved, image_ref));
        image_refs.extend(loaded);
    }
    let missing_album_ids = tracks
        .iter()
        .map(|track| track.album_id.clone())
        .filter(|album_id| !image_refs.contains_key(album_id))
        .fold(Vec::<AlbumId>::new(), |mut ids, album_id| {
            if !ids.iter().any(|existing| existing == &album_id) {
                ids.push(album_id);
            }
            ids
        });
    if !missing_album_ids.is_empty() {
        let mut loaded = store
            .with_store(|store| store.load_albums_by_ids(&saved.server.id, &missing_album_ids))?;
        for album in &mut loaded {
            scrub_source_image_ref(saved, &mut album.image_ref);
            cover_art_policy::bind_album(album, settings);
            if let Some(image_ref) = album.image_ref.clone() {
                image_refs.insert(album.id.clone(), image_ref);
            }
        }
    }
    for track in tracks {
        if let Some(image_ref) = image_refs.get(&track.album_id) {
            cover_art_policy::bind_track_with_album_ref(track, Some(image_ref), settings);
        }
    }
    Ok(())
}
pub(in crate::controller) fn album_track_refs(
    store: &StoreHandle,
    saved: &SavedServer,
    albums: &mut [Album],
) -> Result<(), String> {
    if saved.server.provider != LOCAL_PROVIDER_ID || albums.is_empty() {
        return Ok(());
    }
    let album_ids = albums.iter().map(|album| album.id.clone()).fold(
        Vec::<AlbumId>::new(),
        |mut ids, album_id| {
            if !ids.iter().any(|existing| existing == &album_id) {
                ids.push(album_id);
            }
            ids
        },
    );
    if album_ids.is_empty() {
        return Ok(());
    }
    let mut image_refs =
        store.with_store(|store| store.load_album_image_refs(&saved.server.id, &album_ids))?;
    image_refs.retain(|_, image_ref| source_image_ref_allowed(saved, image_ref));
    for album in albums {
        if let Some(image_ref) = image_refs.get(&album.id) {
            album.image_ref = Some(image_ref.clone());
        }
    }
    Ok(())
}
pub(in crate::controller) fn home_image_refs(
    store: &StoreHandle,
    saved: &SavedServer,
    section: &mut HomeSection,
) -> Result<(), String> {
    let metadata_settings = load_settings_for_saved(store, saved);
    scrub_home_refs(saved, section);
    cover_art_policy::bind_home_section(section, &metadata_settings);
    scrub_home_refs(saved, section);
    home_local_refs(store, saved, section)
}
fn home_local_refs(
    store: &StoreHandle,
    saved: &SavedServer,
    section: &mut HomeSection,
) -> Result<(), String> {
    album_track_refs(store, saved, &mut section.albums)?;
    let albums = section.albums.clone();
    track_album_refs(store, saved, &mut section.tracks, &albums)
}
pub(in crate::controller) fn queue_album_refs(
    store: &StoreHandle,
    server: &ServerIdentity,
    settings: &AppSettings,
    entries: &mut [QueueEntry],
) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    let missing_album_ids = entries
        .iter()
        .filter_map(|entry| entry.album_id.clone())
        .fold(Vec::<AlbumId>::new(), |mut ids, album_id| {
            if !ids.iter().any(|existing| existing == &album_id) {
                ids.push(album_id);
            }
            ids
        });
    if missing_album_ids.is_empty() {
        return Ok(());
    }
    let mut image_refs =
        store.with_store(|store| store.load_album_image_refs(&server.id, &missing_album_ids))?;
    image_refs.retain(|_, image_ref| image_ref_allowed(server, image_ref));
    for entry in entries {
        let Some(album_id) = &entry.album_id else {
            continue;
        };
        if let Some(image_ref) = image_refs.get(album_id) {
            cover_art_policy::bind_queue_entry_with_album_ref(entry, Some(image_ref), settings);
        }
    }
    Ok(())
}
pub(in crate::controller) fn push_unique_cover_ref(
    image_refs: &mut Vec<ImageRef>,
    image_ref: Option<&ImageRef>,
) {
    if image_refs.len() >= GROUPED_COVER_REF_LIMIT {
        return;
    }
    let Some(image_ref) = image_ref else {
        return;
    };
    if !image_refs.iter().any(|existing| existing == image_ref) {
        image_refs.push(image_ref.clone());
    }
}
pub(in crate::controller) fn sync_status_text(state: &SyncState) -> String {
    match state.status.as_str() {
        "running" => "Syncing library…".to_string(),
        "error" => "Sync needs attention".to_string(),
        _ => "Cached library ready".to_string(),
    }
}
pub(in crate::controller) fn cached_library_exists(
    store: &StoreHandle,
    server_id: &ServerId,
) -> bool {
    store
        .with_store(|store| {
            let albums = store.load_albums(server_id, 0, 1)?.total;
            let tracks = store.load_tracks(server_id, 0, 1)?.total;
            Ok(albums.saturating_add(tracks) > 0)
        })
        .unwrap_or(false)
}
pub(in crate::controller) fn load_library_counts(
    store: &StoreHandle,
    server_id: &ServerId,
) -> Result<LibraryCounts, String> {
    store.with_store(|store| {
        Ok(LibraryCounts {
            albums: store.load_albums(server_id, 0, 0)?.total,
            tracks: store.load_tracks(server_id, 0, 0)?.total,
            artists: store.load_artists(server_id, false, 0, 0)?.total,
            album_artists: store.load_artists(server_id, true, 0, 0)?.total,
            genres: store.load_genres(server_id, 0, 0)?.total,
            playlists: store.load_playlists(server_id, 0, 0)?.total,
        })
    })
}
pub(in crate::controller) fn load_home_update(
    store: &StoreHandle,
    saved: &SavedServer,
) -> Result<LibraryHomeUpdate, String> {
    store.with_store(|store| store.ensure_collection_cover_refs(&saved.server.id))?;
    let mut sections = store.with_store(|store| store.load_home_sections(&saved.server.id))?;
    let mut prefetched_explore = store.with_store(|store| {
        store.load_home_section_prefetch(&saved.server.id, HomeSectionKind::Explore)
    })?;
    for section in &mut sections {
        home_image_refs(store, saved, section)?;
    }
    if let Some(section) = &mut prefetched_explore {
        home_image_refs(store, saved, section)?;
    }
    Ok(LibraryHomeUpdate {
        sections,
        prefetched_explore,
    })
}
#[cfg(any(test, feature = "dev-tools"))]
pub(in crate::controller) fn seed_fake_cache(
    store: &StoreHandle,
    scale: FakeScale,
) -> Result<(), String> {
    let started = Instant::now();
    let provider = FakeProvider::new(scale);
    info!(
        ?scale,
        albums = provider.album_count(),
        tracks = provider.track_count(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "generated fake library"
    );
    let server = provider.identity().server.clone();
    let saved = SavedServer {
        server: server.clone(),
        user_id: "fake-user".to_string(),
        username: "fake".to_string(),
        trust_invalid_cert: false,
    };
    store.with_store(|store| {
        store.save_server(&saved)?;
        store.set_active_server(&server.id)?;
        Ok(())
    })?;
    let generation = store.with_store(|store| store.begin_sync(&server.id))?;

    let runtime = Runtime::new().map_err(|error| error.to_string())?;
    let album_limit = match scale {
        FakeScale::Small => provider.album_count(),
        FakeScale::Large => 1_000,
        FakeScale::Stress => provider.album_count(),
        FakeScale::ThirtyK => provider.album_count(),
    };
    let track_limit = match scale {
        FakeScale::Small => provider.track_count(),
        FakeScale::Large => 2_000,
        FakeScale::Stress => provider.track_count(),
        FakeScale::ThirtyK => provider.track_count(),
    };
    runtime.block_on(async {
        let fetch_started = Instant::now();
        let albums = provider
            .albums(PagedRequest::new(0, album_limit))
            .await
            .map_err(|error| error.to_string())?;
        let tracks = provider
            .tracks(PagedRequest::new(0, track_limit))
            .await
            .map_err(|error| error.to_string())?;
        let artists = provider
            .artists(PagedRequest::new(0, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        let album_artists = provider
            .album_artists(PagedRequest::new(0, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        let genres = provider
            .genres(PagedRequest::new(0, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        let playlists = provider
            .playlists(PagedRequest::new(0, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        let home_sections = provider
            .home_sections()
            .await
            .map_err(|error| error.to_string())?;
        info!(
            ?scale,
            album_limit,
            track_limit,
            elapsed_ms = fetch_started.elapsed().as_millis() as u64,
            total_elapsed_ms = started.elapsed().as_millis() as u64,
            "fetched fake cache seed pages"
        );

        let pruned_cover_entries = store.with_store(|store| {
            let write_started = Instant::now();
            let step_started = Instant::now();
            store.upsert_albums(&server.id, &albums.items, generation)?;
            info!(
                ?scale,
                count = albums.items.len(),
                elapsed_ms = step_started.elapsed().as_millis() as u64,
                total_elapsed_ms = started.elapsed().as_millis() as u64,
                "seeded fake albums"
            );
            let step_started = Instant::now();
            store.upsert_tracks(&server.id, &tracks.items, generation)?;
            info!(
                ?scale,
                count = tracks.items.len(),
                elapsed_ms = step_started.elapsed().as_millis() as u64,
                total_elapsed_ms = started.elapsed().as_millis() as u64,
                "seeded fake tracks"
            );
            let step_started = Instant::now();
            store.upsert_artists(&server.id, &artists.items, false, generation)?;
            store.upsert_artists(&server.id, &album_artists.items, true, generation)?;
            store.refresh_library_counts(&server.id)?;
            store.upsert_genres(&server.id, &genres.items, generation)?;
            store.upsert_playlists(&server.id, &playlists.items, generation)?;
            store.upsert_home_sections(&server.id, &home_sections, generation)?;
            info!(
                ?scale,
                elapsed_ms = step_started.elapsed().as_millis() as u64,
                total_elapsed_ms = started.elapsed().as_millis() as u64,
                "seeded fake library metadata"
            );
            let result = store.complete_sync(&server.id, generation);
            info!(
                ?scale,
                elapsed_ms = write_started.elapsed().as_millis() as u64,
                total_elapsed_ms = started.elapsed().as_millis() as u64,
                "finished fake cache writes"
            );
            result
        })?;
        let prune_started = Instant::now();
        prune_successful_sync_image_cache(store, &server.id, pruned_cover_entries);
        info!(
            ?scale,
            elapsed_ms = prune_started.elapsed().as_millis() as u64,
            total_elapsed_ms = started.elapsed().as_millis() as u64,
            "finished fake cache seed"
        );
        Ok::<(), String>(())
    })?;
    Ok(())
}
pub(in crate::controller) fn restore_queue(
    store: &StoreHandle,
    server: Option<&ServerIdentity>,
) -> Option<QueueEngine> {
    let server = server?;
    let settings = load_settings_for_server(store, server);
    match store.with_store(|store| store.load_queue_snapshot(&server.id)) {
        Ok(Some(mut snapshot)) => {
            cover_art_policy::bind_queue_snapshot(&mut snapshot, &settings);
            if let Err(error) = queue_album_refs(store, server, &settings, &mut snapshot.entries) {
                warn!(%error, "failed to normalize queue image refs");
            }
            Some(QueueEngine::restore(snapshot))
        }
        Ok(None) => Some(QueueEngine::new(server.id.clone())),
        Err(error) => {
            warn!(%error, "failed to restore queue snapshot");
            Some(QueueEngine::new(server.id.clone()))
        }
    }
}

pub(in crate::controller) struct QueueActivationContext<'a> {
    pub(in crate::controller) store: &'a StoreHandle,
    pub(in crate::controller) queue: &'a Arc<Mutex<Option<QueueEngine>>>,
    pub(in crate::controller) playback_request_generation: &'a Arc<AtomicU64>,
    pub(in crate::controller) next_preload: &'a Arc<Mutex<NextPreloadState>>,
    pub(in crate::controller) playback: &'a Arc<Mutex<Box<dyn PlaybackBackend>>>,
    pub(in crate::controller) playback_snapshot: &'a Arc<Mutex<PlaybackSnapshot>>,
    pub(in crate::controller) auto_dj_enabled: &'a Arc<Mutex<bool>>,
    pub(in crate::controller) events: &'a Sender<ControllerEvent>,
}

pub(in crate::controller) type LoginActivationContext<'a> = QueueActivationContext<'a>;

#[derive(Clone, Copy)]
pub(in crate::controller) struct LoginActivationRequest<'a> {
    pub(in crate::controller) session: &'a ProviderSession,
    pub(in crate::controller) trust_invalid_cert: bool,
    pub(in crate::controller) local_access_root: Option<&'a Path>,
    pub(in crate::controller) path_replace_from: Option<&'a str>,
}

pub(in crate::controller) fn activate_logged_in_server(
    context: &LoginActivationContext<'_>,
    request: LoginActivationRequest<'_>,
) -> Result<SavedServer, String> {
    let session = request.session;
    let saved = SavedServer {
        server: session.server.clone(),
        user_id: session.user_id.clone(),
        username: session.username.clone(),
        trust_invalid_cert: request.trust_invalid_cert,
    };
    context.store.with_store(|store| {
        store.save_server(&saved)?;
        if let Some(root) = request.local_access_root.and_then(Path::to_str) {
            store.save_server_local_access(&ServerLocalAccess {
                server_id: saved.server.id.clone(),
                root_path: root.to_string(),
                path_replace_from: trimmed_optional(request.path_replace_from),
                path_replace_to: Some(root.to_string()),
            })?;
        }
        store.set_active_server(&saved.server.id)?;
        Ok(())
    })?;
    let mut settings = load_settings_from_store(context.store);
    settings.sources.selected = Some(LibrarySourceSelection::Server(saved.server.id.clone()));
    settings.migrate_defaults();
    context.store.save_settings(&settings)?;

    activate_saved_queue(context, &saved)?;
    let _sent = context.events.send(ControllerEvent::LoginStatus(
        "Connected. Loading cached library…".to_string(),
    ));
    emit_snapshot(context.store, context.events);
    Ok(saved)
}

pub(in crate::controller) fn activate_with_token(
    context: &LoginActivationContext<'_>,
    secrets: &Arc<dyn SecretStore>,
    request: LoginActivationRequest<'_>,
) -> Result<SavedServer, String> {
    let session = request.session;
    secrets
        .save_token(&session.server.id, &session.access_token)
        .map_err(|error| error.to_string())?;
    match activate_logged_in_server(context, request) {
        Ok(saved) => Ok(saved),
        Err(error) => {
            if let Err(delete_error) = secrets.delete_token(&session.server.id) {
                warn!(
                    %delete_error,
                    server_id = %session.server.id,
                    "failed to delete token after login activation failed"
                );
            }
            Err(error)
        }
    }
}
pub(in crate::controller) fn activate_saved_queue(
    context: &QueueActivationContext<'_>,
    saved: &SavedServer,
) -> Result<(), String> {
    let Some((queue_snapshot, player)) = activate_queue_for_saved(
        context.store,
        context.queue,
        context.playback_snapshot,
        context.auto_dj_enabled,
        saved,
    )?
    else {
        return Ok(());
    };
    invalidate_playback_requests(context.playback_request_generation);
    stop_playback_backend(context.playback, context.next_preload, context.events);
    let _sent = context
        .events
        .send(ControllerEvent::Queue(Box::new(Some(queue_snapshot))));
    let _sent = context
        .events
        .send(ControllerEvent::Playback(Box::new(player)));
    Ok(())
}
pub(in crate::controller) fn activate_queue_for_saved(
    store: &StoreHandle,
    queue: &Arc<Mutex<Option<QueueEngine>>>,
    playback_snapshot: &Arc<Mutex<PlaybackSnapshot>>,
    auto_dj_enabled: &Arc<Mutex<bool>>,
    saved: &SavedServer,
) -> Result<Option<(QueueSnapshot, PlaybackSnapshot)>, String> {
    let mut queue = queue
        .lock()
        .map_err(|_| "queue lock was poisoned".to_string())?;
    let current_server_id = queue.as_ref().map(|queue| queue.server_id().clone());
    if current_server_id.as_ref() == Some(&saved.server.id) {
        return Ok(None);
    }

    let restored = restore_queue(store, Some(&saved.server))
        .unwrap_or_else(|| QueueEngine::new(saved.server.id.clone()));
    let queue_snapshot = restored.snapshot();
    let auto_dj_enabled = auto_dj_enabled
        .lock()
        .map(|enabled| *enabled)
        .unwrap_or_default();
    let player = playback_snapshot_from_queue(
        Some(&restored),
        auto_dj_enabled,
        &load_settings_for_saved(store, saved).playback,
    );
    *queue = Some(restored);
    drop(queue);

    if let Ok(mut snapshot) = playback_snapshot.lock() {
        *snapshot = player.clone();
    }

    Ok(Some((queue_snapshot, player)))
}
pub(in crate::controller) fn stop_playback_backend(
    playback: &Arc<Mutex<Box<dyn PlaybackBackend>>>,
    next_preload: &Arc<Mutex<NextPreloadState>>,
    events: &Sender<ControllerEvent>,
) {
    clear_next_preload(next_preload);
    if let Err(error) = playback
        .lock()
        .map_err(|_| "playback lock was poisoned".to_string())
        .and_then(|mut playback| {
            playback
                .send(PlaybackCommand::Stop)
                .map_err(|error| error.to_string())
        })
    {
        let _sent = events.send(ControllerEvent::Error(error));
    }
}
pub(in crate::controller) fn invalidate_playback_requests(
    playback_request_generation: &Arc<AtomicU64>,
) {
    playback_request_generation.fetch_add(1, Ordering::AcqRel);
}
pub(in crate::controller) fn next_playback_request_generation(
    playback_request_generation: &Arc<AtomicU64>,
) -> u64 {
    playback_request_generation.fetch_add(1, Ordering::AcqRel) + 1
}
pub(in crate::controller) fn playback_request_generation_matches(
    playback_request_generation: &Arc<AtomicU64>,
    request_generation: u64,
) -> bool {
    playback_request_generation.load(Ordering::Acquire) == request_generation
}
pub(in crate::controller) fn clear_queue_and_stop_playback(
    queue: &Arc<Mutex<Option<QueueEngine>>>,
    playback_request_generation: &Arc<AtomicU64>,
    next_preload: &Arc<Mutex<NextPreloadState>>,
    playback: &Arc<Mutex<Box<dyn PlaybackBackend>>>,
    playback_snapshot: &Arc<Mutex<PlaybackSnapshot>>,
    auto_dj_enabled: &Arc<Mutex<bool>>,
    events: &Sender<ControllerEvent>,
) {
    invalidate_playback_requests(playback_request_generation);
    if let Ok(mut queue) = queue.lock() {
        *queue = None;
    }
    stop_playback_backend(playback, next_preload, events);
    let player = PlaybackSnapshot {
        auto_dj_enabled: auto_dj_enabled
            .lock()
            .map(|enabled| *enabled)
            .unwrap_or_default(),
        ..PlaybackSnapshot::default()
    };
    if let Ok(mut snapshot) = playback_snapshot.lock() {
        *snapshot = player.clone();
    }
    let _sent = events.send(ControllerEvent::Queue(Box::new(None)));
    let _sent = events.send(ControllerEvent::Playback(Box::new(player)));
}
pub(in crate::controller) fn emit_snapshot(store: &StoreHandle, events: &Sender<ControllerEvent>) {
    match load_snapshot(store) {
        Ok(snapshot) => {
            let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
        }
        Err(error) => {
            let _sent = events.send(ControllerEvent::Error(error));
        }
    }
}
#[derive(Clone, Debug)]
struct SnapshotSourceReconciliation {
    selected_source: LibrarySourceSelection,
    saved: SavedServer,
}
fn reconcile_snapshot_source(
    store: &StoreHandle,
    settings: &AppSettings,
    remote_saved_servers: &[SavedServer],
) -> Result<Option<SnapshotSourceReconciliation>, String> {
    let selected_source = resolve_selected_source(
        settings,
        remote_saved_servers,
        store.with_store(|store| store.active_server())?,
    );
    let Some(selected_source) = selected_source else {
        return Ok(None);
    };

    let saved = saved_server_for_snapshot_source(store, remote_saved_servers, &selected_source)?;

    // Keep active_server aligned for follow-up cache, sync, and queue work.
    store.with_store(|store| store.set_active_server(&saved.server.id))?;
    Ok(Some(SnapshotSourceReconciliation {
        selected_source,
        saved,
    }))
}
fn saved_server_for_snapshot_source(
    store: &StoreHandle,
    remote_saved_servers: &[SavedServer],
    selected_source: &LibrarySourceSelection,
) -> Result<SavedServer, String> {
    match selected_source {
        LibrarySourceSelection::Local => ensure_local_source_server(store),
        LibrarySourceSelection::Server(server_id) => remote_saved_servers
            .iter()
            .find(|saved| &saved.server.id == server_id)
            .cloned()
            .ok_or_else(|| "The selected source is no longer saved.".to_string()),
    }
}
pub(in crate::controller) fn resolve_selected_source(
    settings: &AppSettings,
    remote_saved_servers: &[SavedServer],
    active_server: Option<SavedServer>,
) -> Option<LibrarySourceSelection> {
    match &settings.sources.selected {
        Some(LibrarySourceSelection::Local) => return Some(LibrarySourceSelection::Local),
        Some(LibrarySourceSelection::Server(server_id))
            if remote_saved_servers
                .iter()
                .any(|saved| saved.server.id == *server_id) =>
        {
            return Some(LibrarySourceSelection::Server(server_id.clone()));
        }
        _ => {}
    }

    if let Some(saved) = active_server
        && saved.server.provider != LOCAL_PROVIDER_ID
    {
        return Some(LibrarySourceSelection::Server(saved.server.id));
    }
    if !settings.sources.local_folders.is_empty() {
        return Some(LibrarySourceSelection::Local);
    }
    remote_saved_servers
        .first()
        .map(|saved| LibrarySourceSelection::Server(saved.server.id.clone()))
}
pub(in crate::controller) fn active_server_needs_sync(
    store: &StoreHandle,
    server_id: &ServerId,
) -> bool {
    active_source_readiness_inner(store, server_id, false)
        .map(|readiness| readiness.sync_required_reason.is_some())
        .unwrap_or(true)
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::controller) enum SyncRequiredReason {
    EmptyCache,
    PreviousSyncError,
    RemoteCacheStale,
    LocalManifestRefresh,
    LocalArtworkMissing,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::controller) struct SourceSyncReadiness {
    pub(in crate::controller) metadata_fresh: bool,
    pub(in crate::controller) artwork_fresh: bool,
    pub(in crate::controller) sync_required_reason: Option<SyncRequiredReason>,
    pub(in crate::controller) prefetch_required_reason: Option<SyncRequiredReason>,
    pub(in crate::controller) startup_delay_ms: Option<u64>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::controller) struct SourceSyncReadinessInput<'a> {
    pub(in crate::controller) provider: &'a str,
    pub(in crate::controller) cached_item_count: usize,
    pub(in crate::controller) sync_status: Option<&'a str>,
    pub(in crate::controller) sync_completed_age_seconds: Option<i64>,
    pub(in crate::controller) local_library_configured: bool,
    pub(in crate::controller) local_artwork_missing: bool,
}
pub(in crate::controller) fn source_sync_readiness(
    input: SourceSyncReadinessInput<'_>,
) -> SourceSyncReadiness {
    let sync_required_reason = if input.sync_status == Some("error") {
        Some(SyncRequiredReason::PreviousSyncError)
    } else if input.cached_item_count == 0 && input.sync_completed_age_seconds.is_none() {
        Some(SyncRequiredReason::EmptyCache)
    } else if input
        .sync_completed_age_seconds
        .is_none_or(|age| age >= STARTUP_CACHE_STALE_SECONDS)
        && input.provider != LOCAL_PROVIDER_ID
    {
        Some(SyncRequiredReason::RemoteCacheStale)
    } else if input.provider == LOCAL_PROVIDER_ID
        && input.local_library_configured
        && input.cached_item_count == 0
    {
        Some(SyncRequiredReason::LocalManifestRefresh)
    } else {
        None
    };
    let prefetch_required_reason = (input.provider == LOCAL_PROVIDER_ID
        && input.cached_item_count > 0
        && input.local_artwork_missing)
        .then_some(SyncRequiredReason::LocalArtworkMissing);
    let startup_delay_ms = match sync_required_reason {
        Some(SyncRequiredReason::EmptyCache) => Some(500),
        Some(_) => Some(8_000),
        None => None,
    };
    SourceSyncReadiness {
        metadata_fresh: !matches!(
            sync_required_reason,
            Some(
                SyncRequiredReason::EmptyCache
                    | SyncRequiredReason::PreviousSyncError
                    | SyncRequiredReason::RemoteCacheStale
            )
        ),
        artwork_fresh: prefetch_required_reason.is_none(),
        sync_required_reason,
        prefetch_required_reason,
        startup_delay_ms,
    }
}
#[cfg(test)]
pub(in crate::controller) fn active_source_readiness(
    store: &StoreHandle,
    server_id: &ServerId,
) -> Result<SourceSyncReadiness, String> {
    active_source_readiness_inner(store, server_id, true)
}
pub(in crate::controller) fn active_source_startup_readiness(
    store: &StoreHandle,
    server_id: &ServerId,
) -> Result<SourceSyncReadiness, String> {
    active_source_readiness_inner(store, server_id, true)
}
fn active_source_readiness_inner(
    store: &StoreHandle,
    server_id: &ServerId,
    include_local_artwork: bool,
) -> Result<SourceSyncReadiness, String> {
    let local_library_configured = server_id.as_str() == LOCAL_SOURCE_SERVER_ID
        && !load_settings_from_store(store)
            .sources
            .local_folders
            .is_empty();
    let (provider, cached_item_count, sync_status, sync_completed_age_seconds) =
        store.with_store(|store| {
            let provider = store
                .list_servers()?
                .into_iter()
                .find(|saved| saved.server.id == *server_id)
                .map(|saved| saved.server.provider)
                .unwrap_or_else(|| {
                    if server_id.as_str() == LOCAL_SOURCE_SERVER_ID {
                        LOCAL_PROVIDER_ID.to_string()
                    } else {
                        String::new()
                    }
                });
            let albums = store.load_albums(server_id, 0, 1)?.total;
            let tracks = store.load_tracks(server_id, 0, 1)?.total;
            let sync_status = store.sync_state(server_id).ok().map(|state| state.status);
            let sync_completed_age_seconds = store.sync_completed_age_seconds(server_id)?;
            Ok((
                provider,
                albums.saturating_add(tracks),
                sync_status,
                sync_completed_age_seconds,
            ))
        })?;
    let local_artwork_missing = include_local_artwork
        && provider == LOCAL_PROVIDER_ID
        && local_cover_cache_missing(store, server_id, false);
    Ok(source_sync_readiness(SourceSyncReadinessInput {
        provider: &provider,
        cached_item_count,
        sync_status: sync_status.as_deref(),
        sync_completed_age_seconds,
        local_library_configured,
        local_artwork_missing,
    }))
}
pub(in crate::controller) fn trimmed_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}
pub(in crate::controller) fn load_settings_from_store(store: &StoreHandle) -> AppSettings {
    let mut settings = store.load_settings().unwrap_or_default();
    settings.migrate_defaults();
    settings
}
pub(in crate::controller) fn local_folder_paths(settings: &AppSettings) -> Vec<PathBuf> {
    settings
        .sources
        .local_folders
        .iter()
        .map(|folder| PathBuf::from(&folder.path))
        .collect()
}
pub(in crate::controller) fn local_source_server() -> ServerIdentity {
    ServerIdentity {
        id: ServerId::new(LOCAL_SOURCE_SERVER_ID),
        provider: LOCAL_PROVIDER_ID.to_string(),
        name: "Local".to_string(),
        base_url: String::new(),
    }
}
pub(in crate::controller) fn local_source_saved() -> SavedServer {
    SavedServer {
        server: local_source_server(),
        user_id: "local".to_string(),
        username: "Local".to_string(),
        trust_invalid_cert: false,
    }
}
pub(in crate::controller) fn ensure_local_source_server(
    store: &StoreHandle,
) -> Result<SavedServer, String> {
    let saved = local_source_saved();
    store.with_store(|store| store.save_server(&saved))?;
    Ok(saved)
}
pub(in crate::controller) fn load_settings_for_active_server(store: &StoreHandle) -> AppSettings {
    let settings = load_settings_from_store(store);
    match store.with_store(|store| store.active_server()) {
        Ok(Some(saved)) => settings_for_server(settings, &saved.server),
        _ => settings,
    }
}
pub(in crate::controller) fn load_settings_for_saved(
    store: &StoreHandle,
    saved: &SavedServer,
) -> AppSettings {
    settings_for_server(load_settings_from_store(store), &saved.server)
}
pub(in crate::controller) fn load_settings_for_server(
    store: &StoreHandle,
    server: &ServerIdentity,
) -> AppSettings {
    settings_for_server(load_settings_from_store(store), server)
}

pub(in crate::controller) fn prune_successful_sync_image_cache(
    store: &StoreHandle,
    server_id: &ServerId,
    mut pruned_entries: Vec<CoverCacheEntry>,
) {
    match stale_external_images(store, server_id) {
        Ok(mut entries) => pruned_entries.append(&mut entries),
        Err(error) => warn!(%error, "failed to prune generated external image cache entries"),
    }
    prune_disk_cover_cache_entries(&pruned_entries);
}

fn stale_external_images(
    store: &StoreHandle,
    server_id: &ServerId,
) -> Result<Vec<CoverCacheEntry>, String> {
    let saved = store.with_store(|store| store.saved_server(server_id))?;
    let settings = saved
        .as_ref()
        .map(|saved| load_settings_for_saved(store, saved))
        .unwrap_or_else(|| load_settings_from_store(store));
    let prune_all_external = !external_metadata::enabled(&settings);
    let live_refs = if prune_all_external {
        Vec::new()
    } else {
        generated_external_image_refs(store, server_id, &settings)?
    };
    store.with_store(|store| store.prune_external_images(server_id, &live_refs, prune_all_external))
}

fn generated_external_image_refs(
    store: &StoreHandle,
    server_id: &ServerId,
    settings: &AppSettings,
) -> Result<Vec<ImageRef>, String> {
    let mut albums = external_prune_albums(store, server_id)?;
    let mut tracks = external_prune_tracks(store, server_id)?;
    cover_art_policy::bind_albums(&mut albums, settings);
    cover_art_policy::bind_tracks(&mut tracks, settings);
    let mut seen = HashSet::<(String, Option<String>)>::new();
    let mut refs = Vec::new();
    for image_ref in albums
        .into_iter()
        .filter_map(|album| album.image_ref)
        .chain(tracks.into_iter().filter_map(|track| track.image_ref))
    {
        if !external_metadata::is_external_image_ref(&image_ref) {
            continue;
        }
        if seen.insert((image_ref.item_id.clone(), image_ref.tag.clone())) {
            refs.push(image_ref);
        }
    }
    Ok(refs)
}

fn external_prune_albums(store: &StoreHandle, server_id: &ServerId) -> Result<Vec<Album>, String> {
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

fn external_prune_tracks(store: &StoreHandle, server_id: &ServerId) -> Result<Vec<Track>, String> {
    let mut tracks = Vec::new();
    let mut offset = 0;
    loop {
        let page = store.with_store(|store| store.load_tracks(server_id, offset, PAGE_SIZE))?;
        let item_count = page.items.len();
        tracks.extend(page.items);
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(tracks);
        }
    }
}

pub(in crate::controller) fn settings_for_server(
    mut settings: AppSettings,
    server: &ServerIdentity,
) -> AppSettings {
    if server.provider == "fake" {
        settings.external_metadata_enabled = false;
    }
    settings
}
pub(in crate::controller) fn local_initial_cover_cache_required(
    store: &StoreHandle,
    server_id: &ServerId,
) -> bool {
    local_cover_cache_missing(store, server_id, true)
}
fn local_cover_cache_missing(
    store: &StoreHandle,
    server_id: &ServerId,
    missing_library_requires_prefetch: bool,
) -> bool {
    if server_id.as_str() != LOCAL_SOURCE_SERVER_ID {
        return false;
    }
    store
        .with_store(|store| {
            let album_count = store.load_albums(server_id, 0, 1)?.total;
            let track_count = store.load_tracks(server_id, 0, 1)?.total;
            if album_count == 0 && track_count == 0 {
                return Ok(missing_library_requires_prefetch);
            }
            missing_provider_refs(store, server_id)
        })
        .unwrap_or(true)
}
fn missing_provider_refs(store: &Store, server_id: &ServerId) -> Result<bool, StoreError> {
    let mut seen = HashSet::new();
    if local_album_cover_refs_missing(store, server_id, &mut seen)? {
        return Ok(true);
    }
    if local_track_cover_refs_missing(store, server_id, &mut seen)? {
        return Ok(true);
    }
    if local_artist_cover_refs_missing(store, server_id, false, &mut seen)? {
        return Ok(true);
    }
    local_artist_cover_refs_missing(store, server_id, true, &mut seen)
}
fn local_album_cover_refs_missing(
    store: &Store,
    server_id: &ServerId,
    seen: &mut HashSet<(String, String)>,
) -> Result<bool, StoreError> {
    let mut offset = 0;
    loop {
        let page = store.load_albums(server_id, offset, PAGE_SIZE)?;
        for album in &page.items {
            if !is_local_album_id(&album.id) {
                return Ok(true);
            }
            if local_image_ref_cache_missing(store, server_id, album.image_ref.as_ref(), seen)? {
                return Ok(true);
            }
        }
        if sync_page_finished(page.items.len(), page.total, offset + page.items.len()) {
            return Ok(false);
        }
        offset += page.items.len();
    }
}
fn local_track_cover_refs_missing(
    store: &Store,
    server_id: &ServerId,
    seen: &mut HashSet<(String, String)>,
) -> Result<bool, StoreError> {
    let mut offset = 0;
    loop {
        let page = store.load_tracks(server_id, offset, PAGE_SIZE)?;
        let album_ids = page
            .items
            .iter()
            .map(|track| track.album_id.clone())
            .collect::<Vec<_>>();
        let album_image_refs = store.load_album_image_refs(server_id, &album_ids)?;
        for track in &page.items {
            if !is_local_track_id(&track.id) || !is_local_album_id(&track.album_id) {
                return Ok(true);
            }
            let image_ref = album_image_refs
                .get(&track.album_id)
                .or(track.image_ref.as_ref());
            if local_image_ref_cache_missing(store, server_id, image_ref, seen)? {
                return Ok(true);
            }
        }
        if sync_page_finished(page.items.len(), page.total, offset + page.items.len()) {
            return Ok(false);
        }
        offset += page.items.len();
    }
}
fn local_artist_cover_refs_missing(
    store: &Store,
    server_id: &ServerId,
    album_artist: bool,
    seen: &mut HashSet<(String, String)>,
) -> Result<bool, StoreError> {
    let mut offset = 0;
    loop {
        let page = store.load_artists(server_id, album_artist, offset, PAGE_SIZE)?;
        for artist in &page.items {
            if !is_local_artist_id(&artist.id) {
                return Ok(true);
            }
            if local_image_ref_cache_missing(store, server_id, artist.image_ref.as_ref(), seen)? {
                return Ok(true);
            }
        }
        if sync_page_finished(page.items.len(), page.total, offset + page.items.len()) {
            return Ok(false);
        }
        offset += page.items.len();
    }
}
fn local_image_ref_cache_missing(
    store: &Store,
    server_id: &ServerId,
    image_ref: Option<&ImageRef>,
    seen: &mut HashSet<(String, String)>,
) -> Result<bool, StoreError> {
    let Some(image_ref) = image_ref else {
        return Ok(false);
    };
    if !is_local_provider_image_ref(image_ref) {
        return Ok(false);
    }
    let tag = image_ref
        .tag
        .as_deref()
        .unwrap_or(IMAGE_TAG_UNTAGGED)
        .to_string();
    if !seen.insert((image_ref.item_id.clone(), tag.clone())) {
        return Ok(false);
    }
    for size in [256, 512] {
        if cover_cache_entry_exists(store, server_id, image_ref, &tag, size)? {
            return Ok(false);
        }
    }
    Ok(true)
}
fn cover_cache_entry_exists(
    store: &Store,
    server_id: &ServerId,
    image_ref: &ImageRef,
    tag: &str,
    size: u32,
) -> Result<bool, StoreError> {
    let key = library::image_cache_key(server_id, &image_ref.item_id, tag, size);
    if cover_cache_path_for_key(&key).is_some_and(|path| path.exists()) {
        return Ok(true);
    }
    let Some(entry) = store.load_cover_cache_entry(server_id, &image_ref.item_id, tag, size)?
    else {
        return Ok(false);
    };
    Ok(Path::new(&entry.path).exists())
}
pub(in crate::controller) fn scrub_source_album_image_refs(
    saved: &SavedServer,
    albums: &mut [Album],
) {
    for album in albums {
        scrub_source_image_ref(saved, &mut album.image_ref);
    }
}
pub(in crate::controller) fn scrub_selected_album_image_refs(
    saved: &SavedServer,
    settings: &AppSettings,
    albums: &mut [Album],
) {
    let allow_external_identity_refs = external_metadata::enabled(settings);
    scrub_snapshot_album_image_refs(saved, albums, allow_external_identity_refs);
}
pub(in crate::controller) fn scrub_source_track_image_refs(
    saved: &SavedServer,
    tracks: &mut [Track],
) {
    for track in tracks {
        scrub_source_image_ref(saved, &mut track.image_ref);
    }
}
pub(in crate::controller) fn scrub_selected_track_image_refs(
    saved: &SavedServer,
    settings: &AppSettings,
    tracks: &mut [Track],
) {
    let allow_external_identity_refs = external_metadata::enabled(settings);
    scrub_snapshot_track_image_refs(saved, tracks, allow_external_identity_refs);
}
pub(in crate::controller) fn scrub_selected_artist_image_refs(
    saved: &SavedServer,
    settings: &AppSettings,
    artists: &mut [Artist],
) {
    let allow_external_identity_refs = external_metadata::enabled(settings);
    scrub_snapshot_artist_image_refs(saved, artists, allow_external_identity_refs);
}
pub(in crate::controller) fn scrub_selected_genre_image_refs(
    saved: &SavedServer,
    settings: &AppSettings,
    genres: &mut [Genre],
) {
    let allow_external_identity_refs = external_metadata::enabled(settings);
    scrub_snapshot_genre_image_refs(saved, genres, allow_external_identity_refs);
}
pub(in crate::controller) fn scrub_selected_playlist_image_refs(
    saved: &SavedServer,
    settings: &AppSettings,
    playlists: &mut [Playlist],
) {
    let allow_external_identity_refs = external_metadata::enabled(settings);
    scrub_snapshot_playlist_image_refs(saved, playlists, allow_external_identity_refs);
}
pub(in crate::controller) fn scrub_smart_refs(
    saved: &SavedServer,
    playlists: &mut [SmartPlaylist],
) {
    for playlist in playlists {
        scrub_source_image_ref(saved, &mut playlist.image_ref);
        scrub_source_image_ref_vec(saved, &mut playlist.image_refs);
    }
}
pub(in crate::controller) fn scrub_home_refs(saved: &SavedServer, section: &mut HomeSection) {
    if saved.server.provider == LOCAL_PROVIDER_ID {
        section.albums.retain(|album| is_local_album_id(&album.id));
        section.tracks.retain(|track| is_local_track_id(&track.id));
    }
    scrub_source_album_image_refs(saved, &mut section.albums);
    scrub_source_track_image_refs(saved, &mut section.tracks);
}
#[allow(clippy::too_many_arguments)]
fn scrub_snapshot_image_refs(
    saved: &SavedServer,
    home_sections: &mut [HomeSection],
    prefetched_explore: Option<&mut HomeSection>,
    albums: &mut [Album],
    tracks: &mut [Track],
    artists: &mut [Artist],
    album_artists: &mut [Artist],
    genres: &mut [Genre],
    playlists: &mut [Playlist],
    favorites: &mut [Track],
    allow_external_identity_refs: bool,
) {
    for section in home_sections {
        scrub_snapshot_home_refs(saved, section, allow_external_identity_refs);
    }
    if let Some(section) = prefetched_explore {
        scrub_snapshot_home_refs(saved, section, allow_external_identity_refs);
    }
    scrub_snapshot_album_image_refs(saved, albums, allow_external_identity_refs);
    scrub_snapshot_track_image_refs(saved, tracks, allow_external_identity_refs);
    scrub_snapshot_artist_image_refs(saved, artists, allow_external_identity_refs);
    scrub_snapshot_artist_image_refs(saved, album_artists, allow_external_identity_refs);
    scrub_snapshot_genre_image_refs(saved, genres, allow_external_identity_refs);
    scrub_snapshot_playlist_image_refs(saved, playlists, allow_external_identity_refs);
    scrub_snapshot_track_image_refs(saved, favorites, allow_external_identity_refs);
}
fn scrub_snapshot_home_refs(
    saved: &SavedServer,
    section: &mut HomeSection,
    allow_external_identity_refs: bool,
) {
    if saved.server.provider == LOCAL_PROVIDER_ID {
        section.albums.retain(|album| is_local_album_id(&album.id));
        section.tracks.retain(|track| is_local_track_id(&track.id));
    }
    scrub_snapshot_album_image_refs(saved, &mut section.albums, allow_external_identity_refs);
    scrub_snapshot_track_image_refs(saved, &mut section.tracks, allow_external_identity_refs);
}
fn scrub_snapshot_album_image_refs(
    saved: &SavedServer,
    albums: &mut [Album],
    allow_external_identity_refs: bool,
) {
    for album in albums {
        scrub_snapshot_image_ref(saved, &mut album.image_ref, allow_external_identity_refs);
    }
}
fn scrub_snapshot_track_image_refs(
    saved: &SavedServer,
    tracks: &mut [Track],
    allow_external_identity_refs: bool,
) {
    for track in tracks {
        scrub_snapshot_image_ref(saved, &mut track.image_ref, allow_external_identity_refs);
    }
}
fn scrub_snapshot_artist_image_refs(
    saved: &SavedServer,
    artists: &mut [Artist],
    allow_external_identity_refs: bool,
) {
    for artist in artists {
        scrub_snapshot_image_ref(saved, &mut artist.image_ref, allow_external_identity_refs);
    }
}
fn scrub_snapshot_genre_image_refs(
    saved: &SavedServer,
    genres: &mut [Genre],
    allow_external_identity_refs: bool,
) {
    for genre in genres {
        scrub_snapshot_image_ref(saved, &mut genre.image_ref, allow_external_identity_refs);
        scrub_snapshot_image_ref_vec(saved, &mut genre.image_refs, allow_external_identity_refs);
    }
}
fn scrub_snapshot_playlist_image_refs(
    saved: &SavedServer,
    playlists: &mut [Playlist],
    allow_external_identity_refs: bool,
) {
    for playlist in playlists {
        scrub_snapshot_image_ref(saved, &mut playlist.image_ref, allow_external_identity_refs);
        scrub_snapshot_image_ref_vec(
            saved,
            &mut playlist.image_refs,
            allow_external_identity_refs,
        );
    }
}
fn scrub_snapshot_image_ref(
    saved: &SavedServer,
    image_ref: &mut Option<ImageRef>,
    allow_external_identity_refs: bool,
) {
    let Some(ref_value) = image_ref else {
        return;
    };
    if snapshot_image_ref_allowed(saved, ref_value, allow_external_identity_refs) {
        return;
    }
    *image_ref = None;
}
fn scrub_snapshot_image_ref_vec(
    saved: &SavedServer,
    image_refs: &mut Vec<ImageRef>,
    allow_external_identity_refs: bool,
) {
    image_refs.retain(|image_ref| {
        snapshot_image_ref_allowed(saved, image_ref, allow_external_identity_refs)
    });
}
fn snapshot_image_ref_allowed(
    saved: &SavedServer,
    image_ref: &ImageRef,
    allow_external_identity_refs: bool,
) -> bool {
    source_image_ref_allowed(saved, image_ref)
        || (allow_external_identity_refs && external_identity_image_ref(image_ref))
}
fn external_identity_image_ref(image_ref: &ImageRef) -> bool {
    external_metadata::album_art_from_image_ref(image_ref).is_some_and(|art| {
        art.musicbrainz_release_id.is_some() || art.musicbrainz_release_group_id.is_some()
    })
}
fn scrub_source_image_ref(saved: &SavedServer, image_ref: &mut Option<ImageRef>) {
    let Some(ref_value) = image_ref else {
        return;
    };
    if source_image_ref_allowed(saved, ref_value) {
        return;
    }
    *image_ref = None;
}
fn scrub_source_image_ref_vec(saved: &SavedServer, image_refs: &mut Vec<ImageRef>) {
    image_refs.retain(|image_ref| source_image_ref_allowed(saved, image_ref));
}
fn source_image_ref_allowed(saved: &SavedServer, image_ref: &ImageRef) -> bool {
    image_ref_allowed(&saved.server, image_ref)
}
fn image_ref_allowed(server: &ServerIdentity, image_ref: &ImageRef) -> bool {
    if server.provider == LOCAL_PROVIDER_ID {
        return is_local_provider_image_ref(image_ref);
    }
    !is_local_provider_image_ref(image_ref)
}
fn is_local_provider_image_ref(image_ref: &ImageRef) -> bool {
    image_ref.item_id.starts_with("local:cover:")
}
fn is_local_album_id(album_id: &AlbumId) -> bool {
    album_id.as_str().starts_with("local:album:")
}
fn is_local_track_id(track_id: &TrackId) -> bool {
    track_id.as_str().starts_with("local:track:")
}
fn is_local_artist_id(artist_id: &ArtistId) -> bool {
    artist_id.as_str().starts_with("local:artist:")
}
pub(in crate::controller) fn playback_snapshot_from_queue(
    queue: Option<&QueueEngine>,
    auto_dj_enabled: bool,
    playback_settings: &PlaybackSettings,
) -> PlaybackSnapshot {
    queue
        .map(|queue| {
            let duration_seconds = queue
                .current()
                .map(|entry| entry.duration_seconds)
                .unwrap_or_default();
            let waveform_cache_key = waveform_cache_key_for_queue(Some(queue));
            let waveform_peaks = waveform_cache_key
                .as_deref()
                .and_then(|key| cached_waveform_peaks(key, duration_seconds));

            PlaybackSnapshot {
                current_server_id: Some(queue.server_id().clone()),
                current: queue.current().cloned(),
                state: PlaybackState::Stopped,
                position_seconds: queue.progress_seconds(),
                position_millis: u64::from(queue.progress_seconds()) * 1_000,
                duration_seconds,
                volume: playback_settings.volume,
                muted: playback_settings.muted,
                repeat_mode: queue.repeat_mode(),
                shuffle_enabled: queue.shuffle().enabled,
                auto_dj_enabled,
                buffering_percent: None,
                last_error: None,
                waveform_cache_key,
                waveform_peaks,
            }
        })
        .unwrap_or_else(|| PlaybackSnapshot {
            auto_dj_enabled,
            volume: playback_settings.volume,
            muted: playback_settings.muted,
            ..PlaybackSnapshot::default()
        })
}
pub(in crate::controller) fn next_queue_entry_after_current(
    queue: &QueueEngine,
) -> Option<QueueEntry> {
    queue.next_after_end_of_stream().cloned()
}

pub(in crate::controller) fn current_request_match(
    queue: Option<&QueueEngine>,
    server_id: &ServerId,
    entry: &QueueEntry,
) -> bool {
    let Some(queue) = queue else {
        return false;
    };
    if queue.server_id() != server_id {
        return false;
    }
    queue
        .current()
        .is_some_and(|current| current.id == entry.id && current.track_id == entry.track_id)
}
pub(in crate::controller) fn current_request_valid(
    queue: &Arc<Mutex<Option<QueueEngine>>>,
    server_id: &ServerId,
    entry: &QueueEntry,
) -> bool {
    queue
        .lock()
        .ok()
        .is_some_and(|queue| current_request_match(queue.as_ref(), server_id, entry))
}
pub(in crate::controller) fn request_generation_match(
    playback_request_generation: &Arc<AtomicU64>,
    request_generation: u64,
    queue: &Arc<Mutex<Option<QueueEngine>>>,
    server_id: &ServerId,
    entry: &QueueEntry,
) -> bool {
    playback_request_generation_matches(playback_request_generation, request_generation)
        && current_request_valid(queue, server_id, entry)
}

#[derive(Clone, Debug, Default)]
pub(in crate::controller) struct NextPreloadState {
    generation: u64,
    request: Option<NextPreloadRequest>,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::controller) struct NextPreloadRequest {
    pub(in crate::controller) server_id: ServerId,
    pub(in crate::controller) current_entry_id: QueueEntryId,
    pub(in crate::controller) next_entry_id: QueueEntryId,
    pub(in crate::controller) next_entry: QueueEntry,
    pub(in crate::controller) stream_quality: StreamQuality,
}

#[derive(Clone, Debug)]
struct NextPreloadTicket {
    generation: u64,
    request: NextPreloadRequest,
}

fn preload_request_match(queue: Option<&QueueEngine>, request: &NextPreloadRequest) -> bool {
    let Some(queue) = queue else {
        return false;
    };
    if queue.server_id() != &request.server_id {
        return false;
    }
    let Some(current) = queue.current() else {
        return false;
    };
    if current.id != request.current_entry_id {
        return false;
    }
    next_queue_entry_after_current(queue).is_some_and(|entry| entry.id == request.next_entry_id)
}
fn begin_next_preload(
    next_preload: &Arc<Mutex<NextPreloadState>>,
    request: NextPreloadRequest,
) -> Option<NextPreloadTicket> {
    let mut state = next_preload.lock().ok()?;
    if state.request.as_ref() == Some(&request) {
        return None;
    }
    state.generation = state.generation.wrapping_add(1);
    state.request = Some(request.clone());
    Some(NextPreloadTicket {
        generation: state.generation,
        request,
    })
}

pub(in crate::controller) fn clear_next_preload(next_preload: &Arc<Mutex<NextPreloadState>>) {
    if let Ok(mut state) = next_preload.lock() {
        state.generation = state.generation.wrapping_add(1);
        state.request = None;
    }
}

fn next_preload_ticket_valid(
    next_preload: &Arc<Mutex<NextPreloadState>>,
    ticket: &NextPreloadTicket,
) -> bool {
    next_preload.lock().ok().is_some_and(|state| {
        state.generation == ticket.generation && state.request.as_ref() == Some(&ticket.request)
    })
}

fn clear_matching_next_preload(
    next_preload: &Arc<Mutex<NextPreloadState>>,
    ticket: &NextPreloadTicket,
) {
    if let Ok(mut state) = next_preload.lock()
        && state.generation == ticket.generation
        && state.request.as_ref() == Some(&ticket.request)
    {
        state.generation = state.generation.wrapping_add(1);
        state.request = None;
    }
}
pub(in crate::controller) fn shuffle_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(1)
}
pub(in crate::controller) fn auto_dj_candidates(
    tracks: &[Track],
    current: &QueueEntry,
    queued_track_ids: &HashSet<TrackId>,
    seed: u64,
) -> Vec<Track> {
    let current_genres = tracks
        .iter()
        .find(|track| track.id == current.track_id)
        .map(|track| {
            track
                .genres
                .iter()
                .map(|genre| genre.to_lowercase())
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();

    let mut candidates = tracks
        .iter()
        .filter(|track| !queued_track_ids.contains(&track.id))
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by_key(|track| {
        (
            std::cmp::Reverse(auto_dj_score(track, current, &current_genres)),
            auto_dj_shuffle_key(seed, track.id.as_str()),
        )
    });
    candidates.truncate(AUTO_DJ_ITEM_COUNT);
    candidates
}
pub(in crate::controller) fn auto_dj_score(
    track: &Track,
    current: &QueueEntry,
    current_genres: &HashSet<String>,
) -> u8 {
    let mut score = 0;
    if !current_genres.is_empty()
        && track
            .genres
            .iter()
            .any(|genre| current_genres.contains(&genre.to_lowercase()))
    {
        score += 80;
    }
    if current
        .artist_id
        .as_ref()
        .is_some_and(|artist_id| track.artist_id.as_ref() == Some(artist_id))
    {
        score += 60;
    } else if !current.artist.trim().is_empty()
        && track.artist.eq_ignore_ascii_case(current.artist.as_str())
    {
        score += 50;
    }
    if current
        .album_id
        .as_ref()
        .is_some_and(|album_id| track.album_id == *album_id)
    {
        score += 25;
    }
    score
}
pub(in crate::controller) fn auto_dj_shuffle_key(seed: u64, value: &str) -> u64 {
    value
        .bytes()
        .fold(seed ^ 0xa24b_aed4_963e_e407, |hash, byte| {
            hash.rotate_left(7) ^ u64::from(byte)
        })
}
pub(in crate::controller) fn playback_backend(fake: bool) -> Box<dyn PlaybackBackend> {
    if fake {
        return Box::new(FakePlaybackBackend::new());
    }
    Box::new(LazyGStreamerPlaybackBackend::new())
}
pub(in crate::controller) fn platform_secret_store(settings: &AppSettings) -> Arc<dyn SecretStore> {
    match settings.secret_storage_mode {
        SecretStorageMode::ConfigFile => Arc::new(CachedSecretStore::new(Arc::new(
            ConfigSecretStore::with_scope(config_secrets_path(), settings.secret_scope_id.clone()),
        ))),
        SecretStorageMode::SystemKeyring => system_keyring_secret_store(&settings.secret_scope_id),
    }
}

#[cfg(unix)]
fn system_keyring_secret_store(scope_id: &str) -> Arc<dyn SecretStore> {
    Arc::new(CachedSecretStore::new(Arc::new(SecretServiceStore::new(
        scope_id.to_string(),
    ))))
}

#[cfg(not(unix))]
fn system_keyring_secret_store(_scope_id: &str) -> Arc<dyn SecretStore> {
    Arc::new(UnavailableSecretStore::new(
        "system keyring is unavailable on this platform",
    ))
}
pub(in crate::controller) fn saved_server_needs_auth(
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
) -> bool {
    if saved.server.provider == LOCAL_PROVIDER_ID || saved.server.provider == "fake" {
        return false;
    }
    !config_token_available(secrets, &saved.server.id)
}
pub(in crate::controller) fn config_token_available(
    secrets: &Arc<dyn SecretStore>,
    server_id: &ServerId,
) -> bool {
    match secrets.load_token(server_id) {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(error) => {
            warn!(%error, server_id = %server_id, "failed to load saved token");
            false
        }
    }
}
pub(in crate::controller) fn emit_runtime_snapshot(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    events: &Sender<ControllerEvent>,
) {
    match load_runtime_snapshot(store, secrets) {
        Ok(snapshot) => {
            let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
        }
        Err(error) => {
            let _sent = events.send(ControllerEvent::Error(error));
        }
    }
}
pub(in crate::controller) fn playback_track_from_entry(entry: &QueueEntry) -> PlaybackTrack {
    PlaybackTrack {
        id: entry.track_id.clone(),
        album_id: entry.album_id.clone(),
        title: entry.title.clone(),
        artist: entry.artist.clone(),
        album: entry.album.clone(),
        duration_seconds: entry.duration_seconds,
    }
}
pub(in crate::controller) fn prepared_item_from_entry(
    entry: &QueueEntry,
    stream: StreamDescriptor,
) -> PreparedPlaybackItem {
    PreparedPlaybackItem::new(playback_track_from_entry(entry), stream)
}
pub(in crate::controller) fn resolve_prepared_item(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    server_id: &ServerId,
    entry: &QueueEntry,
    playback_settings: &PlaybackSettings,
) -> Result<PreparedPlaybackItem, String> {
    let stream = resolve_stream(
        store,
        runtime,
        secrets,
        server_id,
        &entry.track_id,
        playback_settings,
    )?;
    Ok(prepared_item_from_entry(entry, stream))
}
pub(in crate::controller) fn send_prepared_next(
    playback: &Arc<Mutex<Box<dyn PlaybackBackend>>>,
    queue: &Arc<Mutex<Option<QueueEngine>>>,
    events: &Sender<ControllerEvent>,
    request: &NextPreloadRequest,
    prepared: PreparedPlaybackItem,
) -> bool {
    let Ok(queue) = queue.lock() else {
        return false;
    };
    if !preload_request_match(queue.as_ref(), request) {
        return false;
    }
    let track_id = prepared.track.id.clone();
    if let Err(error) = playback
        .lock()
        .map_err(|_| "playback lock was poisoned".to_string())
        .and_then(|mut playback| {
            playback
                .send(PlaybackCommand::PrepareNext(Some(prepared)))
                .map_err(|error| error.to_string())
        })
    {
        let _sent = events.send(ControllerEvent::Error(error));
        return false;
    }
    info!(track_id = %track_id.as_str(), "sent next playback stream");
    true
}
pub(in crate::controller) fn prepare_next_stream_from_handles(
    store: StoreHandle,
    runtime: Arc<Runtime>,
    secrets: Arc<dyn SecretStore>,
    playback: Arc<Mutex<Box<dyn PlaybackBackend>>>,
    queue: Arc<Mutex<Option<QueueEngine>>>,
    next_preload: Arc<Mutex<NextPreloadState>>,
    events: Sender<ControllerEvent>,
) {
    let playback_settings = load_settings_from_store(&store).playback;
    let Some(request) = next_preload_request_from_queue(&queue, &playback_settings) else {
        clear_next_preload(&next_preload);
        if let Err(error) = playback
            .lock()
            .map_err(|_| "playback lock was poisoned".to_string())
            .and_then(|mut playback| {
                playback
                    .send(PlaybackCommand::PrepareNext(None))
                    .map_err(|error| error.to_string())
            })
        {
            let _sent = events.send(ControllerEvent::Error(error));
        }
        return;
    };
    let Some(ticket) = begin_next_preload(&next_preload, request) else {
        return;
    };

    thread::spawn(move || {
        let preload_started_at = Instant::now();
        let prepared = match resolve_prepared_item(
            &store,
            &runtime,
            &secrets,
            &ticket.request.server_id,
            &ticket.request.next_entry,
            &playback_settings,
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                clear_matching_next_preload(&next_preload, &ticket);
                if preload_error_is_transient(&error) {
                    debug!(%error, "skipped next playback preload while store is busy");
                    return;
                }
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
        };
        let elapsed_ms = preload_started_at.elapsed().as_millis();
        if !next_preload_ticket_valid(&next_preload, &ticket) {
            debug!(
                track_id = %ticket.request.next_entry.track_id.as_str(),
                elapsed_ms,
                "discarded stale next playback stream"
            );
            return;
        }
        info!(
            track_id = %ticket.request.next_entry.track_id.as_str(),
            elapsed_ms,
            "resolved next playback stream"
        );
        if !send_prepared_next(&playback, &queue, &events, &ticket.request, prepared) {
            clear_matching_next_preload(&next_preload, &ticket);
        }
    });
}
pub(in crate::controller) fn next_preload_request_from_queue(
    queue: &Arc<Mutex<Option<QueueEngine>>>,
    playback_settings: &PlaybackSettings,
) -> Option<NextPreloadRequest> {
    queue.lock().ok().and_then(|queue| {
        let queue = queue.as_ref()?;
        let server_id = queue.server_id().clone();
        let current_entry_id = queue.current()?.id.clone();
        let next_entry = next_queue_entry_after_current(queue)?;
        let next_entry_id = next_entry.id.clone();
        if next_entry_id == current_entry_id {
            return None;
        }
        Some(NextPreloadRequest {
            server_id,
            current_entry_id,
            next_entry_id,
            next_entry,
            stream_quality: playback_settings.stream_quality,
        })
    })
}

fn preload_error_is_transient(error: &str) -> bool {
    error.contains("database is locked") || error.contains("database table is locked")
}
