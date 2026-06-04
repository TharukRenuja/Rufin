use super::*;

pub(in crate::ui) struct StartupCoverTarget {
    pub(in crate::ui) image_ref: ImageRef,
    pub(in crate::ui) fetch_size: u32,
    pub(in crate::ui) size: i32,
}
pub(in crate::ui) fn startup_cover_prime_jobs(shell: &Shell) -> Vec<CoverWarmJob> {
    startup_cover_jobs_from_targets(
        shell,
        startup_cover_prime_targets(shell),
        Some(STARTUP_CACHED_COVER_PRIME_LIMIT),
    )
}
pub(in crate::ui) fn startup_cover_jobs_from_targets(
    shell: &Shell,
    targets: Vec<StartupCoverTarget>,
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
#[cfg(test)]
pub(in crate::ui) fn sidebar_route_visible(settings: &AppSettings, item: SidebarRouteItem) -> bool {
    settings
        .sidebar
        .route_items
        .iter()
        .any(|entry| entry.item == item && entry.visible)
}
pub(in crate::ui) fn startup_cover_prime_targets(shell: &Shell) -> Vec<StartupCoverTarget> {
    startup_cover_prime_targets_from_snapshot(
        &shell.state.library.borrow(),
        &shell.state.settings.borrow(),
        shell.state.home_showcase_seed.get(),
    )
}
pub(in crate::ui) fn startup_cover_prime_targets_from_snapshot(
    library: &LibrarySnapshot,
    settings: &AppSettings,
    home_showcase_seed: u64,
) -> Vec<StartupCoverTarget> {
    startup_home_cover_prime_targets_from_snapshot(library, settings, home_showcase_seed)
}
#[cfg(test)]
pub(in crate::ui) fn library_route_cover_prime_targets_from_snapshot(
    library: &LibrarySnapshot,
    settings: &AppSettings,
) -> Vec<StartupCoverTarget> {
    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    push_startup_route_prime_targets(&mut targets, &mut seen, library, settings);
    targets
}
pub(in crate::ui) fn startup_home_cover_prime_targets(shell: &Shell) -> Vec<StartupCoverTarget> {
    startup_home_cover_prime_targets_from_snapshot(
        &shell.state.library.borrow(),
        &shell.state.settings.borrow(),
        shell.state.home_showcase_seed.get(),
    )
}
pub(in crate::ui) fn startup_home_cover_prime_targets_from_snapshot(
    library: &LibrarySnapshot,
    settings: &AppSettings,
    home_showcase_seed: u64,
) -> Vec<StartupCoverTarget> {
    let mut targets = Vec::new();
    push_startup_home_prime_targets(&mut targets, library, settings, home_showcase_seed);
    targets
}
fn push_startup_home_prime_targets(
    targets: &mut Vec<StartupCoverTarget>,
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
    targets: &mut Vec<StartupCoverTarget>,
    image_ref: Option<&ImageRef>,
    fetch_size: u32,
    size: i32,
) {
    let Some(image_ref) = image_ref else {
        return;
    };
    targets.push(StartupCoverTarget {
        image_ref: image_ref.clone(),
        fetch_size,
        size,
    });
}
#[cfg(test)]
fn push_startup_route_prime_targets(
    targets: &mut Vec<StartupCoverTarget>,
    seen: &mut HashSet<String>,
    library: &LibrarySnapshot,
    settings: &AppSettings,
) {
    if sidebar_route_visible(settings, SidebarRouteItem::Tracks) {
        let list_settings = settings.library_list(LibraryListKey::Tracks);
        push_track_startup_prime_targets(
            targets,
            seen,
            library.tracks.clone(),
            &list_settings,
            false,
        );
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Albums) {
        let list_settings = settings.library_list(LibraryListKey::Albums);
        push_album_startup_prime_targets(targets, seen, library.albums.clone(), &list_settings);
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Artists) {
        let list_settings = settings.library_list(LibraryListKey::Artists);
        push_artist_startup_prime_targets(targets, seen, library.artists.clone(), &list_settings);
    }
    if sidebar_route_visible(settings, SidebarRouteItem::AlbumArtists) {
        let list_settings = settings.library_list(LibraryListKey::AlbumArtists);
        push_artist_startup_prime_targets(
            targets,
            seen,
            library.album_artists.clone(),
            &list_settings,
        );
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Genres) {
        let list_settings = settings.library_list(LibraryListKey::Genres);
        push_genre_startup_prime_targets(targets, seen, library, &list_settings);
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Favorites) {
        let list_settings = settings.library_list(LibraryListKey::FavoriteTracks);
        push_track_startup_prime_targets(
            targets,
            seen,
            library.favorites.clone(),
            &list_settings,
            true,
        );
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Playlists) {
        let list_settings = settings.library_list(LibraryListKey::Playlists);
        push_playlist_startup_prime_targets(
            targets,
            seen,
            library.playlists.clone(),
            &list_settings,
        );
    }
}
#[cfg(test)]
fn push_track_startup_prime_targets(
    targets: &mut Vec<StartupCoverTarget>,
    seen: &mut HashSet<String>,
    mut tracks: Vec<Track>,
    settings: &LibraryListSettings,
    favorite_first: bool,
) {
    let Some((fetch_size, size)) = startup_route_cover_size(settings) else {
        return;
    };
    library::sort_tracks(&mut tracks, settings, favorite_first);
    for track in &tracks {
        push_unique_startup_cover_target(targets, seen, track.image_ref.as_ref(), fetch_size, size);
    }
}
#[cfg(test)]
fn push_album_startup_prime_targets(
    targets: &mut Vec<StartupCoverTarget>,
    seen: &mut HashSet<String>,
    mut albums: Vec<Album>,
    settings: &LibraryListSettings,
) {
    let Some((fetch_size, size)) = startup_route_cover_size(settings) else {
        return;
    };
    library::sort_albums(&mut albums, settings);
    for album in &albums {
        push_unique_startup_cover_target(targets, seen, album.image_ref.as_ref(), fetch_size, size);
    }
}
#[cfg(test)]
fn push_artist_startup_prime_targets(
    targets: &mut Vec<StartupCoverTarget>,
    seen: &mut HashSet<String>,
    mut artists: Vec<Artist>,
    settings: &LibraryListSettings,
) {
    let Some((fetch_size, size)) = startup_route_cover_size(settings) else {
        return;
    };
    library::sort_artists(&mut artists, settings);
    for artist in &artists {
        push_unique_startup_cover_target(
            targets,
            seen,
            artist.image_ref.as_ref(),
            fetch_size,
            size,
        );
    }
}
#[cfg(test)]
fn push_genre_startup_prime_targets(
    targets: &mut Vec<StartupCoverTarget>,
    seen: &mut HashSet<String>,
    library: &LibrarySnapshot,
    settings: &LibraryListSettings,
) {
    let Some((fetch_size, size)) = startup_route_cover_size(settings) else {
        return;
    };
    let mut genres = library.genres.clone();
    library::sort_genres(&mut genres, settings);
    for genre in &genres {
        for image_ref in &genre.image_refs {
            push_unique_startup_cover_target(targets, seen, Some(image_ref), fetch_size, size);
        }
        push_unique_startup_cover_target(targets, seen, genre.image_ref.as_ref(), fetch_size, size);
    }
}
#[cfg(test)]
fn push_playlist_startup_prime_targets(
    targets: &mut Vec<StartupCoverTarget>,
    seen: &mut HashSet<String>,
    mut playlists: Vec<Playlist>,
    settings: &LibraryListSettings,
) {
    let Some((fetch_size, size)) = startup_route_cover_size(settings) else {
        return;
    };
    library::sort_playlists(&mut playlists, settings);
    for playlist in &playlists {
        for image_ref in &playlist.image_refs {
            push_unique_startup_cover_target(targets, seen, Some(image_ref), fetch_size, size);
        }
        push_unique_startup_cover_target(
            targets,
            seen,
            playlist.image_ref.as_ref(),
            fetch_size,
            size,
        );
    }
}
#[cfg(test)]
fn startup_route_cover_size(settings: &LibraryListSettings) -> Option<(u32, i32)> {
    match settings.layout {
        LibraryLayout::Grid | LibraryLayout::Detail => {
            Some((GRID_COVER_SIZE, GRID_COVER_SIZE as i32))
        }
        LibraryLayout::Row if row_layout_uses_cover(settings) => Some((THUMB_COVER_SIZE, 48)),
        LibraryLayout::Row => None,
    }
}
#[cfg(test)]
fn push_unique_startup_cover_target(
    targets: &mut Vec<StartupCoverTarget>,
    seen: &mut HashSet<String>,
    image_ref: Option<&ImageRef>,
    fetch_size: u32,
    size: i32,
) {
    if targets.len() >= STARTUP_CACHED_COVER_PRIME_LIMIT {
        return;
    }
    let Some(image_ref) = image_ref else {
        return;
    };
    let seen_key = startup_cover_target_dedupe_key(image_ref, fetch_size);
    if !seen.insert(seen_key) {
        return;
    }
    push_startup_cover_target(targets, Some(image_ref), fetch_size, size);
}
#[cfg(test)]
fn startup_cover_target_dedupe_key(image_ref: &ImageRef, fetch_size: u32) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        image_ref.item_id,
        image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED),
        fetch_size
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
pub(in crate::ui) fn playback_notification_icon_bytes_from_path(path: &Path) -> Option<Vec<u8>> {
    let pixbuf = Pixbuf::from_file(path).ok()?;
    playback_notification_icon_bytes_from_pixbuf(&pixbuf)
}
pub(in crate::ui) fn playback_notification_icon_bytes_from_pixbuf(
    pixbuf: &Pixbuf,
) -> Option<Vec<u8>> {
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
        for album in section
            .albums
            .iter()
            .take(FIRST_RUN_HOME_SECTION_COVER_LIMIT)
        {
            push_first_run_cover_ref(&mut refs, &mut seen, album.image_ref.as_ref());
        }
        for track in section
            .tracks
            .iter()
            .take(FIRST_RUN_HOME_SECTION_COVER_LIMIT)
        {
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
    if refs.len() >= FIRST_RUN_GRID_COVER_PRIME_LIMIT {
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
