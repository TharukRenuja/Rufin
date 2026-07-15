use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::{Rc, Weak},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    time::Instant,
};

use ::library::play_context::PlayContextDescriptor;
use ::library::{ActiveLibraryQuery, Album, Artist, Track};
use adw::prelude::*;
use gtk::{gio, glib};
use tracing::{info, warn};

use crate::localization::bind_search_placeholder;
use crate::shell::Shell;
use crate::shell::route::{MountedRoute, MountedRouteDeltaApplier};
use crate::{LibraryLayout, LibraryListKey};
use localization::msgid;

use super::route_layout::PRIMARY_ROUTE_HORIZONTAL_INSET;

use super::album_detail::{AlbumCollectionModels, populate_album_collection_model};
use super::collection_routes::{
    CollectionRouteSpec, MountedRefreshLoader, MountedRouteRefresh, load_complete_cached_items,
};
use super::collections::{
    TrackModelIndex, TrackTableSelectionHandle, album_collection_projection,
    artist_collection_projection, library_route_inset, playlist_collection_projection,
    smart_playlist_collection_projection, track_collection_projection,
};
use super::library_fields::{
    album_matches_query, artist_matches_query, playlist_matches_query, replace_tracks_in_model,
    smart_playlist_matches_query,
};
use super::models::{
    populate_artist_model, populate_playlist_model, populate_smart_playlist_model,
    tracks_for_settings,
};
use super::play_context::{selected_music_folder_id, track_collection_play_context};
use super::route::Route;
use super::route_shell::{LibraryPageShell, LibraryPageShellOptions, LibraryToolbarProjection};

const SLOW_LIBRARY_ROUTE_SETUP_MS: u64 = 100;
const EMBEDDED_SCROLL_LATCH_MS: u128 = 280;
const EMBEDDED_SURFACE_SCROLL_FACTOR: f64 = 2.5;

fn changed_track_ids(delta: &::library::TrackDelta) -> Vec<::library::TrackId> {
    let mut seen = HashSet::new();
    delta
        .added
        .iter()
        .chain(&delta.deleted)
        .chain(&delta.fields)
        .chain(&delta.metadata)
        .chain(&delta.stats)
        .chain(&delta.skip_stats)
        .chain(&delta.favorite)
        .chain(&delta.cover_refs)
        .filter(|track_id| seen.insert((*track_id).clone()))
        .cloned()
        .collect()
}

fn retain_confirmed_track_deletions(
    delta: &mut ::library::TrackDelta,
    present_track_ids: &HashSet<::library::TrackId>,
) {
    delta
        .deleted
        .retain(|track_id| !present_track_ids.contains(track_id));
}

fn track_delta_can_preserve_model_order(
    delta: &::library::TrackDelta,
    sort_key: crate::LibraryField,
) -> bool {
    if !delta.added.is_empty()
        || !delta.deleted.is_empty()
        || !delta.fields.is_empty()
        || !delta.metadata.is_empty()
    {
        return false;
    }
    if !delta.stats.is_empty()
        && matches!(
            sort_key,
            crate::LibraryField::LastPlayed
                | crate::LibraryField::PlayCount
                | crate::LibraryField::UserRating
        )
    {
        return false;
    }
    delta.favorite.is_empty() || sort_key != crate::LibraryField::Favorite
}

fn release_replaced_tracks(tracks: Arc<Vec<Track>>) {
    let Some(tracks) = Arc::into_inner(tracks) else {
        return;
    };
    if tracks.is_empty() {
        return;
    }
    glib::spawn_future_local(async move {
        let _ = gio::spawn_blocking(move || drop(tracks)).await;
    });
}

fn invalidate_album_track_projection<T: Default>(projection: &RefCell<T>, loaded: &Cell<bool>) {
    projection.replace(T::default());
    loaded.set(false);
}

pub(crate) struct SearchableTrackOptions {
    pub(crate) on_visible_count_changed: Option<Rc<dyn Fn(usize)>>,
    pub(crate) source_descriptor: Option<PlayContextDescriptor>,
    pub(crate) favorites_only: bool,
    pub(crate) content_inset: i32,
    pub(crate) selection_handle: Option<TrackTableSelectionHandle>,
    pub(crate) fixed_layout: Option<LibraryLayout>,
}

#[derive(Clone)]
pub(crate) struct TrackListProjection {
    key: LibraryListKey,
    search: gtk::SearchEntry,
    collection: super::collections::LibraryCollectionProjection,
    track_index: TrackModelIndex,
    source_tracks: Rc<RefCell<Arc<Vec<Track>>>>,
    replace_tracks: Rc<dyn Fn(Arc<Vec<Track>>)>,
    refresh_tracks: Rc<dyn Fn()>,
    source_descriptor: Option<Rc<RefCell<PlayContextDescriptor>>>,
    settings: Rc<RefCell<crate::LibraryListSettings>>,
    fixed_layout: Option<LibraryLayout>,
}

struct MountedTracksDeltaQueue {
    shell: Weak<Shell>,
    library_query: ActiveLibraryQuery,
    projection: TrackListProjection,
    page_shell: LibraryPageShell,
    source_id: ::library::SourceId,
    music_folder_id: RefCell<Option<::library::MusicFolderId>>,
    pending: RefCell<::library::LibraryDelta>,
    running: Cell<bool>,
    epoch: Cell<u64>,
}

impl MountedTracksDeltaQueue {
    fn new(
        shell: Weak<Shell>,
        library_query: ActiveLibraryQuery,
        projection: TrackListProjection,
        page_shell: LibraryPageShell,
        music_folder_id: Option<::library::MusicFolderId>,
    ) -> Self {
        Self {
            shell,
            source_id: library_query.source_id().clone(),
            library_query,
            projection,
            page_shell,
            music_folder_id: RefCell::new(music_folder_id),
            pending: RefCell::new(::library::LibraryDelta::default()),
            running: Cell::new(false),
            epoch: Cell::new(0),
        }
    }

    fn enqueue(self: &Rc<Self>, delta: &::library::TrackDelta) {
        self.pending.borrow_mut().merge(::library::LibraryDelta {
            tracks: delta.clone(),
            ..::library::LibraryDelta::default()
        });
        self.start_next();
    }

