use super::*;
use crate::source_setup::{
    configured_source_identity, configured_source_needs_auth, configured_source_selection,
    local_configured_source_for_store, resolve_source_registration,
};

#[derive(Clone, Debug)]
struct SnapshotSourceResolution {
    selected_source: LibrarySourceSelection,
    saved: StoredSource,
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

fn snapshot_remote_servers(saved_sources: &[StoredSource]) -> Vec<StoredSource> {
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

fn snapshot_source_identity(saved: &StoredSource) -> Result<SourceIdentity, String> {
    if resolve_source_registration(&saved.kind).is_some() {
        return configured_source_identity(saved);
    }
    Ok(SourceIdentity {
        id: saved.source_id.clone(),
        kind: saved.kind.clone(),
        name: saved.name.clone(),
        base_url: String::new(),
    })
}

fn snapshot_source_identities(
    remote_saved_sources: &[StoredSource],
) -> Result<Vec<SourceIdentity>, String> {
    remote_saved_sources
        .iter()
        .map(snapshot_source_identity)
        .collect()
}

fn snapshot_source_local_access(
    store: &StoreHandle,
    remote_saved_sources: &[StoredSource],
) -> Result<Vec<SourceLocalAccessSnapshot>, String> {
    remote_saved_sources
        .iter()
        .map(|saved| snapshot_source_local_access_summary(store, saved))
        .collect()
}

fn snapshot_source_local_access_summary(
    store: &StoreHandle,
    saved: &StoredSource,
) -> Result<SourceLocalAccessSnapshot, String> {
    let access = store.with_store(|store| store.source_local_access(&saved.source_id))?;
    let status = local_access_status_for_server(store, access.as_ref())?;
    let cached_album_count = store
        .with_store(|store| {
            store
                .load_albums(&saved.source_id, 0, 1)
                .map(|page| page.total)
        })
        .unwrap_or_default();
    let cached_track_count = store
        .with_store(|store| {
            store
                .load_tracks(&saved.source_id, 0, 1)
                .map(|page| page.total)
        })
        .unwrap_or_default();
    let selected_music_folder_name = store
        .with_store(|store| {
            let selected = store.selected_music_folder_id(&saved.source_id)?;
            let folders = store.list_music_folders(&saved.source_id)?;
            Ok(selected.and_then(|selected| {
                folders
                    .into_iter()
                    .find(|folder| folder.id == selected)
                    .map(|folder| folder.name)
            }))
        })
        .unwrap_or_default();
    Ok(SourceLocalAccessSnapshot {
        source_id: saved.source_id.clone(),
        access,
        status,
        selected_music_folder_name,
        cached_album_count,
        cached_track_count,
    })
}

fn resolve_snapshot_source(
    store: &StoreHandle,
    settings: &StoredSettings,
    saved_sources: &[StoredSource],
    remote_saved_sources: &[StoredSource],
) -> Result<Option<SnapshotSourceResolution>, String> {
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

    Ok(Some(SnapshotSourceResolution {
        selected_source,
        saved,
    }))
}

fn saved_server_for_snapshot_source(
    store: &StoreHandle,
    remote_saved_sources: &[StoredSource],
    persisted_active: Option<&StoredSource>,
    selected_source: &LibrarySourceSelection,
) -> Result<StoredSource, String> {
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
            .find(|saved| &saved.source_id == source_id)
            .cloned()
            .ok_or_else(|| "The selected source is no longer saved.".to_string()),
    }
}

fn local_source_configured(
    store: &StoreHandle,
    settings: &StoredSettings,
    saved_sources: &[StoredSource],
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
                let tracks = store.load_tracks(&saved.source_id, 0, 1)?.total;
                let albums = store.load_albums(&saved.source_id, 0, 1)?.total;
                Ok(tracks > 0 || albums > 0)
            })
            .unwrap_or(false)
    })
}

