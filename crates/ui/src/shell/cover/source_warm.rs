use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use artwork::{ArtworkBinding, ArtworkRequest, ExternalPolicy, PrefetchOwner, PrefetchPriority};
use gtk::{gio, glib};
use library::{ActiveLibraryQuery, HomeBlockKind, MusicFolderId, SourceId};
use tracing::{debug, warn};

use crate::routes::home::showcase_album;
use crate::{
    LibraryField, LibraryLayout, LibraryListKey, LibraryListSettings, Settings, SidebarRouteItem,
};

use super::{GRID_COVER_SIZE, Shell, THUMB_COVER_SIZE, artwork_external_policy};

const TARGET_LIMIT: usize = 4_096;
const FRONT_LIMIT: usize = 48;
const ROUTE_KEYS: [(SidebarRouteItem, LibraryListKey); 9] = [
    (SidebarRouteItem::Tracks, LibraryListKey::Tracks),
    (SidebarRouteItem::Albums, LibraryListKey::Albums),
    (SidebarRouteItem::Artists, LibraryListKey::Artists),
    (SidebarRouteItem::AlbumArtists, LibraryListKey::AlbumArtists),
    (SidebarRouteItem::Genres, LibraryListKey::Genres),
    (SidebarRouteItem::Favorites, LibraryListKey::FavoriteTracks),
    (SidebarRouteItem::Playlists, LibraryListKey::Playlists),
    (
        SidebarRouteItem::SmartPlaylists,
        LibraryListKey::SmartPlaylists,
    ),
    (SidebarRouteItem::Moods, LibraryListKey::Moods),
];

#[derive(Clone, Eq, PartialEq)]
struct WarmKey {
    source_id: SourceId,
    music_folder_id: Option<MusicFolderId>,
    showcase_seed: u64,
    home_blocks: Vec<HomeBlockKind>,
    routes: Vec<(LibraryListKey, LibraryListSettings)>,
    prefer_server_playlist_covers: bool,
    external: ExternalPolicy,
}

impl WarmKey {
    fn new(
        source_id: SourceId,
        music_folder_id: Option<MusicFolderId>,
        showcase_seed: u64,
        settings: &Settings,
    ) -> Self {
        let routes = ROUTE_KEYS
            .into_iter()
            .filter(|(route, _)| {
                settings
                    .sidebar
                    .route_items
                    .iter()
                    .any(|entry| entry.item == *route && entry.visible)
            })
            .map(|(_, key)| (key, settings.library_list(key)))
            .collect();
        Self {
            source_id,
            music_folder_id,
            showcase_seed,
            home_blocks: settings.home_blocks.clone(),
            routes,
            prefer_server_playlist_covers: settings.prefer_server_playlist_covers,
            external: source_warm_external_policy(settings),
        }
    }

    fn list(&self, key: LibraryListKey) -> Option<&LibraryListSettings> {
        self.routes
            .iter()
            .find_map(|(candidate, settings)| (*candidate == key).then_some(settings))
    }
}

fn source_warm_external_policy(settings: &Settings) -> ExternalPolicy {
    let mut external = artwork_external_policy(settings);
    external.allow_network = false;
    external
}

pub(in crate::shell) struct SourceWarmState {
    owner: PrefetchOwner,
    generation: Cell<u64>,
    key: RefCell<Option<WarmKey>>,
    task: RefCell<Option<glib::JoinHandle<()>>>,
}

impl SourceWarmState {
    pub(in crate::shell) fn new(owner: PrefetchOwner) -> Self {
        Self {
            owner,
            generation: Cell::new(0),
            key: RefCell::new(None),
            task: RefCell::new(None),
        }
    }
}