    fn reset_for_current_scope(&self) -> bool {
        self.epoch.set(self.epoch.get().wrapping_add(1));
        self.pending.replace(::library::LibraryDelta::default());
        let Some(shell) = self.shell.upgrade() else {
            return false;
        };
        if shell.navigation.routes.borrow().current() != &Route::Tracks
            || shell
                .library
                .query
                .borrow()
                .as_ref()
                .is_none_or(|query| query.source_id() != &self.source_id)
        {
            return false;
        }
        let music_folder_id = selected_music_folder_id(&shell);
        self.music_folder_id.replace(music_folder_id.clone());
        self.projection
            .set_source_descriptor(PlayContextDescriptor::Global { music_folder_id });
        true
    }

    fn context_is_current(&self, shell: &Shell) -> bool {
        shell.navigation.routes.borrow().current() == &Route::Tracks
            && shell
                .library
                .query
                .borrow()
                .as_ref()
                .is_some_and(|query| query.source_id() == &self.source_id)
            && selected_music_folder_id(shell) == *self.music_folder_id.borrow()
    }

    fn start_next(self: &Rc<Self>) {
        if self.running.get() {
            return;
        }
        let delta = self.pending.take();
        if delta.tracks.is_empty() {
            return;
        }
        self.running.set(true);
        let epoch = self.epoch.get();
        let ids = changed_track_ids(&delta.tracks);
        if ids.is_empty() {
            self.finish(epoch, delta, Vec::new());
            return;
        }

        let query = self.library_query.clone();
        let state = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || query.tracks_by_ids(&ids)).await;
            let Some(state) = state.upgrade() else {
                return;
            };
            match result {
                Ok(Ok(changed)) => state.finish(epoch, delta, changed),
                Ok(Err(error)) => {
                    warn!(%error, "failed to refresh changed Tracks route rows");
                    state.recover_from_failed_read(epoch);
                }
                Err(_) => {
                    warn!("changed Tracks route row refresh task panicked");
                    state.recover_from_failed_read(epoch);
                }
            }
        });
    }

    fn finish(
        self: &Rc<Self>,
        epoch: u64,
        mut delta: ::library::LibraryDelta,
        changed: Vec<Track>,
    ) {
        self.running.set(false);
        if self.epoch.get() != epoch {
            self.start_next();
            return;
        }
        let Some(shell) = self.shell.upgrade() else {
            return;
        };
        if !self.context_is_current(&shell) {
            self.pending.replace(::library::LibraryDelta::default());
            self.epoch.set(self.epoch.get().wrapping_add(1));
            return;
        }

        let present_track_ids = changed
            .iter()
            .map(|track| track.id.clone())
            .collect::<HashSet<_>>();
        retain_confirmed_track_deletions(&mut delta.tracks, &present_track_ids);
        self.projection.patch(changed, &delta.tracks);
        self.page_shell.set_empty(self.projection.source_is_empty());
        shell.refresh_current_route_now_playing_selections();
        self.start_next();
    }

    fn recover_from_failed_read(self: &Rc<Self>, epoch: u64) {
        self.running.set(false);
        if self.epoch.get() != epoch {
            self.start_next();
            return;
        }
        let Some(shell) = self.shell.upgrade() else {
            return;
        };
        if !self.context_is_current(&shell) {
            self.pending.replace(::library::LibraryDelta::default());
            self.epoch.set(self.epoch.get().wrapping_add(1));
            return;
        }

        self.epoch.set(self.epoch.get().wrapping_add(1));
        self.pending.replace(::library::LibraryDelta::default());
        let projection = self.projection.clone();
        let page_shell = self.page_shell.clone();
        shell.refresh_mounted_tracks_from_prepared(
            self.library_query.clone(),
            Rc::new(move |tracks| {
                projection.replace_shared(tracks);
                page_shell.set_empty(projection.source_is_empty());
            }),
        );
    }
}

impl TrackListProjection {
    pub(crate) fn search(&self) -> gtk::SearchEntry {
        self.search.clone()
    }

    pub(crate) fn scrolling_widget(&self) -> gtk::Widget {
        self.collection.scrolling_widget()
    }

    pub(crate) fn mount_in_scroller(&self, scroller: &gtk::ScrolledWindow) -> gtk::Widget {
        self.collection.mount_in_scroller(scroller, 0, 0)
    }

    pub(crate) fn replace(&self, tracks: Vec<Track>) {
        self.replace_shared(Arc::new(tracks));
    }

    pub(crate) fn replace_shared(&self, tracks: Arc<Vec<Track>>) {
        (self.replace_tracks)(tracks);
    }

    pub(crate) fn patch(&self, changed: Vec<Track>, delta: &::library::TrackDelta) {
        let deleted = delta.deleted.iter().cloned().collect::<HashSet<_>>();
        let mut changed = changed
            .into_iter()
            .map(|track| (track.id.clone(), track))
            .collect::<HashMap<_, _>>();
        let model_changes = changed.clone();
        let added_unknown;
        {
            let mut shared_source = self.source_tracks.borrow_mut();
            let source = Arc::make_mut(&mut *shared_source);
            source.retain_mut(|track| {
                if deleted.contains(&track.id) {
                    return false;
                }
                if let Some(replacement) = changed.remove(&track.id) {
                    *track = replacement;
                }
                true
            });
            added_unknown = !changed.is_empty();
            source.extend(changed.into_values());
        }
        if !added_unknown
            && track_delta_can_preserve_model_order(delta, self.settings.borrow().sort_key)
        {
            self.track_index
                .replace_existing(model_changes.into_values());
        } else {
            (self.refresh_tracks)();
        }
    }

    pub(crate) fn source_is_empty(&self) -> bool {
        self.source_tracks.borrow().is_empty()
    }

    pub(crate) fn set_source_descriptor(&self, descriptor: PlayContextDescriptor) {
        if let Some(current) = self.source_descriptor.as_ref() {
            *current.borrow_mut() = descriptor;
        }
    }

