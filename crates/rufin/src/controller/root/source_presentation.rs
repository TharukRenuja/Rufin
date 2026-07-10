use super::source_image_policy::{
    SnapshotExternalRefPolicy, scrub_snapshot_album_image_refs, scrub_snapshot_artist_image_refs,
    scrub_snapshot_genre_image_refs, scrub_snapshot_home_refs, scrub_snapshot_playlist_image_refs,
    scrub_snapshot_track_image_refs, snapshot_external_ref_policy,
};
use super::*;
use crate::sources::{
    configured_source_needs_auth, configured_source_selection, local_configured_source_for_store,
    resolve_source_registration,
};

#[derive(Clone, Debug)]
struct SnapshotSourceReconciliation {
    selected_source: LibrarySourceSelection,
    saved: SavedSource,
}

struct ActiveSourceProjection {
    cached_album_count: usize,
    cached_track_count: usize,
    cached_artist_count: usize,
    cached_album_artist_count: usize,
    cached_genre_count: usize,
    cached_playlist_count: usize,
    home_sections: Vec<HomeSection>,
    prefetched_explore: Option<HomeSection>,
    albums: Vec<Album>,
    tracks: Vec<Track>,
    artists: Vec<Artist>,
    album_artists: Vec<Artist>,
    genres: Vec<Genre>,
    playlists: Vec<Playlist>,
    playlist_entry_keys: HashMap<PlaylistId, Vec<(String, TrackId)>>,
    favorites: Vec<Track>,
}

fn snapshot_remote_servers(saved_sources: &[SavedSource]) -> Vec<SavedSource> {
    saved_sources
        .iter()
        .filter(|saved| {
            matches!(
                configured_source_selection(saved),
                LibrarySourceSelection::Source(_)
            )
        })
        .cloned()
        .collect()
}

fn snapshot_source_identities(remote_saved_sources: &[SavedSource]) -> Vec<SourceIdentity> {
    remote_saved_sources
        .iter()
        .map(|saved| saved.source.clone())
        .collect()
}

fn snapshot_source_local_access(
    store: &StoreHandle,
    remote_saved_sources: &[SavedSource],
) -> Result<Vec<SourceLocalAccessSnapshot>, String> {
    remote_saved_sources
        .iter()
        .map(|saved| snapshot_source_local_access_summary(store, saved))
        .collect()
}

fn snapshot_source_local_access_summary(
    store: &StoreHandle,
    saved: &SavedSource,
) -> Result<SourceLocalAccessSnapshot, String> {
    let access = store.with_store(|store| store.source_local_access(&saved.source.id))?;
    let status = local_access_status_for_server(store, access.as_ref())?;
    let sync_state = store
        .with_store(|store| store.sync_state(&saved.source.id))
        .ok();
    let sync_status = sync_state
        .as_ref()
        .map(sync_status_text)
        .unwrap_or_else(|| "Cached library ready".to_string());
    let cached_album_count = store
        .with_store(|store| {
            store
                .load_albums(&saved.source.id, 0, 1)
                .map(|page| page.total)
        })
        .unwrap_or_default();
    let cached_track_count = store
        .with_store(|store| {
            store
                .load_tracks(&saved.source.id, 0, 1)
                .map(|page| page.total)
        })
        .unwrap_or_default();
    let selected_music_folder_name = store
        .with_store(|store| {
            let selected = store.selected_music_folder_id(&saved.source.id)?;
            let folders = store.list_music_folders(&saved.source.id)?;
            Ok(selected.and_then(|selected| {
                folders
                    .into_iter()
                    .find(|folder| folder.id == selected)
                    .map(|folder| folder.name)
            }))
        })
        .unwrap_or_default();
    Ok(SourceLocalAccessSnapshot {
        source_id: saved.source.id.clone(),
        access,
        status,
        selected_music_folder_name,
        sync_status,
        cached_album_count,
        cached_track_count,
    })
}

fn resolve_snapshot_source(
    store: &StoreHandle,
    settings: &AppSettings,
    saved_sources: &[SavedSource],
    remote_saved_sources: &[SavedSource],
) -> Result<Option<SnapshotSourceReconciliation>, String> {
    let local_source_configured = local_source_configured(store, settings, saved_sources);
    let persisted_active = store.with_store(|store| store.active_source())?;
    let selected_source = resolve_selected_source(
        settings,
        remote_saved_sources,
        persisted_active.clone(),
        local_source_configured,
    );
    let Some(selected_source) = selected_source else {
        return Ok(None);
    };

    let saved = saved_server_for_snapshot_source(
        store,
        remote_saved_sources,
        persisted_active.as_ref(),
        &selected_source,
    )?;

    Ok(Some(SnapshotSourceReconciliation {
        selected_source,
        saved,
    }))
}