impl Shell {
    pub(crate) fn schedule_source_artwork_warm(&self) {
        if !self.startup.route_revealed.get() || !self.has_active_mounted_route() {
            return;
        }
        let (source_id, music_folder_id) = {
            let source = self.source.presentation.borrow();
            let Some(active) = source
                .source
                .as_ref()
                .filter(|_| source.cache.is_committed())
            else {
                return;
            };
            (active.id.clone(), source.selected_music_folder_id.clone())
        };
        let Some(query) = self
            .library
            .query
            .borrow()
            .as_ref()
            .filter(|query| query.source_id() == &source_id)
            .cloned()
        else {
            return;
        };
        let key = WarmKey::new(
            source_id,
            music_folder_id,
            self.library.home_showcase_seed.get(),
            &self.settings.current.borrow(),
        );
        if self.artwork.source_warm.key.borrow().as_ref() == Some(&key) {
            return;
        }

        self.cancel_source_artwork_warm();
        self.artwork.source_warm.key.replace(Some(key.clone()));
        let state = Rc::clone(&self.artwork.source_warm);
        let artwork = self.products.artwork.clone();
        let generation = state.generation.get();
        let owner = state.owner;
        let task = glib::spawn_future_local(async move {
            // Let the coherent route reveal win the first frame before the source-wide read begins.
            glib::timeout_future(Duration::from_millis(80)).await;
            if state.generation.get() != generation || state.key.borrow().as_ref() != Some(&key) {
                return;
            }
            let started = Instant::now();
            let result = gio::spawn_blocking(move || source_warm_requests(&query, &key)).await;
            if state.generation.get() != generation {
                return;
            }
            let build_ms = started.elapsed().as_millis() as u64;
            let requests = match result {
                Ok(Ok(requests)) => requests,
                Ok(Err(error)) => {
                    state.key.borrow_mut().take();
                    warn!(%error, build_ms, "failed to build source artwork warm plan");
                    return;
                }
                Err(_) => {
                    state.key.borrow_mut().take();
                    warn!(build_ms, "source artwork warm plan task panicked");
                    return;
                }
            };
            let target_count = requests.len();
            let submitted = Instant::now();
            if let Err(error) = artwork.replace_prefetch(owner, PrefetchPriority::Idle, requests) {
                state.key.borrow_mut().take();
                warn!(%error, target_count, build_ms, "failed to submit source artwork warm plan");
                return;
            }
            debug!(
                target_count,
                build_ms,
                submit_ms = submitted.elapsed().as_millis() as u64,
                "submitted source artwork warm plan"
            );
        });
        self.artwork.source_warm.task.replace(Some(task));
    }

    pub(crate) fn cancel_source_artwork_warm(&self) {
        let state = &self.artwork.source_warm;
        state.generation.set(state.generation.get().wrapping_add(1));
        state.key.borrow_mut().take();
        if let Some(task) = state.task.borrow_mut().take() {
            task.abort();
        }
        self.products.artwork.clear_prefetch(state.owner);
    }
}

fn source_warm_requests(
    query: &ActiveLibraryQuery,
    key: &WarmKey,
) -> Result<Vec<ArtworkRequest>, String> {
    // UI owns target order; artwork owns coalescing, admission, and execution.
    let mut plan = Plan::new(key.external.clone());
    push_home(&mut plan, query, key)?;

    let mut track_total = 0;
    for family in Family::FRONTS {
        let total = push_family(&mut plan, query, key, family, 0, FRONT_LIMIT)?;
        if family == Family::Track {
            track_total = total;
        }
        if plan.full() {
            break;
        }
    }
    push_track_samples(&mut plan, query, key, track_total)?;
    for family in Family::BACKGROUND {
        let take = plan.remaining();
        // Resume after the bounded front so duplicates cannot consume the raw plan budget.
        push_family(&mut plan, query, key, family, FRONT_LIMIT, take)?;
        if plan.full() {
            break;
        }
    }
    Ok(plan.finish())
}