    pub(crate) fn apply_library_list_settings(
        &self,
        key: LibraryListKey,
        settings: &crate::LibraryListSettings,
    ) {
        if key != self.key {
            return;
        }
        let mut settings = settings.clone();
        if let Some(layout) = self.fixed_layout {
            settings.layout = layout;
        }
        let previous = self.settings.borrow().clone();
        if previous.sort_key != settings.sort_key || previous.descending != settings.descending {
            (self.refresh_tracks)();
        }
        self.collection.apply_settings(&settings);
        *self.settings.borrow_mut() = settings;
    }
}

impl Shell {
    pub(crate) fn load_albums_route_data(
        library_query: &ActiveLibraryQuery,
        revision: i64,
        include_tracks: bool,
    ) -> Result<
        (
            Arc<Vec<Album>>,
            Option<HashMap<::library::AlbumId, Vec<Track>>>,
        ),
        String,
    > {
        let albums = library_query
            .prepared_albums_if_cached(revision)
            .filter(|prepared| prepared.items.len() == prepared.total)
            .map(|prepared| prepared.items)
            .map(Ok)
            .unwrap_or_else(|| {
                load_complete_cached_items(|limit| library_query.albums_page(0, limit))
                    .map(Arc::new)
            })?;
        let album_tracks = if include_tracks {
            let ids = albums
                .iter()
                .map(|album| album.id.clone())
                .collect::<Vec<_>>();
            Some(
                library_query
                    .prepared_album_tracks_if_cached(revision, &ids)
                    .unwrap_or_else(|| {
                        library_query.album_tracks(&ids).unwrap_or_else(|error| {
                            warn!(%error, "failed to load Albums detail track projection");
                            HashMap::new()
                        })
                    }),
            )
        } else {
            None
        };
        Ok((albums, album_tracks))
    }

    pub(crate) fn library_albums_route_from_prepared(
        self: &Rc<Self>,
        library_query: ActiveLibraryQuery,
        revision: i64,
        prepared: (
            Arc<Vec<Album>>,
            Option<HashMap<::library::AlbumId, Vec<Track>>>,
        ),
    ) -> MountedRoute {
        let (loaded, prepared_album_tracks) = prepared;
        let view_started = Instant::now();
        let settings = self
            .settings
            .current
            .borrow()
            .library_list(LibraryListKey::Albums);
        let applied_settings = Rc::new(RefCell::new(settings.clone()));
        let page_total = loaded.len();
        let source_albums = Rc::new(RefCell::new(Arc::clone(&loaded)));
        let albums = Rc::new(RefCell::new(loaded));
        let album_count = albums.borrow().len();
        let album_tracks_loaded = Rc::new(Cell::new(prepared_album_tracks.is_some()));
        let album_tracks = Rc::new(RefCell::new(prepared_album_tracks.unwrap_or_default()));
        let models = AlbumCollectionModels::new();
        let model_started = Instant::now();
        populate_album_collection_model(
            &models,
            &albums.borrow(),
            &settings,
            &album_tracks.borrow(),
        );
        let model_ms = model_started.elapsed().as_millis() as u64;

        let search = gtk::SearchEntry::new();
        bind_search_placeholder(&search, "Search");
        search.set_hexpand(true);
        let query = Rc::new(RefCell::new(String::new()));

        {
            let shell = Rc::clone(self);
            let models = models.clone();
            let source_albums = Rc::clone(&source_albums);
            let albums = Rc::clone(&albums);
            let album_tracks = Rc::clone(&album_tracks);
            let query = Rc::clone(&query);
            search.connect_search_changed(move |entry| {
                let text = entry.text().trim().to_string();
                *query.borrow_mut() = text.clone();
                let normalized = text.to_lowercase();
                let values = {
                    let source_albums = source_albums.borrow();
                    if normalized.is_empty() {
                        Arc::clone(&source_albums)
                    } else {
                        Arc::new(
                            source_albums
                                .iter()
                                .filter(|album| album_matches_query(album, &normalized))
                                .cloned()
                                .collect::<Vec<_>>(),
                        )
                    }
                };
                *albums.borrow_mut() = values;
                let settings = shell
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::Albums);
                populate_album_collection_model(
                    &models,
                    &albums.borrow(),
                    &settings,
                    &album_tracks.borrow(),
                );
                models.clear_inactive(settings.layout);
            });
        }

