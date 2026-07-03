use super::*;

#[derive(Clone, Copy)]
pub(in crate::ui) struct InitialRouteCoverMetrics {
    pub(in crate::ui) route_height: i32,
    pub(in crate::ui) app_height: i32,
    pub(in crate::ui) grid_columns: usize,
    pub(in crate::ui) grid_card_size: i32,
    pub(in crate::ui) album_grid_columns: usize,
    pub(in crate::ui) album_grid_card_size: i32,
    pub(in crate::ui) home_showcase_seed: u64,
}
impl InitialRouteCoverMetrics {
    fn initial_visible_count(self, key: LibraryListKey, settings: &LibraryListSettings) -> usize {
        let viewport_height = self.route_height.max(self.app_height).max(1);
        match settings.layout {
            LibraryLayout::Row => {
                let row_height = library::LIBRARY_TABLE_ROW_HEIGHT.max(1);
                (viewport_height / row_height).saturating_add(2).max(1) as usize
            }
            LibraryLayout::Grid | LibraryLayout::Detail => {
                let (columns, card_size) = self.collection_grid_metrics(key, settings);
                let item_extent = library::collection_grid_item_extent(card_size, settings);
                let rows = (viewport_height / item_extent).saturating_add(2).max(1) as usize;
                rows.saturating_mul(columns)
            }
        }
    }

    fn collection_grid_metrics(
        self,
        key: LibraryListKey,
        settings: &LibraryListSettings,
    ) -> (usize, i32) {
        if key == LibraryListKey::Albums && settings.layout == LibraryLayout::Grid {
            (self.album_grid_columns, self.album_grid_card_size)
        } else {
            (self.grid_columns, self.grid_card_size)
        }
    }
}

const SOURCE_BACKGROUND_COVER_WARM_LIMIT: usize = DECODED_COVER_CACHE_LIMIT;

