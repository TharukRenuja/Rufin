use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::ui) struct CoverWarmTarget {
    pub(in crate::ui) image_ref: ImageRef,
    pub(in crate::ui) fetch_size: u32,
    pub(in crate::ui) size: i32,
}
#[derive(Clone, Copy)]
pub(in crate::ui) struct InitialRouteCoverMetrics {
    pub(in crate::ui) route_height: i32,
    pub(in crate::ui) app_height: i32,
    pub(in crate::ui) grid_columns: usize,
    pub(in crate::ui) grid_card_size: i32,
    pub(in crate::ui) home_showcase_seed: u64,
}
impl InitialRouteCoverMetrics {
    fn initial_visible_count(self, layout: LibraryLayout) -> usize {
        let viewport_height = self.route_height.max(self.app_height).max(1);
        match layout {
            LibraryLayout::Row => {
                let row_height = library::LIBRARY_TABLE_ROW_HEIGHT.max(1);
                (viewport_height / row_height).saturating_add(2).max(1) as usize
            }
            LibraryLayout::Grid | LibraryLayout::Detail => {
                let columns = self.grid_columns.max(1);
                let item_extent = self.grid_card_size.saturating_add(88).max(1);
                let rows = (viewport_height / item_extent).saturating_add(2).max(1) as usize;
                rows.saturating_mul(columns)
            }
        }
    }
}

const STARTUP_QUEUE_ROW_HEIGHT: i32 = 58;
const STARTUP_QUEUE_COVER_SIZE: i32 = 50;
const SOURCE_BACKGROUND_COVER_WARM_LIMIT: usize = DECODED_COVER_CACHE_LIMIT;

pub(in crate::ui) fn startup_cover_prime_jobs(shell: &Shell) -> Vec<CoverWarmJob> {
    startup_cover_jobs_from_targets(
        shell,
        startup_cover_prime_targets(shell),
        Some(STARTUP_CACHED_COVER_PRIME_LIMIT),
    )
}
pub(in crate::ui) fn startup_cover_jobs_from_targets(
    shell: &Shell,
    targets: Vec<CoverWarmTarget>,
    limit: Option<usize>,
) -> Vec<CoverWarmJob> {
    let mut seen = HashSet::new();
    let mut jobs = Vec::new();

    for target in targets {
        let decode_size = cover_decode_size(target.size, target.fetch_size);
        let Some(key) = shell.cover_cache_key(&target.image_ref, target.fetch_size) else {
            continue;
        };
        if !seen.insert(key.clone())
            || shell
                .decoded_cover_for_ref(&target.image_ref, target.fetch_size, decode_size)
                .is_some()
        {
            continue;
        }
        jobs.push(CoverWarmJob {
            key,
            image_ref: target.image_ref,
            fetch_size: target.fetch_size,
            size: decode_size,
        });
        if limit.is_some_and(|limit| jobs.len() >= limit) {
            break;
        }
    }

    jobs
}
pub(in crate::ui) fn sidebar_route_visible(settings: &AppSettings, item: SidebarRouteItem) -> bool {
    settings
        .sidebar
        .route_items
        .iter()
        .any(|entry| entry.item == item && entry.visible)
}
pub(in crate::ui) fn startup_cover_prime_targets(shell: &Shell) -> Vec<CoverWarmTarget> {
    let mut targets = startup_home_cover_prime_targets(shell);
    push_startup_playback_targets(&mut targets, &shell.state.player.borrow());
    push_startup_queue_targets(
        &mut targets,
        shell.state.queue.borrow().as_ref(),
        shell.state.queue_filter.borrow().trim(),
        shell.state.resolved_right_sidebar.get().is_visible(),
        shell.state.fullscreen_player_visible.get(),
        shell.app_root.height(),
        shell
            .state
            .library
            .borrow()
            .server
            .as_ref()
            .map(|server| &server.id),
    );
    let route = shell.state.routes.borrow().current().clone();
    if matches!(route, Route::SmartPlaylists) && shell.state.smart_playlists.borrow().is_empty() {
        let playlists = shell
            .controller
            .cached_smart_playlists_page(0, 1_000)
            .map(|page| page.items)
            .unwrap_or_else(|error| {
                warn!(%error, "failed to load cached smart playlists for startup cover prime");
                Vec::new()
            });
        *shell.state.smart_playlists.borrow_mut() = playlists;
    }
    targets.extend(startup_route_cover_targets(shell, &route));
    let Some(server_id) = shell
        .state
        .library
        .borrow()
        .server
        .as_ref()
        .map(|server| server.id.clone())
    else {
        return targets;
    };
    dedupe_warm_targets(&mut targets, &server_id);
    targets
}

