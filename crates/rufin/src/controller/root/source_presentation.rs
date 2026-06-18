use super::*;

#[derive(Clone, Debug)]
pub(in crate::controller) struct SnapshotSourceReconciliation {
    pub(in crate::controller) selected_source: LibrarySourceSelection,
    pub(in crate::controller) saved: SavedServer,
}

pub(in crate::controller) fn snapshot_remote_servers(
    saved_servers: &[SavedServer],
) -> Vec<SavedServer> {
    saved_servers
        .iter()
        .filter(|saved| saved.server.provider != LOCAL_PROVIDER_ID)
        .cloned()
        .collect()
}

pub(in crate::controller) fn snapshot_server_identities(
    remote_saved_servers: &[SavedServer],
) -> Vec<ServerIdentity> {
    remote_saved_servers
        .iter()
        .map(|saved| saved.server.clone())
        .collect()
}

pub(in crate::controller) fn snapshot_server_local_access(
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

pub(in crate::controller) fn reconcile_snapshot_source(
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

pub(in crate::controller) fn load_active_source_snapshot(
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
    let external_ref_policy = snapshot_external_ref_policy(settings);
    scrub_snapshot_album_image_refs(saved, albums, external_ref_policy);
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
    let external_ref_policy = snapshot_external_ref_policy(settings);
    scrub_snapshot_track_image_refs(saved, tracks, external_ref_policy);
}

pub(in crate::controller) fn scrub_selected_artist_image_refs(
    saved: &SavedServer,
    settings: &AppSettings,
    artists: &mut [Artist],
) {
    let external_ref_policy = snapshot_external_ref_policy(settings);
    scrub_snapshot_artist_image_refs(saved, artists, external_ref_policy);
}

pub(in crate::controller) fn scrub_selected_genre_image_refs(
    saved: &SavedServer,
    settings: &AppSettings,
    genres: &mut [Genre],
) {
    let external_ref_policy = snapshot_external_ref_policy(settings);
    scrub_snapshot_genre_image_refs(saved, genres, external_ref_policy);
}

pub(in crate::controller) fn scrub_selected_playlist_image_refs(
    saved: &SavedServer,
    settings: &AppSettings,
    playlists: &mut [Playlist],
) {
    let external_ref_policy = snapshot_external_ref_policy(settings);
    scrub_snapshot_playlist_image_refs(saved, playlists, external_ref_policy);
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

#[derive(Clone, Copy)]
pub(in crate::controller) struct SnapshotExternalRefPolicy {
    allow_identity_refs: bool,
    allow_cached_refs: bool,
}

pub(in crate::controller) fn snapshot_external_ref_policy(
    settings: &AppSettings,
) -> SnapshotExternalRefPolicy {
    let cached_refs_enabled = external_metadata::cached_refs_enabled(settings);
    SnapshotExternalRefPolicy {
        allow_identity_refs: cached_refs_enabled,
        allow_cached_refs: cached_refs_enabled && !external_metadata::enabled(settings),
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::controller) fn scrub_snapshot_image_refs(
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
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    for section in home_sections {
        scrub_snapshot_home_refs(saved, section, external_ref_policy);
    }
    if let Some(section) = prefetched_explore {
        scrub_snapshot_home_refs(saved, section, external_ref_policy);
    }
    scrub_snapshot_album_image_refs(saved, albums, external_ref_policy);
    scrub_snapshot_track_image_refs(saved, tracks, external_ref_policy);
    scrub_snapshot_artist_image_refs(saved, artists, external_ref_policy);
    scrub_snapshot_artist_image_refs(saved, album_artists, external_ref_policy);
    scrub_snapshot_genre_image_refs(saved, genres, external_ref_policy);
    scrub_snapshot_playlist_image_refs(saved, playlists, external_ref_policy);
    scrub_snapshot_track_image_refs(saved, favorites, external_ref_policy);
}

fn scrub_snapshot_home_refs(
    saved: &SavedServer,
    section: &mut HomeSection,
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    if saved.server.provider == LOCAL_PROVIDER_ID {
        section.albums.retain(|album| is_local_album_id(&album.id));
        section.tracks.retain(|track| is_local_track_id(&track.id));
    }
    scrub_snapshot_album_image_refs(saved, &mut section.albums, external_ref_policy);
    scrub_snapshot_track_image_refs(saved, &mut section.tracks, external_ref_policy);
}

fn scrub_snapshot_album_image_refs(
    saved: &SavedServer,
    albums: &mut [Album],
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    for album in albums {
        scrub_snapshot_image_ref(saved, &mut album.image_ref, external_ref_policy);
    }
}

fn scrub_snapshot_track_image_refs(
    saved: &SavedServer,
    tracks: &mut [Track],
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    for track in tracks {
        scrub_snapshot_image_ref(saved, &mut track.image_ref, external_ref_policy);
    }
}

fn scrub_snapshot_artist_image_refs(
    saved: &SavedServer,
    artists: &mut [Artist],
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    for artist in artists {
        scrub_snapshot_image_ref(saved, &mut artist.image_ref, external_ref_policy);
    }
}

fn scrub_snapshot_genre_image_refs(
    saved: &SavedServer,
    genres: &mut [Genre],
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    for genre in genres {
        scrub_snapshot_image_ref(saved, &mut genre.image_ref, external_ref_policy);
        scrub_snapshot_image_ref_vec(saved, &mut genre.image_refs, external_ref_policy);
    }
}

fn scrub_snapshot_playlist_image_refs(
    saved: &SavedServer,
    playlists: &mut [Playlist],
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    for playlist in playlists {
        scrub_snapshot_image_ref(saved, &mut playlist.image_ref, external_ref_policy);
        scrub_snapshot_image_ref_vec(saved, &mut playlist.image_refs, external_ref_policy);
    }
}

fn scrub_snapshot_image_ref(
    saved: &SavedServer,
    image_ref: &mut Option<ImageRef>,
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    let Some(ref_value) = image_ref else {
        return;
    };
    if snapshot_image_ref_allowed(saved, ref_value, external_ref_policy) {
        return;
    }
    *image_ref = None;
}

fn scrub_snapshot_image_ref_vec(
    saved: &SavedServer,
    image_refs: &mut Vec<ImageRef>,
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    image_refs
        .retain(|image_ref| snapshot_image_ref_allowed(saved, image_ref, external_ref_policy));
}

fn snapshot_image_ref_allowed(
    saved: &SavedServer,
    image_ref: &ImageRef,
    external_ref_policy: SnapshotExternalRefPolicy,
) -> bool {
    source_image_ref_allowed(saved, image_ref)
        || (external_ref_policy.allow_cached_refs
            && external_metadata::is_external_image_ref(image_ref))
        || (external_ref_policy.allow_identity_refs && external_identity_image_ref(image_ref))
}

fn external_identity_image_ref(image_ref: &ImageRef) -> bool {
    external_metadata::album_art_from_image_ref(image_ref).is_some_and(|art| {
        art.musicbrainz_release_id.is_some() || art.musicbrainz_release_group_id.is_some()
    })
}

pub(in crate::controller) fn scrub_source_image_ref(
    saved: &SavedServer,
    image_ref: &mut Option<ImageRef>,
) {
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

pub(in crate::controller) fn source_image_ref_allowed(
    saved: &SavedServer,
    image_ref: &ImageRef,
) -> bool {
    image_ref_allowed(&saved.server, image_ref)
}

pub(in crate::controller) fn image_ref_allowed(
    server: &ServerIdentity,
    image_ref: &ImageRef,
) -> bool {
    if server.provider == LOCAL_PROVIDER_ID {
        return is_local_provider_image_ref(image_ref);
    }
    !is_local_provider_image_ref(image_ref)
}

pub(in crate::controller) fn is_local_provider_image_ref(image_ref: &ImageRef) -> bool {
    image_ref.item_id.starts_with("local:cover:")
}

pub(in crate::controller) fn is_local_album_id(album_id: &AlbumId) -> bool {
    album_id.as_str().starts_with("local:album:")
}

pub(in crate::controller) fn is_local_track_id(track_id: &TrackId) -> bool {
    track_id.as_str().starts_with("local:track:")
}

pub(in crate::controller) fn is_local_artist_id(artist_id: &ArtistId) -> bool {
    artist_id.as_str().starts_with("local:artist:")
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
    let stale = input
        .sync_completed_age_seconds
        .is_none_or(|age| age >= STARTUP_CACHE_STALE_SECONDS);
    let sync_required_reason = if input.sync_status == Some("error") {
        Some(SyncRequiredReason::PreviousSyncError)
    } else if input.cached_item_count == 0 && input.sync_completed_age_seconds.is_none() {
        Some(SyncRequiredReason::EmptyCache)
    } else if input.provider == LOCAL_PROVIDER_ID
        && input.local_library_configured
        && (input.cached_item_count == 0 || stale)
    {
        Some(SyncRequiredReason::LocalManifestRefresh)
    } else if stale && input.provider != LOCAL_PROVIDER_ID {
        Some(SyncRequiredReason::RemoteCacheStale)
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
