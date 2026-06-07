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
            self.show_reconnect_notice_if_needed();
            return;
        }

        let route = self.state.routes.borrow().current().clone();
        self.prepare_route_host(&route);
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

        self.finish_route_view(&route, view);
    }
    pub(in crate::ui) fn request_current_route_render(self: &Rc<Self>) {
        self.render_current_route();
    }
    pub(in crate::ui) fn render_current_route_preserving_scroll(self: &Rc<Self>) {
        self.render_current_route();
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
    fn prepare_route_host(self: &Rc<Self>, route: &Route) {
        clear_favorite_controls(&self.state.favorite_controls);
        self.state.type_to_search.borrow_mut().take();
        while let Some(child) = self.route_host.first_child() {
            self.route_host.remove(&child);
        }
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
    }

    fn finish_route_view(self: &Rc<Self>, route: &Route, view: gtk::Widget) {
        self.route_host
            .append(&route_boundary_for_route(route, view));
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
            !delta.tracks.is_empty()
                || !delta.albums.is_empty()
                || !delta.artists.is_empty()
                || !delta.album_artists.is_empty()
                || !delta.genres.is_empty()
                || !delta.playlists.is_empty()
        }
    }
}