pub(in crate::ui) fn push_startup_playback_targets(
    targets: &mut Vec<CoverWarmTarget>,
    player: &PlaybackSnapshot,
) {
    push_startup_cover_target(
        targets,
        player
            .current
            .as_ref()
            .and_then(|entry| entry.image_ref.as_ref()),
        THUMB_COVER_SIZE,
        player::BOTTOM_PLAYER_COVER_SIZE,
    );
}

pub(in crate::ui) fn push_startup_queue_targets(
    targets: &mut Vec<CoverWarmTarget>,
    queue: Option<&QueueSnapshot>,
    filter: &str,
    right_visible: bool,
    fullscreen_visible: bool,
    app_height: i32,
    active_server_id: Option<&ServerId>,
) {
    if !right_visible && !fullscreen_visible {
        return;
    }
    let Some(queue) = queue else {
        return;
    };
    if active_server_id.is_some_and(|server_id| server_id != &queue.server_id) {
        return;
    }
    let count = startup_queue_visible_count(app_height);
    let filter = filter.trim().to_lowercase();
    if right_visible {
        push_queue_entry_targets(targets, queue, &filter, count);
    }
    if fullscreen_visible {
        push_queue_entry_targets(targets, queue, "", count);
    }
}

fn push_queue_entry_targets(
    targets: &mut Vec<CoverWarmTarget>,
    queue: &QueueSnapshot,
    filter: &str,
    count: usize,
) {
    for entry in queue
        .entries
        .iter()
        .filter(|entry| queue_entry_matches_startup_filter(entry, filter))
        .take(count)
    {
        push_startup_cover_target(
            targets,
            entry.image_ref.as_ref(),
            THUMB_COVER_SIZE,
            STARTUP_QUEUE_COVER_SIZE,
        );
    }
}

fn queue_entry_matches_startup_filter(entry: &QueueEntry, filter: &str) -> bool {
    filter.is_empty()
        || entry.title.to_lowercase().contains(filter)
        || entry.artist.to_lowercase().contains(filter)
        || entry.album.to_lowercase().contains(filter)
        || (entry.year != 0 && entry.year.to_string().contains(filter))
}

fn startup_queue_visible_count(app_height: i32) -> usize {
    let height = app_height
        .saturating_sub(player::BOTTOM_PLAYER_HEIGHT)
        .max(STARTUP_QUEUE_ROW_HEIGHT);
    (height / STARTUP_QUEUE_ROW_HEIGHT).saturating_add(2) as usize
}

fn startup_route_cover_targets(shell: &Shell, route: &Route) -> Vec<CoverWarmTarget> {
    let targets = route_visible_cover_targets(shell, route);
    if !targets.is_empty() {
        return targets;
    }
    startup_route_cover_fallback_targets(shell, route)
}

fn startup_route_cover_fallback_targets(shell: &Shell, route: &Route) -> Vec<CoverWarmTarget> {
    let library = shell.state.library.borrow();
    let settings = shell.state.settings.borrow();
    let metrics = shell.source_route_initial_cover_metrics();
    let mut targets = Vec::new();
    match route {
        Route::Tracks if library.tracks.is_empty() && library.cached_track_count > 0 => {
            let list_settings = settings.library_list(LibraryListKey::Tracks);
            let limit = metrics.initial_visible_count(list_settings.layout);
            drop(settings);
            drop(library);
            if let Ok(page) = shell.controller.cached_tracks_page(0, limit) {
                push_track_source_warm_targets(
                    &mut targets,
                    page.items,
                    &list_settings,
                    false,
                    metrics,
                );
            }
        }
        Route::Favorites if library.favorites.is_empty() && library.cached_track_count > 0 => {
            let list_settings = settings.library_list(LibraryListKey::FavoriteTracks);
            drop(settings);
            drop(library);
            if let Ok(tracks) = shell.controller.cached_favorite_tracks() {
                push_track_source_warm_targets(&mut targets, tracks, &list_settings, true, metrics);
            }
        }
        Route::Albums if library.albums.is_empty() && library.cached_album_count > 0 => {
            let list_settings = settings.library_list(LibraryListKey::Albums);
            let limit = metrics.initial_visible_count(list_settings.layout);
            drop(settings);
            drop(library);
            if let Ok(page) = shell.controller.cached_albums_page(0, limit) {
                push_album_source_warm_targets(&mut targets, page.items, &list_settings, metrics);
            }
        }
        Route::Artists if library.artists.is_empty() && library.cached_artist_count > 0 => {
            let list_settings = settings.library_list(LibraryListKey::Artists);
            let limit = metrics.initial_visible_count(list_settings.layout);
            drop(settings);
            drop(library);
            if let Ok(page) = shell.controller.cached_artists_page(false, 0, limit) {
                push_artist_source_warm_targets(&mut targets, page.items, &list_settings, metrics);
            }
        }
        Route::AlbumArtists
            if library.album_artists.is_empty() && library.cached_album_artist_count > 0 =>
        {
            let list_settings = settings.library_list(LibraryListKey::AlbumArtists);
            let limit = metrics.initial_visible_count(list_settings.layout);
            drop(settings);
            drop(library);
            if let Ok(page) = shell.controller.cached_artists_page(true, 0, limit) {
                push_artist_source_warm_targets(&mut targets, page.items, &list_settings, metrics);
            }
        }
        Route::Genres if library.genres.is_empty() && library.cached_genre_count > 0 => {
            let list_settings = settings.library_list(LibraryListKey::Genres);
            let limit = metrics.initial_visible_count(list_settings.layout);
            drop(settings);
            drop(library);
            if let Ok(page) = shell.controller.cached_genres_page(0, limit) {
                push_genre_source_warm_targets(&mut targets, page.items, &list_settings, metrics);
            }
        }
        Route::Playlists if library.playlists.is_empty() && library.cached_playlist_count > 0 => {
            let list_settings = settings.library_list(LibraryListKey::Playlists);
            let limit = metrics.initial_visible_count(list_settings.layout);
            drop(settings);
            drop(library);
            if let Ok(page) = shell.controller.cached_playlists_page(0, limit) {
                push_playlist_source_warm_targets(
                    &mut targets,
                    page.items,
                    &list_settings,
                    metrics,
                );
            }
        }
        Route::SmartPlaylists if shell.state.smart_playlists.borrow().is_empty() => {
            let list_settings = settings.library_list(LibraryListKey::SmartPlaylists);
            drop(settings);
            drop(library);
            let playlists = shell.state.smart_playlists.borrow().clone();
            push_smart_targets(&mut targets, playlists, &list_settings, metrics);
        }
        _ => {}
    }
    targets
}