fn push_home(plan: &mut Plan, query: &ActiveLibraryQuery, key: &WarmKey) -> Result<(), String> {
    let sections = query.home_sections()?;
    let albums = query.albums_page(0, 64)?.items;
    let mut section_count = 0;
    for block in &key.home_blocks {
        if plan.full() {
            break;
        }
        match block {
            HomeBlockKind::Showcase => {
                if let Some(album) = showcase_album(&sections, &albums, key.showcase_seed) {
                    plan.push(ArtworkBinding::album(&album), Shape::Grid);
                }
            }
            HomeBlockKind::Genres => {}
            block if section_count < 3 => {
                let Some(kind) = block.section_kind() else {
                    continue;
                };
                let Some(section) = sections.iter().find(|section| section.kind == kind) else {
                    continue;
                };
                section_count += 1;
                add_single(plan, section.albums.iter().take(4), Shape::Grid, |album| {
                    ArtworkBinding::album(album)
                });
                add_single(plan, section.tracks.iter().take(4), Shape::Grid, |track| {
                    ArtworkBinding::track(track)
                });
            }
            _ => {}
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Family {
    Track,
    Album,
    Artist,
    AlbumArtist,
    Genre,
    Favorite,
    Playlist,
    SmartPlaylist,
    Mood,
}

impl Family {
    const FRONTS: [Self; 9] = [
        Self::Track,
        Self::Album,
        Self::Artist,
        Self::AlbumArtist,
        Self::Genre,
        Self::Favorite,
        Self::Playlist,
        Self::SmartPlaylist,
        Self::Mood,
    ];
    const BACKGROUND: [Self; 9] = [
        Self::Album,
        Self::Artist,
        Self::AlbumArtist,
        Self::Genre,
        Self::Playlist,
        Self::SmartPlaylist,
        Self::Track,
        Self::Favorite,
        Self::Mood,
    ];

    fn key(self) -> LibraryListKey {
        match self {
            Self::Track => LibraryListKey::Tracks,
            Self::Album => LibraryListKey::Albums,
            Self::Artist => LibraryListKey::Artists,
            Self::AlbumArtist => LibraryListKey::AlbumArtists,
            Self::Genre => LibraryListKey::Genres,
            Self::Favorite => LibraryListKey::FavoriteTracks,
            Self::Playlist => LibraryListKey::Playlists,
            Self::SmartPlaylist => LibraryListKey::SmartPlaylists,
            Self::Mood => LibraryListKey::Moods,
        }
    }
}

fn push_family(
    plan: &mut Plan,
    query: &ActiveLibraryQuery,
    policy: &WarmKey,
    family: Family,
    offset: usize,
    take: usize,
) -> Result<usize, String> {
    if plan.full() {
        return Ok(0);
    }
    let key = family.key();
    let Some(settings) = policy.list(key) else {
        return Ok(0);
    };
    let Some(shape) = target_shape(key, settings) else {
        return Ok(0);
    };
    let total = match family {
        Family::Track => {
            let page = query.tracks_page(
                settings.sort_key.track_sort(),
                settings.descending,
                offset,
                take,
            )?;
            let total = page.total;
            add_single(plan, page.items, shape, ArtworkBinding::track);
            total
        }
        Family::Album => {
            add_single(
                plan,
                query.albums_page(offset, take)?.items,
                shape,
                ArtworkBinding::album,
            );
            0
        }
        Family::Artist | Family::AlbumArtist => {
            add_single(
                plan,
                query
                    .artists_page(family == Family::AlbumArtist, offset, take)?
                    .items,
                shape,
                ArtworkBinding::artist,
            );
            0
        }
        Family::Genre => {
            for genre in query.genres_page(offset, take)?.items {
                plan.push_collection(
                    ArtworkBinding::genre(&genre),
                    ArtworkBinding::genre_slots(&genre),
                    shape,
                );
                if plan.full() {
                    break;
                }
            }
            0
        }
        Family::Favorite => {
            let items = query.favorite_tracks()?.into_iter().skip(offset).take(take);
            add_single(plan, items, shape, ArtworkBinding::track);
            0
        }
        Family::Playlist => {
            let prefer_server = policy.prefer_server_playlist_covers;
            for playlist in query.playlists_page(offset, take)?.items {
                plan.push_collection(
                    ArtworkBinding::playlist(&playlist, prefer_server),
                    ArtworkBinding::playlist_slots(&playlist, prefer_server),
                    shape,
                );
                if plan.full() {
                    break;
                }
            }
            0
        }
        Family::SmartPlaylist => {
            for playlist in query.smart_playlists_page(offset, take)?.items {
                plan.push_collection(
                    ArtworkBinding::smart_playlist(&playlist),
                    ArtworkBinding::smart_playlist_slots(&playlist),
                    shape,
                );
                if plan.full() {
                    break;
                }
            }
            0
        }
        Family::Mood => {
            for mood in query.moods_page(offset, take)?.items {
                plan.push_collection(
                    ArtworkBinding::mood(&mood),
                    ArtworkBinding::mood_slots(&mood),
                    shape,
                );
                if plan.full() {
                    break;
                }
            }
            0
        }
    };
    Ok(total)
}

fn push_track_samples(
    plan: &mut Plan,
    query: &ActiveLibraryQuery,
    policy: &WarmKey,
    total: usize,
) -> Result<(), String> {
    let Some(settings) = policy.list(LibraryListKey::Tracks) else {
        return Ok(());
    };
    let Some(shape) = target_shape(LibraryListKey::Tracks, settings) else {
        return Ok(());
    };
    let count = FRONT_LIMIT.min(total);
    for numerator in [1_usize, 2, 3, 4] {
        if count == 0 || plan.full() {
            break;
        }
        let start = total.saturating_sub(count).saturating_mul(numerator) / 4;
        let items = query
            .tracks_page(
                settings.sort_key.track_sort(),
                settings.descending,
                start,
                count,
            )?
            .items;
        add_single(plan, items, shape, ArtworkBinding::track);
    }
    Ok(())
}

struct Plan {
    external: ExternalPolicy,
    targets: Vec<ArtworkRequest>,
}

impl Plan {
    fn new(external: ExternalPolicy) -> Self {
        Self {
            external,
            targets: Vec::new(),
        }
    }

    fn full(&self) -> bool {
        self.targets.len() == TARGET_LIMIT
    }

    fn remaining(&self) -> usize {
        TARGET_LIMIT.saturating_sub(self.targets.len())
    }

    fn push(&mut self, candidates: ArtworkBinding, shape: Shape) {
        if candidates.is_empty() || self.full() {
            return;
        }
        let (fetch, render) = shape.sizes();
        self.targets.push(
            ArtworkRequest::new(candidates, fetch, render).with_external(self.external.clone()),
        );
    }

    fn push_collection(
        &mut self,
        single: ArtworkBinding,
        slots: Vec<ArtworkBinding>,
        shape: Shape,
    ) {
        if shape == Shape::Group {
            for candidates in slots {
                self.push(candidates, shape);
                if self.full() {
                    break;
                }
            }
        } else {
            self.push(single, shape);
        }
    }

    fn finish(self) -> Vec<ArtworkRequest> {
        self.targets
    }
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum Shape {
    Row,
    Group,
    Grid,
}

impl Shape {
    fn sizes(self) -> (u32, u32) {
        match self {
            Self::Row => (THUMB_COVER_SIZE, 48),
            Self::Group => (THUMB_COVER_SIZE, THUMB_COVER_SIZE),
            Self::Grid => (GRID_COVER_SIZE, GRID_COVER_SIZE),
        }
    }
}

fn add_single<T>(
    plan: &mut Plan,
    items: impl IntoIterator<Item = T>,
    shape: Shape,
    candidates: impl Fn(&T) -> ArtworkBinding,
) {
    for item in items {
        plan.push(candidates(&item), shape);
        if plan.full() {
            break;
        }
    }
}

fn target_shape(key: LibraryListKey, settings: &LibraryListSettings) -> Option<Shape> {
    match settings.layout {
        LibraryLayout::Row
            if settings
                .row_fields
                .iter()
                .any(|field| matches!(field, LibraryField::Image | LibraryField::TitleMerged)) =>
        {
            Some(Shape::Row)
        }
        LibraryLayout::Row => None,
        LibraryLayout::Grid | LibraryLayout::Detail
            if matches!(
                key,
                LibraryListKey::Genres
                    | LibraryListKey::Moods
                    | LibraryListKey::Playlists
                    | LibraryListKey::SmartPlaylists
            ) =>
        {
            Some(Shape::Group)
        }
        LibraryLayout::Grid | LibraryLayout::Detail => Some(Shape::Grid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_warm_keeps_external_fallback_cache_only() {
        let mut settings = Settings::default();
        settings.metadata.external_metadata_enabled = true;
        settings.lastfm_api_key = "external-key".into();

        let key = WarmKey::new(SourceId::new("source:test"), None, 0, &settings);

        assert!(key.external.allow_cached);
        assert!(!key.external.allow_network);
        assert_eq!(key.external.lastfm_api_key, "external-key");
    }
}
