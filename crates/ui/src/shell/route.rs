use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use adw::prelude::*;
use gtk::{gio, glib};
use library::{
    ActiveLibraryQuery, Album, AlbumId, LibraryDelta, MusicFolderId, PreparedRead, SourceId, Track,
    TrackId, TrackSort,
};
use tracing::{debug, warn};

use super::Shell;
use super::navigation::update_navigation_selection;
use super::route_position::{
    RoutePositionKey, RoutePositionMemory, restore_route_position_before_snapshot,
};
use crate::routes::complete_prepared_items;
use crate::routes::home::HOME_GENRE_LIMIT;
use crate::routes::route::Route;
use crate::routes::route_layout::{primary_route_scroll_adjustment, route_boundary};

const SLOW_ROUTE_RENDER_MS: u64 = 100;

fn commit_refreshes_visible_route(route: &Route, manual: bool) -> bool {
    manual || !matches!(route, Route::Home)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RouteCurrentTrackContext {
    pub(crate) context_id: String,
    pub(crate) source_rank: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RouteCurrentTrack {
    pub(crate) source_id: SourceId,
    pub(crate) track_id: TrackId,
    pub(crate) occurrence: playback::OccurrenceId,
    pub(crate) context: Option<RouteCurrentTrackContext>,
}

pub(crate) fn route_current_track(
    player: Option<&playback::PlaybackView>,
) -> Option<RouteCurrentTrack> {
    let player = player?;
    let entry = player.transport.current.as_ref()?;
    let context = match &entry.provenance {
        playback::Provenance::Context {
            context_id,
            source_rank,
        } => Some(RouteCurrentTrackContext {
            context_id: context_id.clone(),
            source_rank: *source_rank,
        }),
        playback::Provenance::Manual
        | playback::Provenance::Random
        | playback::Provenance::Radio
        | playback::Provenance::AutoDj
        | playback::Provenance::Legacy => None,
    };
    Some(RouteCurrentTrack {
        source_id: player.transport.source_id.clone(),
        track_id: entry.track.id.clone(),
        occurrence: entry.occurrence.clone(),
        context,
    })
}

pub(crate) type RouteCurrentTrackSelection = Rc<dyn Fn(Option<&RouteCurrentTrack>) -> bool>;

struct PreparedAlbumsRouteData {
    albums: Arc<Vec<Album>>,
    album_tracks: Option<HashMap<AlbumId, Vec<Track>>>,
    prepared_guard: Arc<Vec<Album>>,
}

struct PreparedTracksRouteData {
    tracks: Arc<Vec<Track>>,
    prepared_guard: Arc<Vec<Track>>,
}

pub(super) fn warm_prepared_library_routes(
    query: &ActiveLibraryQuery,
    revision: i64,
    track_sort: TrackSort,
    tracks_descending: bool,
) {
    if let Err(error) = query.prepared_albums(revision) {
        warn!(%error, "failed to warm Albums route data");
    }
    if let Err(error) = query.prepared_tracks(revision, track_sort, tracks_descending) {
        warn!(%error, "failed to warm Tracks route data");
    }
}

fn load_prepared_albums_route_data(
    query: &ActiveLibraryQuery,
    revision: i64,
    include_tracks: bool,
) -> Result<PreparedRead<PreparedAlbumsRouteData>, String> {
    let prepared = match query.prepared_albums(revision)? {
        PreparedRead::Ready(prepared) => prepared,
        PreparedRead::Invalidated => return Ok(PreparedRead::Invalidated),
    };
    let completed = complete_prepared_items(prepared, |limit| query.albums_page(0, limit))?;
    let albums = completed.items;
    let album_tracks = include_tracks.then(|| {
        let ids = albums
            .iter()
            .map(|album| album.id.clone())
            .collect::<Vec<_>>();
        query
            .prepared_album_tracks_if_cached(revision, &ids)
            .unwrap_or_else(|| {
                query.album_tracks(&ids).unwrap_or_else(|error| {
                    warn!(%error, "failed to load Albums detail track projection");
                    HashMap::new()
                })
            })
    });
    Ok(PreparedRead::Ready(PreparedAlbumsRouteData {
        albums,
        album_tracks,
        prepared_guard: completed.prepared_guard,
    }))
}

fn load_prepared_tracks_route_data(
    query: &ActiveLibraryQuery,
    revision: i64,
    sort: TrackSort,
    descending: bool,
) -> Result<PreparedRead<PreparedTracksRouteData>, String> {
    let prepared = match query.prepared_tracks(revision, sort, descending)? {
        PreparedRead::Ready(prepared) => prepared,
        PreparedRead::Invalidated => return Ok(PreparedRead::Invalidated),
    };
    let completed = complete_prepared_items(prepared, |limit| {
        query.tracks_page(sort, descending, 0, limit)
    })?;
    Ok(PreparedRead::Ready(PreparedTracksRouteData {
        tracks: completed.items,
        prepared_guard: completed.prepared_guard,
    }))
}

fn release_prepared_route_value<T: Send + 'static>(value: T) {
    glib::spawn_future_local(async move {
        let _ = gio::spawn_blocking(move || drop(value)).await;
    });
}

pub(crate) struct RouteViewport {
    pub(super) route_host: gtk::Stack,
    mounted_route: RefCell<Option<MountedRouteEntry>>,
    position_memory: RefCell<RoutePositionMemory>,
    preparation_generation: Cell<u64>,
    pending_preparation: RefCell<Option<RoutePreparationToken>>,
    pub(crate) current_library_toolbar_controls: RefCell<Option<glib::WeakRef<gtk::Box>>>,
    current_track_selections: RefCell<Vec<RouteCurrentTrackSelection>>,
    pub(crate) route_search: RefCell<Option<gtk::SearchEntry>>,
    pub(crate) route_search_focus: RefCell<Option<Rc<dyn Fn()>>>,
}

#[derive(Default)]
struct RouteActivationContext {
    library_toolbar_controls: Option<glib::WeakRef<gtk::Box>>,
    current_track_selections: Vec<RouteCurrentTrackSelection>,
    route_search: Option<gtk::SearchEntry>,
    route_search_focus: Option<Rc<dyn Fn()>>,
}

struct MountedRouteEntry {
    route: Route,
    view: MountedRoute,
    surface: gtk::Widget,
    position_key: Option<RoutePositionKey>,
    scroll_adjustment: Option<gtk::Adjustment>,
}

impl RouteViewport {
    pub(super) fn new(route_host: gtk::Stack) -> Self {
        Self {
            route_host,
            mounted_route: RefCell::new(None),
            position_memory: RefCell::new(RoutePositionMemory::default()),
            preparation_generation: Cell::new(0),
            pending_preparation: RefCell::new(None),
            current_library_toolbar_controls: RefCell::new(None),
            current_track_selections: RefCell::new(Vec::new()),
            route_search: RefCell::new(None),
            route_search_focus: RefCell::new(None),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RoutePreparationToken {
    generation: u64,
    route: Route,
    source_id: SourceId,
    revision: i64,
    selected_music_folder_id: Option<MusicFolderId>,
    tracks_order: Option<(TrackSort, bool)>,
}

impl RoutePreparationToken {
    fn matches(
        &self,
        generation: u64,
        route: &Route,
        query_source_id: Option<&SourceId>,
        presentation_source_id: Option<&SourceId>,
        revision: i64,
        selected_music_folder_id: Option<&MusicFolderId>,
        tracks_order: Option<(TrackSort, bool)>,
    ) -> bool {
        self.generation == generation
            && &self.route == route
            && query_source_id == Some(&self.source_id)
            && presentation_source_id == Some(&self.source_id)
            && self.revision == revision
            && self.selected_music_folder_id.as_ref() == selected_music_folder_id
            && self.tracks_order == tracks_order
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RouteStack {
    back: Vec<Route>,
    current: Route,
    forward: Vec<Route>,
}

impl RouteStack {
    pub(crate) fn new(initial: Route) -> Self {
        Self {
            back: Vec::new(),
            current: initial,
            forward: Vec::new(),
        }
    }

    pub(crate) fn current(&self) -> &Route {
        &self.current
    }

    pub(crate) fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub(crate) fn navigate(&mut self, route: Route) {
        if self.current == route {
            return;
        }

        let previous = std::mem::replace(&mut self.current, route);
        self.back.push(previous);
        self.forward.clear();
    }

    pub(crate) fn back(&mut self) -> Option<&Route> {
        let previous = self.back.pop()?;
        let current = std::mem::replace(&mut self.current, previous);
        self.forward.push(current);
        Some(&self.current)
    }

    pub(crate) fn forward(&mut self) -> Option<&Route> {
        let next = self.forward.pop()?;
        let current = std::mem::replace(&mut self.current, next);
        self.back.push(current);
        Some(&self.current)
    }
}

impl Shell {
    fn release_prepared_query_evictions(evictions: library::PreparedReadEvictions) {
        let evictions = evictions.release_shared_references();
        if evictions.is_empty() {
            return;
        }
        glib::spawn_future_local(async move {
            let _ = gio::spawn_blocking(move || drop(evictions)).await;
        });
    }

    pub(crate) fn invalidate_prepared_query_reads(
        &self,
        query: &ActiveLibraryQuery,
        delta: &LibraryDelta,
    ) {
        Self::release_prepared_query_evictions(query.invalidate_prepared_reads(delta));
    }

    fn advance_prepared_query_reads(
        &self,
        query: &ActiveLibraryQuery,
        revision: i64,
        delta: &LibraryDelta,
    ) {
        Self::release_prepared_query_evictions(query.advance_prepared_reads(revision, delta));
    }

    pub(crate) fn schedule_prepared_library_warm(self: &Rc<Self>) {
        let (committed, revision) = {
            let presentation = self.source.presentation.borrow();
            (
                presentation.cache.is_committed(),
                presentation.cache.revision(),
            )
        };
        if !committed {
            return;
        }
        let Some(query) = self.library.query.borrow().clone() else {
            return;
        };
        let tracks = self
            .settings
            .current
            .borrow()
            .library_list(crate::LibraryListKey::Tracks);
        let track_sort = tracks.sort_key.track_sort();
        let tracks_descending = tracks.descending;
        glib::spawn_future_local(async move {
            if gio::spawn_blocking(move || {
                warm_prepared_library_routes(&query, revision, track_sort, tracks_descending);
            })
            .await
            .is_err()
            {
                warn!("prepared library warm task panicked");
            }
        });
    }

    pub(crate) fn register_current_route_track_selection(
        &self,
        selection: RouteCurrentTrackSelection,
    ) {
        let current = route_current_track(self.playback.player.borrow().as_ref());
        if selection(current.as_ref()) {
            self.route_viewport
                .current_track_selections
                .borrow_mut()
                .push(selection);
        }
    }

    pub(crate) fn refresh_current_route_now_playing_selections(&self) {
        let current = route_current_track(self.playback.player.borrow().as_ref());
        self.route_viewport
            .current_track_selections
            .borrow_mut()
            .retain(|selection| selection(current.as_ref()));
    }
}

pub(crate) type MountedRouteDeltaApplier = Rc<dyn Fn(&LibraryDelta)>;
pub(crate) type MountedRouteDeltaPredicate = Rc<dyn Fn(&LibraryDelta) -> bool>;
pub(crate) type MountedRouteResume = Rc<dyn Fn()>;
pub(crate) type MountedHomeSectionApplier =
    Rc<dyn Fn(library::HomeSectionKind, Option<library::HomeSection>, Option<library::Album>)>;

#[derive(Clone)]
pub(crate) struct MountedRoute {
    widget: gtk::Widget,
    affected_by: MountedRouteDeltaPredicate,
    apply_delta: MountedRouteDeltaApplier,
    resume: MountedRouteResume,
    apply_home_section: Option<MountedHomeSectionApplier>,
}

impl MountedRoute {
    pub(crate) fn new(
        widget: gtk::Widget,
        affected_by: MountedRouteDeltaPredicate,
        apply_delta: MountedRouteDeltaApplier,
        resume: MountedRouteResume,
    ) -> Self {
        Self {
            widget,
            affected_by,
            apply_delta,
            resume,
            apply_home_section: None,
        }
    }

    pub(crate) fn static_widget(widget: gtk::Widget) -> Self {
        Self {
            widget,
            affected_by: Rc::new(|_| false),
            apply_delta: Rc::new(|_| {}),
            resume: Rc::new(|| {}),
            apply_home_section: None,
        }
    }

    pub(crate) fn with_home_section_applier(
        mut self,
        apply_home_section: MountedHomeSectionApplier,
    ) -> Self {
        self.apply_home_section = Some(apply_home_section);
        self
    }

    pub(crate) fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    pub(crate) fn apply_delta(&self, delta: &LibraryDelta) {
        (self.apply_delta)(delta);
    }

    pub(crate) fn affected_by(&self, delta: &LibraryDelta) -> bool {
        (self.affected_by)(delta)
    }

    pub(crate) fn resume(&self) {
        (self.resume)();
    }

    pub(crate) fn apply_home_section(
        &self,
        kind: library::HomeSectionKind,
        section: Option<library::HomeSection>,
        showcase_fallback: Option<library::Album>,
    ) {
        if let Some(apply) = &self.apply_home_section {
            apply(kind, section, showcase_fallback);
        }
    }
}

impl Shell {
    pub(crate) fn prepare_home_route(self: &Rc<Self>) {
        self.reset_cover_pipeline_state();
        self.navigation.routes.borrow_mut().navigate(Route::Home);
    }

    pub(crate) fn navigate(self: &Rc<Self>, route: Route) {
        debug!(?route, "navigate");
        self.close_fullscreen_player();
        let previous = self.navigation.routes.borrow().current().clone();
        self.navigation.routes.borrow_mut().navigate(route.clone());
        self.render_current_route();
        self.handle_home_route_transition(&previous, &route);
    }

    pub(crate) fn go_back(self: &Rc<Self>) {
        let previous = self.navigation.routes.borrow().current().clone();
        let route = self.navigation.routes.borrow_mut().back().cloned();
        if let Some(route) = route {
            debug!(?route, "navigate back");
            self.render_current_route();
            self.handle_home_route_transition(&previous, &route);
        }
    }

    pub(crate) fn go_forward(self: &Rc<Self>) {
        let previous = self.navigation.routes.borrow().current().clone();
        let route = self.navigation.routes.borrow_mut().forward().cloned();
        if let Some(route) = route {
            debug!(?route, "navigate forward");
            self.render_current_route();
            self.handle_home_route_transition(&previous, &route);
        }
    }
}

impl Shell {
    pub(crate) fn render_current_route(self: &Rc<Self>) {
        if !self.startup.route_revealed.get() && !self.source.login_screen_active() {
            self.render_startup_loading_view();
            return;
        }
        if self.source.login_screen_active() {
            self.clear_mounted_routes();
            while let Some(child) = self.chrome.login_host.first_child() {
                self.chrome.login_host.remove(&child);
            }
            let view = self.add_server_view();
            self.chrome.login_host.append(&view);
            self.show_reconnect_notice_if_needed();
            return;
        }

        self.render_current_route_content();
    }

    pub(crate) fn render_current_route_content(self: &Rc<Self>) {
        let route = self.navigation.routes.borrow().current().clone();
        update_navigation_selection(self);
        if self
            .route_viewport
            .mounted_route
            .borrow()
            .as_ref()
            .is_some_and(|entry| entry.route == route)
        {
            return;
        }
        let pending = self.route_viewport.pending_preparation.borrow().clone();
        if pending
            .as_ref()
            .is_some_and(|token| self.route_preparation_is_current(token))
        {
            return;
        }

        let render_started = Instant::now();
        let generation = self.next_route_preparation_generation();
        match route.clone() {
            Route::Home => {
                self.prepare_home_overview_route(route, generation, render_started);
                return;
            }
            Route::Albums => {
                self.prepare_albums_route(route, generation, render_started);
                return;
            }
            Route::Tracks => {
                self.prepare_tracks_route(route, generation, render_started);
                return;
            }
            Route::AlbumDetail(_)
            | Route::ArtistDetail(_)
            | Route::ArtistDiscography(_)
            | Route::ArtistTracks(_)
            | Route::GenreDetail(_)
            | Route::MoodDetail(_)
            | Route::PlaylistDetail(_) => {
                self.prepare_detail_route(route, generation, render_started);
                return;
            }
            Route::SmartPlaylistDetail(smart_playlist_id) => {
                self.prepare_smart_playlist_detail_route(
                    route,
                    smart_playlist_id,
                    generation,
                    render_started,
                );
                return;
            }
            Route::Artists
            | Route::AlbumArtists
            | Route::Favorites
            | Route::Genres
            | Route::Moods
            | Route::Playlists
            | Route::SmartPlaylists => {
                self.prepare_collection_route(route, generation, render_started);
                return;
            }
            _ => {}
        };

        self.replace_mounted_route(route.clone(), render_started, None, || match route {
            Route::Home => unreachable!(),
            Route::Albums
            | Route::Artists
            | Route::AlbumArtists
            | Route::Tracks
            | Route::SmartPlaylistDetail(_)
            | Route::Favorites
            | Route::Genres
            | Route::Moods
            | Route::Playlists
            | Route::SmartPlaylists
            | Route::AlbumDetail(_)
            | Route::ArtistDetail(_)
            | Route::ArtistDiscography(_)
            | Route::ArtistTracks(_)
            | Route::GenreDetail(_)
            | Route::MoodDetail(_)
            | Route::PlaylistDetail(_) => unreachable!(),
            Route::Folders { path } => self.folders_route(path),
            Route::Search { query, kind } => self.search_route(&query, kind),
        });
    }

    pub(crate) fn refresh_current_prepared_route(self: &Rc<Self>) {
        let route = self.navigation.routes.borrow().current().clone();
        let render_started = Instant::now();
        let generation = self.next_route_preparation_generation();
        match route.clone() {
            Route::Home => self.prepare_home_overview_route(route, generation, render_started),
            Route::Albums => self.prepare_albums_route(route, generation, render_started),
            Route::Tracks => self.prepare_tracks_route(route, generation, render_started),
            Route::AlbumDetail(_)
            | Route::ArtistDetail(_)
            | Route::ArtistDiscography(_)
            | Route::ArtistTracks(_)
            | Route::GenreDetail(_)
            | Route::MoodDetail(_)
            | Route::PlaylistDetail(_) => {
                self.prepare_detail_route(route, generation, render_started)
            }
            Route::SmartPlaylistDetail(smart_playlist_id) => self
                .prepare_smart_playlist_detail_route(
                    route,
                    smart_playlist_id,
                    generation,
                    render_started,
                ),
            Route::Artists
            | Route::AlbumArtists
            | Route::Favorites
            | Route::Genres
            | Route::Moods
            | Route::Playlists
            | Route::SmartPlaylists => {
                self.prepare_collection_route(route, generation, render_started)
            }
            _ => debug_assert!(false, "only prepared routes use forced refresh"),
        }
    }

    pub(crate) fn refresh_mounted_tracks_from_prepared(
        self: &Rc<Self>,
        query: ActiveLibraryQuery,
        apply: Rc<dyn Fn(std::sync::Arc<Vec<Track>>)>,
    ) {
        let route = self.navigation.routes.borrow().current().clone();
        if route != Route::Tracks {
            return;
        }
        let settings = self
            .settings
            .current
            .borrow()
            .library_list(crate::LibraryListKey::Tracks);
        let sort = settings.sort_key.track_sort();
        let descending = settings.descending;
        let generation = self.next_route_preparation_generation();
        let token = self.route_preparation_token(
            generation,
            route,
            query.source_id().clone(),
            Some((sort, descending)),
        );
        if let Some(prepared) = query.prepared_tracks_if_cached(token.revision, sort, descending)
            && prepared.items.len() == prepared.total
        {
            apply(prepared.items);
            return;
        }

        let shell = Rc::clone(self);
        let load_query = query.clone();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                load_prepared_tracks_route_data(&load_query, token.revision, sort, descending)
            })
            .await;
            if !shell.route_preparation_is_current(&token) {
                release_prepared_route_value(result);
                if shell.preparation_token_still_owns_retry(&token) {
                    shell.refresh_mounted_tracks_from_prepared(query, apply);
                }
                return;
            }
            let prepared = match result {
                Ok(Ok(PreparedRead::Ready(prepared))) => prepared,
                Ok(Ok(PreparedRead::Invalidated)) => {
                    if shell.preparation_token_still_owns_retry(&token) {
                        shell.refresh_mounted_tracks_from_prepared(query, apply);
                    }
                    return;
                }
                Ok(Err(error)) => {
                    warn!(%error, "failed to refresh mounted Tracks route");
                    return;
                }
                Err(_) => {
                    warn!("mounted Tracks route refresh task panicked");
                    return;
                }
            };
            if query
                .prepared_tracks_if_cached(token.revision, sort, descending)
                .is_none_or(|current| !Arc::ptr_eq(&current.items, &prepared.prepared_guard))
            {
                release_prepared_route_value(prepared);
                if shell.preparation_token_still_owns_retry(&token) {
                    shell.refresh_mounted_tracks_from_prepared(query, apply);
                }
                return;
            }
            apply(prepared.tracks);
        });
    }

    fn replace_mounted_route(
        self: &Rc<Self>,
        route: Route,
        render_started: Instant,
        prepared_read_ms: Option<u64>,
        build: impl FnOnce() -> MountedRoute,
    ) {
        self.route_viewport.pending_preparation.borrow_mut().take();
        let replacement_started = Instant::now();
        let previous = self.begin_mounted_route_replacement();
        if let Some(previous) = previous {
            self.favorites.clear_controls();
            self.route_viewport.route_host.remove(&previous.surface);
            drop(previous);
        }
        self.cancel_route_artwork_interaction();
        let teardown_ms = replacement_started.elapsed().as_millis() as u64;
        let cover_reset_started = Instant::now();
        self.reset_route_covers();
        let cover_reset_ms = cover_reset_started.elapsed().as_millis() as u64;
        let gtk_build_started = Instant::now();
        let view = build();
        let gtk_build_ms = gtk_build_started.elapsed().as_millis() as u64;
        let mount_started = Instant::now();
        let widget = view.widget();
        let scroll_adjustment = primary_route_scroll_adjustment(&widget);
        let position_key = scroll_adjustment.as_ref().and_then(|_| {
            self.library
                .query
                .borrow()
                .as_ref()
                .map(|query| RoutePositionKey::new(query.source_id().clone(), route.clone()))
        });
        let restore_position = position_key.as_ref().and_then(|key| {
            self.route_viewport
                .position_memory
                .borrow_mut()
                .restore(key)
        });
        let boundary = route_boundary(widget);
        let surface = match (scroll_adjustment.as_ref(), restore_position) {
            (Some(adjustment), Some(position)) => {
                restore_route_position_before_snapshot(&boundary, adjustment, position)
            }
            _ => boundary,
        };
        let context = self.take_current_route_context();
        self.route_viewport.route_host.add_child(&surface);
        if let Some(adjustment) = scroll_adjustment.as_ref() {
            self.install_route_artwork_interaction(adjustment);
        }
        let displaced = self
            .route_viewport
            .mounted_route
            .replace(Some(MountedRouteEntry {
                route: route.clone(),
                view: view.clone(),
                surface: surface.clone(),
                position_key,
                scroll_adjustment,
            }));
        debug_assert!(displaced.is_none());
        self.install_current_route_context(context);
        view.resume();
        self.route_viewport.route_host.set_visible_child(&surface);
        self.sync_library_toolbar_end_margin();
        let mount_ms = mount_started.elapsed().as_millis() as u64;
        let total_ms = render_started.elapsed().as_millis() as u64;
        let before_replacement_ms = replacement_started
            .duration_since(render_started)
            .as_millis() as u64;
        debug!(
            ?route,
            ?prepared_read_ms,
            before_replacement_ms,
            teardown_ms,
            cover_reset_ms,
            gtk_build_ms,
            mount_ms,
            total_ms,
            "route render timing"
        );
        if total_ms >= SLOW_ROUTE_RENDER_MS {
            warn!(
                ?route,
                ?prepared_read_ms,
                before_replacement_ms,
                teardown_ms,
                cover_reset_ms,
                gtk_build_ms,
                mount_ms,
                total_ms,
                "slow route render"
            );
        }
    }

    fn next_route_preparation_generation(&self) -> u64 {
        let generation = self
            .route_viewport
            .preparation_generation
            .get()
            .wrapping_add(1);
        self.route_viewport.preparation_generation.set(generation);
        generation
    }

    fn route_preparation_token(
        &self,
        generation: u64,
        route: Route,
        source_id: SourceId,
        tracks_order: Option<(TrackSort, bool)>,
    ) -> RoutePreparationToken {
        let presentation = self.source.presentation.borrow();
        RoutePreparationToken {
            generation,
            route,
            source_id,
            revision: presentation.cache.revision(),
            selected_music_folder_id: presentation.selected_music_folder_id.clone(),
            tracks_order,
        }
    }

    fn route_preparation_is_current(&self, token: &RoutePreparationToken) -> bool {
        let routes = self.navigation.routes.borrow();
        let query = self.library.query.borrow();
        let presentation = self.source.presentation.borrow();
        let tracks_order = matches!(token.route, Route::Tracks).then(|| {
            let settings = self
                .settings
                .current
                .borrow()
                .library_list(crate::LibraryListKey::Tracks);
            (settings.sort_key.track_sort(), settings.descending)
        });
        token.matches(
            self.route_viewport.preparation_generation.get(),
            routes.current(),
            query.as_ref().map(|query| query.source_id()),
            presentation.source.as_ref().map(|source| &source.id),
            presentation.cache.revision(),
            presentation.selected_music_folder_id.as_ref(),
            tracks_order,
        )
    }

    fn retry_current_route_after_stale_preparation(self: &Rc<Self>, token: &RoutePreparationToken) {
        if self.preparation_token_still_owns_retry(token) {
            self.refresh_current_prepared_route();
        }
    }

    fn preparation_token_still_owns_retry(&self, token: &RoutePreparationToken) -> bool {
        self.route_viewport.preparation_generation.get() == token.generation
            && self.navigation.routes.borrow().current() == &token.route
    }

    fn prepare_albums_route(
        self: &Rc<Self>,
        route: Route,
        generation: u64,
        render_started: Instant,
    ) {
        let Some(query) = self.library.query.borrow().clone() else {
            self.replace_mounted_route(route, render_started, None, || {
                MountedRoute::static_widget(self.route_empty_view(localization::msgid(
                    "Cached entries will appear here after sync finishes",
                )))
            });
            return;
        };
        let include_tracks = self
            .settings
            .current
            .borrow()
            .library_list(crate::LibraryListKey::Albums)
            .layout
            == crate::LibraryLayout::Detail;
        let token =
            self.route_preparation_token(generation, route, query.source_id().clone(), None);

        if !include_tracks
            && let Some(prepared) = query.prepared_albums_if_cached(token.revision)
            && prepared.items.len() == prepared.total
        {
            let build_shell = Rc::clone(self);
            self.replace_mounted_route(token.route, render_started, Some(0), move || {
                build_shell.library_albums_route_from_prepared(
                    query,
                    token.revision,
                    (prepared.items, None),
                )
            });
            return;
        }

        self.route_viewport
            .pending_preparation
            .replace(Some(token.clone()));
        let shell = Rc::clone(self);
        let load_query = query.clone();
        let prepared_read_started = Instant::now();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                load_prepared_albums_route_data(&load_query, token.revision, include_tracks)
            })
            .await;
            let prepared_read_ms = prepared_read_started.elapsed().as_millis() as u64;
            if !shell.route_preparation_is_current(&token) {
                release_prepared_route_value(result);
                shell.retry_current_route_after_stale_preparation(&token);
                return;
            }
            let (prepared, cache_backed) = match result {
                Ok(Ok(PreparedRead::Ready(prepared))) => (prepared, true),
                Ok(Ok(PreparedRead::Invalidated)) => {
                    shell.retry_current_route_after_stale_preparation(&token);
                    return;
                }
                Ok(Err(error)) => {
                    warn!(%error, "failed to prepare Albums route");
                    (
                        PreparedAlbumsRouteData {
                            albums: Arc::new(Vec::new()),
                            album_tracks: include_tracks.then(HashMap::new),
                            prepared_guard: Arc::new(Vec::new()),
                        },
                        false,
                    )
                }
                Err(_) => {
                    warn!("Albums route preparation task panicked");
                    (
                        PreparedAlbumsRouteData {
                            albums: Arc::new(Vec::new()),
                            album_tracks: include_tracks.then(HashMap::new),
                            prepared_guard: Arc::new(Vec::new()),
                        },
                        false,
                    )
                }
            };
            if cache_backed
                && query
                    .prepared_albums_if_cached(token.revision)
                    .is_none_or(|current| !Arc::ptr_eq(&current.items, &prepared.prepared_guard))
            {
                release_prepared_route_value(prepared);
                shell.retry_current_route_after_stale_preparation(&token);
                return;
            }
            let route = token.route.clone();
            let build_shell = Rc::clone(&shell);
            shell.replace_mounted_route(route, render_started, Some(prepared_read_ms), move || {
                build_shell.library_albums_route_from_prepared(
                    query,
                    token.revision,
                    (prepared.albums, prepared.album_tracks),
                )
            });
        });
    }

    fn prepare_detail_route(
        self: &Rc<Self>,
        route: Route,
        generation: u64,
        render_started: Instant,
    ) {
        let Some(query) = self.library.query.borrow().clone() else {
            let missing_route = route.clone();
            self.replace_mounted_route(route, render_started, None, || {
                let (title, body) = match missing_route {
                    Route::AlbumDetail(_) => ("Album", "The selected cached album was not found."),
                    Route::ArtistDetail(_) => {
                        ("Artist", "The selected cached artist was not found.")
                    }
                    Route::ArtistDiscography(_) => (
                        localization::msgid("Discography"),
                        "The selected cached artist was not found.",
                    ),
                    Route::ArtistTracks(_) => {
                        ("Tracks", "The selected cached artist was not found.")
                    }
                    Route::GenreDetail(_) => ("Genre", "The selected cached genre was not found."),
                    Route::MoodDetail(_) => (
                        "Mood",
                        "Files need Mood/BPM tags written on them. Not supported for Jellyfin",
                    ),
                    Route::PlaylistDetail(_) => {
                        ("Playlist", "The selected cached playlist was not found.")
                    }
                    _ => unreachable!(),
                };
                MountedRoute::static_widget(self.placeholder_view(title, body))
            });
            return;
        };

        match route.clone() {
            Route::AlbumDetail(album_id) => {
                let load_album_id = album_id.clone();
                self.prepare_store_route(
                    route,
                    generation,
                    render_started,
                    query,
                    move |query, revision| {
                        crate::routes::load_album_detail_for_revision(
                            &query,
                            revision,
                            &load_album_id,
                        )
                        .map(Some)
                        .unwrap_or_else(|error| {
                            warn!(%error, "failed to prepare Album detail route");
                            None
                        })
                    },
                    move |shell, query, loaded| {
                        shell.album_detail_view_from_loaded(query, album_id, loaded)
                    },
                );
            }
            Route::ArtistDetail(artist_id)
            | Route::ArtistDiscography(artist_id)
            | Route::ArtistTracks(artist_id) => {
                let load_artist_id = artist_id.clone();
                let build_route = route.clone();
                self.prepare_store_route(
                    route,
                    generation,
                    render_started,
                    query,
                    move |query, _| {
                        query
                            .artist_detail(&load_artist_id)
                            .unwrap_or_else(|error| {
                                warn!(%error, "failed to prepare Artist detail route");
                                None
                            })
                    },
                    move |shell, query, detail| match build_route {
                        Route::ArtistDetail(_) => {
                            shell.artist_detail_view_from_loaded(query, artist_id, detail)
                        }
                        Route::ArtistDiscography(_) => {
                            shell.artist_discography_view_from_loaded(query, artist_id, detail)
                        }
                        Route::ArtistTracks(_) => {
                            shell.artist_tracks_view_from_loaded(query, artist_id, detail)
                        }
                        _ => unreachable!(),
                    },
                );
            }
            Route::GenreDetail(genre_id) => {
                let load_genre_id = genre_id.clone();
                self.prepare_store_route(
                    route,
                    generation,
                    render_started,
                    query,
                    move |query, _| {
                        query.genre_detail(&load_genre_id).unwrap_or_else(|error| {
                            warn!(%error, "failed to prepare Genre detail route");
                            None
                        })
                    },
                    move |shell, query, detail| {
                        shell.genre_detail_view_from_loaded(query, genre_id, detail)
                    },
                );
            }
            Route::MoodDetail(mood_id) => {
                let load_mood_id = mood_id.clone();
                self.prepare_store_route(
                    route,
                    generation,
                    render_started,
                    query,
                    move |query, _| {
                        query.mood_detail(&load_mood_id).unwrap_or_else(|error| {
                            warn!(%error, "failed to prepare Mood detail route");
                            None
                        })
                    },
                    move |shell, query, detail| {
                        shell.mood_detail_view_from_loaded(query, mood_id, detail)
                    },
                );
            }
            Route::PlaylistDetail(playlist_id) => {
                let load_playlist_id = playlist_id.clone();
                self.prepare_store_route(
                    route,
                    generation,
                    render_started,
                    query,
                    move |query, _| {
                        crate::routes::load_playlist_detail_refresh(&query, &load_playlist_id)
                            .map(Some)
                            .unwrap_or_else(|error| {
                                warn!(%error, "failed to prepare Playlist detail route");
                                None
                            })
                    },
                    move |shell, query, loaded| {
                        shell.playlist_detail_route_from_loaded(query, playlist_id, loaded)
                    },
                );
            }
            _ => unreachable!(),
        }
    }

    fn prepare_collection_route(
        self: &Rc<Self>,
        route: Route,
        generation: u64,
        render_started: Instant,
    ) {
        let Some(query) = self.library.query.borrow().clone() else {
            self.replace_mounted_route(route.clone(), render_started, None, || {
                let message = match route {
                    Route::Artists | Route::AlbumArtists | Route::Genres | Route::Playlists => {
                        localization::msgid("Cached entries will appear here after sync finishes")
                    }
                    Route::Favorites => "Favorite tracks will appear here after you add them.",
                    Route::Moods => localization::msgid(
                        "Files need Mood/BPM tags written on them. Not supported for Jellyfin",
                    ),
                    Route::SmartPlaylists => localization::msgid(
                        "Smart playlists will appear here after the default set is seeded.",
                    ),
                    _ => unreachable!(),
                };
                MountedRoute::static_widget(self.route_empty_view(message))
            });
            return;
        };

        match route.clone() {
            Route::Artists | Route::AlbumArtists => {
                let album_artist = route == Route::AlbumArtists;
                self.prepare_store_route(
                    route,
                    generation,
                    render_started,
                    query,
                    move |query, _| {
                        crate::routes::load_complete_cached_items(|limit| {
                            query.artists_page(album_artist, 0, limit)
                        })
                        .unwrap_or_else(|error| {
                            warn!(%error, album_artist, "failed to prepare Artists route");
                            Vec::new()
                        })
                    },
                    move |shell, query, artists| {
                        shell.library_artist_list_route_from_prepared(album_artist, artists, query)
                    },
                )
            }
            Route::Favorites => self.prepare_store_route(
                route,
                generation,
                render_started,
                query,
                |query, _| {
                    query.favorite_tracks().unwrap_or_else(|error| {
                        warn!(%error, "failed to load favorite tracks");
                        Vec::new()
                    })
                },
                |shell, query, favorites| shell.favorites_route_from_prepared(query, favorites),
            ),
            Route::Genres | Route::Moods => {
                let kind = if route == Route::Genres {
                    crate::routes::named_collections::NamedCollectionKind::Genres
                } else {
                    crate::routes::named_collections::NamedCollectionKind::Moods
                };
                self.prepare_store_route(
                    route,
                    generation,
                    render_started,
                    query,
                    move |query, _| kind.load_items(&query),
                    move |shell, query, items| {
                        shell.library_named_collection_route_from_prepared(kind, query, items)
                    },
                );
            }
            Route::Playlists => self.prepare_store_route(
                route,
                generation,
                render_started,
                query,
                |query, _| {
                    crate::routes::load_complete_cached_items(|limit| {
                        query.playlists_page(0, limit)
                    })
                    .unwrap_or_else(|error| {
                        warn!(%error, "failed to load playlists page");
                        Vec::new()
                    })
                },
                |shell, query, playlists| {
                    shell.library_playlists_route_from_prepared(query, playlists)
                },
            ),
            Route::SmartPlaylists => self.prepare_store_route(
                route,
                generation,
                render_started,
                query,
                |query, _| {
                    crate::routes::load_complete_cached_items(|limit| {
                        query.smart_playlists_page(0, limit)
                    })
                    .unwrap_or_else(|error| {
                        warn!(%error, "failed to load smart playlists page");
                        Vec::new()
                    })
                },
                |shell, query, playlists| {
                    shell.library_smart_playlists_route_from_prepared(query, playlists)
                },
            ),
            _ => unreachable!(),
        }
    }

    fn prepare_home_overview_route(
        self: &Rc<Self>,
        route: Route,
        generation: u64,
        render_started: Instant,
    ) {
        let Some(query) = self.library.query.borrow().clone() else {
            self.replace_mounted_route(route, render_started, None, || {
                MountedRoute::static_widget(self.route_empty_view(localization::msgid(
                    "Cached entries will appear here after sync finishes",
                )))
            });
            return;
        };
        self.prepare_store_route(
            route,
            generation,
            render_started,
            query,
            |query, _| {
                query
                    .home_overview(HOME_GENRE_LIMIT)
                    .unwrap_or_else(|error| {
                        warn!(%error, "failed to prepare Home route");
                        library::HomeOverview::default()
                    })
            },
            |shell, query, overview| shell.home_route_from_prepared(query, overview),
        );
    }

    fn prepare_store_route<T, Load, Build>(
        self: &Rc<Self>,
        route: Route,
        generation: u64,
        render_started: Instant,
        query: ActiveLibraryQuery,
        load: Load,
        build: Build,
    ) where
        T: Default + Send + 'static,
        Load: FnOnce(ActiveLibraryQuery, i64) -> T + Send + 'static,
        Build: FnOnce(&Rc<Shell>, ActiveLibraryQuery, T) -> MountedRoute + 'static,
    {
        let token =
            self.route_preparation_token(generation, route, query.source_id().clone(), None);
        self.route_viewport
            .pending_preparation
            .replace(Some(token.clone()));

        let shell = Rc::downgrade(self);
        let load_query = query.clone();
        let revision = token.revision;
        let prepared_read_started = Instant::now();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || load(load_query, revision)).await;
            let loaded = match result {
                Ok(loaded) => loaded,
                Err(_) => {
                    warn!(route = ?token.route, "route preparation task panicked");
                    T::default()
                }
            };
            let Some(shell) = shell.upgrade() else {
                release_prepared_route_value(loaded);
                return;
            };
            let prepared_read_ms = prepared_read_started.elapsed().as_millis() as u64;
            if !shell.route_preparation_is_current(&token) {
                release_prepared_route_value(loaded);
                shell.retry_current_route_after_stale_preparation(&token);
                return;
            }

            let route = token.route.clone();
            let build_shell = Rc::clone(&shell);
            shell.replace_mounted_route(route, render_started, Some(prepared_read_ms), move || {
                build(&build_shell, query, loaded)
            });
        });
    }

    fn prepare_tracks_route(
        self: &Rc<Self>,
        route: Route,
        generation: u64,
        render_started: Instant,
    ) {
        let Some(query) = self.library.query.borrow().clone() else {
            self.replace_mounted_route(route, render_started, None, || {
                MountedRoute::static_widget(self.route_empty_view(localization::msgid(
                    "Cached entries will appear here after sync finishes",
                )))
            });
            return;
        };
        let settings = self
            .settings
            .current
            .borrow()
            .library_list(crate::LibraryListKey::Tracks);
        let sort = settings.sort_key.track_sort();
        let descending = settings.descending;
        let token = self.route_preparation_token(
            generation,
            route,
            query.source_id().clone(),
            Some((sort, descending)),
        );
        if let Some(prepared) = query.prepared_tracks_if_cached(token.revision, sort, descending)
            && prepared.items.len() == prepared.total
        {
            let build_shell = Rc::clone(self);
            self.replace_mounted_route(token.route, render_started, Some(0), move || {
                build_shell.library_tracks_route_from_prepared(query, prepared.items)
            });
            return;
        }

        self.route_viewport
            .pending_preparation
            .replace(Some(token.clone()));
        let shell = Rc::clone(self);
        let load_query = query.clone();
        let prepared_read_started = Instant::now();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                load_prepared_tracks_route_data(&load_query, token.revision, sort, descending)
            })
            .await;
            let prepared_read_ms = prepared_read_started.elapsed().as_millis() as u64;
            if !shell.route_preparation_is_current(&token) {
                release_prepared_route_value(result);
                shell.retry_current_route_after_stale_preparation(&token);
                return;
            }
            let (prepared, cache_backed) = match result {
                Ok(Ok(PreparedRead::Ready(prepared))) => (prepared, true),
                Ok(Ok(PreparedRead::Invalidated)) => {
                    shell.retry_current_route_after_stale_preparation(&token);
                    return;
                }
                Ok(Err(error)) => {
                    warn!(%error, "failed to prepare Tracks route");
                    (
                        PreparedTracksRouteData {
                            tracks: Arc::new(Vec::new()),
                            prepared_guard: Arc::new(Vec::new()),
                        },
                        false,
                    )
                }
                Err(_) => {
                    warn!("Tracks route preparation task panicked");
                    (
                        PreparedTracksRouteData {
                            tracks: Arc::new(Vec::new()),
                            prepared_guard: Arc::new(Vec::new()),
                        },
                        false,
                    )
                }
            };
            if cache_backed
                && query
                    .prepared_tracks_if_cached(token.revision, sort, descending)
                    .is_none_or(|current| !Arc::ptr_eq(&current.items, &prepared.prepared_guard))
            {
                release_prepared_route_value(prepared);
                shell.retry_current_route_after_stale_preparation(&token);
                return;
            }
            let route = token.route.clone();
            let build_shell = Rc::clone(&shell);
            shell.replace_mounted_route(route, render_started, Some(prepared_read_ms), move || {
                build_shell.library_tracks_route_from_prepared(query, prepared.tracks)
            });
        });
    }

    fn prepare_smart_playlist_detail_route(
        self: &Rc<Self>,
        route: Route,
        smart_playlist_id: library::SmartPlaylistId,
        generation: u64,
        render_started: Instant,
    ) {
        let Some(query) = self.library.query.borrow().clone() else {
            self.replace_mounted_route(route, render_started, None, || {
                MountedRoute::static_widget(self.placeholder_view(
                    localization::msgid("Smart Playlist"),
                    "The selected smart playlist was not found.",
                ))
            });
            return;
        };
        let token =
            self.route_preparation_token(generation, route, query.source_id().clone(), None);

        self.route_viewport
            .pending_preparation
            .replace(Some(token.clone()));
        let shell = Rc::clone(self);
        let load_query = query.clone();
        let load_smart_playlist_id = smart_playlist_id.clone();
        let prepared_read_started = Instant::now();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                load_query.smart_playlist_detail(&load_smart_playlist_id)
            })
            .await;
            let prepared_read_ms = prepared_read_started.elapsed().as_millis() as u64;
            if !shell.route_preparation_is_current(&token) {
                release_prepared_route_value(result);
                shell.retry_current_route_after_stale_preparation(&token);
                return;
            }
            let detail = match result {
                Ok(Ok(detail)) => detail,
                Ok(Err(error)) => {
                    warn!(%error, "failed to prepare smart playlist detail route");
                    None
                }
                Err(_) => {
                    warn!("smart playlist detail route preparation task panicked");
                    None
                }
            };
            let route = token.route.clone();
            let build_shell = Rc::clone(&shell);
            shell.replace_mounted_route(route, render_started, Some(prepared_read_ms), move || {
                build_shell.smart_playlist_detail_route_from_loaded(
                    query,
                    smart_playlist_id,
                    detail,
                )
            });
        });
    }

    pub(crate) fn apply_library_delta(self: &Rc<Self>, delta: LibraryDelta) {
        if delta.is_empty() {
            return;
        }
        self.invalidate_home_projection_overlay_for(&delta);
        if let Some(query) = self.library.query.borrow().as_ref() {
            self.invalidate_prepared_query_reads(query, &delta);
        }
        let restart_pending_route = self
            .route_viewport
            .pending_preparation
            .borrow()
            .as_ref()
            .filter(|token| self.route_preparation_is_current(token))
            .is_some();
        self.apply_delta_to_mounted_route(&delta);
        if restart_pending_route {
            self.refresh_current_prepared_route();
        }
    }

    pub(crate) fn apply_committed_library_delta(
        self: &Rc<Self>,
        revision: i64,
        delta: LibraryDelta,
        manual: bool,
    ) {
        if let Some(query) = self.library.query.borrow().as_ref() {
            self.advance_prepared_query_reads(query, revision, &delta);
        }
        if delta.is_empty() {
            return;
        }
        self.invalidate_home_projection_overlay_for(&delta);
        if commit_refreshes_visible_route(self.navigation.routes.borrow().current(), manual) {
            self.apply_delta_to_mounted_route(&delta);
        }
    }

    fn apply_delta_to_mounted_route(&self, delta: &LibraryDelta) {
        let current_route = self.navigation.routes.borrow().current().clone();
        let active_view = self
            .route_viewport
            .mounted_route
            .borrow()
            .as_ref()
            .filter(|entry| entry.route == current_route)
            .map(|entry| entry.view.clone())
            .filter(|view| view.affected_by(delta));
        if let Some(view) = active_view {
            view.apply_delta(delta);
        }
    }

    pub(crate) fn apply_home_section_to_mounted_route(
        &self,
        kind: library::HomeSectionKind,
        section: Option<library::HomeSection>,
        showcase_fallback: Option<library::Album>,
    ) {
        let active_view = self
            .route_viewport
            .mounted_route
            .borrow()
            .as_ref()
            .filter(|entry| entry.route == Route::Home)
            .map(|entry| entry.view.clone());
        if let Some(view) = active_view {
            view.apply_home_section(kind, section, showcase_fallback);
        }
    }

    pub(crate) fn reconcile_mounted_route(&self) {
        let active_view = self
            .route_viewport
            .mounted_route
            .borrow()
            .as_ref()
            .map(|entry| entry.view.clone());
        if let Some(view) = active_view {
            view.resume();
        }
    }

    pub(crate) fn has_active_mounted_route(&self) -> bool {
        self.route_viewport.mounted_route.borrow().is_some()
    }

    fn begin_mounted_route_replacement(&self) -> Option<MountedRouteEntry> {
        let previous = self.route_viewport.mounted_route.borrow_mut().take();
        if let Some(previous) = previous.as_ref()
            && let (Some(key), Some(adjustment)) = (
                previous.position_key.as_ref(),
                previous.scroll_adjustment.as_ref(),
            )
        {
            self.route_viewport
                .position_memory
                .borrow_mut()
                .record(key.clone(), adjustment.value());
        }
        self.clear_current_route_context();
        previous
    }

    fn take_current_route_context(&self) -> RouteActivationContext {
        RouteActivationContext {
            library_toolbar_controls: self
                .route_viewport
                .current_library_toolbar_controls
                .borrow_mut()
                .take(),
            current_track_selections: std::mem::take(
                &mut *self.route_viewport.current_track_selections.borrow_mut(),
            ),
            route_search: self.route_viewport.route_search.borrow_mut().take(),
            route_search_focus: self.route_viewport.route_search_focus.borrow_mut().take(),
        }
    }

    fn install_current_route_context(&self, context: RouteActivationContext) {
        self.route_viewport
            .current_library_toolbar_controls
            .replace(context.library_toolbar_controls);
        self.route_viewport
            .current_track_selections
            .replace(context.current_track_selections);
        self.route_viewport
            .route_search
            .replace(context.route_search);
        self.route_viewport
            .route_search_focus
            .replace(context.route_search_focus);
    }

    fn clear_current_route_context(&self) {
        self.route_viewport
            .current_library_toolbar_controls
            .borrow_mut()
            .take();
        self.route_viewport.route_search.borrow_mut().take();
        self.route_viewport.route_search_focus.borrow_mut().take();
        self.route_viewport
            .current_track_selections
            .borrow_mut()
            .clear();
    }

    pub(crate) fn clear_mounted_routes(&self) {
        self.next_route_preparation_generation();
        self.route_viewport.pending_preparation.borrow_mut().take();
        let mounted_route = self.begin_mounted_route_replacement();
        if mounted_route.is_none() && self.route_viewport.route_host.first_child().is_none() {
            self.cancel_route_artwork_interaction();
            return;
        }
        self.favorites.clear_controls();
        while let Some(child) = self.route_viewport.route_host.first_child() {
            self.route_viewport.route_host.remove(&child);
        }
        drop(mounted_route);
        self.cancel_route_artwork_interaction();
    }
}

