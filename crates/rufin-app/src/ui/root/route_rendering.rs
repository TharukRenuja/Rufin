use super::*;

impl Shell {
    pub(in crate::ui) fn render_current_route(self: &Rc<Self>) {
        self.reset_queued_cover_work_for_route_render();
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

        clear_favorite_controls(&self.state.favorite_controls);
        while let Some(child) = self.route_host.first_child() {
            self.route_host.remove(&child);
        }

        let route = self.state.routes.borrow().current().clone();
        self.route_title.set_title(&tr(route.title()));
        self.set_history_buttons_sensitive(
            self.state.routes.borrow().can_back(),
            self.state.routes.borrow().can_forward(),
        );
        update_navigation_selection(self.as_ref());
        if route_uses_responsive_cards(&route) {
            self.state
                .responsive_route_render_width
                .set(route_content_width(self.as_ref()));
        } else {
            self.state.responsive_route_render_width.set(0);
        }

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

        self.route_host
            .append(&route_boundary_for_route(&route, view));
        self.prime_route_visible_cover_window(&route);
        {
            let shell = Rc::clone(self);
            let route = route.clone();
            glib::idle_add_local_once(move || {
                shell.prime_route_visible_cover_window(&route);
            });
        }
    }
    pub(in crate::ui) fn render_current_route_preserving_scroll(self: &Rc<Self>) {
        let scroll_value = self.current_route_scroll_value();
        self.render_current_route();
        if let Some(value) = scroll_value {
            self.restore_current_route_scroll(value);
        }
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
}