        let content_started = Instant::now();
        let content = album_collection_projection(
            self,
            models.clone(),
            LibraryListKey::Albums,
            library_query.clone(),
        );
        models.clear_inactive(settings.layout);
        let content_ms = content_started.elapsed().as_millis() as u64;
        let content_surface = content.scrolling_widget();
        let shell_started = Instant::now();
        let page_shell = self.library_page_shell(LibraryPageShellOptions {
            key: LibraryListKey::Albums,
            empty: albums.borrow().is_empty(),
            empty_body: msgid("Cached entries will appear here after sync finishes"),
            search,
            content: content_surface,
        });
        let shell_ms = shell_started.elapsed().as_millis() as u64;
        let total_ms = view_started.elapsed().as_millis() as u64;
        info!(
            route = ?Route::Albums,
            layout = ?settings.layout,
            source = "store",
            albums = album_count,
            total = page_total,
            model_ms,
            content_ms,
            shell_ms,
            total_ms,
            "library route setup timing"
        );
        if total_ms >= SLOW_LIBRARY_ROUTE_SETUP_MS {
            warn!(
                route = ?Route::Albums,
                layout = ?settings.layout,
                albums = album_count,
                total = page_total,
                total_ms,
                "slow library route setup"
            );
        }
        if settings.layout == LibraryLayout::Detail {
            info!(
                albums = album_count,
                total = page_total,
                model_ms,
                content_ms,
                shell_ms,
                total_ms,
                "albums detail view timing"
            );
        }
        let apply_loaded: Rc<
            dyn Fn(
                Result<
                    (
                        Arc<Vec<Album>>,
                        Option<HashMap<::library::AlbumId, Vec<Track>>>,
                    ),
                    String,
                >,
            ),
        > = {
            let shell = Rc::clone(self);
            let models = models.clone();
            let source_albums = Rc::clone(&source_albums);
            let albums = Rc::clone(&albums);
            let album_tracks = Rc::clone(&album_tracks);
            let album_tracks_loaded = Rc::clone(&album_tracks_loaded);
            let query = Rc::clone(&query);
            let content = content.clone();
            let page_shell = page_shell.clone();
            let applied_settings = Rc::clone(&applied_settings);
            Rc::new(move |result| {
                let (loaded, loaded_album_tracks) = match result {
                    Ok(loaded) => loaded,
                    Err(error) => {
                        warn!(%error, "failed to refresh Albums route projection");
                        return;
                    }
                };
                let settings = shell
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::Albums);
                *source_albums.borrow_mut() = Arc::clone(&loaded);
                if let Some(loaded_album_tracks) = loaded_album_tracks
                    && settings.layout == LibraryLayout::Detail
                {
                    album_tracks.replace(loaded_album_tracks);
                    album_tracks_loaded.set(true);
                } else {
                    invalidate_album_track_projection(&album_tracks, &album_tracks_loaded);
                }
                let normalized = query.borrow().trim().to_lowercase();
                let visible = if normalized.is_empty() {
                    loaded
                } else {
                    Arc::new(
                        loaded
                            .iter()
                            .filter(|album| album_matches_query(album, &normalized))
                            .cloned()
                            .collect::<Vec<_>>(),
                    )
                };
                *albums.borrow_mut() = visible;
                populate_album_collection_model(
                    &models,
                    &albums.borrow(),
                    &settings,
                    &album_tracks.borrow(),
                );
                page_shell.set_empty(albums.borrow().is_empty());
                content.apply_settings(&settings);
                models.clear_inactive(settings.layout);
                page_shell.apply_library_list_settings(LibraryListKey::Albums, &settings);
                *applied_settings.borrow_mut() = settings;
            })
        };
        let detail_requested = Arc::new(AtomicBool::new(settings.layout == LibraryLayout::Detail));
        let load_revision = Arc::new(AtomicI64::new(revision));
        let load_query = library_query.clone();
        let load_detail_requested = Arc::clone(&detail_requested);
        let loader_revision = Arc::clone(&load_revision);
        let load: MountedRefreshLoader<
            Result<
                (
                    Arc<Vec<Album>>,
                    Option<HashMap<::library::AlbumId, Vec<Track>>>,
                ),
                String,
            >,
        > = Arc::new(move || {
            Shell::load_albums_route_data(
                &load_query,
                loader_revision.load(Ordering::Acquire),
                load_detail_requested.load(Ordering::Acquire),
            )
        });
        let refresh =
            MountedRouteRefresh::new(Rc::downgrade(&apply_loaded), load, "mounted Albums");
        let affected_by = {
            let shell = Rc::clone(self);
            let album_tracks_loaded = Rc::clone(&album_tracks_loaded);
            Rc::new(move |delta: &library::LibraryDelta| {
                let settings = shell
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::Albums);
                delta.reset.is_some()
                    || !delta.albums.is_empty()
                    || (!delta.tracks.is_empty()
                        && (settings.layout == LibraryLayout::Detail || album_tracks_loaded.get()))
            })
        };
        let apply_delta = {
            let shell = Rc::clone(self);
            let apply_loaded = Rc::clone(&apply_loaded);
            let refresh = Rc::clone(&refresh);
            let album_tracks = Rc::clone(&album_tracks);
            let album_tracks_loaded = Rc::clone(&album_tracks_loaded);
            let detail_requested = Arc::clone(&detail_requested);
            let load_revision = Arc::clone(&load_revision);
            Rc::new(move |delta: &library::LibraryDelta| {
                load_revision.store(
                    shell.source.presentation.borrow().cache.revision(),
                    Ordering::Release,
                );
                if delta.reset.is_some() || !delta.albums.is_empty() {
                    let detail = shell
                        .settings
                        .current
                        .borrow()
                        .library_list(LibraryListKey::Albums)
                        .layout
                        == LibraryLayout::Detail;
                    detail_requested.store(detail, Ordering::Release);
                    let _ = &apply_loaded;
                    refresh.request();
                    return;
                }
                if delta.tracks.is_empty() {
                    return;
                }
                let settings = shell
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::Albums);
                if settings.layout == LibraryLayout::Detail {
                    detail_requested.store(true, Ordering::Release);
                    let _ = &apply_loaded;
                    refresh.request();
                } else {
                    invalidate_album_track_projection(&album_tracks, &album_tracks_loaded);
                }
            }) as MountedRouteDeltaApplier
        };
        let resume = {
            let shell = Rc::clone(self);
            let content = content.clone();
            let page_shell = page_shell.clone();
            let models = models.clone();
            let albums = Rc::clone(&albums);
            let album_tracks = Rc::clone(&album_tracks);
            let album_tracks_loaded = Rc::clone(&album_tracks_loaded);
            let applied_settings = Rc::clone(&applied_settings);
            let apply_loaded = Rc::clone(&apply_loaded);
            let detail_requested = Arc::clone(&detail_requested);
            let load_revision = Arc::clone(&load_revision);
            let refresh = Rc::clone(&refresh);
            Rc::new(move || {
                load_revision.store(
                    shell.source.presentation.borrow().cache.revision(),
                    Ordering::Release,
                );
                let settings = shell
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::Albums);
                let previous = applied_settings.borrow().clone();
                let requested_detail = settings.layout == LibraryLayout::Detail;
                detail_requested.store(requested_detail, Ordering::Release);
                if requested_detail && !album_tracks_loaded.get() {
                    let _ = &apply_loaded;
                    refresh.request();
                    page_shell.apply_library_list_settings(LibraryListKey::Albums, &settings);
                    return;
                }
                let entering_detail = previous.layout != LibraryLayout::Detail && requested_detail;
                let leaving_detail = previous.layout == LibraryLayout::Detail && !requested_detail;
                if leaving_detail {
                    invalidate_album_track_projection(&album_tracks, &album_tracks_loaded);
                }
                if previous.sort_key != settings.sort_key
                    || previous.descending != settings.descending
                    || entering_detail
                    || leaving_detail
                {
                    populate_album_collection_model(
                        &models,
                        &albums.borrow(),
                        &settings,
                        &album_tracks.borrow(),
                    );
                }
                content.apply_settings(&settings);
                models.clear_inactive(settings.layout);
                page_shell.apply_library_list_settings(LibraryListKey::Albums, &settings);
                *applied_settings.borrow_mut() = settings;
            })
        };
        MountedRoute::new(page_shell.widget(), affected_by, apply_delta, resume)
    }
    pub(crate) fn library_tracks_route_from_prepared(
        self: &Rc<Self>,
        library_query: ActiveLibraryQuery,
        tracks: Arc<Vec<Track>>,
    ) -> MountedRoute {
        let projection = self.searchable_track_collection_from_sorted_store(
            tracks,
            LibraryListKey::Tracks,
            SearchableTrackOptions {
                on_visible_count_changed: None,
                source_descriptor: Some(PlayContextDescriptor::Global {
                    music_folder_id: selected_music_folder_id(self),
                }),
                favorites_only: false,
                content_inset: PRIMARY_ROUTE_HORIZONTAL_INSET,
                selection_handle: None,
                fixed_layout: None,
            },
        );
        let page_shell = self.library_page_shell(LibraryPageShellOptions {
            key: LibraryListKey::Tracks,
            empty: projection.source_is_empty(),
            empty_body: msgid("Cached entries will appear here after sync finishes"),
            search: projection.search(),
            content: projection.scrolling_widget(),
        });
        let delta_queue = Rc::new(MountedTracksDeltaQueue::new(
            Rc::downgrade(self),
            library_query.clone(),
            projection.clone(),
            page_shell.clone(),
            selected_music_folder_id(self),
        ));
        let apply_prepared = {
            let projection = projection.clone();
            let page_shell = page_shell.clone();
            Rc::new(move |tracks: Arc<Vec<Track>>| {
                projection.replace_shared(tracks);
                page_shell.set_empty(projection.source_is_empty());
            }) as Rc<dyn Fn(Arc<Vec<Track>>)>
        };
        let affected_by = Rc::new(|delta: &library::LibraryDelta| {
            delta.reset.is_some() || !delta.tracks.is_empty()
        });
        let apply_delta = {
            let shell = Rc::clone(self);
            let apply_prepared = Rc::clone(&apply_prepared);
            let delta_queue = Rc::clone(&delta_queue);
            let library_query = library_query.clone();
            Rc::new(move |delta: &library::LibraryDelta| {
                if delta.reset.is_some() {
                    if delta_queue.reset_for_current_scope() {
                        shell.refresh_mounted_tracks_from_prepared(
                            library_query.clone(),
                            Rc::clone(&apply_prepared),
                        );
                    }
                    return;
                }
                delta_queue.enqueue(&delta.tracks);
            }) as MountedRouteDeltaApplier
        };
        let resume = {
            let shell = Rc::clone(self);
            let projection = projection.clone();
            let page_shell = page_shell.clone();
            Rc::new(move || {
                let settings = shell
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::Tracks);
                projection.apply_library_list_settings(LibraryListKey::Tracks, &settings);
                page_shell.apply_library_list_settings(LibraryListKey::Tracks, &settings);
            })
        };
        MountedRoute::new(page_shell.widget(), affected_by, apply_delta, resume)
    }
    pub(crate) fn library_artist_list_route_from_prepared(
        self: &Rc<Self>,
        album_artist: bool,
        loaded: Vec<Artist>,
        library_query: ActiveLibraryQuery,
    ) -> MountedRoute {
        let view_started = Instant::now();
        let key = if album_artist {
            LibraryListKey::AlbumArtists
        } else {
            LibraryListKey::Artists
        };
        let route = if album_artist {
            Route::AlbumArtists
        } else {
            Route::Artists
        };
        let settings = self.settings.current.borrow().library_list(key);
        let applied_settings = Rc::new(RefCell::new(settings.clone()));
        let page_total = loaded.len();
        let loaded = Arc::new(loaded);
        let source_artists = Rc::new(RefCell::new(Arc::clone(&loaded)));
        let artists = Rc::new(RefCell::new(loaded));
        let artist_count = artists.borrow().len();
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let model_started = Instant::now();
        populate_artist_model(&model, &artists.borrow(), &settings);
        let model_ms = model_started.elapsed().as_millis() as u64;

        let search = gtk::SearchEntry::new();
        bind_search_placeholder(&search, "Search");
        search.set_hexpand(true);
        let query = Rc::new(RefCell::new(String::new()));

        {
            let shell = Rc::clone(self);
            let model = model.clone();
            let source_artists = Rc::clone(&source_artists);
            let artists = Rc::clone(&artists);
            let query = Rc::clone(&query);
            search.connect_search_changed(move |entry| {
                let text = entry.text().trim().to_string();
                *query.borrow_mut() = text.clone();
                let normalized = text.to_lowercase();
                let values = {
                    let source_artists = source_artists.borrow();
                    if normalized.is_empty() {
                        Arc::clone(&source_artists)
                    } else {
                        Arc::new(
                            source_artists
                                .iter()
                                .filter(|artist| artist_matches_query(artist, &normalized))
                                .cloned()
                                .collect::<Vec<_>>(),
                        )
                    }
                };
                *artists.borrow_mut() = values;
                populate_artist_model(
                    &model,
                    &artists.borrow(),
                    &shell.settings.current.borrow().library_list(key),
                );
            });
        }
        let content_started = Instant::now();
        let content = artist_collection_projection(self, model.clone(), key, library_query.clone());
        let content_ms = content_started.elapsed().as_millis() as u64;
        let shell_started = Instant::now();
        let page_shell = self.library_page_shell(LibraryPageShellOptions {
            key,
            empty: artists.borrow().is_empty(),
            empty_body: msgid("Cached entries will appear here after sync finishes"),
            search,
            content: content.scrolling_widget(),
        });
        let shell_ms = shell_started.elapsed().as_millis() as u64;
        let total_ms = view_started.elapsed().as_millis() as u64;
        info!(
            route = ?route,
            layout = ?settings.layout,
            source = "store",
            artists = artist_count,
            total = page_total,
            model_ms,
            content_ms,
            shell_ms,
            total_ms,
            "library route setup timing"
        );
        if total_ms >= SLOW_LIBRARY_ROUTE_SETUP_MS {
            warn!(
                route = ?route,
                layout = ?settings.layout,
                artists = artist_count,
                total = page_total,
                total_ms,
                "slow library route setup"
            );
        }
        let apply_loaded: Rc<dyn Fn(Result<Vec<Artist>, String>)> = {
            let shell = Rc::clone(self);
            let model = model.clone();
            let source_artists = Rc::clone(&source_artists);
            let artists = Rc::clone(&artists);
            let query = Rc::clone(&query);
            let page_shell = page_shell.clone();
            Rc::new(move |result| {
                let loaded = match result {
                    Ok(loaded) => loaded,
                    Err(error) => {
                        warn!(%error, album_artist, "failed to refresh Artists route projection");
                        return;
                    }
                };
                let settings = shell.settings.current.borrow().library_list(key);
                let loaded = Arc::new(loaded);
                *source_artists.borrow_mut() = Arc::clone(&loaded);
                let normalized = query.borrow().trim().to_lowercase();
                let visible = if normalized.is_empty() {
                    loaded
                } else {
                    Arc::new(
                        loaded
                            .iter()
                            .filter(|artist| artist_matches_query(artist, &normalized))
                            .cloned()
                            .collect::<Vec<_>>(),
                    )
                };
                *artists.borrow_mut() = visible;
                populate_artist_model(&model, &artists.borrow(), &settings);
                page_shell.set_empty(artists.borrow().is_empty());
            })
        };
        let load_query = library_query.clone();
        let load: MountedRefreshLoader<Result<Vec<Artist>, String>> = Arc::new(move || {
            load_complete_cached_items(|limit| load_query.artists_page(album_artist, 0, limit))
        });
        let refresh = MountedRouteRefresh::new(
            Rc::downgrade(&apply_loaded),
            load,
            if album_artist {
                "mounted Album Artists"
            } else {
                "mounted Artists"
            },
        );
        let affected_by = {
            Rc::new(move |delta: &library::LibraryDelta| {
                let changed = if album_artist {
                    !delta.album_artists.is_empty()
                } else {
                    !delta.artists.is_empty()
                };
                delta.reset.is_some() || changed
            })
        };
        let apply_delta = {
            let apply_loaded = Rc::clone(&apply_loaded);
            let refresh = Rc::clone(&refresh);
            Rc::new(move |_: &library::LibraryDelta| {
                let _ = &apply_loaded;
                refresh.request();
            }) as MountedRouteDeltaApplier
        };
        let resume = {
            let shell = Rc::clone(self);
            let content = content.clone();
            let page_shell = page_shell.clone();
            let model = model.clone();
            let artists = Rc::clone(&artists);
            let applied_settings = Rc::clone(&applied_settings);
            Rc::new(move || {
                let settings = shell.settings.current.borrow().library_list(key);
                let previous = applied_settings.borrow().clone();
                if previous.sort_key != settings.sort_key
                    || previous.descending != settings.descending
                {
                    populate_artist_model(&model, &artists.borrow(), &settings);
                }
                content.apply_settings(&settings);
                page_shell.apply_library_list_settings(key, &settings);
                *applied_settings.borrow_mut() = settings;
            })
        };
        MountedRoute::new(page_shell.widget(), affected_by, apply_delta, resume)
    }
    pub(crate) fn library_playlists_route_from_prepared(
        self: &Rc<Self>,
        library_query: ActiveLibraryQuery,
        playlists: Vec<::library::Playlist>,
    ) -> MountedRoute {
        let load_query = library_query.clone();
        CollectionRouteSpec {
            key: LibraryListKey::Playlists,
            empty_body: msgid("Cached entries will appear here after sync finishes"),
            load_items: Arc::new(move || {
                load_complete_cached_items(|limit| load_query.playlists_page(0, limit))
                    .unwrap_or_else(|error| {
                        warn!(%error, "failed to load playlists page");
                        Vec::new()
                    })
            }),
            matches_query: Rc::new(playlist_matches_query),
            populate_model: Rc::new(populate_playlist_model),
            build_content: Rc::new(playlist_collection_projection),
            affected: Rc::new(|delta| delta.reset.is_some() || !delta.playlists.is_empty()),
        }
        .view_from_items(self, playlists)
    }
    pub(crate) fn library_smart_playlists_route_from_prepared(
        self: &Rc<Self>,
        library_query: ActiveLibraryQuery,
        playlists: Vec<::library::SmartPlaylist>,
    ) -> MountedRoute {
        let initial_query = library_query.clone();
        let content_query = library_query;
        CollectionRouteSpec {
            key: LibraryListKey::SmartPlaylists,
            empty_body: msgid("Smart playlists will appear here after the default set is seeded."),
            load_items: Arc::new(move || {
                load_complete_cached_items(|limit| initial_query.smart_playlists_page(0, limit))
                    .unwrap_or_else(|error| {
                        warn!(%error, "failed to load smart playlists page");
                        Vec::new()
                    })
            }),
            matches_query: Rc::new(smart_playlist_matches_query),
            populate_model: Rc::new(populate_smart_playlist_model),
            build_content: Rc::new(move |shell, model| {
                smart_playlist_collection_projection(shell, model, content_query.clone())
            }),
            affected: Rc::new(|delta| {
                delta.reset.is_some()
                    || !delta.tracks.added.is_empty()
                    || !delta.tracks.deleted.is_empty()
                    || !delta.tracks.fields.is_empty()
                    || !delta.tracks.metadata.is_empty()
                    || !delta.tracks.stats.is_empty()
                    || !delta.tracks.skip_stats.is_empty()
                    || !delta.tracks.favorite.is_empty()
                    || !delta.tracks.cover_refs.is_empty()
                    || !delta.smart_playlists.is_empty()
            }),
        }
        .view_from_items(self, playlists)
    }
    pub(crate) fn scrolling_track_projection_with_selection(
        self: &Rc<Self>,
        tracks: impl Into<Arc<Vec<Track>>>,
        key: LibraryListKey,
        context: &str,
        source_descriptor: Option<PlayContextDescriptor>,
        selection_handle: Option<TrackTableSelectionHandle>,
    ) -> (gtk::Widget, TrackListProjection, LibraryToolbarProjection) {
        let projection = self.searchable_track_collection_shared(
            tracks.into(),
            key,
            SearchableTrackOptions {
                on_visible_count_changed: None,
                source_descriptor,
                favorites_only: false,
                content_inset: PRIMARY_ROUTE_HORIZONTAL_INSET,
                selection_handle,
                fixed_layout: None,
            },
        );
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 10);
        wrapper.set_widget_name(context);
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_vexpand(true);
        let toolbar = self.library_toolbar_projection(key, projection.search());
        wrapper.append(&library_route_inset(toolbar.widget()));
        self.set_route_search(Some(projection.search()));

        wrapper.append(&projection.scrolling_widget());
        (wrapper.upcast(), projection, toolbar)
    }
    pub(crate) fn searchable_track_collection(
        self: &Rc<Self>,
        tracks: Vec<Track>,
        key: LibraryListKey,
        options: SearchableTrackOptions,
    ) -> TrackListProjection {
        self.searchable_track_collection_shared(Arc::new(tracks), key, options)
    }

    fn searchable_track_collection_shared(
        self: &Rc<Self>,
        tracks: Arc<Vec<Track>>,
        key: LibraryListKey,
        options: SearchableTrackOptions,
    ) -> TrackListProjection {
        self.searchable_track_collection_with_initial_order(tracks, key, options, false)
    }

    fn searchable_track_collection_from_sorted_store(
        self: &Rc<Self>,
        tracks: Arc<Vec<Track>>,
        key: LibraryListKey,
        options: SearchableTrackOptions,
    ) -> TrackListProjection {
        self.searchable_track_collection_with_initial_order(tracks, key, options, true)
    }

    fn searchable_track_collection_with_initial_order(
        self: &Rc<Self>,
        tracks: Arc<Vec<Track>>,
        key: LibraryListKey,
        options: SearchableTrackOptions,
        initial_tracks_are_sorted: bool,
    ) -> TrackListProjection {
        let source_tracks = Rc::new(RefCell::new(tracks));
        let query = Rc::new(RefCell::new(String::new()));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let mut settings = self.settings.current.borrow().library_list(key);
        if let Some(layout) = options.fixed_layout {
            settings.layout = layout;
        }
        // The global Tracks route has just loaded this complete vector from the Store with the
        // same sort settings. Seed the GTK model from that proven order instead of sorting it a
        // second time. This is only a construction fast path, not a retained route/data cache;
        // search, settings changes, and deltas still rebuild through `tracks_for_settings`.
        let visible_tracks = if initial_tracks_are_sorted {
            source_tracks.borrow().iter().cloned().collect()
        } else {
            tracks_for_settings(source_tracks.borrow().as_slice(), &settings, "", false)
        };
        let visible_count = visible_tracks.len();
        replace_tracks_in_model(&model, visible_tracks);
        let track_index = TrackModelIndex::new(&model);
        if let Some(on_visible_count_changed) = options.on_visible_count_changed.as_ref() {
            on_visible_count_changed(visible_count);
        }
        let search = gtk::SearchEntry::new();
        bind_search_placeholder(&search, "Search");
        search.set_hexpand(true);
        let refresh_tracks = {
            let shell = Rc::clone(self);
            let model = model.clone();
            let source_tracks = Rc::clone(&source_tracks);
            let on_visible_count_changed = options.on_visible_count_changed.clone();
            let query = Rc::clone(&query);
            Rc::new(move || {
                let settings = shell.settings.current.borrow().library_list(key);
                let visible_tracks = tracks_for_settings(
                    source_tracks.borrow().as_slice(),
                    &settings,
                    &query.borrow(),
                    false,
                );
                let visible_count = visible_tracks.len();
                replace_tracks_in_model(&model, visible_tracks);
                shell.refresh_current_route_now_playing_selections();
                if let Some(on_visible_count_changed) = on_visible_count_changed.as_ref() {
                    on_visible_count_changed(visible_count);
                }
            }) as Rc<dyn Fn()>
        };
        let replace_tracks = {
            let source_tracks = Rc::clone(&source_tracks);
            let refresh_tracks = Rc::clone(&refresh_tracks);
            Rc::new(move |tracks: Arc<Vec<Track>>| {
                let replaced = source_tracks.replace(tracks);
                release_replaced_tracks(replaced);
                refresh_tracks();
            }) as Rc<dyn Fn(Arc<Vec<Track>>)>
        };
        {
            let query = Rc::clone(&query);
            let refresh_tracks = Rc::clone(&refresh_tracks);
            search.connect_search_changed(move |entry| {
                *query.borrow_mut() = entry.text().trim().to_string();
                refresh_tracks();
            });
        }
        let source_descriptor = options
            .source_descriptor
            .map(|descriptor| Rc::new(RefCell::new(descriptor)));
        let play_context = source_descriptor.as_ref().map(|descriptor| {
            track_collection_play_context(
                self,
                Rc::clone(descriptor),
                key,
                Rc::clone(&query),
                options.favorites_only,
                false,
            )
        });
        let collection = track_collection_projection(
            self,
            model.clone(),
            key,
            settings.clone(),
            play_context,
            options.content_inset,
            options.selection_handle,
            track_index.clone(),
        );
        TrackListProjection {
            key,
            search,
            collection,
            track_index,
            source_tracks,
            replace_tracks,
            refresh_tracks,
            source_descriptor,
            settings: Rc::new(RefCell::new(settings)),
            fixed_layout: options.fixed_layout,
        }
    }
}