#[cfg(test)]
mod tests {
    use library::{MusicFolderId, SourceId, TrackSort};

    use crate::routes::route::{FolderPathItem, Route};

    use super::{RoutePreparationToken, RouteStack, commit_refreshes_visible_route};

    #[test]
    fn automatic_commit_keeps_visible_home_stable() {
        assert!(!commit_refreshes_visible_route(&Route::Home, false));
        assert!(commit_refreshes_visible_route(&Route::Home, true));
        assert!(commit_refreshes_visible_route(&Route::Tracks, false));
    }

    #[test]
    fn route_track_history() {
        let mut stack = RouteStack::new(Route::Home);

        stack.navigate(Route::Albums);
        stack.navigate(Route::Tracks);

        assert_eq!(stack.current(), &Route::Tracks);
        assert_eq!(stack.back(), Some(&Route::Albums));
        assert_eq!(stack.back(), Some(&Route::Home));
        assert_eq!(stack.back(), None);
        assert_eq!(stack.forward(), Some(&Route::Albums));

        stack.navigate(Route::Favorites);

        assert!(!stack.can_forward());
        assert_eq!(stack.current(), &Route::Favorites);
    }

    #[test]
    fn repeated_route_navigation_is_ignored() {
        let mut stack = RouteStack::new(Route::Home);
        stack.navigate(Route::Home);

        assert!(!stack.can_back());
    }

