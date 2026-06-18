use super::*;

#[derive(Clone, Debug)]
struct SnapshotSourceReconciliation {
    selected_source: LibrarySourceSelection,
    saved: SavedServer,
}

fn snapshot_remote_servers(saved_servers: &[SavedServer]) -> Vec<SavedServer> {
    saved_servers
        .iter()
        .filter(|saved| saved.server.provider != LOCAL_PROVIDER_ID)
        .cloned()
        .collect()
}

fn snapshot_server_identities(remote_saved_servers: &[SavedServer]) -> Vec<ServerIdentity> {
    remote_saved_servers
        .iter()
        .map(|saved| saved.server.clone())
        .collect()
}

fn snapshot_server_local_access(
    store: &StoreHandle,
    remote_saved_servers: &[SavedServer],
) -> Result<Vec<ServerLocalAccessSnapshot>, String> {
    remote_saved_servers
        .iter()
        .map(|saved| snapshot_server_local_access_summary(store, saved))
        .collect()
}

fn snapshot_server_local_access_summary(
    store: &StoreHandle,
    saved: &SavedServer,
) -> Result<ServerLocalAccessSnapshot, String> {
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

fn resolve_selected_source(
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

pub(in crate::controller) fn load_snapshot(store: &StoreHandle) -> Result<LibrarySnapshot, String> {
    let source_settings = load_settings_from_store(store);
    let saved_servers = store.with_store(|store| store.list_servers())?;
    let remote_saved_servers = snapshot_remote_servers(&saved_servers);
    let servers = snapshot_server_identities(&remote_saved_servers);
    let server_local_access = snapshot_server_local_access(store, &remote_saved_servers)?;
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
    load_active_source_snapshot(
        store,
        source_settings,
        servers,
        server_local_access,
        selected_source,
        saved,
    )
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

fn load_active_source_snapshot(
    store: &StoreHandle,
    source_settings: AppSettings,
    servers: Vec<ServerIdentity>,
    server_local_access: Vec<ServerLocalAccessSnapshot>,
    selected_source: LibrarySourceSelection,
    saved: SavedServer,
) -> Result<LibrarySnapshot, String> {
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
    let external_ref_policy = snapshot_external_ref_policy(&metadata_settings);
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
        external_ref_policy,
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
        external_ref_policy,
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
