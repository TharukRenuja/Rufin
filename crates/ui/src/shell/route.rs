//! Navigation and the one mounted route.
//!
//! A route prepares one complete projection from the selected `Library`
//! away from GTK, then builds and mounts its GTK model. Home and playlist
//! detail have narrow point-update hooks because those operations are
//! intentionally visible without remounting the whole page.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use adw::prelude::*;
use gtk::{gio, glib};
use library::{HomeSectionKind, HomeSnapshot, MusicFolderId, SourceId, TrackId};
use playback::{SourceSessionEpoch, TransportStatus};
use tracing::{debug, warn};

use super::Shell;
use super::navigation::update_navigation_selection;
use super::route_position::{
    RoutePositionKey, RoutePositionMemory, restore_route_position_before_snapshot,
};
use crate::routes::named_collections::{NamedCollectionKind, load_named_collection};
use crate::routes::route::Route;
use crate::routes::route_layout::{primary_route_scroll_adjustment, route_boundary};
use crate::routes::{
    load_album_detail, load_albums, load_artist_discography, load_artist_overview,
    load_artist_tracks, load_artists, load_favorite_tracks, load_genre_detail, load_history_tracks,
    load_mood_detail, load_playlist_detail, load_playlists, load_smart_playlist_detail,
    load_smart_playlists, load_tracks, prepare_playlist_entry_positions,
};
use crate::runtime::SelectedLibraryUpdate;
use crate::{LibraryListKey, LibraryListSettings};

const SLOW_ROUTE_RENDER_MS: u64 = 250;

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
    pub(crate) paused: bool,
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
        occurrence: entry.id.occurrence.clone(),
        context,
        paused: player.transport.effective_state() == TransportStatus::Paused,
    })
}

pub(crate) type RouteCurrentTrackSelection = Rc<dyn Fn(Option<&RouteCurrentTrack>) -> bool>;
pub(crate) type MountedRouteResume = Rc<dyn Fn()>;
pub(crate) type MountedLibraryUpdate = Rc<dyn Fn(&SelectedLibraryUpdate)>;
pub(crate) type MountedHomeSectionApplier = Rc<dyn Fn(HomeSectionKind, Arc<HomeSnapshot>)>;
pub(crate) type MountedRouteItemNavigation = Rc<dyn Fn(gtk::DirectionType) -> glib::Propagation>;

pub(crate) fn item_navigation_entry_position(
    current: u32,
    item_count: u32,
    direction: gtk::DirectionType,
) -> Option<u32> {
    if item_count == 0 {
        return None;
    }
    if current != gtk::INVALID_LIST_POSITION {
        return Some(current.min(item_count - 1));
    }
    match direction {
        gtk::DirectionType::Up | gtk::DirectionType::Left => Some(item_count - 1),
        gtk::DirectionType::Down | gtk::DirectionType::Right => Some(0),
        _ => None,
    }
}

/// Runs at most one mounted-route read at a time and retains only the newest
/// request while the route still owns this value.
pub(crate) struct LatestMountedRouteRead<T: Send + 'static, R: Send + 'static = ()> {
    apply: Rc<dyn Fn(R, T)>,
    load: Arc<dyn Fn(&R) -> T + Send + Sync>,
    context: &'static str,
    generation: Cell<u64>,
    running: Cell<Option<u64>>,
    pending: RefCell<Option<(u64, R)>>,
}

impl<T: Send + 'static> LatestMountedRouteRead<T> {
    pub(crate) fn new(
        apply: Rc<dyn Fn(T)>,
        load: Arc<dyn Fn() -> T + Send + Sync>,
        context: &'static str,
    ) -> Rc<Self> {
        Self::new_with_request(
            Rc::new(move |(), value| apply(value)),
            Arc::new(move |()| load()),
            context,
        )
    }

    pub(crate) fn request(self: &Rc<Self>) {
        self.request_with(());
    }
}

impl<T: Send + 'static, R: Send + 'static> LatestMountedRouteRead<T, R> {
    pub(crate) fn new_with_request(
        apply: Rc<dyn Fn(R, T)>,
        load: Arc<dyn Fn(&R) -> T + Send + Sync>,
        context: &'static str,
    ) -> Rc<Self> {
        Rc::new(Self {
            apply,
            load,
            context,
            generation: Cell::new(0),
            running: Cell::new(None),
            pending: RefCell::new(None),
        })
    }

    pub(crate) fn request_with(self: &Rc<Self>, request: R) {
        self.queue(request);
        self.start();
    }

    /// Keeps an in-flight read aligned with a search or settings change without
    /// turning ordinary interactive changes into Store reads on their own.
    pub(crate) fn request_with_if_running(self: &Rc<Self>, request: R) {
        if self.running.get().is_none() {
            return;
        }
        self.queue(request);
    }

    fn queue(&self, request: R) {
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        self.pending.replace(Some((generation, request)));
    }

    fn start(self: &Rc<Self>) {
        if self.running.get().is_some() {
            return;
        }
        let Some((generation, request)) = self.pending.borrow_mut().take() else {
            return;
        };
        self.running.set(Some(generation));
        let load = Arc::clone(&self.load);
        let read = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                let value = load(&request);
                (request, value)
            })
            .await;
            let Some(read) = read.upgrade() else {
                return;
            };
            read.running.set(None);
            let (request, value) = match result {
                Ok(value) => value,
                Err(_) => {
                    warn!(context = read.context, "route projection task panicked");
                    read.start();
                    return;
                }
            };
            if read.generation.get() != generation {
                read.start();
                return;
            }
            (read.apply)(request, value);
        });
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RouteProjectionContext {
    source_id: SourceId,
    source_session_epoch: SourceSessionEpoch,
    route: Route,
}