    #[test]
    fn prepared_route_result_requires_the_same_generation_source_revision_scope_and_order() {
        let source_id = SourceId::new("source:test");
        let next_source_id = SourceId::new("source:next");
        let folder_id = MusicFolderId::new("folder:test");
        let token = RoutePreparationToken {
            generation: 7,
            route: Route::Tracks,
            source_id: source_id.clone(),
            revision: 11,
            selected_music_folder_id: Some(folder_id.clone()),
            tracks_order: Some((TrackSort::Title, false)),
        };
        let matches = |generation, query_source, presentation_source, revision, folder, order| {
            token.matches(
                generation,
                &Route::Tracks,
                query_source,
                presentation_source,
                revision,
                folder,
                order,
            )
        };

        assert!(matches(
            7,
            Some(&source_id),
            Some(&source_id),
            11,
            Some(&folder_id),
            Some((TrackSort::Title, false)),
        ));
        assert!(!matches(
            8,
            Some(&source_id),
            Some(&source_id),
            11,
            Some(&folder_id),
            Some((TrackSort::Title, false)),
        ));
        assert!(!token.matches(
            7,
            &Route::Albums,
            Some(&source_id),
            Some(&source_id),
            11,
            Some(&folder_id),
            Some((TrackSort::Title, false)),
        ));
        assert!(!matches(
            7,
            Some(&next_source_id),
            Some(&source_id),
            11,
            Some(&folder_id),
            Some((TrackSort::Title, false)),
        ));
        assert!(!matches(
            7,
            Some(&source_id),
            Some(&source_id),
            12,
            Some(&folder_id),
            Some((TrackSort::Title, false)),
        ));
        assert!(!matches(
            7,
            Some(&source_id),
            Some(&source_id),
            11,
            None,
            Some((TrackSort::Title, false)),
        ));
        assert!(!matches(
            7,
            Some(&source_id),
            Some(&source_id),
            11,
            Some(&folder_id),
            Some((TrackSort::Album, true)),
        ));
    }

    #[test]
    fn route_support_id() {
        let album_route = Route::AlbumDetail(library::AlbumId::new("jellyfin:album:abc"));
        let mut stack = RouteStack::new(Route::Home);

        stack.navigate(album_route.clone());

        assert_eq!(stack.current(), &album_route);
    }

    #[test]
    fn route_keep_history() {
        let root = Route::Folders { path: Vec::new() };
        let nested = Route::Folders {
            path: vec![FolderPathItem {
                id: library::FolderId::new("jellyfin:folder:music"),
                name: "Music".to_string(),
            }],
        };
        let mut stack = RouteStack::new(Route::Home);

        stack.navigate(root.clone());
        stack.navigate(nested.clone());

        assert_eq!(stack.current(), &nested);
        assert_eq!(stack.back(), Some(&root));
    }
}