fn resolve_selected_source(
    settings: &StoredSettings,
    remote_saved_sources: &[StoredSource],
    active_source: Option<StoredSource>,
    local_source_configured: bool,
) -> Option<LibrarySourceSelection> {
    match &settings.sources.selected {
        Some(LibrarySourceSelection::Local) if local_source_configured => {
            return Some(LibrarySourceSelection::Local);
        }
        Some(LibrarySourceSelection::Source(source_id))
            if remote_saved_sources
                .iter()
                .any(|saved| saved.source_id == *source_id) =>
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
    let sources = snapshot_source_identities(&remote_saved_sources)?;
    let source_local_access = snapshot_source_local_access(store, &remote_saved_sources)?;
    let Some(resolved_source) = resolve_snapshot_source(
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
    let SnapshotSourceResolution {
        selected_source,
        saved,
    } = resolved_source;
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
        .with_store(|store| store.stored_source(&server.id))?
        .ok_or_else(|| "The selected source is no longer saved.".to_string())?;
    if resolve_source_registration(&saved.kind).is_none() {
        return Ok(true);
    }
    configured_source_needs_auth(secrets, &saved)
}

fn load_active_source_projection(
    store: &Store,
    saved: &StoredSource,
) -> StoreResult<ActiveSourceProjection> {
    let home_sections = store.load_home_sections(&saved.source_id)?;
    let prefetched_explore =
        store.load_home_section_prefetch(&saved.source_id, HomeSectionKind::Explore)?;
    let album_page = store.load_albums(&saved.source_id, 0, SNAPSHOT_GRID_LIMIT)?;
    let track_page = store.load_tracks(&saved.source_id, 0, SNAPSHOT_TRACK_LIMIT)?;
    let artist_page = store.load_artists(&saved.source_id, false, 0, SNAPSHOT_GRID_LIMIT)?;
    let album_artist_page = store.load_artists(&saved.source_id, true, 0, SNAPSHOT_GRID_LIMIT)?;
    let genre_page = store.load_genres(&saved.source_id, 0, SNAPSHOT_GRID_LIMIT)?;
    let playlist_page = store.load_playlists(&saved.source_id, 0, SNAPSHOT_GRID_LIMIT)?;
    let playlist_ids = playlist_page
        .items
        .iter()
        .map(|playlist| playlist.id.clone())
        .collect::<Vec<_>>();
    let playlist_entry_keys =
        store.playlist_entry_keys_for_playlists(&saved.source_id, &playlist_ids)?;
    let favorites = store.load_favorite_tracks(&saved.source_id)?;
    let projection = ActiveSourceProjection {
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
    Ok(projection)
}

fn load_active_source_snapshot(
    store: &StoreHandle,
    source_settings: StoredSettings,
    sources: Vec<SourceIdentity>,
    mut source_local_access: Vec<SourceLocalAccessSnapshot>,
    selected_source: LibrarySourceSelection,
    saved: StoredSource,
) -> Result<LibrarySnapshot, String> {
    let source = snapshot_source_identity(&saved)?;
    let (
        local_access,
        local_access_status,
        music_folders,
        selected_music_folder_id,
        sync_state,
        projection,
    ) = store.with_store_session(|store| {
        store
            .read_snapshot(|store| {
                let local_access = store.source_local_access(&saved.source_id)?;
                let local_access_status =
                    local_access_status_from_store(store, local_access.as_ref())?;
                Ok((
                    local_access,
                    local_access_status,
                    store.list_music_folders(&saved.source_id)?,
                    store.selected_music_folder_id(&saved.source_id)?,
                    store.sync_state(&saved.source_id).ok(),
                    load_active_source_projection(store, &saved)?,
                ))
            })
            .map_err(|error| error.to_string())
    })?;
    if let Some(summary) = source_local_access
        .iter_mut()
        .find(|summary| summary.source_id == saved.source_id)
    {
        summary.access = local_access.clone();
        summary.status = local_access_status.clone();
        summary.cached_album_count = projection.cached_album_count;
        summary.cached_track_count = projection.cached_track_count;
        summary.selected_music_folder_name =
            selected_music_folder_id.as_ref().and_then(|selected| {
                music_folders
                    .iter()
                    .find(|folder| &folder.id == selected)
                    .map(|folder| folder.name.clone())
            });
    }
    let cache = sync_state.map_or(LibraryCacheState::NoCache { revision: 0 }, |state| {
        if state.last_all_completed_at.is_some() {
            LibraryCacheState::Committed {
                revision: state.cache_revision,
            }
        } else {
            LibraryCacheState::NoCache {
                revision: state.cache_revision,
            }
        }
    });

    Ok(LibrarySnapshot {
        source: Some(source),
        sources,
        selected_source: Some(selected_source),
        local_folders: source_settings.sources.local_folders,
        source_local_access,
        local_access,
        local_access_status,
        music_folders,
        selected_music_folder_id,
        first_run: false,
        cache,
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
