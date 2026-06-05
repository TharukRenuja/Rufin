use super::startup_reveal::{StartupRevealAction, startup_prime_action};
use super::*;

impl Shell {
    pub(in crate::ui) fn render_current_route(self: &Rc<Self>) {
        self.cancel_route_loads();
        self.reset_route_covers();
        self.update_layout();
        self.state.home_section_views.borrow_mut().clear();
        if !self.state.startup_route_revealed.get() && !self.login_screen_active() {
            self.render_startup_loading_view();
            return;
        }
        if self.state.startup_route_render_pending.get() {
            return;
        }
        if self.login_screen_active() {
            clear_favorite_controls(&self.state.favorite_controls);
            while let Some(child) = self.login_host.first_child() {
                self.login_host.remove(&child);
            }
            self.route_title.set_title(&tr("Connect to Music Server"));
            self.set_history_buttons_sensitive(false, false);
            let view = self.add_server_view();
            self.login_host.append(&view);
            return;
        }

        let route = self.state.routes.borrow().current().clone();
        let saved_scroll = self.prepare_route_host(&route);
        let view = match route.clone() {
            Route::Home => self.home_view(),
            Route::Albums => self.library_albums_view(),
            Route::AlbumDetail(album_id) => self.album_detail_view(album_id),
            Route::Tracks => self.library_tracks_route_view(),
            Route::Favorites => self.favorites_view(),
            Route::Artists => self.library_artist_list_view(false),
            Route::ArtistDetail(artist_id) => self.artist_detail_view(artist_id),
            Route::ArtistDiscography(artist_id) => self.artist_discography_view(artist_id),
            Route::ArtistTracks(artist_id) => self.artist_tracks_view(artist_id),
            Route::AlbumArtists => self.library_artist_list_view(true),
            Route::Genres => self.library_genre_list_view(),
            Route::GenreDetail(genre_id) => self.genre_detail_view(genre_id),
            Route::Folders { path } => self.folders_view(path),
            Route::Playlists => self.library_playlists_view(),
            Route::PlaylistDetail(playlist_id) => self.playlist_detail_view(playlist_id),
            Route::SmartPlaylists => self.library_smart_playlists_view(),
            Route::SmartPlaylistDetail(smart_playlist_id) => {
                self.smart_playlist_detail_view(smart_playlist_id)
            }
            Route::Search { query, .. } => {
                let library = self.state.library.borrow().clone();
                self.search_view(&query, library)
            }
        };

        self.finish_route_view(&route, view, saved_scroll);
    }
    pub(in crate::ui) fn request_current_route_render(self: &Rc<Self>) {
        let route = self.state.routes.borrow().current().clone();
        let Some(kind) = route_load_kind(&route) else {
            self.render_current_route();
            return;
        };

        if self.login_screen_active()
            || !self.state.startup_route_revealed.get()
            || self.state.startup_route_render_pending.get()
        {
            self.render_current_route();
            return;
        }

        let generation = self.next_route_load_generation();
        let server_id = self.active_route_server_id();
        let selected_source = self.active_route_source();
        let settings = self.library_settings(kind.key());
        self.render_route_loading(&route);

        let controller = self.controller.clone();
        let route_for_thread = route.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = load_route_data(&controller, kind, settings);
            let _sent = sender.send(RouteLoadMessage {
                generation,
                route: route_for_thread,
                server_id,
                selected_source,
                result,
            });
        });

        let shell = Rc::clone(self);
        glib::timeout_add_local(Duration::from_millis(50), move || {
            match receiver.try_recv() {
                Ok(message) => {
                    shell.apply_route_load_message(message);
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });
    }
    pub(in crate::ui) fn render_current_route_preserving_scroll(self: &Rc<Self>) {
        let route = self.state.routes.borrow().current().clone();
        let scroll_value = self.current_route_scroll_value();
        if let Some(value) = scroll_value {
            self.store_route_scroll(&route, Some(value));
        }
        if route_load_kind(&route).is_some()
            && !self.login_screen_active()
            && self.state.startup_route_revealed.get()
            && !self.state.startup_route_render_pending.get()
        {
            self.request_current_route_render();
            return;
        }
        self.render_current_route();
        if let Some(value) = scroll_value {
            self.restore_current_route_scroll(value);
        }
    }
    pub(in crate::ui) fn apply_library_delta(self: &Rc<Self>, delta: LibraryDelta) {
        if delta.is_empty() {
            return;
        }
        if self.login_screen_active()
            || !self.state.startup_route_revealed.get()
            || self.state.startup_route_render_pending.get()
        {
            return;
        }

        let route = self.state.routes.borrow().current().clone();
        if !route_delta_affects(&route, &delta) {
            return;
        }
        if matches!(route, Route::Home) {
            self.state.home_refresh_started_for_visit.set(false);
            reset_home_section_pages(&mut self.state.home_section_state.borrow_mut());
        }
        if matches!(route, Route::Playlists) {
            self.state.playlist_refresh_started_for_visit.set(false);
        }
        if let Route::Search { query, .. } = &route {
            match self.controller.cached_search_results(query, 50) {
                Ok(results) => {
                    self.state.library.borrow_mut().search = results;
                }
                Err(error) => {
                    warn!(%error, "failed to refresh cached search results after sync");
                }
            }
        }
        self.render_current_route_preserving_scroll();
        if matches!(route, Route::Home) {
            self.refresh_home_for_current_visit();
        } else if matches!(route, Route::Playlists) {
            self.refresh_playlists_for_current_visit();
        }
    }
    fn render_route_loading(self: &Rc<Self>, route: &Route) {
        self.reset_route_covers();
        self.update_layout();
        self.state.home_section_views.borrow_mut().clear();
        self.prepare_route_host(route);
        self.route_host
            .append(&route_boundary_for_route(route, self.route_loading_view()));
    }

    fn route_loading_view(&self) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 8);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(24);
        wrapper.set_margin_bottom(super::library::LIBRARY_ROUTE_BOTTOM_MARGIN);
        wrapper.set_hexpand(true);
        wrapper.set_vexpand(true);
        wrapper.set_halign(gtk::Align::Center);
        wrapper.set_valign(gtk::Align::Center);

        let spinner = gtk::Spinner::new();
        spinner.start();
        wrapper.append(&spinner);

        let label = gtk::Label::new(Some(&tr("Loading…")));
        label.add_css_class("dim-label");
        wrapper.append(&label);
        wrapper.upcast()
    }

    fn apply_route_load_message(self: &Rc<Self>, message: RouteLoadMessage) {
        if self.state.route_load_generation.get() != message.generation {
            return;
        }
        if self.state.routes.borrow().current() != &message.route {
            return;
        }
        if self.active_route_server_id() != message.server_id {
            return;
        }
        if self.active_route_source() != message.selected_source {
            return;
        }

        match message.result {
            Ok(data) => self.render_route_when_covers_ready(
                message.generation,
                message.route,
                message.server_id,
                message.selected_source,
                data,
            ),
            Err(error) => {
                warn!(%error, route = ?message.route, "failed to load route data");
                self.show_preferences_toast(&error);
                self.render_current_route();
            }
        }
    }

    fn render_route_when_covers_ready(
        self: &Rc<Self>,
        route_generation: u64,
        route: Route,
        server_id: Option<ServerId>,
        selected_source: Option<rufin_core::LibrarySourceSelection>,
        data: RouteLoadData,
    ) {
        let saved_scroll = self.open_route_scroll(&route);
        let Some(cover_generation) = self.begin_route_cover_prime(route_load_cover_targets(
            self.as_ref(),
            &data,
            saved_scroll,
        )) else {
            self.render_loaded_route(route, data);
            return;
        };

        let shell = Rc::clone(self);
        let started_at = Instant::now();
        let timeout_logged = Rc::new(Cell::new(false));
        let mut data = Some(data);
        glib::timeout_add_local(Duration::from_millis(PRIME_POLL_MS), move || {
            if shell.state.route_load_generation.get() != route_generation
                || shell.state.routes.borrow().current() != &route
                || shell.active_route_server_id() != server_id
                || shell.active_route_source() != selected_source
            {
                shell.cancel_route_cover_prime();
                return glib::ControlFlow::Break;
            }

            shell.reconcile_route_cover_prime_pending();
            let pending_covers =
                if shell.state.route_cover_prime_generation.get() == cover_generation {
                    shell.state.route_cover_prime_pending.borrow().len()
                } else {
                    0
                };
            match startup_prime_action(pending_covers, started_at.elapsed()) {
                StartupRevealAction::RevealReady => {
                    shell.finish_route_cover_prime(cover_generation);
                    if let Some(data) = data.take() {
                        shell.render_loaded_route(route.clone(), data);
                    }
                    glib::ControlFlow::Break
                }
                StartupRevealAction::RevealExpired => {
                    if pending_covers > 0 && !timeout_logged.replace(true) {
                        debug!(
                            pending_covers,
                            route = ?route,
                            "route cover prime expired"
                        );
                    }
                    shell.finish_route_cover_prime(cover_generation);
                    if let Some(data) = data.take() {
                        shell.render_loaded_route(route.clone(), data);
                    }
                    glib::ControlFlow::Break
                }
                StartupRevealAction::Wait => glib::ControlFlow::Continue,
            }
        });
    }

    fn render_loaded_route(self: &Rc<Self>, route: Route, data: RouteLoadData) {
        let saved_scroll = self.prepare_route_host(&route);
        let view = match data {
            RouteLoadData::Albums(page) => self.library_albums_view_from_page(page),
            RouteLoadData::Tracks(page) => self.library_tracks_route_view_from_page(page),
            RouteLoadData::Artists { album_artist, page } => {
                self.library_artist_list_view_from_page(album_artist, page)
            }
            RouteLoadData::Genres(page) => self.library_genre_list_view_from_page(page),
            RouteLoadData::Playlists(page) => self.library_playlists_view_from_page(page),
        };
        self.finish_route_view(&route, view, saved_scroll);
    }

    fn prepare_route_host(self: &Rc<Self>, route: &Route) -> Option<f64> {
        clear_favorite_controls(&self.state.favorite_controls);
        while let Some(child) = self.route_host.first_child() {
            self.route_host.remove(&child);
        }
        self.mark_current_route_open(route);
        self.route_title.set_title(&tr(route.title()));
        self.set_history_buttons_sensitive(
            self.state.routes.borrow().can_back(),
            self.state.routes.borrow().can_forward(),
        );
        update_navigation_selection(self.as_ref());
        if route_uses_responsive_cards(route) {
            self.state
                .responsive_route_render_width
                .set(route_content_width(self.as_ref()));
        } else {
            self.state.responsive_route_render_width.set(0);
        }
        self.open_route_scroll(route)
    }

    fn finish_route_view(
        self: &Rc<Self>,
        route: &Route,
        view: gtk::Widget,
        saved_scroll: Option<f64>,
    ) {
        self.route_host
            .append(&route_boundary_for_route(route, view));
        if let Some(value) = saved_scroll {
            self.restore_current_route_scroll(value);
        }
        self.prime_route_visible_cover_window(route);
        {
            let shell = Rc::clone(self);
            let route = route.clone();
            glib::idle_add_local_once(move || {
                shell.prime_route_visible_cover_window(&route);
            });
        }
    }

    fn next_route_load_generation(&self) -> u64 {
        let generation = self.state.route_load_generation.get().saturating_add(1);
        self.state.route_load_generation.set(generation);
        generation
    }

    fn cancel_route_loads(&self) {
        self.state
            .route_load_generation
            .set(self.state.route_load_generation.get().saturating_add(1));
        self.cancel_route_cover_prime();
    }

    fn begin_route_cover_prime(self: &Rc<Self>, targets: Vec<CoverWarmTarget>) -> Option<u64> {
        if targets.is_empty() {
            return None;
        }

        let generation = self
            .state
            .route_cover_prime_generation
            .get()
            .saturating_add(1);
        self.state.route_cover_prime_generation.set(generation);
        self.state.route_cover_prime_pending.borrow_mut().clear();

        let jobs = startup_cover_jobs_from_targets(self.as_ref(), targets, None);
        let mut pending = HashSet::new();
        for job in jobs {
            if self.state.cover_unavailable.borrow().contains(&job.key) {
                continue;
            }
            pending.insert(job.key.clone());
            self.start_cached_cover_path_lookup(CoverPathLookupRequest {
                key: job.key,
                image_ref: job.image_ref,
                fetch_size: job.fetch_size,
                size: job.size,
                intent: CoverPathLookupIntent::RoutePrime,
            });
        }
        *self.state.route_cover_prime_pending.borrow_mut() = pending;
        self.reconcile_route_cover_prime_pending();
        if self.state.route_cover_prime_pending.borrow().is_empty() {
            None
        } else {
            Some(generation)
        }
    }

    fn finish_route_cover_prime(&self, generation: u64) {
        if self.state.route_cover_prime_generation.get() != generation {
            return;
        }
        self.cancel_route_cover_prime();
    }

    fn cancel_route_cover_prime(&self) {
        self.state.route_cover_prime_generation.set(
            self.state
                .route_cover_prime_generation
                .get()
                .saturating_add(1),
        );
        self.state.route_cover_prime_pending.borrow_mut().clear();
    }

    fn active_route_server_id(&self) -> Option<ServerId> {
        self.state
            .library
            .borrow()
            .server
            .as_ref()
            .map(|server| server.id.clone())
    }

    fn active_route_source(&self) -> Option<rufin_core::LibrarySourceSelection> {
        self.state.library.borrow().selected_source.clone()
    }

    pub(in crate::ui) fn current_route_scroll_value(&self) -> Option<f64> {
        find_largest_scrolled_window(&self.route_host.clone().upcast())
            .map(|scroller| scroller.vadjustment().value())
    }
    pub(in crate::ui) fn restore_current_route_scroll(&self, value: f64) {
        let route_host = self.route_host.clone();
        glib::idle_add_local_once(move || {
            restore_scrolled_window_value(&route_host.clone().upcast(), value);
            glib::timeout_add_local_once(Duration::from_millis(16), move || {
                restore_scrolled_window_value(&route_host.clone().upcast(), value);
            });
        });
    }

    pub(in crate::ui) fn save_current_route_state(&self) {
        let route = self.state.routes.borrow().current().clone();
        let scroll = self.current_route_scroll_value();
        self.store_route_scroll(&route, scroll);
    }

    fn store_route_scroll(&self, route: &Route, scroll: Option<f64>) {
        let key = route_key(route);
        self.state
            .open_routes
            .borrow_mut()
            .entry(key)
            .and_modify(|state| {
                if scroll.is_some() {
                    state.scroll = scroll;
                }
            })
            .or_insert(OpenRouteState { scroll });
    }

    fn mark_current_route_open(&self, route: &Route) {
        let key = route_key(route);
        self.state
            .open_routes
            .borrow_mut()
            .entry(key)
            .or_insert(OpenRouteState { scroll: None });
    }

    fn open_route_scroll(&self, route: &Route) -> Option<f64> {
        self.state
            .open_routes
            .borrow()
            .get(&route_key(route))
            .and_then(|state| state.scroll)
    }
}