pub(in crate::ui) fn sidebar_route_visible(settings: &AppSettings, item: SidebarRouteItem) -> bool {
    settings
        .sidebar
        .route_items
        .iter()
        .any(|entry| entry.item == item && entry.visible)
}
#[cfg(test)]
fn startup_cover_targets(
    library: &LibrarySnapshot,
    settings: &AppSettings,
    home_showcase_seed: u64,
) -> Vec<CoverWarmTarget> {
    startup_prime_targets(library, settings, home_showcase_seed)
}
pub(in crate::ui::root::cover) fn source_warm_targets(
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
pub(in crate::ui::root::cover) fn startup_home_cover_prime_targets(
    shell: &Shell,
) -> Vec<CoverWarmTarget> {
    startup_prime_targets(
        &shell.state.library.borrow(),
        &shell.state.settings.borrow(),
        shell.state.home_showcase_seed.get(),
    )
}
fn startup_prime_targets(
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
pub(in crate::ui::root) fn push_startup_cover_target(
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
            LibraryListKey::Tracks,
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
            LibraryListKey::Artists,
            &list_settings,
            route_metrics,
        );
    }
    if sidebar_route_visible(settings, SidebarRouteItem::AlbumArtists) {
        let list_settings = settings.library_list(LibraryListKey::AlbumArtists);
        push_artist_source_warm_targets(
            targets,
            library.album_artists.clone(),
            LibraryListKey::AlbumArtists,
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
            LibraryListKey::FavoriteTracks,
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
    key: LibraryListKey,
    settings: &LibraryListSettings,
    favorite_first: bool,
    route_metrics: InitialRouteCoverMetrics,
) {
    let Some((fetch_size, size)) = source_route_cover_size(key, settings, route_metrics) else {
        return;
    };
    library::sort_tracks(&mut tracks, settings, favorite_first);
    for track in tracks
        .iter()
        .take(route_metrics.initial_visible_count(key, settings))
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
    let Some((fetch_size, size)) =
        source_route_cover_size(LibraryListKey::Tracks, settings, route_metrics)
    else {
        return;
    };
    library::sort_tracks(&mut tracks, settings, false);
    let total = tracks.len();
    if total == 0 {
        return;
    }
    let visible_rows = route_metrics
        .initial_visible_count(LibraryListKey::Tracks, settings)
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
    let Some((fetch_size, size)) =
        source_route_cover_size(LibraryListKey::Albums, settings, route_metrics)
    else {
        return;
    };
    library::sort_albums(&mut albums, settings);
    for album in albums
        .iter()
        .take(route_metrics.initial_visible_count(LibraryListKey::Albums, settings))
    {
        push_startup_cover_target(targets, album.image_ref.as_ref(), fetch_size, size);
    }
}
fn push_artist_source_warm_targets(
    targets: &mut Vec<CoverWarmTarget>,
    mut artists: Vec<Artist>,
    key: LibraryListKey,
    settings: &LibraryListSettings,
    route_metrics: InitialRouteCoverMetrics,
) {
    let Some((fetch_size, size)) = source_route_cover_size(key, settings, route_metrics) else {
        return;
    };
    library::sort_artists(&mut artists, settings);
    for artist in artists
        .iter()
        .take(route_metrics.initial_visible_count(key, settings))
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
        .take(route_metrics.initial_visible_count(LibraryListKey::Genres, settings))
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
        .take(route_metrics.initial_visible_count(LibraryListKey::Playlists, settings))
    {
        for image_ref in &playlist.image_refs {
            push_startup_cover_target(targets, Some(image_ref), fetch_size, size);
        }
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
        .take(route_metrics.initial_visible_count(LibraryListKey::SmartPlaylists, settings))
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
        if let Some((fetch_size, size)) =
            source_route_cover_size(LibraryListKey::Albums, &list_settings, route_metrics)
        {
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
        if let Some((fetch_size, size)) =
            source_route_cover_size(LibraryListKey::Artists, &list_settings, route_metrics)
        {
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
        if let Some((fetch_size, size)) =
            source_route_cover_size(LibraryListKey::AlbumArtists, &list_settings, route_metrics)
        {
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
            push_background_cover_ref_values(
                targets,
                &mut seen,
                &mut remaining,
                library.genres.iter().flat_map(|genre| {
                    crate::cover_art_policy::selected_genre_artwork(genre).image_refs
                }),
                fetch_size,
                size,
            );
        }
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Playlists) {
        let list_settings = settings.library_list(LibraryListKey::Playlists);
        if let Some((fetch_size, size)) = source_collection_route_cover_size(&list_settings) {
            push_background_cover_ref_values(
                targets,
                &mut seen,
                &mut remaining,
                library.playlists.iter().flat_map(|playlist| {
                    crate::cover_art_policy::selected_playlist_artwork(playlist, settings)
                        .image_refs
                }),
                fetch_size,
                size,
            );
        }
    }
    if sidebar_route_visible(settings, SidebarRouteItem::SmartPlaylists) {
        let list_settings = settings.library_list(LibraryListKey::SmartPlaylists);
        if let Some((fetch_size, size)) = source_collection_route_cover_size(&list_settings) {
            push_background_cover_ref_values(
                targets,
                &mut seen,
                &mut remaining,
                smart_playlists.iter().flat_map(|playlist| {
                    crate::cover_art_policy::selected_smart_playlist_artwork(playlist).image_refs
                }),
                fetch_size,
                size,
            );
        }
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Tracks) {
        let list_settings = settings.library_list(LibraryListKey::Tracks);
        if let Some((fetch_size, size)) =
            source_route_cover_size(LibraryListKey::Tracks, &list_settings, route_metrics)
        {
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
        if let Some((fetch_size, size)) = source_route_cover_size(
            LibraryListKey::FavoriteTracks,
            &list_settings,
            route_metrics,
        ) {
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

fn push_background_cover_ref_values(
    targets: &mut Vec<CoverWarmTarget>,
    seen: &mut HashSet<String>,
    remaining: &mut usize,
    image_refs: impl IntoIterator<Item = ImageRef>,
    fetch_size: u32,
    size: i32,
) {
    for image_ref in image_refs {
        push_background_cover_target(targets, seen, remaining, &image_ref, fetch_size, size);
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
    key: LibraryListKey,
    settings: &LibraryListSettings,
    route_metrics: InitialRouteCoverMetrics,
) -> Option<(u32, i32)> {
    match settings.layout {
        LibraryLayout::Grid => Some((
            GRID_COVER_SIZE,
            route_metrics.collection_grid_metrics(key, settings).1,
        )),
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
fn dedupe_warm_targets(targets: &mut Vec<CoverWarmTarget>, server_id: &ServerId) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::root::shell_tests::{
        test_album, test_image_ref, test_initial_route_metrics, test_library_snapshot,
        test_playlist, test_server, test_smart_playlist, test_track,
    };
    use domain::GenreId;

    #[test]
    fn startup_home_targets_ignore_route_sources() {
        let mut library = test_library_snapshot();
        let home_ref = test_image_ref("home");
        let mut home_album = test_album("Home Artist", Some(ArtistId::fake(90)));
        home_album.image_ref = Some(home_ref.clone());
        library.home_sections = vec![HomeSection {
            kind: HomeSectionKind::Explore,
            albums: vec![home_album],
            tracks: Vec::new(),
        }];

        let first_track_ref = test_image_ref("track-a");
        let mut first_track = test_track("Route Artist", Some(ArtistId::fake(1)));
        first_track.title = "A route track".to_string();
        first_track.image_ref = Some(first_track_ref.clone());
        let mut second_track = test_track("Route Artist", Some(ArtistId::fake(1)));
        second_track.id = TrackId::fake(2);
        second_track.title = "B route track".to_string();
        second_track.image_ref = Some(test_image_ref("track-b"));
        library.tracks = vec![second_track, first_track];

        let first_album_ref = test_image_ref("album-a");
        let mut first_album = test_album("Route Artist", Some(ArtistId::fake(2)));
        first_album.title = "A route album".to_string();
        first_album.image_ref = Some(first_album_ref.clone());
        let mut second_album = test_album("Route Artist", Some(ArtistId::fake(2)));
        second_album.id = AlbumId::fake(2);
        second_album.title = "B route album".to_string();
        second_album.image_ref = Some(test_image_ref("album-b"));
        library.albums = vec![second_album, first_album];

        let settings = AppSettings {
            home_blocks: vec![HomeBlockKind::Explore],
            ..Default::default()
        };
        let targets = startup_cover_targets(&library, &settings, 0);
        let target_refs = targets
            .iter()
            .map(|target| target.image_ref.item_id.as_str())
            .collect::<Vec<_>>();

        assert!(target_refs.contains(&home_ref.item_id.as_str()));
        assert!(!target_refs.contains(&first_track_ref.item_id.as_str()));
        assert!(!target_refs.contains(&first_album_ref.item_id.as_str()));

        let home_targets = startup_prime_targets(&library, &settings, 0);
        let home_target_refs = home_targets
            .iter()
            .map(|target| target.image_ref.item_id.as_str())
            .collect::<Vec<_>>();
        assert!(home_target_refs.contains(&home_ref.item_id.as_str()));
        assert!(!home_target_refs.contains(&first_track_ref.item_id.as_str()));
        assert!(!home_target_refs.contains(&first_album_ref.item_id.as_str()));
    }

    #[test]
    fn source_warm_includes_route_matrix_once() {
        let mut library = test_library_snapshot();
        library.server = Some(test_server("source"));
        let first_track_ref = test_image_ref("track-a");
        let mut first_track = test_track("Route Artist", Some(ArtistId::fake(1)));
        first_track.title = "A route track".to_string();
        first_track.image_ref = Some(first_track_ref.clone());
        let mut second_track = test_track("Route Artist", Some(ArtistId::fake(1)));
        second_track.id = TrackId::fake(2);
        second_track.title = "B route track".to_string();
        second_track.image_ref = Some(test_image_ref("track-b"));
        library.tracks = vec![second_track, first_track];

        let first_album_ref = test_image_ref("album-a");
        let mut first_album = test_album("Route Artist", Some(ArtistId::fake(2)));
        first_album.title = "A route album".to_string();
        first_album.image_ref = Some(first_album_ref.clone());
        let mut second_album = test_album("Route Artist", Some(ArtistId::fake(2)));
        second_album.id = AlbumId::fake(2);
        second_album.title = "B route album".to_string();
        second_album.image_ref = Some(test_image_ref("album-b"));
        library.albums = vec![second_album, first_album];

        let settings = AppSettings::default();
        let targets = source_warm_targets(&library, &[], &settings, test_initial_route_metrics());
        let target_refs = targets
            .iter()
            .map(|target| target.image_ref.item_id.as_str())
            .collect::<Vec<_>>();

        assert!(target_refs.contains(&first_track_ref.item_id.as_str()));
        assert!(target_refs.contains(&first_album_ref.item_id.as_str()));
        assert_eq!(
            target_refs
                .iter()
                .filter(|item_id| **item_id == first_track_ref.item_id)
                .count(),
            1
        );
        assert!(
            target_refs
                .iter()
                .position(|item_id| *item_id == first_track_ref.item_id)
                < target_refs
                    .iter()
                    .position(|item_id| *item_id == first_album_ref.item_id)
        );
    }

    #[test]
    fn source_warm_includes_group_refs() {
        let shared = test_image_ref("shared-art");
        let genre_only = test_image_ref("genre-only");
        let mut library = test_library_snapshot();
        library.server = Some(test_server("source"));
        let mut track = test_track("Route Artist", Some(ArtistId::fake(1)));
        track.image_ref = Some(shared.clone());
        library.tracks = vec![track];
        library.genres = vec![Genre {
            id: GenreId::fake(1),
            name: "Genre".to_string(),
            album_count: 1,
            track_count: 1,
            duration_seconds: 180,
            image_refs: vec![shared.clone(), genre_only.clone()],
            image_ref: Some(shared.clone()),
        }];

        let settings = AppSettings::default();
        let targets = source_warm_targets(&library, &[], &settings, test_initial_route_metrics());

        assert_eq!(
            targets
                .iter()
                .filter(|target| target.image_ref.item_id == shared.item_id)
                .count(),
            1
        );
        assert!(
            targets
                .iter()
                .any(|target| target.image_ref.item_id == genre_only.item_id)
        );
    }

    #[test]
    fn source_warm_includes_playlists() {
        let playlist_ref = test_image_ref("playlist-group");
        let smart_ref = test_image_ref("smart-group");
        let mut library = test_library_snapshot();
        library.server = Some(test_server("source"));
        library.playlists = vec![test_playlist("Regular", playlist_ref.clone())];
        let smart_playlists = vec![test_smart_playlist("Smart", smart_ref.clone())];

        let settings = AppSettings::default();
        let targets = source_warm_targets(
            &library,
            &smart_playlists,
            &settings,
            test_initial_route_metrics(),
        );
        let target_refs = targets
            .iter()
            .map(|target| target.image_ref.item_id.as_str())
            .collect::<Vec<_>>();

        assert!(target_refs.contains(&playlist_ref.item_id.as_str()));
        assert!(target_refs.contains(&smart_ref.item_id.as_str()));
    }

    #[test]
    fn source_warm_includes_background_refs() {
        let mut library = test_library_snapshot();
        library.server = Some(test_server("source"));
        let background_ref = test_image_ref("background-album");
        library.albums = (0..24)
            .map(|index| {
                let mut album = test_album("Route Artist", Some(ArtistId::fake(index + 1)));
                album.id = AlbumId::fake(index + 1);
                album.title = format!("Album {index:02}");
                album.image_ref = Some(if index == 23 {
                    background_ref.clone()
                } else {
                    test_image_ref(&format!("album-{index:02}"))
                });
                album
            })
            .collect();

        let targets = source_warm_targets(
            &library,
            &[],
            &AppSettings::default(),
            test_initial_route_metrics(),
        );

        assert!(
            targets
                .iter()
                .any(|target| target.image_ref.item_id == background_ref.item_id)
        );
    }

    #[test]
    fn album_grid_warm_uses_album_metrics() {
        let metrics = test_initial_route_metrics();
        let album_settings = LibraryListSettings {
            layout: LibraryLayout::Grid,
            ..LibraryListSettings::for_key(LibraryListKey::Albums)
        };
        let track_settings = LibraryListSettings {
            layout: LibraryLayout::Grid,
            ..LibraryListSettings::for_key(LibraryListKey::Tracks)
        };

        assert_eq!(
            source_route_cover_size(LibraryListKey::Albums, &album_settings, metrics),
            Some((GRID_COVER_SIZE, metrics.album_grid_card_size))
        );
        assert_eq!(
            source_route_cover_size(LibraryListKey::Tracks, &track_settings, metrics),
            Some((GRID_COVER_SIZE, metrics.grid_card_size))
        );
        assert_ne!(metrics.album_grid_card_size, metrics.grid_card_size);
    }

    #[test]
    fn source_warm_skips_hidden_routes() {
        let genre_ref = test_image_ref("hidden-genre");
        let playlist_ref = test_image_ref("hidden-playlist");
        let mut library = test_library_snapshot();
        library.server = Some(test_server("source"));
        library.genres = vec![Genre {
            id: GenreId::fake(1),
            name: "Genre".to_string(),
            album_count: 1,
            track_count: 1,
            duration_seconds: 180,
            image_refs: vec![genre_ref.clone()],
            image_ref: None,
        }];
        library.playlists = vec![test_playlist("Regular", playlist_ref.clone())];
        let mut settings = AppSettings::default();
        for entry in &mut settings.sidebar.route_items {
            if matches!(
                entry.item,
                SidebarRouteItem::Genres | SidebarRouteItem::Playlists
            ) {
                entry.visible = false;
            }
        }

        let targets = source_warm_targets(&library, &[], &settings, test_initial_route_metrics());
        let target_refs = targets
            .iter()
            .map(|target| target.image_ref.item_id.as_str())
            .collect::<Vec<_>>();

        assert!(!target_refs.contains(&genre_ref.item_id.as_str()));
        assert!(!target_refs.contains(&playlist_ref.item_id.as_str()));
    }
}