fn saved_server_for_snapshot_source(
    store: &StoreHandle,
    remote_saved_sources: &[SavedSource],
    persisted_active: Option<&SavedSource>,
    selected_source: &LibrarySourceSelection,
) -> Result<SavedSource, String> {
    match selected_source {
        LibrarySourceSelection::Local => persisted_active
            .filter(|saved| {
                matches!(
                    configured_source_selection(saved),
                    LibrarySourceSelection::Local
                )
            })
            .cloned()
            .map_or_else(|| local_configured_source_for_store(store), Ok),
        LibrarySourceSelection::Source(source_id) => remote_saved_sources
            .iter()
            .find(|saved| &saved.source.id == source_id)
            .cloned()
            .ok_or_else(|| "The selected source is no longer saved.".to_string()),
    }
}

fn local_source_configured(
    store: &StoreHandle,
    settings: &AppSettings,
    saved_sources: &[SavedSource],
) -> bool {
    if !settings.sources.local_folders.is_empty() {
        return true;
    }
    saved_sources.iter().any(|saved| {
        matches!(
            configured_source_selection(saved),
            LibrarySourceSelection::Local
        ) && store
            .with_store(|store| {
                let tracks = store.load_tracks(&saved.source.id, 0, 1)?.total;
                let albums = store.load_albums(&saved.source.id, 0, 1)?.total;
                Ok(tracks > 0 || albums > 0)
            })
            .unwrap_or(false)
    })
}

fn resolve_selected_source(
    settings: &AppSettings,
    remote_saved_sources: &[SavedSource],
    active_source: Option<SavedSource>,
    local_source_configured: bool,
) -> Option<LibrarySourceSelection> {
    match &settings.sources.selected {
        Some(LibrarySourceSelection::Local) if local_source_configured => {
            return Some(LibrarySourceSelection::Local);
        }
        Some(LibrarySourceSelection::Source(source_id))
            if remote_saved_sources
                .iter()
                .any(|saved| saved.source.id == *source_id) =>
        {
            return Some(LibrarySourceSelection::Source(source_id.clone()));
        }
        _ => {}
    }

    if let Some(saved) = active_source {
        match configured_source_selection(&saved) {
            LibrarySourceSelection::Local if local_source_configured => {
                return Some(LibrarySourceSelection::Local);
            }
            LibrarySourceSelection::Source(source_id) => {
                return Some(LibrarySourceSelection::Source(source_id));
            }
            LibrarySourceSelection::Local => {}
        }
    }
    None
}

pub(in crate::controller) fn load_snapshot(store: &StoreHandle) -> Result<LibrarySnapshot, String> {
    let source_settings = load_settings_from_store(store);
    let saved_sources = store.with_store(|store| store.list_sources())?;
    let remote_saved_sources = snapshot_remote_servers(&saved_sources);
    let sources = snapshot_source_identities(&remote_saved_sources);
    let source_local_access = snapshot_source_local_access(store, &remote_saved_sources)?;
    let Some(reconciled_source) = resolve_snapshot_source(
        store,
        &source_settings,
        &saved_sources,
        &remote_saved_sources,
    )?
    else {
        let mut snapshot = LibrarySnapshot::first_run();
        snapshot.sources = sources;
        snapshot.local_folders = source_settings.sources.local_folders.clone();
        snapshot.source_local_access = source_local_access;
        return Ok(snapshot);
    };
    let SnapshotSourceReconciliation {
        selected_source,
        saved,
    } = reconciled_source;
    load_active_source_snapshot(
        store,
        source_settings,
        sources,
        source_local_access,
        selected_source,
        saved,
    )
}

pub(in crate::controller) fn load_runtime_snapshot(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
) -> Result<LibrarySnapshot, String> {
    let mut snapshot = load_snapshot(store)?;
    if active_source_needs_auth(store, &snapshot, secrets)? {
        snapshot.first_run = true;
        snapshot.sync_status = "Connect once more to continue using this server.".to_string();
        snapshot.last_error = None;
    }
    Ok(snapshot)
}