fn route_key(route: &Route) -> String {
    format!("{route:?}")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RouteLoadKind {
    Albums,
    Tracks,
    Artists { album_artist: bool },
    Genres,
    Playlists,
}

impl RouteLoadKind {
    fn key(self) -> LibraryListKey {
        match self {
            Self::Albums => LibraryListKey::Albums,
            Self::Tracks => LibraryListKey::Tracks,
            Self::Artists { album_artist: true } => LibraryListKey::AlbumArtists,
            Self::Artists {
                album_artist: false,
            } => LibraryListKey::Artists,
            Self::Genres => LibraryListKey::Genres,
            Self::Playlists => LibraryListKey::Playlists,
        }
    }
}

enum RouteLoadData {
    Albums(rufin_provider::PagedResponse<Album>),
    Tracks(rufin_provider::PagedResponse<Track>),
    Artists {
        album_artist: bool,
        page: rufin_provider::PagedResponse<Artist>,
    },
    Genres(rufin_provider::PagedResponse<Genre>),
    Playlists(rufin_provider::PagedResponse<Playlist>),
}

struct RouteLoadMessage {
    generation: u64,
    route: Route,
    server_id: Option<ServerId>,
    selected_source: Option<rufin_core::LibrarySourceSelection>,
    result: Result<RouteLoadData, String>,
}

fn route_load_kind(route: &Route) -> Option<RouteLoadKind> {
    match route {
        Route::Albums => Some(RouteLoadKind::Albums),
        Route::Tracks => Some(RouteLoadKind::Tracks),
        Route::Artists => Some(RouteLoadKind::Artists {
            album_artist: false,
        }),
        Route::AlbumArtists => Some(RouteLoadKind::Artists { album_artist: true }),
        Route::Genres => Some(RouteLoadKind::Genres),
        Route::Playlists => Some(RouteLoadKind::Playlists),
        _ => None,
    }
}

fn load_route_data(
    controller: &AppController,
    kind: RouteLoadKind,
    settings: LibraryListSettings,
) -> Result<RouteLoadData, String> {
    let load_complete = kind.key().supports_layout(settings.layout);
    match kind {
        RouteLoadKind::Albums => {
            let page = controller.cached_albums_page(0, GRID_ROUTE_PAGE_SIZE)?;
            let page = super::library::complete_cached_page(
                page,
                load_complete,
                |limit| controller.cached_albums_page(0, limit),
                "albums",
            );
            Ok(RouteLoadData::Albums(page))
        }
        RouteLoadKind::Tracks => {
            let page = controller.cached_tracks_page(0, TRACK_ROUTE_PAGE_SIZE)?;
            let page = super::library::complete_cached_page(
                page,
                load_complete,
                |limit| controller.cached_tracks_page(0, limit),
                "tracks",
            );
            Ok(RouteLoadData::Tracks(page))
        }
        RouteLoadKind::Artists { album_artist } => {
            let page = controller.cached_artists_page(album_artist, 0, GRID_ROUTE_PAGE_SIZE)?;
            let page = super::library::complete_cached_page(
                page,
                load_complete,
                |limit| controller.cached_artists_page(album_artist, 0, limit),
                "artists",
            );
            Ok(RouteLoadData::Artists { album_artist, page })
        }
        RouteLoadKind::Genres => {
            let page = controller.cached_genres_page(0, GRID_ROUTE_PAGE_SIZE)?;
            let page = super::library::complete_cached_page(
                page,
                load_complete,
                |limit| controller.cached_genres_page(0, limit),
                "genres",
            );
            Ok(RouteLoadData::Genres(page))
        }
        RouteLoadKind::Playlists => {
            let page = controller.cached_playlists_page(0, GRID_ROUTE_PAGE_SIZE)?;
            let page = super::library::complete_cached_page(
                page,
                load_complete,
                |limit| controller.cached_playlists_page(0, limit),
                "playlists",
            );
            Ok(RouteLoadData::Playlists(page))
        }
    }
}

fn route_load_cover_targets(
    shell: &Shell,
    data: &RouteLoadData,
    saved_scroll: Option<f64>,
) -> Vec<CoverWarmTarget> {
    match data {
        RouteLoadData::Albums(page) => {
            let settings = shell.library_settings(LibraryListKey::Albums);
            let Some((fetch_size, size)) = cover_prime_sizes(shell, &settings) else {
                return Vec::new();
            };
            let mut albums = page.items.clone();
            super::library::sort_albums(&mut albums, &settings);
            let (start, end) =
                route_load_visible_range(shell, &settings, albums.len(), saved_scroll);
            albums[start..end]
                .iter()
                .filter_map(|album| album.image_ref.clone())
                .map(|image_ref| CoverWarmTarget {
                    image_ref,
                    fetch_size,
                    size,
                })
                .collect()
        }
        RouteLoadData::Tracks(page) => {
            let settings = shell.library_settings(LibraryListKey::Tracks);
            let Some((fetch_size, size)) = cover_prime_sizes(shell, &settings) else {
                return Vec::new();
            };
            let tracks = super::library::tracks_for_settings(&page.items, &settings, "", false);
            let (start, end) =
                route_load_visible_range(shell, &settings, tracks.len(), saved_scroll);
            tracks[start..end]
                .iter()
                .filter_map(|track| track.image_ref.clone())
                .map(|image_ref| CoverWarmTarget {
                    image_ref,
                    fetch_size,
                    size,
                })
                .collect()
        }
        RouteLoadData::Artists { album_artist, page } => {
            let key = if *album_artist {
                LibraryListKey::AlbumArtists
            } else {
                LibraryListKey::Artists
            };
            let settings = shell.library_settings(key);
            let Some((fetch_size, size)) = cover_prime_sizes(shell, &settings) else {
                return Vec::new();
            };
            let mut artists = page.items.clone();
            super::library::sort_artists(&mut artists, &settings);
            let (start, end) =
                route_load_visible_range(shell, &settings, artists.len(), saved_scroll);
            artists[start..end]
                .iter()
                .filter_map(|artist| artist.image_ref.clone())
                .map(|image_ref| CoverWarmTarget {
                    image_ref,
                    fetch_size,
                    size,
                })
                .collect()
        }
        RouteLoadData::Genres(page) => {
            let settings = shell.library_settings(LibraryListKey::Genres);
            let Some((fetch_size, size)) = cover_prime_sizes(shell, &settings) else {
                return Vec::new();
            };
            let mut genres = page.items.clone();
            super::library::sort_genres(&mut genres, &settings);
            let (start, end) =
                route_load_visible_range(shell, &settings, genres.len(), saved_scroll);
            genres[start..end]
                .iter()
                .flat_map(|genre| {
                    let mut refs = genre.image_refs.clone();
                    refs.extend(genre.image_ref.iter().cloned());
                    refs
                })
                .map(|image_ref| CoverWarmTarget {
                    image_ref,
                    fetch_size,
                    size,
                })
                .collect()
        }
        RouteLoadData::Playlists(page) => {
            let settings = shell.library_settings(LibraryListKey::Playlists);
            let Some((fetch_size, size)) = cover_prime_sizes(shell, &settings) else {
                return Vec::new();
            };
            let mut playlists = page.items.clone();
            super::library::sort_playlists(&mut playlists, &settings);
            let (start, end) =
                route_load_visible_range(shell, &settings, playlists.len(), saved_scroll);
            playlists[start..end]
                .iter()
                .flat_map(|playlist| {
                    let mut refs = playlist.image_refs.clone();
                    refs.extend(playlist.image_ref.iter().cloned());
                    refs
                })
                .map(|image_ref| CoverWarmTarget {
                    image_ref,
                    fetch_size,
                    size,
                })
                .collect()
        }
    }
}

fn route_load_visible_range(
    shell: &Shell,
    settings: &LibraryListSettings,
    total: usize,
    saved_scroll: Option<f64>,
) -> (usize, usize) {
    let Some(offset) = saved_scroll else {
        return visible_index_range(shell, total, settings.layout);
    };
    let page_size = f64::from(
        shell
            .route_host
            .height()
            .max(shell.app_root.height())
            .max(1),
    );
    let (columns, card_size) = shell.responsive_card_grid_metrics();
    visible_index_range_from_metrics(
        total,
        settings.layout,
        offset,
        page_size,
        super::library::LIBRARY_TABLE_ROW_HEIGHT.max(1),
        columns,
        card_size,
    )
}

fn route_delta_affects(route: &Route, delta: &LibraryDelta) -> bool {
    if delta.reset.is_some() {
        return true;
    }
    match route {
        Route::Home => delta.home_changed,
        Route::Favorites => {
            !delta.tracks.added.is_empty()
                || !delta.tracks.deleted.is_empty()
                || !delta.tracks.favorite.is_empty()
                || !delta.tracks.fields.is_empty()
                || !delta.tracks.cover_refs.is_empty()
        }
        Route::Albums => {
            !delta.albums.added.is_empty()
                || !delta.albums.deleted.is_empty()
                || !delta.albums.fields.is_empty()
                || !delta.albums.links.is_empty()
                || !delta.albums.cover_refs.is_empty()
        }
        Route::AlbumDetail(album_id) => {
            delta.albums.added.contains(album_id)
                || delta.albums.deleted.contains(album_id)
                || delta.albums.fields.contains(album_id)
                || delta.albums.stats.contains(album_id)
                || delta.albums.links.contains(album_id)
                || delta.albums.cover_refs.contains(album_id)
                || !delta.tracks.added.is_empty()
                || !delta.tracks.deleted.is_empty()
                || !delta.tracks.fields.is_empty()
                || !delta.tracks.favorite.is_empty()
                || !delta.tracks.cover_refs.is_empty()
        }
        Route::Tracks => !delta.tracks.is_empty(),
        Route::Artists => {
            !delta.artists.added.is_empty()
                || !delta.artists.deleted.is_empty()
                || !delta.artists.fields.is_empty()
                || !delta.artists.links.is_empty()
                || !delta.artists.cover_refs.is_empty()
        }
        Route::ArtistDetail(artist_id)
        | Route::ArtistDiscography(artist_id)
        | Route::ArtistTracks(artist_id) => {
            delta.artists.added.contains(artist_id)
                || delta.artists.deleted.contains(artist_id)
                || delta.artists.fields.contains(artist_id)
                || delta.artists.stats.contains(artist_id)
                || delta.artists.links.contains(artist_id)
                || delta.artists.cover_refs.contains(artist_id)
                || !delta.albums.is_empty()
                || !delta.tracks.is_empty()
        }
        Route::AlbumArtists => {
            !delta.album_artists.added.is_empty()
                || !delta.album_artists.deleted.is_empty()
                || !delta.album_artists.fields.is_empty()
                || !delta.album_artists.links.is_empty()
                || !delta.album_artists.cover_refs.is_empty()
        }
        Route::Genres => {
            !delta.genres.added.is_empty()
                || !delta.genres.deleted.is_empty()
                || !delta.genres.fields.is_empty()
                || !delta.genres.links.is_empty()
                || !delta.genres.cover_refs.is_empty()
        }
        Route::GenreDetail(genre_id) => {
            delta.genres.added.contains(genre_id)
                || delta.genres.deleted.contains(genre_id)
                || delta.genres.fields.contains(genre_id)
                || delta.genres.stats.contains(genre_id)
                || delta.genres.links.contains(genre_id)
                || delta.genres.cover_refs.contains(genre_id)
                || !delta.tracks.is_empty()
                || !delta.albums.is_empty()
        }
        Route::Folders { .. } => delta.folders_changed || !delta.tracks.is_empty(),
        Route::Playlists => {
            !delta.playlists.added.is_empty()
                || !delta.playlists.deleted.is_empty()
                || !delta.playlists.fields.is_empty()
                || !delta.playlists.cover_refs.is_empty()
        }
        Route::PlaylistDetail(playlist_id) => {
            delta.playlists.added.contains(playlist_id)
                || delta.playlists.deleted.contains(playlist_id)
                || delta.playlists.fields.contains(playlist_id)
                || delta.playlists.entries.contains(playlist_id)
                || delta.playlists.cover_refs.contains(playlist_id)
                || !delta.tracks.is_empty()
        }
        Route::SmartPlaylists | Route::SmartPlaylistDetail(_) => !delta.tracks.is_empty(),
        Route::Search { .. } => {
            !delta.tracks.is_empty()
                || !delta.albums.is_empty()
                || !delta.artists.is_empty()
                || !delta.album_artists.is_empty()
                || !delta.genres.is_empty()
                || !delta.playlists.is_empty()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_routes_use_deferred_content_load() {
        assert_eq!(route_load_kind(&Route::Albums), Some(RouteLoadKind::Albums));
        assert_eq!(route_load_kind(&Route::Tracks), Some(RouteLoadKind::Tracks));
        assert_eq!(
            route_load_kind(&Route::Artists),
            Some(RouteLoadKind::Artists {
                album_artist: false,
            }),
        );
        assert_eq!(
            route_load_kind(&Route::AlbumArtists),
            Some(RouteLoadKind::Artists { album_artist: true }),
        );
        assert_eq!(route_load_kind(&Route::Genres), Some(RouteLoadKind::Genres));
        assert_eq!(
            route_load_kind(&Route::Playlists),
            Some(RouteLoadKind::Playlists),
        );

        assert_eq!(route_load_kind(&Route::Home), None);
        assert_eq!(
            route_load_kind(&Route::AlbumDetail(AlbumId::new("album"))),
            None,
        );
        assert_eq!(
            route_load_kind(&Route::PlaylistDetail(PlaylistId::new("playlist"))),
            None,
        );
    }
}