pub(crate) fn install_embedded_track_scroll_latch(
    scroller: &gtk::ScrolledWindow,
    header_height: i32,
) {
    let last_parent_scroll = Rc::new(Cell::new(None::<Instant>));
    let connected_parent = Rc::new(Cell::new(false));
    let pointer_y = Rc::new(Cell::new(None::<f64>));

    let motion = gtk::EventControllerMotion::new();
    let motion_y = Rc::clone(&pointer_y);
    motion.connect_motion(move |_, _, y| {
        motion_y.set(Some(y));
    });
    let leave_y = Rc::clone(&pointer_y);
    motion.connect_leave(move |_| {
        leave_y.set(None);
    });
    scroller.add_controller(motion);

    let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
    let scroller_weak = scroller.downgrade();
    wheel.connect_scroll(move |controller, _, dy| {
        if dy == 0.0 {
            return gtk::glib::Propagation::Proceed;
        }

        let Some(scroller) = scroller_weak.upgrade() else {
            return gtk::glib::Propagation::Stop;
        };
        let Some(parent) =
            nearest_parent_scrolled_window(&scroller.clone().upcast::<gtk::Widget>())
        else {
            return gtk::glib::Propagation::Proceed;
        };
        if !connected_parent.get() {
            connected_parent.set(true);
            let last_parent_scroll = Rc::clone(&last_parent_scroll);
            parent.vadjustment().connect_value_changed(move |_| {
                last_parent_scroll.set(Some(Instant::now()));
            });
        }

        let unit = controller.unit();
        let parent_latched = parent_scroll_is_latched(Instant::now(), last_parent_scroll.get());
        if pointer_is_embedded_table_header(pointer_y.get(), header_height)
            || parent_latched
            || !adjustment_can_scroll(&scroller.vadjustment(), dy, unit)
        {
            scroll_adjustment(&parent.vadjustment(), dy, unit);
            return gtk::glib::Propagation::Stop;
        }
        gtk::glib::Propagation::Proceed
    });
    scroller.add_controller(wheel);
}