fn active_source_needs_auth(
    store: &StoreHandle,
    snapshot: &LibrarySnapshot,
    secrets: &Arc<dyn SecretStore>,
) -> Result<bool, String> {
    if matches!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Local)
    ) {
        return Ok(false);
    }
    let Some(server) = snapshot.source.as_ref() else {
        return Ok(false);
    };
    let saved = store
        .with_store(|store| store.saved_source(&server.id))?
        .ok_or_else(|| "The selected source is no longer saved.".to_string())?;
    if resolve_source_registration(&saved.source.kind).is_none() {
        return Ok(true);
    }
    configured_source_needs_auth(secrets, &saved)
}

fn scrub_projection_refs(
    saved: &SavedSource,
    projection: &mut ActiveSourceProjection,
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    for section in &mut projection.home_sections {
        scrub_snapshot_home_refs(saved, section, external_ref_policy);
    }
    if let Some(section) = &mut projection.prefetched_explore {
        scrub_snapshot_home_refs(saved, section, external_ref_policy);
    }
    scrub_snapshot_album_image_refs(saved, &mut projection.albums, external_ref_policy);
    scrub_snapshot_track_image_refs(saved, &mut projection.tracks, external_ref_policy);
    scrub_snapshot_artist_image_refs(saved, &mut projection.artists, external_ref_policy);
    scrub_snapshot_artist_image_refs(saved, &mut projection.album_artists, external_ref_policy);
    scrub_snapshot_genre_image_refs(saved, &mut projection.genres, external_ref_policy);
    scrub_snapshot_playlist_image_refs(saved, &mut projection.playlists, external_ref_policy);
    scrub_snapshot_track_image_refs(saved, &mut projection.favorites, external_ref_policy);
}

fn bind_projection_refs(projection: &mut ActiveSourceProjection, metadata_settings: &AppSettings) {
    cover_art_policy::bind_home_sections(&mut projection.home_sections, metadata_settings);
    if let Some(section) = &mut projection.prefetched_explore {
        cover_art_policy::bind_home_section(section, metadata_settings);
    }
    cover_art_policy::bind_albums(&mut projection.albums, metadata_settings);
    cover_art_policy::bind_tracks(&mut projection.tracks, metadata_settings);
    cover_art_policy::bind_artists(&mut projection.artists, metadata_settings);
    cover_art_policy::bind_artists(&mut projection.album_artists, metadata_settings);
    cover_art_policy::bind_playlists(&mut projection.playlists, metadata_settings);
    cover_art_policy::bind_tracks(&mut projection.favorites, metadata_settings);
}

