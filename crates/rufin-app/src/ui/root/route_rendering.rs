use super::*;

const SLOW_ROUTE_RENDER_MS: u64 = 100;
const SLOW_ROUTE_RENDER_IDLE_MS: u64 = 100;
const SLOW_SYNC_ROUTE_REFRESH_MS: u64 = 100;

struct RouteView {
    widget: gtk::Widget,
    resize: RouteResizePolicy,
}

impl RouteView {
    fn new(widget: gtk::Widget) -> Self {
        Self {
            widget,
            resize: RouteResizePolicy::LayoutSignature,
        }
    }

    fn settled_width(widget: gtk::Widget) -> Self {
        Self {
            widget,
            resize: RouteResizePolicy::SettledWidth,
        }
    }

    fn with_resize(mut self, resize: RouteResizePolicy) -> Self {
        self.resize = resize;
        self
    }
}

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
            self.show_reconnect_notice_if_needed();
            return;
        }

        self.render_current_route_content();
    }
    pub(in crate::ui) fn request_current_route_render(self: &Rc<Self>) {
        self.render_current_route();
    }
    pub(in crate::ui) fn render_current_route_preserving_scroll(self: &Rc<Self>) {
        self.render_current_route();
    }
    pub(in crate::ui) fn render_current_route_content(self: &Rc<Self>) {
        let route = self.state.routes.borrow().current().clone();
        let render_started = Instant::now();
        let prepare_started = Instant::now();
        self.prepare_route_host(&route);
        let prepare_ms = prepare_started.elapsed().as_millis() as u64;
        let view_started = Instant::now();
        let view = match route.clone() {
            Route::Home => RouteView::new(self.home_view()),
            Route::Albums => RouteView::new(self.library_albums_view())
                .with_resize(self.library_route_resize_policy(LibraryListKey::Albums)),
            Route::AlbumDetail(album_id) => {
                RouteView::settled_width(self.album_detail_view(album_id))
            }
            Route::Tracks => RouteView::new(self.library_tracks_route_view())
                .with_resize(self.library_route_resize_policy(LibraryListKey::Tracks)),
            Route::Favorites => RouteView::new(self.favorites_view())
                .with_resize(self.library_route_resize_policy(LibraryListKey::FavoriteTracks)),
            Route::Artists => RouteView::new(self.library_artist_list_view(false))
                .with_resize(self.library_route_resize_policy(LibraryListKey::Artists)),
            Route::ArtistDetail(artist_id) => {
                RouteView::settled_width(self.artist_detail_view(artist_id))
            }
            Route::ArtistDiscography(artist_id) => {
                RouteView::settled_width(self.artist_discography_view(artist_id))
            }
            Route::ArtistTracks(artist_id) => RouteView::new(self.artist_tracks_view(artist_id))
                .with_resize(self.library_route_resize_policy(LibraryListKey::ArtistTracks)),
            Route::AlbumArtists => RouteView::new(self.library_artist_list_view(true))
                .with_resize(self.library_route_resize_policy(LibraryListKey::AlbumArtists)),
            Route::Genres => RouteView::new(self.library_genre_list_view())
                .with_resize(self.library_route_resize_policy(LibraryListKey::Genres)),
            Route::GenreDetail(genre_id) => {
                RouteView::settled_width(self.genre_detail_view(genre_id))
            }
            Route::Folders { path } => RouteView::settled_width(self.folders_view(path)),
            Route::Playlists => RouteView::new(self.library_playlists_view())
                .with_resize(self.library_route_resize_policy(LibraryListKey::Playlists)),
            Route::PlaylistDetail(playlist_id) => {
                RouteView::settled_width(self.playlist_detail_view(playlist_id))
            }
            Route::SmartPlaylists => RouteView::new(self.library_smart_playlists_view())
                .with_resize(self.library_route_resize_policy(LibraryListKey::SmartPlaylists)),
            Route::SmartPlaylistDetail(smart_playlist_id) => {
                RouteView::settled_width(self.smart_playlist_detail_view(smart_playlist_id))
            }
            Route::Search { query, .. } => {
                let library = self.state.library.borrow().clone();
                RouteView::settled_width(self.search_view(&query, library))
            }
        };

        let resize = view.resize;
        let build_ms = render_started.elapsed().as_millis() as u64;
        let view_ms = view_started.elapsed().as_millis() as u64;
        let finish_started = Instant::now();
        self.finish_route_view(&route, view);
        let finish_ms = finish_started.elapsed().as_millis() as u64;
        let total_ms = render_started.elapsed().as_millis() as u64;
        self.log_post_render_idle_delay(route.clone(), total_ms);
        if total_ms >= SLOW_ROUTE_RENDER_MS {
            warn!(
                ?route,
                ?resize,
                build_ms,
                prepare_ms,
                view_ms,
                finish_ms,
                total_ms,
                signature = self.state.responsive_render_signature.get(),
                "slow route render"
            );
        }
        if route_resize_diagnostics_enabled() {
            info!(
                ?route,
                ?resize,
                build_ms,
                prepare_ms,
                view_ms,
                finish_ms,
                total_ms,
                signature = self.state.responsive_render_signature.get(),
                "route render timing"
            );
        }
    }
    pub(in crate::ui) fn apply_library_delta(self: &Rc<Self>, delta: LibraryDelta) {
        let route_ready = sync_route_surface_ready(
            self.login_screen_active(),
            self.state.startup_route_revealed.get(),
            self.state.startup_route_render_pending.get(),
        );
        let route = self.state.routes.borrow().current().clone();
        if !sync_delta_refreshes_route(&route, &delta, route_ready) {
            return;
        }

        self.queue_sync_route_refresh(route, delta);
    }

    fn queue_sync_route_refresh(self: &Rc<Self>, route: Route, delta: LibraryDelta) {
        merge_pending_sync_route_delta(
            &mut self.state.pending_sync_route_delta.borrow_mut(),
            delta,
        );
        let route_generation = self.state.route_load_generation.get();
        if self
            .state
            .pending_sync_route_refresh
            .borrow()
            .as_ref()
            .is_some_and(|(queued_route, queued_generation)| {
                queued_route == &route && *queued_generation == route_generation
            })
        {
            return;
        }

        self.state
            .pending_sync_route_refresh
            .replace(Some((route.clone(), route_generation)));
        let shell = Rc::clone(self);
        glib::idle_add_local_once(move || {
            shell.run_sync_route_refresh(route, route_generation);
        });
    }

    fn run_sync_route_refresh(self: &Rc<Self>, route: Route, route_generation: u64) {
        let refresh_started = Instant::now();
        let pending_matches = pending_sync_route_refresh_matches(
            self.state.pending_sync_route_refresh.borrow().as_ref(),
            &route,
            route_generation,
        );
        if !pending_matches {
            return;
        }
        self.state.pending_sync_route_refresh.borrow_mut().take();
        if !sync_route_refresh_target_matches(
            &route,
            route_generation,
            self.state.routes.borrow().current(),
            self.state.route_load_generation.get(),
        ) {
            self.state.pending_sync_route_delta.borrow_mut().take();
            return;
        }

        let Some(delta) = self.state.pending_sync_route_delta.borrow_mut().take() else {
            return;
        };
        if !route_delta_affects(&route, &delta) {
            return;
        }
        if matches!(route, Route::Home) {
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
        let render_started = Instant::now();
        self.render_current_route_preserving_scroll();
        let render_ms = render_started.elapsed().as_millis() as u64;
        if matches!(route, Route::Playlists) {
            self.refresh_playlists_for_current_visit();
        }
        let total_ms = refresh_started.elapsed().as_millis() as u64;
        if total_ms >= SLOW_SYNC_ROUTE_REFRESH_MS {
            warn!(
                ?route,
                route_generation, render_ms, total_ms, "slow queued sync route refresh"
            );
        }
    }
    fn log_post_render_idle_delay(self: &Rc<Self>, route: Route, render_ms: u64) {
        let shell = Rc::clone(self);
        let route_generation = self.state.route_load_generation.get();
        let queued_at = Instant::now();
        glib::idle_add_local_once(move || {
            let idle_ms = queued_at.elapsed().as_millis() as u64;
            if idle_ms >= SLOW_ROUTE_RENDER_IDLE_MS {
                warn!(
                    ?route,
                    route_generation,
                    render_ms,
                    idle_ms,
                    current_route = ?shell.state.routes.borrow().current().clone(),
                    current_generation = shell.state.route_load_generation.get(),
                    "route post-render idle delayed"
                );
            }
        });
    }
    fn prepare_route_host(self: &Rc<Self>, route: &Route) {
        clear_favorite_controls(&self.state.favorite_controls);
        self.state.type_to_search.borrow_mut().take();
        self.state.current_route_boundary.borrow_mut().take();
        self.state.column_view_width_fits.borrow_mut().clear();
        while let Some(child) = self.route_host.first_child() {
            self.route_host.remove(&child);
        }
        self.route_title.set_title(&tr(route.title()));
        self.set_history_buttons_sensitive(
            self.state.routes.borrow().can_back(),
            self.state.routes.borrow().can_forward(),
        );
        update_navigation_selection(self.as_ref());
    }

    fn finish_route_view(self: &Rc<Self>, route: &Route, view: RouteView) {
        self.state.current_route_resize_policy.set(view.resize);
        let render_width = route_content_width(self.as_ref());
        self.state
            .responsive_render_signature
            .set(self.route_resize_signature(route, view.resize));
        let boundary = route_boundary_for_route(route, view.widget, render_width);
        self.state
            .current_route_boundary
            .replace(Some(boundary.clone()));
        self.route_host.append(&boundary);
        self.prime_route_visible_cover_window(route);
        {
            let shell = Rc::clone(self);
            let route = route.clone();
            glib::idle_add_local_once(move || {
                shell.prime_route_visible_cover_window(&route);
            });
        }
    }

    fn cancel_route_loads(&self) {
        self.state
            .route_load_generation
            .set(self.state.route_load_generation.get().saturating_add(1));
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
}

fn sync_route_surface_ready(
    login_active: bool,
    startup_revealed: bool,
    startup_render_pending: bool,
) -> bool {
    !login_active && startup_revealed && !startup_render_pending
}

fn sync_delta_refreshes_route(route: &Route, delta: &LibraryDelta, surface_ready: bool) -> bool {
    surface_ready && !delta.is_empty() && route_delta_affects(route, delta)
}

fn pending_sync_route_refresh_matches(
    pending: Option<&(Route, u64)>,
    route: &Route,
    route_generation: u64,
) -> bool {
    pending.is_some_and(|(queued_route, queued_generation)| {
        queued_route == route && *queued_generation == route_generation
    })
}

fn sync_route_refresh_target_matches(
    route: &Route,
    route_generation: u64,
    current_route: &Route,
    current_generation: u64,
) -> bool {
    current_route == route && current_generation == route_generation
}

impl Shell {
    pub(in crate::ui) fn library_route_resize_policy(
        &self,
        key: LibraryListKey,
    ) -> RouteResizePolicy {
        library_route_resize_policy_for(key, &self.library_settings(key))
    }

    pub(in crate::ui) fn route_resize_signature(
        &self,
        route: &Route,
        policy: RouteResizePolicy,
    ) -> i32 {
        match policy {
            RouteResizePolicy::Stable => 0,
            RouteResizePolicy::SettledWidth => route_content_width(self),
            RouteResizePolicy::LayoutSignature => match route {
                Route::Albums => self.library_layout_signature(LibraryListKey::Albums),
                Route::Tracks => self.library_layout_signature(LibraryListKey::Tracks),
                Route::Favorites => self.library_layout_signature(LibraryListKey::FavoriteTracks),
                Route::Artists => self.library_layout_signature(LibraryListKey::Artists),
                Route::ArtistTracks(_) => {
                    self.library_layout_signature(LibraryListKey::ArtistTracks)
                }
                Route::AlbumArtists => self.library_layout_signature(LibraryListKey::AlbumArtists),
                Route::Genres => self.library_layout_signature(LibraryListKey::Genres),
                Route::Playlists => self.library_layout_signature(LibraryListKey::Playlists),
                Route::SmartPlaylists => {
                    self.library_layout_signature(LibraryListKey::SmartPlaylists)
                }
                _ => self.grid_layout_signature(),
            },
        }
    }

    fn library_layout_signature(&self, key: LibraryListKey) -> i32 {
        let settings = self.library_settings(key);
        match normalized_library_layout(key, &settings) {
            LibraryLayout::Row => 0,
            LibraryLayout::Grid => self.grid_layout_signature(),
            LibraryLayout::Detail => route_content_width(self),
        }
    }

    fn grid_layout_signature(&self) -> i32 {
        let (columns, _) = self.collection_card_grid_metrics();
        columns as i32
    }
}

pub(in crate::ui) fn library_route_resize_policy_for(
    key: LibraryListKey,
    settings: &LibraryListSettings,
) -> RouteResizePolicy {
    match normalized_library_layout(key, settings) {
        LibraryLayout::Row => RouteResizePolicy::Stable,
        LibraryLayout::Grid => RouteResizePolicy::LayoutSignature,
        LibraryLayout::Detail => RouteResizePolicy::SettledWidth,
    }
}

fn normalized_library_layout(key: LibraryListKey, settings: &LibraryListSettings) -> LibraryLayout {
    if key.supports_layout(settings.layout) {
        settings.layout
    } else {
        LibraryLayout::Row
    }
}

fn route_delta_affects(route: &Route, delta: &LibraryDelta) -> bool {
    if delta.reset.is_some() {
        return true;
    }
    match route {
        Route::Home => delta.home_changed,
        Route::Favorites => track_table_delta_affects(delta),
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
        Route::Tracks => track_table_delta_affects(delta),
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
                || track_table_delta_affects(delta)
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
                || track_table_delta_affects(delta)
                || !delta.albums.is_empty()
        }
        Route::Folders { .. } => delta.folders_changed || track_table_delta_affects(delta),
        Route::Playlists => {
            !delta.playlists.added.is_empty()
                || !delta.playlists.deleted.is_empty()
                || !delta.playlists.fields.is_empty()
                || !delta.playlists.entries.is_empty()
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
            track_table_delta_affects(delta)
                || !delta.albums.is_empty()
                || !delta.artists.is_empty()
                || !delta.album_artists.is_empty()
                || !delta.genres.is_empty()
                || !delta.playlists.is_empty()
        }
    }
}