fn parent_scroll_is_latched(now: Instant, last_parent_scroll: Option<Instant>) -> bool {
    last_parent_scroll
        .and_then(|last| now.checked_duration_since(last))
        .is_some_and(|elapsed| elapsed.as_millis() <= EMBEDDED_SCROLL_LATCH_MS)
}

fn pointer_is_embedded_table_header(y: Option<f64>, header_height: i32) -> bool {
    y.is_some_and(|y| y >= 0.0 && y < f64::from(header_height))
}

fn adjustment_can_scroll(
    adjustment: &gtk::Adjustment,
    dy: f64,
    unit: gtk::gdk::ScrollUnit,
) -> bool {
    adjusted_scroll_value(adjustment, dy, unit)
        .is_some_and(|value| (value - adjustment.value()).abs() > f64::EPSILON)
}

fn scroll_adjustment(adjustment: &gtk::Adjustment, dy: f64, unit: gtk::gdk::ScrollUnit) {
    if let Some(value) = adjusted_scroll_value(adjustment, dy, unit) {
        adjustment.set_value(value);
    }
}

fn adjusted_scroll_value(
    adjustment: &gtk::Adjustment,
    dy: f64,
    unit: gtk::gdk::ScrollUnit,
) -> Option<f64> {
    let page_size = adjustment.page_size();
    let multiplier = match unit {
        gtk::gdk::ScrollUnit::Surface => EMBEDDED_SURFACE_SCROLL_FACTOR,
        _ => page_size.powf(2.0 / 3.0),
    };
    let max_value = (adjustment.upper() - page_size).max(adjustment.lower());
    let value = (adjustment.value() + dy * multiplier).clamp(adjustment.lower(), max_value);
    (value - adjustment.value())
        .abs()
        .gt(&f64::EPSILON)
        .then_some(value)
}