fn load_active_source_projection(
    store: &StoreHandle,
    saved: &SavedSource,
    metadata_settings: &AppSettings,
) -> Result<ActiveSourceProjection, String> {
    let source_is_persisted = store
        .with_store(|store| store.saved_source(&saved.source.id))?
        .is_some();
    if source_is_persisted {
        store.with_store(|store| store.repair_artwork_projections(&saved.source.id))?;
        store.with_store(|store| store.ensure_collection_cover_refs(&saved.source.id))?;
    }
    let home_sections = store.with_store(|store| store.load_home_sections(&saved.source.id))?;
    let prefetched_explore = store.with_store(|store| {
        store.load_home_section_prefetch(&saved.source.id, HomeSectionKind::Explore)
    })?;
    let album_page =
        store.with_store(|store| store.load_albums(&saved.source.id, 0, SNAPSHOT_GRID_LIMIT))?;
    let track_page =
        store.with_store(|store| store.load_tracks(&saved.source.id, 0, SNAPSHOT_TRACK_LIMIT))?;
    let artist_page = store
        .with_store(|store| store.load_artists(&saved.source.id, false, 0, SNAPSHOT_GRID_LIMIT))?;
    let album_artist_page = store
        .with_store(|store| store.load_artists(&saved.source.id, true, 0, SNAPSHOT_GRID_LIMIT))?;
    let genre_page =
        store.with_store(|store| store.load_genres(&saved.source.id, 0, SNAPSHOT_GRID_LIMIT))?;
    let playlist_page =
        store.with_store(|store| store.load_playlists(&saved.source.id, 0, SNAPSHOT_GRID_LIMIT))?;
    let playlist_ids = playlist_page
        .items
        .iter()
        .map(|playlist| playlist.id.clone())
        .collect::<Vec<_>>();
    let playlist_entry_keys = store.with_store(|store| {
        store.playlist_entry_keys_for_playlists(&saved.source.id, &playlist_ids)
    })?;
    let favorites = store.with_store(|store| store.load_favorite_tracks(&saved.source.id))?;
    let mut projection = ActiveSourceProjection {
        cached_album_count: album_page.total,
        cached_track_count: track_page.total,
        cached_artist_count: artist_page.total,
        cached_album_artist_count: album_artist_page.total,
        cached_genre_count: genre_page.total,
        cached_playlist_count: playlist_page.total,
        home_sections,
        prefetched_explore,
        albums: album_page.items,
        tracks: track_page.items,
        artists: artist_page.items,
        album_artists: album_artist_page.items,
        genres: genre_page.items,
        playlists: playlist_page.items,
        playlist_entry_keys,
        favorites,
    };
    let external_ref_policy = snapshot_external_ref_policy(metadata_settings);
    scrub_projection_refs(saved, &mut projection, external_ref_policy);
    bind_projection_refs(&mut projection, metadata_settings);
    scrub_projection_refs(saved, &mut projection, external_ref_policy);
    album_track_refs(store, saved, &mut projection.albums)?;
    track_album_refs(store, saved, &mut projection.tracks, &projection.albums)?;
    for section in &mut projection.home_sections {
        home_local_refs(store, saved, section)?;
    }
    if let Some(section) = &mut projection.prefetched_explore {
        home_local_refs(store, saved, section)?;
    }
    track_album_refs(store, saved, &mut projection.favorites, &projection.albums)?;
    Ok(projection)
}

fn load_active_source_snapshot(
    store: &StoreHandle,
    source_settings: AppSettings,
    sources: Vec<SourceIdentity>,
    source_local_access: Vec<SourceLocalAccessSnapshot>,
    selected_source: LibrarySourceSelection,
    saved: SavedSource,
) -> Result<LibrarySnapshot, String> {
    let (local_access, local_access_status) = if matches!(
        configured_source_selection(&saved),
        LibrarySourceSelection::Source(_)
    ) && let Some(summary) = source_local_access
        .iter()
        .find(|summary| summary.source_id == saved.source.id)
    {
        (summary.access.clone(), summary.status.clone())
    } else {
        let local_access = store.with_store(|store| store.source_local_access(&saved.source.id))?;
        let local_access_status = local_access_status_for_server(store, local_access.as_ref())?;
        (local_access, local_access_status)
    };
    let music_folders = store.with_store(|store| store.list_music_folders(&saved.source.id))?;
    let selected_music_folder_id =
        store.with_store(|store| store.selected_music_folder_id(&saved.source.id))?;
    let metadata_settings = load_settings_from_store(store);
    let sync_state = store
        .with_store(|store| store.sync_state(&saved.source.id))
        .ok();
    let projection = load_active_source_projection(store, &saved, &metadata_settings)?;
    let status = sync_state
        .as_ref()
        .map(sync_status_text)
        .unwrap_or_else(|| "Cached library ready".to_string());
    let last_error = sync_state.and_then(|state| state.last_error);

    Ok(LibrarySnapshot {
        source: Some(saved.source),
        sources,
        selected_source: Some(selected_source),
        local_folders: source_settings.sources.local_folders,
        source_local_access,
        local_access,
        local_access_status,
        music_folders,
        selected_music_folder_id,
        first_run: false,
        sync_status: status,
        last_error,
        cached_album_count: projection.cached_album_count,
        cached_track_count: projection.cached_track_count,
        cached_artist_count: projection.cached_artist_count,
        cached_album_artist_count: projection.cached_album_artist_count,
        cached_genre_count: projection.cached_genre_count,
        cached_playlist_count: projection.cached_playlist_count,
        home_sections: projection.home_sections,
        prefetched_explore: projection.prefetched_explore,
        albums: projection.albums,
        tracks: projection.tracks,
        artists: projection.artists,
        album_artists: projection.album_artists,
        genres: projection.genres,
        playlists: projection.playlists,
        playlist_entry_keys: projection.playlist_entry_keys,
        favorites: projection.favorites,
        search: SearchResults::default(),
    })
}