impl RouteProjectionContext {
    fn matches(
        &self,
        source_id: &SourceId,
        source_session_epoch: SourceSessionEpoch,
        route: &Route,
    ) -> bool {
        self.source_id == *source_id
            && self.source_session_epoch == source_session_epoch
            && self.route == *route
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SelectedRouteIdentity {
    context: RouteProjectionContext,
    loaded_instance: usize,
    music_folder_id: Option<MusicFolderId>,
}

impl SelectedRouteIdentity {
    fn matches(
        &self,
        route: &Route,
        source_id: &SourceId,
        source_session_epoch: SourceSessionEpoch,
        loaded_instance: usize,
        music_folder_id: Option<&MusicFolderId>,
    ) -> bool {
        self.context.matches(source_id, source_session_epoch, route)
            && self.loaded_instance == loaded_instance
            && self.music_folder_id.as_ref() == music_folder_id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RouteProjectionIdentity {
    selected: SelectedRouteIdentity,
    settings: Vec<(LibraryListKey, LibraryListSettings)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct IntentToken<K> {
    sequence: u64,
    key: K,
}

struct QueuedIntent<K, I> {
    token: IntentToken<K>,
    intent: I,
}

struct LatestIntentLane<K, I> {
    next_sequence: u64,
    active: Option<IntentToken<K>>,
    pending: Option<QueuedIntent<K, I>>,
    latest: Option<IntentToken<K>>,
}

impl<K: Clone + Eq, I> Default for LatestIntentLane<K, I> {
    fn default() -> Self {
        Self {
            next_sequence: 0,
            active: None,
            pending: None,
            latest: None,
        }
    }
}

impl<K: Clone + Eq, I> LatestIntentLane<K, I> {
    fn submit(&mut self, key: K, intent: I) -> Option<QueuedIntent<K, I>> {
        if self.latest.as_ref().is_some_and(|latest| latest.key == key) {
            return None;
        }
        self.next_sequence = self.next_sequence.wrapping_add(1);
        let token = IntentToken {
            sequence: self.next_sequence,
            key,
        };
        self.latest = Some(token.clone());
        let queued = QueuedIntent {
            token: token.clone(),
            intent,
        };
        if self.active.is_none() {
            self.active = Some(token);
            Some(queued)
        } else {
            self.pending = Some(queued);
            None
        }
    }

    fn should_publish(&self, token: &IntentToken<K>) -> bool {
        self.active.as_ref() == Some(token) && self.latest.as_ref() == Some(token)
    }

    fn finish(&mut self, token: &IntentToken<K>) -> Option<QueuedIntent<K, I>> {
        if self.active.as_ref() != Some(token) {
            return None;
        }
        self.active = None;
        if let Some(next) = self.pending.take() {
            self.active = Some(next.token.clone());
            return Some(next);
        }
        if self.latest.as_ref() == Some(token) {
            self.latest = None;
        }
        None
    }

    fn invalidate(&mut self) {
        self.latest = None;
        self.pending = None;
    }

    fn latest_key(&self) -> Option<&K> {
        self.latest.as_ref().map(|latest| &latest.key)
    }
}

struct RouteProjectionIntent {
    load: Box<dyn FnOnce() -> Result<PreparedRouteBuild, String> + Send>,
    render_started: Instant,
}

struct PreparedRoute {
    build: PreparedRouteBuild,
    render_started: Instant,
    projection_ms: u64,
}

type PreparedRouteBuild = Box<dyn FnOnce(&Rc<Shell>) -> MountedRoute + Send>;

impl RouteProjectionIntent {
    fn prepare(self) -> Result<PreparedRoute, String> {
        let projection_started = Instant::now();
        let build = (self.load)()?;
        Ok(PreparedRoute {
            build,
            render_started: self.render_started,
            projection_ms: elapsed_ms(projection_started),
        })
    }
}

fn prepared_route_build(
    build: impl FnOnce(&Rc<Shell>) -> MountedRoute + Send + 'static,
) -> PreparedRouteBuild {
    Box::new(build)
}

pub(crate) struct RouteViewport {
    pub(super) route_host: gtk::Stack,
    mounted_route: RefCell<Option<MountedRouteEntry>>,
    projection_lane: RefCell<LatestIntentLane<RouteProjectionIdentity, RouteProjectionIntent>>,
    position_memory: RefCell<RoutePositionMemory>,
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
            projection_lane: RefCell::new(LatestIntentLane::default()),
            position_memory: RefCell::new(RoutePositionMemory::default()),
            current_library_toolbar_controls: RefCell::new(None),
            current_track_selections: RefCell::new(Vec::new()),
            route_search: RefCell::new(None),
            route_search_focus: RefCell::new(None),
        }
    }
}

#[derive(Clone)]
pub(crate) struct MountedRoute {
    widget: gtk::Widget,
    resume: MountedRouteResume,
    item_navigation: Option<MountedRouteItemNavigation>,
    library_update: Option<MountedLibraryUpdate>,
    apply_home_section: Option<MountedHomeSectionApplier>,
}

impl MountedRoute {
    pub(crate) fn new(widget: gtk::Widget, resume: MountedRouteResume) -> Self {
        Self {
            widget,
            resume,
            item_navigation: None,
            library_update: None,
            apply_home_section: None,
        }
    }

    pub(crate) fn static_widget(widget: gtk::Widget) -> Self {
        Self::new(widget, Rc::new(|| {}))
    }

    pub(crate) fn with_home_section_applier(
        mut self,
        apply_home_section: MountedHomeSectionApplier,
    ) -> Self {
        self.apply_home_section = Some(apply_home_section);
        self
    }

    pub(crate) fn with_item_navigation(
        mut self,
        item_navigation: MountedRouteItemNavigation,
    ) -> Self {
        self.item_navigation = Some(item_navigation);
        self
    }

    pub(crate) fn with_library_update(mut self, apply: MountedLibraryUpdate) -> Self {
        self.library_update = Some(apply);
        self
    }

    pub(crate) fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    pub(crate) fn resume(&self) {
        (self.resume)();
    }

    fn navigate_items(&self, direction: gtk::DirectionType) -> glib::Propagation {
        if let Some(navigate) = &self.item_navigation {
            navigate(direction)
        } else {
            glib::Propagation::Stop
        }
    }

    fn apply_library_update(&self, update: &SelectedLibraryUpdate) {
        if let Some(apply) = &self.library_update {
            apply(update);
        }
    }

    fn apply_home_section(&self, kind: HomeSectionKind, home: Arc<HomeSnapshot>) {
        if let Some(apply) = &self.apply_home_section {
            apply(kind, home);
        }
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
    pub(crate) fn navigate_current_route_items(
        &self,
        direction: gtk::DirectionType,
    ) -> glib::Propagation {
        if let Some(entry) = self.route_viewport.mounted_route.borrow().as_ref() {
            entry.view.navigate_items(direction)
        } else {
            glib::Propagation::Stop
        }
    }

    pub(crate) fn release_first_run_setup(&self) {
        let Some(view) = self.take_first_run_setup_view() else {
            return;
        };
        if view.parent().is_some() {
            self.chrome.login_host.remove(&view);
        }
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

    pub(crate) fn navigate(self: &Rc<Self>, route: Route) {
        debug!(?route, "navigate");
        self.close_fullscreen_player();
        self.navigation.routes.borrow_mut().navigate(route);
        self.render_current_route();
    }

    pub(crate) fn go_back(self: &Rc<Self>) {
        let route = self.navigation.routes.borrow_mut().back().cloned();
        if let Some(route) = route {
            debug!(?route, "navigate back");
            gtk::prelude::RootExt::set_focus(&self.chrome.window, None::<&gtk::Widget>);
            self.render_current_route();
        }
    }

    pub(crate) fn go_forward(self: &Rc<Self>) {
        let route = self.navigation.routes.borrow_mut().forward().cloned();
        if let Some(route) = route {
            debug!(?route, "navigate forward");
            gtk::prelude::RootExt::set_focus(&self.chrome.window, None::<&gtk::Widget>);
            self.render_current_route();
        }
    }

    pub(crate) fn render_current_route(self: &Rc<Self>) {
        if !self.startup.route_revealed.get() && !self.source.login_screen_active() {
            self.render_startup_loading_view();
            return;
        }
        if self.source.login_screen_active() {
            self.clear_mounted_routes();
            let view = self.add_server_view();
            if self.chrome.login_host.first_child().as_ref() != Some(&view) {
                while let Some(child) = self.chrome.login_host.first_child() {
                    self.chrome.login_host.remove(&child);
                }
                self.chrome.login_host.append(&view);
            }
            return;
        }
        self.render_current_route_content();
    }

    pub(crate) fn render_current_route_content(self: &Rc<Self>) {
        self.render_current_route_content_inner(false);
    }

    pub(crate) fn replace_current_route_when_ready(self: &Rc<Self>) {
        self.render_current_route_content_inner(true);
    }

    fn render_current_route_content_inner(self: &Rc<Self>, force: bool) {
        let route = self.navigation.routes.borrow().current().clone();
        update_navigation_selection(self);
        let mounted_current = self
            .route_viewport
            .mounted_route
            .borrow()
            .as_ref()
            .is_some_and(|entry| entry.route == route);
        if !force && mounted_current {
            let latest_is_current = self
                .route_viewport
                .projection_lane
                .borrow()
                .latest_key()
                .is_some_and(|identity| self.route_projection_is_current(identity));
            if !latest_is_current {
                self.invalidate_route_projection_lane();
            }
            return;
        }

        let render_started = Instant::now();
        let Some(selected) = self.library.selected.borrow().clone() else {
            self.invalidate_route_projection_lane();
            self.replace_mounted_route(route, None, render_started, 0, || {
                MountedRoute::static_widget(
                    self.route_empty_view(localization::msgid("Nothing here yet")),
                )
            });
            return;
        };
        let source_id = Some(selected.source_id.clone());
        match route.clone() {
            Route::Home => {
                self.invalidate_route_projection_lane();
                self.replace_mounted_route(route, source_id, render_started, 0, || {
                    self.home_route(Arc::clone(&selected.home))
                });
            }
            Route::Search => {
                self.invalidate_route_projection_lane();
                self.replace_mounted_route(route, source_id, render_started, 0, || {
                    self.search_route(&selected)
                });
            }
            Route::SmartPlaylists => {
                let settings = self
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::SmartPlaylists);
                let loaded = Arc::clone(&selected.library);
                let music_folder_id = selected.music_folder_id.clone();
                self.queue_route_projection(
                    &selected,
                    route,
                    vec![(LibraryListKey::SmartPlaylists, settings)],
                    render_started,
                    move || {
                        let playlists = load_smart_playlists(&loaded, music_folder_id.as_ref())?;
                        Ok(prepared_route_build(move |shell| {
                            shell.library_smart_playlists_route(playlists, loaded, music_folder_id)
                        }))
                    },
                );
            }
            Route::SmartPlaylistDetail(playlist_id) => {
                let settings = self
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::SmartPlaylistTracks);
                let loaded = Arc::clone(&selected.library);
                let music_folder_id = selected.music_folder_id.clone();
                self.queue_route_projection(
                    &selected,
                    route,
                    vec![(LibraryListKey::SmartPlaylistTracks, settings)],
                    render_started,
                    move || {
                        let detail = load_smart_playlist_detail(
                            &loaded,
                            &playlist_id,
                            music_folder_id.as_ref(),
                        )?;
                        Ok(prepared_route_build(move |shell| {
                            shell.smart_playlist_detail_route(
                                playlist_id,
                                detail,
                                loaded,
                                music_folder_id,
                            )
                        }))
                    },
                );
            }
            Route::Folders { path } => {
                self.invalidate_route_projection_lane();
                self.replace_mounted_route(route, source_id, render_started, 0, || {
                    self.folders_route(path, &selected)
                });
            }
            Route::Albums => {
                let settings = self
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::Albums);
                let loaded = Arc::clone(&selected.library);
                let music_folder_id = selected.music_folder_id.clone();
                self.queue_route_projection(
                    &selected,
                    route,
                    vec![(LibraryListKey::Albums, settings.clone())],
                    render_started,
                    move || {
                        let prepared =
                            load_albums(&loaded, music_folder_id.as_ref(), "", &settings)?;
                        Ok(prepared_route_build(move |shell| {
                            shell.library_albums_route(
                                prepared.source,
                                prepared.details,
                                loaded,
                                music_folder_id,
                            )
                        }))
                    },
                );
            }
            Route::Tracks => {
                let settings = self
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::Tracks);
                let loaded = Arc::clone(&selected.library);
                let music_folder_id = selected.music_folder_id.clone();
                self.queue_route_projection(
                    &selected,
                    route,
                    vec![(LibraryListKey::Tracks, settings.clone())],
                    render_started,
                    move || {
                        let tracks = load_tracks(&loaded, music_folder_id.as_ref(), &settings)?;
                        Ok(prepared_route_build(move |shell| {
                            shell.library_tracks_route(tracks, loaded, music_folder_id)
                        }))
                    },
                );
            }
            Route::Artists | Route::AlbumArtists => {
                let album_artists = matches!(&route, Route::AlbumArtists);
                let key = if album_artists {
                    LibraryListKey::AlbumArtists
                } else {
                    LibraryListKey::Artists
                };
                let settings = self.settings.current.borrow().library_list(key);
                let loaded = Arc::clone(&selected.library);
                let music_folder_id = selected.music_folder_id.clone();
                self.queue_route_projection(
                    &selected,
                    route,
                    vec![(key, settings.clone())],
                    render_started,
                    move || {
                        let artists = load_artists(
                            &loaded,
                            music_folder_id.as_ref(),
                            album_artists,
                            "",
                            &settings,
                        )?
                        .source;
                        Ok(prepared_route_build(move |shell| {
                            shell.library_artist_list_route(
                                album_artists,
                                artists,
                                loaded,
                                music_folder_id,
                            )
                        }))
                    },
                );
            }
            Route::Favorites => {
                let settings = self
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::FavoriteTracks);
                let loaded = Arc::clone(&selected.library);
                let music_folder_id = selected.music_folder_id.clone();
                self.queue_route_projection(
                    &selected,
                    route,
                    vec![(LibraryListKey::FavoriteTracks, settings.clone())],
                    render_started,
                    move || {
                        let tracks =
                            load_favorite_tracks(&loaded, music_folder_id.as_ref(), &settings)?;
                        Ok(prepared_route_build(move |shell| {
                            shell.favorites_route(tracks, loaded, music_folder_id)
                        }))
                    },
                );
            }
            Route::History => {
                let settings = self
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::History);
                let loaded = Arc::clone(&selected.library);
                let music_folder_id = selected.music_folder_id.clone();
                self.queue_route_projection(
                    &selected,
                    route,
                    vec![(LibraryListKey::History, settings)],
                    render_started,
                    move || {
                        let tracks = load_history_tracks(&loaded, music_folder_id.as_ref())?;
                        Ok(prepared_route_build(move |shell| {
                            shell.history_route(tracks, loaded, music_folder_id)
                        }))
                    },
                );
            }
            Route::Genres | Route::Moods => {
                let genres = matches!(&route, Route::Genres);
                let key = if genres {
                    LibraryListKey::Genres
                } else {
                    LibraryListKey::Moods
                };
                let kind = if genres {
                    NamedCollectionKind::Genres
                } else {
                    NamedCollectionKind::Moods
                };
                let settings = self.settings.current.borrow().library_list(key);
                let loaded = Arc::clone(&selected.library);
                let music_folder_id = selected.music_folder_id.clone();
                self.queue_route_projection(
                    &selected,
                    route,
                    vec![(key, settings.clone())],
                    render_started,
                    move || {
                        let items = load_named_collection(
                            &loaded,
                            music_folder_id.as_ref(),
                            kind,
                            "",
                            &settings,
                        )?
                        .source;
                        Ok(prepared_route_build(move |shell| {
                            shell.library_named_collection_route(
                                kind,
                                items,
                                loaded,
                                music_folder_id,
                            )
                        }))
                    },
                );
            }
            Route::Playlists => {
                let settings = self
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::Playlists);
                let loaded = Arc::clone(&selected.library);
                self.queue_route_projection(
                    &selected,
                    route,
                    vec![(LibraryListKey::Playlists, settings.clone())],
                    render_started,
                    move || {
                        let playlists = load_playlists(&loaded, "", &settings)?.source;
                        Ok(prepared_route_build(move |shell| {
                            shell.library_playlists_route(playlists, loaded)
                        }))
                    },
                );
            }
            Route::AlbumDetail(album_id) => {
                let settings = self
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::AlbumDetailTracks);
                let loaded = Arc::clone(&selected.library);
                let music_folder_id = selected.music_folder_id.clone();
                self.queue_route_projection(
                    &selected,
                    route,
                    vec![(LibraryListKey::AlbumDetailTracks, settings.clone())],
                    render_started,
                    move || {
                        let detail = load_album_detail(
                            &loaded,
                            &album_id,
                            music_folder_id.as_ref(),
                            &settings,
                        )?;
                        Ok(prepared_route_build(move |shell| {
                            shell.album_detail_view(album_id, detail, loaded, music_folder_id)
                        }))
                    },
                );
            }
            Route::ArtistDetail(artist_id) => {
                let track_settings = self
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::ArtistTracks);
                let album_settings = self
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::ArtistAlbums);
                let loaded = Arc::clone(&selected.library);
                let music_folder_id = selected.music_folder_id.clone();
                self.queue_route_projection(
                    &selected,
                    route,
                    vec![
                        (LibraryListKey::ArtistTracks, track_settings.clone()),
                        (LibraryListKey::ArtistAlbums, album_settings.clone()),
                    ],
                    render_started,
                    move || {
                        let detail = load_artist_overview(
                            &loaded,
                            &artist_id,
                            music_folder_id.as_ref(),
                            &track_settings,
                            &album_settings,
                        )?;
                        Ok(prepared_route_build(move |shell| {
                            shell.artist_detail_view(artist_id, detail, loaded, music_folder_id)
                        }))
                    },
                );
            }
            Route::ArtistDiscography(artist_id) => {
                let settings = self
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::ArtistAlbums);
                let loaded = Arc::clone(&selected.library);
                let music_folder_id = selected.music_folder_id.clone();
                self.queue_route_projection(
                    &selected,
                    route,
                    vec![(LibraryListKey::ArtistAlbums, settings.clone())],
                    render_started,
                    move || {
                        let detail = load_artist_discography(
                            &loaded,
                            &artist_id,
                            music_folder_id.as_ref(),
                            &settings,
                        )?;
                        Ok(prepared_route_build(move |shell| {
                            shell.artist_discography_view(
                                artist_id,
                                detail,
                                loaded,
                                music_folder_id,
                            )
                        }))
                    },
                );
            }
            Route::ArtistTracks(artist_id) => {
                let settings = self
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::ArtistTracks);
                let loaded = Arc::clone(&selected.library);
                let music_folder_id = selected.music_folder_id.clone();
                self.queue_route_projection(
                    &selected,
                    route,
                    vec![(LibraryListKey::ArtistTracks, settings.clone())],
                    render_started,
                    move || {
                        let detail = load_artist_tracks(
                            &loaded,
                            &artist_id,
                            music_folder_id.as_ref(),
                            &settings,
                        )?;
                        Ok(prepared_route_build(move |shell| {
                            shell.artist_tracks_view(artist_id, detail, loaded, music_folder_id)
                        }))
                    },
                );
            }
            Route::GenreDetail(genre_id) => {
                let settings = self
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::GenreTracks);
                let loaded = Arc::clone(&selected.library);
                let music_folder_id = selected.music_folder_id.clone();
                self.queue_route_projection(
                    &selected,
                    route,
                    vec![(LibraryListKey::GenreTracks, settings.clone())],
                    render_started,
                    move || {
                        let detail = load_genre_detail(
                            &loaded,
                            &genre_id,
                            music_folder_id.as_ref(),
                            &settings,
                        )?;
                        Ok(prepared_route_build(move |shell| {
                            shell.genre_detail_view(genre_id, detail, loaded, music_folder_id)
                        }))
                    },
                );
            }
            Route::MoodDetail(mood_id) => {
                let settings = self
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::MoodTracks);
                let loaded = Arc::clone(&selected.library);
                let music_folder_id = selected.music_folder_id.clone();
                self.queue_route_projection(
                    &selected,
                    route,
                    vec![(LibraryListKey::MoodTracks, settings.clone())],
                    render_started,
                    move || {
                        let detail = load_mood_detail(
                            &loaded,
                            &mood_id,
                            music_folder_id.as_ref(),
                            &settings,
                        )?;
                        Ok(prepared_route_build(move |shell| {
                            shell.mood_detail_view(mood_id, detail, loaded, music_folder_id)
                        }))
                    },
                );
            }
            Route::PlaylistDetail(playlist_id) => {
                let settings = self
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::PlaylistTracks);
                let loaded = Arc::clone(&selected.library);
                self.queue_route_projection(
                    &selected,
                    route,
                    vec![(LibraryListKey::PlaylistTracks, settings.clone())],
                    render_started,
                    move || {
                        let detail = load_playlist_detail(&loaded, &playlist_id)?;
                        let positions = detail
                            .as_ref()
                            .map(|detail| {
                                prepare_playlist_entry_positions(&detail.entries, &settings)
                                    .map_err(|error| error.to_string())
                            })
                            .transpose()?
                            .unwrap_or_default();
                        Ok(prepared_route_build(move |shell| {
                            shell.playlist_detail_route(playlist_id, detail, positions, loaded)
                        }))
                    },
                );
            }
        }
    }

    fn queue_route_projection(
        self: &Rc<Self>,
        selected: &crate::runtime::SelectedLibrary,
        route: Route,
        settings: Vec<(LibraryListKey, LibraryListSettings)>,
        render_started: Instant,
        load: impl FnOnce() -> Result<PreparedRouteBuild, String> + Send + 'static,
    ) {
        let identity = RouteProjectionIdentity {
            selected: SelectedRouteIdentity {
                context: RouteProjectionContext {
                    source_id: selected.source_id.clone(),
                    source_session_epoch: selected.source_session_epoch,
                    route,
                },
                loaded_instance: Arc::as_ptr(&selected.library) as usize,
                music_folder_id: selected.music_folder_id.clone(),
            },
            settings,
        };
        let intent = RouteProjectionIntent {
            load: Box::new(load),
            render_started,
        };
        let queued = self
            .route_viewport
            .projection_lane
            .borrow_mut()
            .submit(identity, intent);
        if let Some(queued) = queued {
            self.start_route_projection(queued);
        }
    }

    fn start_route_projection(
        self: &Rc<Self>,
        queued: QueuedIntent<RouteProjectionIdentity, RouteProjectionIntent>,
    ) {
        let QueuedIntent {
            token: worker_token,
            intent,
        } = queued;
        let shell = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || intent.prepare()).await;
            let Some(shell) = shell.upgrade() else {
                if let Ok(Ok(prepared)) = result {
                    let _ = gio::spawn_blocking(move || drop(prepared)).await;
                }
                return;
            };

            let mut retry_current = false;
            match result {
                Ok(Ok(prepared)) => {
                    let latest = shell
                        .route_viewport
                        .projection_lane
                        .borrow()
                        .should_publish(&worker_token);
                    if latest && shell.route_projection_is_current(&worker_token.key) {
                        shell.mount_prepared_route(&worker_token, prepared);
                    } else {
                        retry_current = latest
                            && shell.route_projection_context_is_current(
                                &worker_token.key.selected.context,
                            );
                        let _ = gio::spawn_blocking(move || drop(prepared)).await;
                    }
                }
                Ok(Err(error)) => {
                    warn!(
                        route = ?worker_token.key.selected.context.route,
                        %error,
                        "failed to read route projection"
                    );
                }
                Err(_) => {
                    warn!(
                        route = ?worker_token.key.selected.context.route,
                        "route projection task panicked"
                    );
                }
            }

            let next = shell
                .route_viewport
                .projection_lane
                .borrow_mut()
                .finish(&worker_token);
            if let Some(next) = next {
                shell.start_route_projection(next);
            } else if retry_current {
                shell.render_current_route_content_inner(true);
            }
        });
    }

    fn route_projection_context_is_current(&self, context: &RouteProjectionContext) -> bool {
        let route = self.navigation.routes.borrow();
        self.library
            .selected
            .borrow()
            .as_ref()
            .is_some_and(|selected| {
                context.matches(
                    &selected.source_id,
                    selected.source_session_epoch,
                    route.current(),
                )
            })
    }

    fn route_projection_is_current(&self, identity: &RouteProjectionIdentity) -> bool {
        if !self.route_projection_context_is_current(&identity.selected.context) {
            return false;
        }
        let selected = self.library.selected.borrow();
        let Some(selected) = selected.as_ref() else {
            return false;
        };
        if !identity.selected.matches(
            self.navigation.routes.borrow().current(),
            &selected.source_id,
            selected.source_session_epoch,
            Arc::as_ptr(&selected.library) as usize,
            selected.music_folder_id.as_ref(),
        ) {
            return false;
        }
        let settings = self.settings.current.borrow();
        identity
            .settings
            .iter()
            .all(|(key, expected)| settings.library_list(*key) == *expected)
    }

    pub(crate) fn mounted_route_read_identity(
        &self,
        route: Route,
        loaded: &Arc<library::Library>,
        music_folder_id: Option<MusicFolderId>,
    ) -> SelectedRouteIdentity {
        let selected = self.library.selected.borrow();
        let selected = selected
            .as_ref()
            .expect("a mounted music route requires one selected Library");
        assert!(
            Arc::ptr_eq(&selected.library, loaded)
                && selected.music_folder_id == music_folder_id
                && self.navigation.routes.borrow().current() == &route,
            "a mounted route read must use its selected Library and scope"
        );
        SelectedRouteIdentity {
            context: RouteProjectionContext {
                route,
                source_id: selected.source_id.clone(),
                source_session_epoch: selected.source_session_epoch,
            },
            loaded_instance: Arc::as_ptr(loaded) as usize,
            music_folder_id,
        }
    }

    pub(crate) fn mounted_route_read_is_current(&self, identity: &SelectedRouteIdentity) -> bool {
        let mounted_route = self.route_viewport.mounted_route.borrow();
        let Some(mounted_route) = mounted_route.as_ref() else {
            return false;
        };
        let selected = self.library.selected.borrow();
        let Some(selected) = selected.as_ref() else {
            return false;
        };
        identity.matches(
            &mounted_route.route,
            &selected.source_id,
            selected.source_session_epoch,
            Arc::as_ptr(&selected.library) as usize,
            selected.music_folder_id.as_ref(),
        ) && self.navigation.routes.borrow().current() == &identity.context.route
    }

    fn invalidate_route_projection_lane(&self) {
        self.route_viewport
            .projection_lane
            .borrow_mut()
            .invalidate();
    }

    fn mount_prepared_route(
        self: &Rc<Self>,
        token: &IntentToken<RouteProjectionIdentity>,
        prepared: PreparedRoute,
    ) {
        let PreparedRoute {
            build,
            render_started,
            projection_ms,
        } = prepared;
        let route = token.key.selected.context.route.clone();
        let source_id = Some(token.key.selected.context.source_id.clone());
        let shell = Rc::clone(self);
        self.replace_mounted_route(route, source_id, render_started, projection_ms, move || {
            build(&shell)
        });
    }

    fn replace_mounted_route(
        self: &Rc<Self>,
        route: Route,
        source_id: Option<SourceId>,
        render_started: Instant,
        projection_ms: u64,
        build: impl FnOnce() -> MountedRoute,
    ) {
        let replacement_started = Instant::now();
        if let Some(previous) = self.begin_mounted_route_replacement() {
            self.favorites.clear_controls();
            self.route_viewport.route_host.remove(&previous.surface);
            drop(previous);
        }
        self.cancel_route_artwork_interaction();
        let teardown_ms = elapsed_ms(replacement_started);

        let model_started = Instant::now();
        let view = build();
        let model_ms = elapsed_ms(model_started);

        let mount_started = Instant::now();
        let widget = view.widget();
        let scroll_adjustment = primary_route_scroll_adjustment(&widget);
        let position_key = source_id
            .filter(|_| scroll_adjustment.is_some())
            .map(|source_id| RoutePositionKey::new(source_id, route.clone()));
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
        let mount_ms = elapsed_ms(mount_started);
        let total_ms = elapsed_ms(render_started);
        debug!(
            ?route,
            projection_ms, teardown_ms, model_ms, mount_ms, total_ms, "route render timing"
        );
        if total_ms >= SLOW_ROUTE_RENDER_MS {
            warn!(
                ?route,
                projection_ms, teardown_ms, model_ms, mount_ms, total_ms, "slow route render"
            );
        }
    }

    pub(crate) fn apply_home_section_to_mounted_route(
        &self,
        kind: HomeSectionKind,
        home: Arc<HomeSnapshot>,
    ) {
        if let Some(view) = self
            .route_viewport
            .mounted_route
            .borrow()
            .as_ref()
            .filter(|entry| entry.route == Route::Home)
            .map(|entry| entry.view.clone())
        {
            view.apply_home_section(kind, home);
        }
    }

    pub(crate) fn apply_library_update_to_mounted_route(&self, update: &SelectedLibraryUpdate) {
        if let Some(view) = self
            .route_viewport
            .mounted_route
            .borrow()
            .as_ref()
            .filter(|entry| entry.route != Route::Home)
            .map(|entry| entry.view.clone())
        {
            view.apply_library_update(update);
        }
    }

    pub(crate) fn reconcile_mounted_route(&self) {
        if let Some(view) = self
            .route_viewport
            .mounted_route
            .borrow()
            .as_ref()
            .map(|entry| entry.view.clone())
        {
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
        self.invalidate_route_projection_lane();
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

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;

    use library::{MusicFolderId, SourceId};
    use playback::SourceSessionEpoch;

    use crate::routes::route::{FolderPathItem, Route};

    use super::{
        LatestIntentLane, LatestMountedRouteRead, RouteProjectionContext, RouteStack,
        SelectedRouteIdentity,
    };

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct ProjectionKey {
        source_id: SourceId,
        source_session_epoch: SourceSessionEpoch,
        route: Route,
    }

    #[test]
    fn route_projection_lane_runs_one_build_and_retains_only_the_latest_intent() {
        let first_key = ProjectionKey {
            source_id: SourceId::new("source:first"),
            source_session_epoch: SourceSessionEpoch::new(1),
            route: Route::Tracks,
        };
        let skipped_key = ProjectionKey {
            source_id: first_key.source_id.clone(),
            source_session_epoch: first_key.source_session_epoch,
            route: Route::Albums,
        };
        let latest_key = ProjectionKey {
            source_id: SourceId::new("source:latest"),
            source_session_epoch: SourceSessionEpoch::new(2),
            route: Route::Artists,
        };
        let mut lane = LatestIntentLane::default();

        let first = lane
            .submit(first_key, "first")
            .expect("first projection starts");
        assert!(lane.should_publish(&first.token));
        assert!(lane.submit(skipped_key, "skipped").is_none());
        assert!(lane.submit(latest_key.clone(), "latest").is_none());
        assert!(!lane.should_publish(&first.token));

        let latest = lane
            .finish(&first.token)
            .expect("latest retained projection starts next");
        assert_eq!(latest.intent, "latest");
        assert_eq!(latest.token.key, latest_key);
        assert!(lane.should_publish(&latest.token));
        assert!(lane.finish(&latest.token).is_none());
    }

    #[test]
    fn route_projection_context_requires_the_same_source_session_and_route() {
        let source_id = SourceId::new("source:selected");
        let context = RouteProjectionContext {
            source_id: source_id.clone(),
            source_session_epoch: SourceSessionEpoch::new(4),
            route: Route::Albums,
        };

        assert!(context.matches(&source_id, SourceSessionEpoch::new(4), &Route::Albums));
        assert!(!context.matches(
            &SourceId::new("source:other"),
            SourceSessionEpoch::new(4),
            &Route::Albums,
        ));
        assert!(!context.matches(&source_id, SourceSessionEpoch::new(5), &Route::Albums));
        assert!(!context.matches(&source_id, SourceSessionEpoch::new(4), &Route::Tracks));
    }

    #[test]
    fn mounted_route_read_requires_the_same_loaded_library_and_scope() {
        let source_id = SourceId::new("source:selected");
        let folder_id = MusicFolderId::new("folder:selected");
        let identity = SelectedRouteIdentity {
            context: RouteProjectionContext {
                route: Route::Albums,
                source_id: source_id.clone(),
                source_session_epoch: SourceSessionEpoch::new(4),
            },
            loaded_instance: 17,
            music_folder_id: Some(folder_id.clone()),
        };

        assert!(identity.matches(
            &Route::Albums,
            &source_id,
            SourceSessionEpoch::new(4),
            17,
            Some(&folder_id),
        ));
        assert!(!identity.matches(
            &Route::Albums,
            &source_id,
            SourceSessionEpoch::new(4),
            18,
            Some(&folder_id),
        ));
        assert!(!identity.matches(
            &Route::Albums,
            &source_id,
            SourceSessionEpoch::new(4),
            17,
            None,
        ));
    }

    #[test]
    fn latest_route_read_applies_only_the_newest_request() {
        let context = gtk::glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let (started_sender, started_receiver) = async_channel::bounded(2);
                    let (release_sender, release_receiver) = async_channel::bounded(2);
                    let (applied_sender, applied_receiver) = async_channel::bounded(2);
                    let load = Arc::new(move |request: &usize| {
                        started_sender
                            .send_blocking(*request)
                            .expect("publish started route read");
                        release_receiver
                            .recv_blocking()
                            .expect("release route read");
                        *request
                    }) as Arc<dyn Fn(&usize) -> usize + Send + Sync>;
                    let apply = Rc::new(move |request: usize, value: usize| {
                        assert_eq!(request, value);
                        applied_sender
                            .try_send(value)
                            .expect("publish applied route read");
                    }) as Rc<dyn Fn(usize, usize)>;
                    let read = LatestMountedRouteRead::new_with_request(apply, load, "test route");

                    read.request_with_if_running(99);
                    assert!(started_receiver.is_empty());
                    read.request_with(0);
                    assert_eq!(started_receiver.recv().await.expect("first route read"), 0);
                    read.request_with_if_running(1);
                    read.request_with_if_running(2);
                    release_sender.send(()).await.expect("release first read");
                    assert_eq!(started_receiver.recv().await.expect("second route read"), 2);
                    assert!(applied_receiver.is_empty());
                    release_sender.send(()).await.expect("release second read");
                    assert_eq!(
                        applied_receiver.recv().await.expect("applied route read"),
                        2
                    );
                    assert!(applied_receiver.is_empty());
                })
            })
            .expect("install route test MainContext");
    }

    #[test]
    fn detached_route_read_does_not_publish() {
        struct DropNotice(async_channel::Sender<()>);

        impl Drop for DropNotice {
            fn drop(&mut self) {
                let _ = self.0.try_send(());
            }
        }

        let context = gtk::glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let (started_sender, started_receiver) = async_channel::bounded(1);
                    let (release_sender, release_receiver) = async_channel::bounded(1);
                    let (dropped_sender, dropped_receiver) = async_channel::bounded(1);
                    let applied = Rc::new(Cell::new(false));
                    let load = Arc::new(move || {
                        started_sender
                            .send_blocking(())
                            .expect("publish started detached read");
                        release_receiver
                            .recv_blocking()
                            .expect("release detached read");
                        DropNotice(dropped_sender.clone())
                    }) as Arc<dyn Fn() -> DropNotice + Send + Sync>;
                    let apply = {
                        let applied = Rc::clone(&applied);
                        Rc::new(move |_value| applied.set(true)) as Rc<dyn Fn(DropNotice)>
                    };
                    let read = LatestMountedRouteRead::new(apply, load, "detached test route");

                    read.request();
                    started_receiver
                        .recv()
                        .await
                        .expect("detached route read started");
                    drop(read);
                    release_sender
                        .send(())
                        .await
                        .expect("release detached route read");
                    dropped_receiver
                        .recv()
                        .await
                        .expect("detached route value dropped");
                    assert!(!applied.get());
                })
            })
            .expect("install detached route test MainContext");
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
    fn route_supports_typed_ids() {
        let album_route = Route::AlbumDetail(library::AlbumId::new("jellyfin:album:abc"));
        let mut stack = RouteStack::new(Route::Home);
        stack.navigate(album_route.clone());
        assert_eq!(stack.current(), &album_route);
    }

    #[test]
    fn folder_routes_keep_history() {
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