fn nearest_parent_scrolled_window(widget: &gtk::Widget) -> Option<gtk::ScrolledWindow> {
    let mut parent = widget.parent();
    while let Some(widget) = parent {
        if let Ok(scroller) = widget.clone().downcast::<gtk::ScrolledWindow>() {
            return Some(scroller);
        }
        parent = widget.parent();
    }
    None
}

#[cfg(test)]
mod track_delta_queue_tests {
    use std::collections::HashSet;

    use library::{LibraryDelta, TrackDelta, TrackId};

    use super::{changed_track_ids, retain_confirmed_track_deletions};

    #[test]
    fn coalesced_delete_and_add_uses_final_store_presence() {
        let track_id = TrackId::new("track:structural-conflict");
        let mut merged = LibraryDelta {
            tracks: TrackDelta {
                deleted: vec![track_id.clone()],
                ..TrackDelta::default()
            },
            ..LibraryDelta::default()
        };
        merged.merge(LibraryDelta {
            tracks: TrackDelta {
                added: vec![track_id.clone()],
                ..TrackDelta::default()
            },
            ..LibraryDelta::default()
        });

        assert_eq!(
            changed_track_ids(&merged.tracks),
            std::slice::from_ref(&track_id)
        );

        let mut present = merged.tracks.clone();
        retain_confirmed_track_deletions(&mut present, &HashSet::from([track_id.clone()]));
        assert!(present.deleted.is_empty());

        let mut absent = merged.tracks;
        retain_confirmed_track_deletions(&mut absent, &HashSet::new());
        assert_eq!(absent.deleted, [track_id]);
    }
}

