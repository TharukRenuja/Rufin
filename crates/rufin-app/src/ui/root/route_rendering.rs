use super::*;

const POST_ROUTE_VISIBLE_WARM_DELAY_MS: u64 = 64;
const POST_HOME_TRACK_WARM_ROWS: usize = TRACK_ROUTE_PAGE_SIZE;

#[derive(Clone)]
pub(in crate::ui) struct PostRouteVisibleWarmTarget {
    pub(in crate::ui) route: Route,
    pub(in crate::ui) leading_rows: usize,
}

pub(in crate::ui) fn post_route_visible_warm_targets(
    route: &Route,
) -> Vec<PostRouteVisibleWarmTarget> {
    match route {
        Route::Home => vec![PostRouteVisibleWarmTarget {
            route: Route::Tracks,
            leading_rows: POST_HOME_TRACK_WARM_ROWS,
        }],
        _ => Vec::new(),
    }
}

impl Shell {
    pub(in crate::ui) fn render_current_route(self: &Rc<Self>) {
        let render_started = Instant::now();
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

        let view_started = Instant::now();
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
        let view_ms = view_started.elapsed().as_millis() as u64;

        let append_started = Instant::now();
        self.route_host
            .append(&route_boundary_for_route(&route, view));
        let append_ms = append_started.elapsed().as_millis() as u64;
        let observe_started = Instant::now();
        self.observe_route_scroll(&route_name);
        let observe_ms = observe_started.elapsed().as_millis() as u64;
        self.prime_route_visible_cover_window(&route);
        {
            let shell = Rc::clone(self);
            let route = route.clone();
            glib::idle_add_local_once(move || {
                shell.prime_route_visible_cover_window(&route);
            });
        }
        self.schedule_post_route_visible_cover_warm(&route);
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
    }
    fn schedule_post_route_visible_cover_warm(self: &Rc<Self>, route: &Route) {
        let targets = post_route_visible_warm_targets(route);
        if targets.is_empty() {
            return;
        }
        let shell = Rc::clone(self);
        let source_route = route.clone();
        glib::timeout_add_local_once(
            Duration::from_millis(POST_ROUTE_VISIBLE_WARM_DELAY_MS),
            move || {
                if shell.state.routes.borrow().current() != &source_route {
                    return;
                }
                for target in targets {
                    let refs = shell.prime_route_leading_and_warm_anchor_cover_windows(
                        &target.route,
                        target.leading_rows,
                    );
                    if shell.state.perf.is_some() {
                        println!(
                            "RUFIN_ROUTE_POST_WARM source={source_route:?} target={:?} refs={refs}",
                            target.route
                        );
                    }
                }
            },
        );
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
