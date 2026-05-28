use super::*;

impl Shell {
    pub(in crate::ui) fn render_current_route(self: &Rc<Self>) {
        let render_started = Instant::now();
        self.cancel_cover_warm();
        self.cancel_startup_cover_warm();
        self.update_layout();
        self.state.home_section_views.borrow_mut().clear();
        if !self.state.startup_route_revealed.get() && !self.login_screen_active() {
            self.render_startup_loading_view();
            return;
        }
        if self.login_screen_active() {
            clear_favorite_controls(&self.state.favorite_controls);
            while let Some(child) = self.login_host.first_child() {
                self.login_host.remove(&child);
            }
            let route_name = "FirstRun".to_string();
            self.route_title.set_title(&tr("Connect to Music Server"));
            self.set_history_buttons_sensitive(false, false);
            let view = self.add_server_view();
            self.login_host.append(&view);
            self.observe_route_scroll(&route_name);
            self.record_perf_route_render(route_name, render_started.elapsed());
            return;
        }

        clear_favorite_controls(&self.state.favorite_controls);
        while let Some(child) = self.route_host.first_child() {
            self.route_host.remove(&child);
        }

        let route = self.state.routes.borrow().current().clone();
        let route_name = format!("{route:?}");
        self.reset_inactive_route_cover_gates(match route {
            Route::Tracks => Some("tracks"),
            Route::Albums => Some("albums"),
            Route::Artists => Some("artists"),
            Route::AlbumArtists => Some("album_artists"),
            _ => None,
        });
        self.route_title.set_title(&tr(route.title()));
        self.set_history_buttons_sensitive(
            self.state.routes.borrow().can_back(),
            self.state.routes.borrow().can_forward(),
        );
        update_navigation_selection(self.as_ref());

        let view_started = Instant::now();
        let view = match route {
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
            Route::Search { query, .. } => {
                let library = self.state.library.borrow().clone();
                self.search_view(&query, library)
            }
        };
        let view_ms = view_started.elapsed().as_millis() as u64;

        let append_started = Instant::now();
        self.route_host.append(&route_boundary(view));
        let append_ms = append_started.elapsed().as_millis() as u64;
        let observe_started = Instant::now();
        self.observe_route_scroll(&route_name);
        let observe_ms = observe_started.elapsed().as_millis() as u64;
        if self.state.perf.is_some() {
            println!(
                "RUFIN_PERF_ROUTE_PHASE route={} view_ms={} append_ms={} observe_ms={} total_ms={}",
                route_name,
                view_ms,
                append_ms,
                observe_ms,
                render_started.elapsed().as_millis() as u64
            );
        }
        self.record_perf_route_render(route_name, render_started.elapsed());
        self.schedule_startup_cover_warm();
    }
    pub(in crate::ui) fn reset_inactive_route_cover_gates(
        &self,
        active_route_key: Option<&'static str>,
    ) {
        self.state
            .route_cover_gate_started
            .borrow_mut()
            .retain(|route_key, _| Some(*route_key) == active_route_key);
        self.state
            .route_cover_gate_queued
            .borrow_mut()
            .retain(|route_key| Some(*route_key) == active_route_key);
        self.state
            .route_cover_gate_timed_out
            .borrow_mut()
            .retain(|route_key| Some(*route_key) == active_route_key);
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
    pub(in crate::ui) fn observe_route_scroll(&self, route: &str) {
        let Some(perf) = self
            .state
            .perf
            .as_ref()
            .filter(|perf| perf.options.observe_scroll)
            .cloned()
        else {
            return;
        };
        let host = if self.login_screen_active() {
            self.login_host.clone().upcast::<gtk::Widget>()
        } else {
            self.route_host.clone().upcast::<gtk::Widget>()
        };
        let route = route.to_string();
        glib::idle_add_local_once(move || {
            let Some(scroller) = find_largest_scrolled_window(&host) else {
                perf.record_scroll_note(&route, "no_scrolled_window");
                return;
            };
            let adjustment = scroller.vadjustment();
            adjustment.connect_value_changed(move |adjustment| {
                let max_adjustment = (adjustment.upper() - adjustment.page_size()).max(0.0);
                perf.record_manual_scroll_step(&route, adjustment.value(), max_adjustment);
            });
        });
    }
}