#[cfg(test)]
mod embedded_scroll_tests {
    use std::time::{Duration, Instant};

    use super::{
        EMBEDDED_SCROLL_LATCH_MS, parent_scroll_is_latched, pointer_is_embedded_table_header,
    };

    const COMPACT_HEADER_HEIGHT: i32 = 30;

    #[test]
    fn parent_scroll_latch_expires() {
        let now = Instant::now();

        assert!(parent_scroll_is_latched(now, Some(now)));
        assert!(!parent_scroll_is_latched(
            now,
            Some(now - Duration::from_millis(EMBEDDED_SCROLL_LATCH_MS as u64 + 1))
        ));
        assert!(!parent_scroll_is_latched(now, None));
    }

    #[test]
    fn embedded_table_header_routes_to_parent() {
        assert!(pointer_is_embedded_table_header(
            Some(0.0),
            COMPACT_HEADER_HEIGHT
        ));
        assert!(pointer_is_embedded_table_header(
            Some(f64::from(COMPACT_HEADER_HEIGHT) - 1.0),
            COMPACT_HEADER_HEIGHT
        ));
        assert!(!pointer_is_embedded_table_header(
            Some(f64::from(COMPACT_HEADER_HEIGHT)),
            COMPACT_HEADER_HEIGHT
        ));
        assert!(!pointer_is_embedded_table_header(
            Some(64.0),
            COMPACT_HEADER_HEIGHT
        ));
        assert!(!pointer_is_embedded_table_header(
            None,
            COMPACT_HEADER_HEIGHT
        ));
    }
}