fn track_table_delta_affects(delta: &LibraryDelta) -> bool {
    !delta.tracks.added.is_empty()
        || !delta.tracks.deleted.is_empty()
        || !delta.tracks.fields.is_empty()
        || !delta.tracks.favorite.is_empty()
        || !delta.tracks.cover_refs.is_empty()
}

fn merge_pending_sync_route_delta(pending: &mut Option<LibraryDelta>, delta: LibraryDelta) {
    if let Some(pending) = pending {
        pending.merge(delta);
    } else {
        *pending = Some(delta);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_library_routes_do_not_rerender_for_width() {
        let settings = LibraryListSettings {
            layout: LibraryLayout::Row,
            ..LibraryListSettings::for_key(LibraryListKey::Tracks)
        };

        assert_eq!(
            library_route_resize_policy_for(LibraryListKey::Tracks, &settings),
            RouteResizePolicy::Stable
        );
    }

    #[test]
    fn grid_library_routes_rerender_by_layout_signature() {
        let settings = LibraryListSettings {
            layout: LibraryLayout::Grid,
            ..LibraryListSettings::for_key(LibraryListKey::Artists)
        };

        assert_eq!(
            library_route_resize_policy_for(LibraryListKey::Artists, &settings),
            RouteResizePolicy::LayoutSignature
        );
    }

    #[test]
    fn detail_library_routes_wait_for_settled_width() {
        let settings = LibraryListSettings {
            layout: LibraryLayout::Detail,
            ..LibraryListSettings::for_key(LibraryListKey::Albums)
        };

        assert_eq!(
            library_route_resize_policy_for(LibraryListKey::Albums, &settings),
            RouteResizePolicy::SettledWidth
        );
    }

    #[test]
    fn unsupported_detail_layout_is_stable() {
        let settings = LibraryListSettings {
            layout: LibraryLayout::Detail,
            ..LibraryListSettings::for_key(LibraryListKey::Tracks)
        };

        assert_eq!(
            library_route_resize_policy_for(LibraryListKey::Tracks, &settings),
            RouteResizePolicy::Stable
        );
    }

    #[test]
    fn stats_only_track_delta_skips_plain_track_routes() {
        let mut delta = LibraryDelta::default();
        delta.tracks.stats.push(TrackId::fake(1));

        assert!(!route_delta_affects(&Route::Tracks, &delta));
        assert!(!route_delta_affects(&Route::Favorites, &delta));
        assert!(!route_delta_affects(
            &Route::AlbumDetail(AlbumId::fake(1)),
            &delta
        ));
        assert!(!route_delta_affects(
            &Route::ArtistDetail(ArtistId::fake(1)),
            &delta
        ));
        assert!(!route_delta_affects(
            &Route::ArtistTracks(ArtistId::fake(1)),
            &delta
        ));
        assert!(!route_delta_affects(
            &Route::Folders { path: Vec::new() },
            &delta
        ));
        assert!(!route_delta_affects(
            &Route::Search {
                query: "track".to_string(),
                kind: SearchKind::Tracks
            },
            &delta
        ));
        assert!(route_delta_affects(&Route::SmartPlaylists, &delta));
        assert!(route_delta_affects(
            &Route::SmartPlaylistDetail(SmartPlaylistId::fake(1)),
            &delta
        ));
        assert!(route_delta_affects(
            &Route::PlaylistDetail(PlaylistId::fake(1)),
            &delta
        ));
    }

    #[test]
    fn visible_track_delta_updates_tracks_route() {
        let mut delta = LibraryDelta::default();
        delta.tracks.fields.push(TrackId::fake(1));
        assert!(route_delta_affects(&Route::Tracks, &delta));

        let mut delta = LibraryDelta::default();
        delta.tracks.added.push(TrackId::fake(2));
        assert!(route_delta_affects(&Route::Tracks, &delta));
    }

    #[test]
    fn empty_sync_delta_does_not_refresh_current_route() {
        let delta = LibraryDelta::default();

        assert!(!sync_delta_refreshes_route(&Route::Tracks, &delta, true));
        assert!(!sync_delta_refreshes_route(&Route::Albums, &delta, true));
        assert!(!sync_delta_refreshes_route(&Route::Home, &delta, true));
    }

    #[test]
    fn unrelated_sync_delta_keeps_current_route() {
        let mut delta = LibraryDelta::default();
        delta.tracks.fields.push(TrackId::fake(1));

        assert!(!sync_delta_refreshes_route(&Route::Albums, &delta, true));
        assert!(!sync_delta_refreshes_route(&Route::Artists, &delta, true));
        assert!(sync_delta_refreshes_route(&Route::Tracks, &delta, true));
    }

    #[test]
    fn sync_delta_waits_for_visible_route_surface() {
        let mut delta = LibraryDelta::default();
        delta.tracks.fields.push(TrackId::fake(1));

        assert!(sync_route_surface_ready(false, true, false));
        assert!(!sync_route_surface_ready(true, true, false));
        assert!(!sync_route_surface_ready(false, false, false));
        assert!(!sync_route_surface_ready(false, true, true));
        assert!(!sync_delta_refreshes_route(&Route::Tracks, &delta, false));
    }

    #[test]
    fn stale_sync_route_refresh_target_is_ignored() {
        let route = Route::Tracks;
        let pending = Some((route.clone(), 7));

        assert!(pending_sync_route_refresh_matches(
            pending.as_ref(),
            &route,
            7,
        ));
        assert!(!pending_sync_route_refresh_matches(
            Some(&(Route::Albums, 7)),
            &route,
            7,
        ));
        assert!(sync_route_refresh_target_matches(&route, 7, &route, 7));
        assert!(!sync_route_refresh_target_matches(&route, 7, &route, 8,));
        assert!(!sync_route_refresh_target_matches(
            &route,
            7,
            &Route::Albums,
            7,
        ));
    }

    #[test]
    fn pending_sync_route_delta_merges_changes() {
        let mut first = LibraryDelta::default();
        first.tracks.fields.push(TrackId::fake(1));
        let mut pending = None;
        merge_pending_sync_route_delta(&mut pending, first);

        let mut second = LibraryDelta::default();
        second.albums.fields.push(AlbumId::fake(2));
        second.home_changed = true;
        merge_pending_sync_route_delta(&mut pending, second);

        let pending = pending.expect("merged delta");
        assert_eq!(pending.tracks.fields, vec![TrackId::fake(1)]);
        assert_eq!(pending.albums.fields, vec![AlbumId::fake(2)]);
        assert!(pending.home_changed);
    }

    #[test]
    fn pending_sync_route_delta_keeps_route_affects() {
        let mut first = LibraryDelta::default();
        first.tracks.fields.push(TrackId::fake(1));
        let mut second = LibraryDelta::default();
        second.albums.fields.push(AlbumId::fake(2));
        let mut pending = None;

        merge_pending_sync_route_delta(&mut pending, first);
        merge_pending_sync_route_delta(&mut pending, second);

        let pending = pending.expect("merged delta");
        assert!(route_delta_affects(&Route::Tracks, &pending));
        assert!(route_delta_affects(&Route::Albums, &pending));
    }
}