#[cfg(test)]
pub(in crate::ui) fn startup_cover_targets(
    library: &LibrarySnapshot,
    settings: &AppSettings,
    home_showcase_seed: u64,
) -> Vec<CoverWarmTarget> {
    startup_prime_targets(library, settings, home_showcase_seed)
}
pub(in crate::ui) fn source_warm_targets(
    library: &LibrarySnapshot,
    smart_playlists: &[SmartPlaylist],
    settings: &AppSettings,
    route_metrics: InitialRouteCoverMetrics,
) -> Vec<CoverWarmTarget> {
    let mut targets = Vec::new();
    push_startup_home_prime_targets(
        &mut targets,
        library,
        settings,
        route_metrics.home_showcase_seed,
    );
    let Some(server_id) = library.server.as_ref().map(|server| &server.id) else {
        return targets;
    };
    push_source_route_warm_targets(
        &mut targets,
        server_id,
        library,
        smart_playlists,
        settings,
        route_metrics,
    );
    dedupe_warm_targets(&mut targets, server_id);
    targets
}
pub(in crate::ui) fn startup_home_cover_prime_targets(shell: &Shell) -> Vec<CoverWarmTarget> {
    startup_prime_targets(
        &shell.state.library.borrow(),
        &shell.state.settings.borrow(),
        shell.state.home_showcase_seed.get(),
    )
}
pub(in crate::ui) fn startup_prime_targets(
    library: &LibrarySnapshot,
    settings: &AppSettings,
    home_showcase_seed: u64,
) -> Vec<CoverWarmTarget> {
    let mut targets = Vec::new();
    push_startup_home_prime_targets(&mut targets, library, settings, home_showcase_seed);
    targets
}
fn push_startup_home_prime_targets(
    targets: &mut Vec<CoverWarmTarget>,
    library: &LibrarySnapshot,
    settings: &AppSettings,
    home_showcase_seed: u64,
) {
    let mut section_blocks = 0_usize;
    for block in &settings.home_blocks {
        match block {
            HomeBlockKind::Showcase => {
                if let Some(album) = home::showcase_album(library, home_showcase_seed) {
                    push_startup_cover_target(
                        targets,
                        album.image_ref.as_ref(),
                        GRID_COVER_SIZE,
                        GRID_COVER_SIZE as i32,
                    );
                }
            }
            HomeBlockKind::Genres => {}
            _ => {
                if section_blocks >= STARTUP_HOME_SECTION_LIMIT {
                    continue;
                }
                let Some(kind) = block.section_kind() else {
                    continue;
                };
                let Some(section) = library
                    .home_sections
                    .iter()
                    .find(|section| section.kind == kind)
                else {
                    continue;
                };

                section_blocks = section_blocks.saturating_add(1);
                for album in section.albums.iter().take(STARTUP_HOME_SECTION_COVER_LIMIT) {
                    push_startup_cover_target(
                        targets,
                        album.image_ref.as_ref(),
                        GRID_COVER_SIZE,
                        GRID_COVER_SIZE as i32,
                    );
                }
                for track in section.tracks.iter().take(STARTUP_HOME_SECTION_COVER_LIMIT) {
                    push_startup_cover_target(
                        targets,
                        track.image_ref.as_ref(),
                        GRID_COVER_SIZE,
                        GRID_COVER_SIZE as i32,
                    );
                }
            }
        }
    }
}
pub(in crate::ui) fn row_layout_uses_cover(settings: &LibraryListSettings) -> bool {
    settings
        .row_fields
        .iter()
        .any(|field| matches!(field, LibraryField::Image | LibraryField::TitleMerged))
}
pub(in crate::ui) fn push_startup_cover_target(
    targets: &mut Vec<CoverWarmTarget>,
    image_ref: Option<&ImageRef>,
    fetch_size: u32,
    size: i32,
) {
    let Some(image_ref) = image_ref else {
        return;
    };
    targets.push(CoverWarmTarget {
        image_ref: image_ref.clone(),
        fetch_size,
        size,
    });
}
fn push_source_route_warm_targets(
    targets: &mut Vec<CoverWarmTarget>,
    server_id: &ServerId,
    library: &LibrarySnapshot,
    smart_playlists: &[SmartPlaylist],
    settings: &AppSettings,
    route_metrics: InitialRouteCoverMetrics,
) {
    if sidebar_route_visible(settings, SidebarRouteItem::Tracks) {
        let list_settings = settings.library_list(LibraryListKey::Tracks);
        push_track_source_warm_targets(
            targets,
            library.tracks.clone(),
            &list_settings,
            false,
            route_metrics,
        );
        push_track_targets(
            targets,
            library.tracks.clone(),
            &list_settings,
            route_metrics,
        );
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Albums) {
        let list_settings = settings.library_list(LibraryListKey::Albums);
        push_album_source_warm_targets(
            targets,
            library.albums.clone(),
            &list_settings,
            route_metrics,
        );
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Artists) {
        let list_settings = settings.library_list(LibraryListKey::Artists);
        push_artist_source_warm_targets(
            targets,
            library.artists.clone(),
            &list_settings,
            route_metrics,
        );
    }
    if sidebar_route_visible(settings, SidebarRouteItem::AlbumArtists) {
        let list_settings = settings.library_list(LibraryListKey::AlbumArtists);
        push_artist_source_warm_targets(
            targets,
            library.album_artists.clone(),
            &list_settings,
            route_metrics,
        );
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Genres) {
        let list_settings = settings.library_list(LibraryListKey::Genres);
        push_genre_source_warm_targets(
            targets,
            library.genres.clone(),
            &list_settings,
            route_metrics,
        );
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Favorites) {
        let list_settings = settings.library_list(LibraryListKey::FavoriteTracks);
        push_track_source_warm_targets(
            targets,
            library.favorites.clone(),
            &list_settings,
            true,
            route_metrics,
        );
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Playlists) {
        let list_settings = settings.library_list(LibraryListKey::Playlists);
        push_playlist_source_warm_targets(
            targets,
            library.playlists.clone(),
            &list_settings,
            route_metrics,
        );
    }
    if sidebar_route_visible(settings, SidebarRouteItem::SmartPlaylists) {
        let list_settings = settings.library_list(LibraryListKey::SmartPlaylists);
        push_smart_targets(
            targets,
            smart_playlists.to_vec(),
            &list_settings,
            route_metrics,
        );
    }
    push_source_background_warm_targets(targets, library, smart_playlists, settings, route_metrics);
    dedupe_warm_targets(targets, server_id);
}
fn push_track_source_warm_targets(
    targets: &mut Vec<CoverWarmTarget>,
    mut tracks: Vec<Track>,
    settings: &LibraryListSettings,
    favorite_first: bool,
    route_metrics: InitialRouteCoverMetrics,
) {
    let Some((fetch_size, size)) = source_route_cover_size(settings, route_metrics) else {
        return;
    };
    library::sort_tracks(&mut tracks, settings, favorite_first);
    for track in tracks
        .iter()
        .take(route_metrics.initial_visible_count(settings.layout))
    {
        push_startup_cover_target(targets, track.image_ref.as_ref(), fetch_size, size);
    }
}
fn push_track_targets(
    targets: &mut Vec<CoverWarmTarget>,
    mut tracks: Vec<Track>,
    settings: &LibraryListSettings,
    route_metrics: InitialRouteCoverMetrics,
) {
    let Some((fetch_size, size)) = source_route_cover_size(settings, route_metrics) else {
        return;
    };
    library::sort_tracks(&mut tracks, settings, false);
    let total = tracks.len();
    if total == 0 {
        return;
    }
    let visible_rows = route_metrics
        .initial_visible_count(settings.layout)
        .max(1)
        .min(total);
    for numerator in [1_usize, 2, 3, 4] {
        let start = total.saturating_sub(visible_rows).saturating_mul(numerator) / 4;
        let end = start.saturating_add(visible_rows).min(total);
        for track in &tracks[start..end] {
            push_startup_cover_target(targets, track.image_ref.as_ref(), fetch_size, size);
        }
    }
}
fn push_album_source_warm_targets(
    targets: &mut Vec<CoverWarmTarget>,
    mut albums: Vec<Album>,
    settings: &LibraryListSettings,
    route_metrics: InitialRouteCoverMetrics,
) {
    let Some((fetch_size, size)) = source_route_cover_size(settings, route_metrics) else {
        return;
    };
    library::sort_albums(&mut albums, settings);
    for album in albums
        .iter()
        .take(route_metrics.initial_visible_count(settings.layout))
    {
        push_startup_cover_target(targets, album.image_ref.as_ref(), fetch_size, size);
    }
}
fn push_artist_source_warm_targets(
    targets: &mut Vec<CoverWarmTarget>,
    mut artists: Vec<Artist>,
    settings: &LibraryListSettings,
    route_metrics: InitialRouteCoverMetrics,
) {
    let Some((fetch_size, size)) = source_route_cover_size(settings, route_metrics) else {
        return;
    };
    library::sort_artists(&mut artists, settings);
    for artist in artists
        .iter()
        .take(route_metrics.initial_visible_count(settings.layout))
    {
        push_startup_cover_target(targets, artist.image_ref.as_ref(), fetch_size, size);
    }
}
fn push_genre_source_warm_targets(
    targets: &mut Vec<CoverWarmTarget>,
    mut genres: Vec<Genre>,
    settings: &LibraryListSettings,
    route_metrics: InitialRouteCoverMetrics,
) {
    let Some((fetch_size, size)) = source_collection_route_cover_size(settings) else {
        return;
    };
    library::sort_genres(&mut genres, settings);
    for genre in genres
        .iter()
        .take(route_metrics.initial_visible_count(settings.layout))
    {
        for image_ref in &genre.image_refs {
            push_startup_cover_target(targets, Some(image_ref), fetch_size, size);
        }
        push_startup_cover_target(targets, genre.image_ref.as_ref(), fetch_size, size);
    }
}
fn push_playlist_source_warm_targets(
    targets: &mut Vec<CoverWarmTarget>,
    mut playlists: Vec<Playlist>,
    settings: &LibraryListSettings,
    route_metrics: InitialRouteCoverMetrics,
) {
    let Some((fetch_size, size)) = source_collection_route_cover_size(settings) else {
        return;
    };
    library::sort_playlists(&mut playlists, settings);
    for playlist in playlists
        .iter()
        .take(route_metrics.initial_visible_count(settings.layout))
    {
        for image_ref in &playlist.image_refs {
            push_startup_cover_target(targets, Some(image_ref), fetch_size, size);
        }
        push_startup_cover_target(targets, playlist.image_ref.as_ref(), fetch_size, size);
    }
}
fn push_smart_targets(
    targets: &mut Vec<CoverWarmTarget>,
    mut playlists: Vec<SmartPlaylist>,
    settings: &LibraryListSettings,
    route_metrics: InitialRouteCoverMetrics,
) {
    let Some((fetch_size, size)) = source_collection_route_cover_size(settings) else {
        return;
    };
    library::sort_smart_playlists(&mut playlists, settings);
    for playlist in playlists
        .iter()
        .take(route_metrics.initial_visible_count(settings.layout))
    {
        for image_ref in &playlist.image_refs {
            push_startup_cover_target(targets, Some(image_ref), fetch_size, size);
        }
        push_startup_cover_target(targets, playlist.image_ref.as_ref(), fetch_size, size);
    }
}
fn push_source_background_warm_targets(
    targets: &mut Vec<CoverWarmTarget>,
    library: &LibrarySnapshot,
    smart_playlists: &[SmartPlaylist],
    settings: &AppSettings,
    route_metrics: InitialRouteCoverMetrics,
) {
    let mut seen = HashSet::new();
    let mut remaining = SOURCE_BACKGROUND_COVER_WARM_LIMIT;

    if sidebar_route_visible(settings, SidebarRouteItem::Albums) {
        let list_settings = settings.library_list(LibraryListKey::Albums);
        if let Some((fetch_size, size)) = source_route_cover_size(&list_settings, route_metrics) {
            push_background_cover_refs(
                targets,
                &mut seen,
                &mut remaining,
                library
                    .albums
                    .iter()
                    .filter_map(|album| album.image_ref.as_ref()),
                fetch_size,
                size,
            );
        }
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Artists) {
        let list_settings = settings.library_list(LibraryListKey::Artists);
        if let Some((fetch_size, size)) = source_route_cover_size(&list_settings, route_metrics) {
            push_background_cover_refs(
                targets,
                &mut seen,
                &mut remaining,
                library
                    .artists
                    .iter()
                    .filter_map(|artist| artist.image_ref.as_ref()),
                fetch_size,
                size,
            );
        }
    }
    if sidebar_route_visible(settings, SidebarRouteItem::AlbumArtists) {
        let list_settings = settings.library_list(LibraryListKey::AlbumArtists);
        if let Some((fetch_size, size)) = source_route_cover_size(&list_settings, route_metrics) {
            push_background_cover_refs(
                targets,
                &mut seen,
                &mut remaining,
                library
                    .album_artists
                    .iter()
                    .filter_map(|artist| artist.image_ref.as_ref()),
                fetch_size,
                size,
            );
        }
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Genres) {
        let list_settings = settings.library_list(LibraryListKey::Genres);
        if let Some((fetch_size, size)) = source_collection_route_cover_size(&list_settings) {
            push_background_cover_refs(
                targets,
                &mut seen,
                &mut remaining,
                library
                    .genres
                    .iter()
                    .flat_map(|genre| genre.image_refs.iter().chain(genre.image_ref.iter())),
                fetch_size,
                size,
            );
        }
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Playlists) {
        let list_settings = settings.library_list(LibraryListKey::Playlists);
        if let Some((fetch_size, size)) = source_collection_route_cover_size(&list_settings) {
            push_background_cover_refs(
                targets,
                &mut seen,
                &mut remaining,
                library.playlists.iter().flat_map(|playlist| {
                    playlist.image_refs.iter().chain(playlist.image_ref.iter())
                }),
                fetch_size,
                size,
            );
        }
    }
    if sidebar_route_visible(settings, SidebarRouteItem::SmartPlaylists) {
        let list_settings = settings.library_list(LibraryListKey::SmartPlaylists);
        if let Some((fetch_size, size)) = source_collection_route_cover_size(&list_settings) {
            push_background_cover_refs(
                targets,
                &mut seen,
                &mut remaining,
                smart_playlists.iter().flat_map(|playlist| {
                    playlist.image_refs.iter().chain(playlist.image_ref.iter())
                }),
                fetch_size,
                size,
            );
        }
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Tracks) {
        let list_settings = settings.library_list(LibraryListKey::Tracks);
        if let Some((fetch_size, size)) = source_route_cover_size(&list_settings, route_metrics) {
            push_background_cover_refs(
                targets,
                &mut seen,
                &mut remaining,
                library
                    .tracks
                    .iter()
                    .filter_map(|track| track.image_ref.as_ref()),
                fetch_size,
                size,
            );
        }
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Favorites) {
        let list_settings = settings.library_list(LibraryListKey::FavoriteTracks);
        if let Some((fetch_size, size)) = source_route_cover_size(&list_settings, route_metrics) {
            push_background_cover_refs(
                targets,
                &mut seen,
                &mut remaining,
                library
                    .favorites
                    .iter()
                    .filter_map(|track| track.image_ref.as_ref()),
                fetch_size,
                size,
            );
        }
    }
}
fn push_background_cover_refs<'a>(
    targets: &mut Vec<CoverWarmTarget>,
    seen: &mut HashSet<String>,
    remaining: &mut usize,
    image_refs: impl IntoIterator<Item = &'a ImageRef>,
    fetch_size: u32,
    size: i32,
) {
    for image_ref in image_refs {
        push_background_cover_target(targets, seen, remaining, image_ref, fetch_size, size);
        if *remaining == 0 {
            break;
        }
    }
}
fn push_background_cover_target(
    targets: &mut Vec<CoverWarmTarget>,
    seen: &mut HashSet<String>,
    remaining: &mut usize,
    image_ref: &ImageRef,
    fetch_size: u32,
    size: i32,
) {
    if *remaining == 0 {
        return;
    }
    if !seen.insert(background_warm_key(image_ref)) {
        return;
    }
    targets.push(CoverWarmTarget {
        image_ref: image_ref.clone(),
        fetch_size,
        size,
    });
    *remaining = (*remaining).saturating_sub(1);
}
fn background_warm_key(image_ref: &ImageRef) -> String {
    format!(
        "{}\u{1f}{}",
        image_ref.item_id,
        image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED),
    )
}
fn source_route_cover_size(
    settings: &LibraryListSettings,
    route_metrics: InitialRouteCoverMetrics,
) -> Option<(u32, i32)> {
    match settings.layout {
        LibraryLayout::Grid => Some((GRID_COVER_SIZE, route_metrics.grid_card_size)),
        LibraryLayout::Detail => Some((GRID_COVER_SIZE, GRID_COVER_SIZE as i32)),
        LibraryLayout::Row if row_layout_uses_cover(settings) => Some((THUMB_COVER_SIZE, 48)),
        LibraryLayout::Row => None,
    }
}
fn source_collection_route_cover_size(settings: &LibraryListSettings) -> Option<(u32, i32)> {
    match settings.layout {
        LibraryLayout::Grid | LibraryLayout::Detail => {
            Some((THUMB_COVER_SIZE, THUMB_COVER_SIZE as i32))
        }
        LibraryLayout::Row if row_layout_uses_cover(settings) => Some((THUMB_COVER_SIZE, 48)),
        LibraryLayout::Row => None,
    }
}
pub(in crate::ui) fn dedupe_warm_targets(targets: &mut Vec<CoverWarmTarget>, server_id: &ServerId) {
    let mut positions = HashMap::<String, usize>::new();
    let mut deduped = Vec::<CoverWarmTarget>::new();
    for target in targets.drain(..) {
        let key = warm_dedupe_key(server_id, &target.image_ref);
        if let Some(index) = positions.get(&key).copied() {
            let existing = &mut deduped[index];
            let existing_decode_size = cover_decode_size(existing.size, existing.fetch_size);
            let target_decode_size = cover_decode_size(target.size, target.fetch_size);
            if (target.fetch_size, target_decode_size) > (existing.fetch_size, existing_decode_size)
            {
                existing.fetch_size = target.fetch_size;
                existing.size = target.size;
            }
            continue;
        }
        positions.insert(key, deduped.len());
        deduped.push(target);
    }
    *targets = deduped;
}
fn warm_dedupe_key(server_id: &ServerId, image_ref: &ImageRef) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        server_id.as_str(),
        image_ref.item_id,
        image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED),
    )
}
pub(in crate::ui) fn cover_group_slots(image_refs: &[ImageRef]) -> Vec<ImageRef> {
    let Some(first) = image_refs.first() else {
        return Vec::new();
    };
    if image_refs.len() == 1 {
        return vec![first.clone()];
    }
    (0..4)
        .filter_map(|index| image_refs.get(index % image_refs.len()).cloned())
        .collect()
}
pub(in crate::ui) fn decoded_cover_candidate_sizes(preferred_size: u32) -> Vec<u32> {
    let mut sizes = Vec::from([preferred_size]);
    if preferred_size <= THUMB_COVER_SIZE {
        sizes.extend([THUMB_COVER_SIZE, GRID_COVER_SIZE, DETAIL_COVER_SIZE]);
    } else if preferred_size <= GRID_COVER_SIZE {
        sizes.extend([GRID_COVER_SIZE, DETAIL_COVER_SIZE]);
    } else {
        sizes.extend([DETAIL_COVER_SIZE, GRID_COVER_SIZE]);
    }
    let mut seen = HashSet::new();
    sizes.retain(|size| seen.insert(*size));
    sizes
}
pub(in crate::ui) fn playback_artwork_cache_keys(
    server_id: &ServerId,
    image_ref: &ImageRef,
    preferred_size: u32,
) -> Vec<String> {
    decoded_cover_candidate_sizes(preferred_size)
        .into_iter()
        .map(|size| {
            image_cache_key(
                server_id,
                &image_ref.item_id,
                image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED),
                size,
            )
        })
        .collect()
}
pub(in crate::ui) fn playback_artwork_path_from_lookup(
    server_id: &ServerId,
    image_ref: &ImageRef,
    preferred_size: u32,
    mut lookup: impl FnMut(&str) -> Option<PathBuf>,
) -> Option<PlaybackArtworkPath> {
    playback_artwork_cache_keys(server_id, image_ref, preferred_size)
        .into_iter()
        .find_map(|key| lookup(&key).map(|path| PlaybackArtworkPath { key, path }))
}
pub(in crate::ui) fn playback_artwork_key_matches(
    server_id: &ServerId,
    image_ref: &ImageRef,
    preferred_size: u32,
    key: &str,
) -> bool {
    playback_artwork_cache_keys(server_id, image_ref, preferred_size)
        .iter()
        .any(|candidate| candidate == key)
}
pub(in crate::ui) fn notification_icon_path(path: &Path) -> Option<Vec<u8>> {
    let pixbuf = Pixbuf::from_file(path).ok()?;
    notification_icon_pixbuf(&pixbuf)
}
pub(in crate::ui) fn notification_icon_pixbuf(pixbuf: &Pixbuf) -> Option<Vec<u8>> {
    let target_size = THUMB_COVER_SIZE.clamp(1, 512) as i32;
    let width = pixbuf.width().max(1);
    let height = pixbuf.height().max(1);
    let crop_size = width.min(height);
    let crop_x = (width - crop_size) / 2;
    let crop_y = (height - crop_size) / 2;
    let cropped = Pixbuf::new(Colorspace::Rgb, pixbuf.has_alpha(), 8, crop_size, crop_size)?;
    pixbuf.copy_area(crop_x, crop_y, crop_size, crop_size, &cropped, 0, 0);
    let icon = if crop_size == target_size {
        cropped
    } else {
        cropped.scale_simple(target_size, target_size, InterpType::Bilinear)?
    };

    icon.save_to_bufferv("png", &[]).ok()
}
pub(in crate::ui) fn cover_decode_size(display_size: i32, fetch_size: u32) -> i32 {
    display_size.max(fetch_size as i32).max(1)
}
pub(in crate::ui) fn first_run_cover_prime_refs(library: &LibrarySnapshot) -> Vec<ImageRef> {
    let mut refs = Vec::new();
    let mut seen = HashSet::new();

    for section in library
        .home_sections
        .iter()
        .take(FIRST_RUN_HOME_SECTION_LIMIT)
    {
        for album in section.albums.iter().take(HOME_COVER_LIMIT) {
            push_first_run_cover_ref(&mut refs, &mut seen, album.image_ref.as_ref());
        }
        for track in section.tracks.iter().take(HOME_COVER_LIMIT) {
            push_first_run_cover_ref(&mut refs, &mut seen, track.image_ref.as_ref());
        }
    }

    for track in library.tracks.iter().take(TRACK_ROUTE_PAGE_SIZE) {
        push_first_run_cover_ref(&mut refs, &mut seen, track.image_ref.as_ref());
    }
    for album in library.albums.iter().take(GRID_ROUTE_PAGE_SIZE) {
        push_first_run_cover_ref(&mut refs, &mut seen, album.image_ref.as_ref());
    }
    for artist in library.artists.iter().take(GRID_ROUTE_PAGE_SIZE) {
        push_first_run_cover_ref(&mut refs, &mut seen, artist.image_ref.as_ref());
    }
    for artist in library.album_artists.iter().take(GRID_ROUTE_PAGE_SIZE) {
        push_first_run_cover_ref(&mut refs, &mut seen, artist.image_ref.as_ref());
    }
    for genre in library.genres.iter().take(GRID_ROUTE_PAGE_SIZE) {
        for image_ref in &genre.image_refs {
            push_first_run_cover_ref(&mut refs, &mut seen, Some(image_ref));
        }
        push_first_run_cover_ref(&mut refs, &mut seen, genre.image_ref.as_ref());
    }
    for playlist in library.playlists.iter().take(GRID_ROUTE_PAGE_SIZE) {
        for image_ref in &playlist.image_refs {
            push_first_run_cover_ref(&mut refs, &mut seen, Some(image_ref));
        }
        push_first_run_cover_ref(&mut refs, &mut seen, playlist.image_ref.as_ref());
    }

    refs
}
pub(in crate::ui) fn push_first_run_cover_ref(
    refs: &mut Vec<ImageRef>,
    seen: &mut HashSet<(String, String)>,
    image_ref: Option<&ImageRef>,
) {
    if refs.len() >= GRID_COVER_LIMIT {
        return;
    }
    let Some(image_ref) = image_ref else {
        return;
    };
    let key = (
        image_ref.item_id.clone(),
        image_ref.tag.clone().unwrap_or_default(),
    );
    if seen.insert(key) {
        refs.push(image_ref.clone());
    }
}
pub(in crate::ui) fn prefetched_explore_from_snapshot(
    snapshot: &LibrarySnapshot,
) -> Option<PrefetchedHomeSection> {
    Some(PrefetchedHomeSection {
        server_id: snapshot.server.as_ref()?.id.clone(),
        section: snapshot.prefetched_explore.clone()?,
    })
}
pub(in crate::ui) fn upsert_snapshot_home_section(
    sections: &mut Vec<HomeSection>,
    section: HomeSection,
) {
    if let Some(existing) = sections
        .iter_mut()
        .find(|existing| existing.kind == section.kind)
    {
        *existing = section;
    } else if section.kind == HomeSectionKind::Explore {
        sections.insert(0, section);
    } else {
        sections.push(section);
    }
}
pub(in crate::ui) fn reset_home_section_pages(
    states: &mut HashMap<HomeSectionKind, HomeSectionState>,
) {
    states.clear();
}
