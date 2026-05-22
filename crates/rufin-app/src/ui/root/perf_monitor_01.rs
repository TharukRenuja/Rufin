impl Shell {
    fn register_home_section_view(
        &self,
        section_kind: HomeSectionKind,
        root: &gtk::Box,
        row: &gtk::Box,
        previous: &gtk::Button,
        next: &gtk::Button,
    ) {
        if !matches!(self.state.routes.borrow().current(), Route::Home) {
            return;
        }

        self.state.home_section_views.borrow_mut().insert(
            section_kind,
            HomeSectionView {
                root: root.clone().upcast::<gtk::Widget>(),
                row: row.clone(),
                previous: previous.clone(),
                next: next.clone(),
            },
        );
    }
    fn refresh_visible_home_section(
        self: &Rc<Self>,
        section_kind: HomeSectionKind,
        sections: &[HomeSection],
    ) {
        if !matches!(self.state.routes.borrow().current(), Route::Home) {
            return;
        }

        if let Some(section) = sections.iter().find(|section| section.kind == section_kind) {
            self.render_visible_home_section(section);
        } else {
            self.hide_visible_home_section(section_kind);
        }
    }
    fn refresh_visible_home_sections(
        self: &Rc<Self>,
        sections: &[HomeSection],
        include_explore: bool,
    ) {
        if !matches!(self.state.routes.borrow().current(), Route::Home) {
            return;
        }

        let section_kinds = self
            .state
            .home_section_views
            .borrow()
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for section_kind in section_kinds {
            if !include_explore && section_kind == HomeSectionKind::Explore {
                continue;
            }
            self.refresh_visible_home_section(section_kind, sections);
        }
    }
    fn render_visible_home_section(self: &Rc<Self>, section: &HomeSection) -> bool {
        let view = self
            .state
            .home_section_views
            .borrow()
            .get(&section.kind)
            .cloned();
        let Some(view) = view else {
            return false;
        };

        view.root.set_visible(true);
        if !section.tracks.is_empty() {
            cards::render_home_track_page(
                self,
                &view.row,
                &view.previous,
                &view.next,
                section.kind,
                &section.tracks,
            );
        } else {
            cards::render_home_album_page(
                self,
                &view.row,
                &view.previous,
                &view.next,
                section.kind,
                &section.albums,
            );
        }
        true
    }
    fn hide_visible_home_section(&self, section_kind: HomeSectionKind) -> bool {
        let view = self
            .state
            .home_section_views
            .borrow()
            .get(&section_kind)
            .cloned();
        let Some(view) = view else {
            return false;
        };
        view.root.set_visible(false);
        true
    }
    fn navigate(self: &Rc<Self>, route: Route) {
        debug!(?route, "navigate");
        let previous = self.state.routes.borrow().current().clone();
        self.refresh_search_results_for_route(&route);
        self.state.routes.borrow_mut().navigate(route.clone());
        self.handle_home_route_transition(&previous, &route);
        self.render_current_route();
        if matches!(route, Route::Home) {
            self.refresh_home_for_current_visit();
        }
        if matches!(route, Route::Playlists) {
            self.refresh_playlists_for_current_visit();
        }
    }
    fn go_back(self: &Rc<Self>) {
        let previous = self.state.routes.borrow().current().clone();
        let route = self.state.routes.borrow_mut().back().cloned();
        if let Some(route) = route {
            debug!(?route, "navigate back");
            self.refresh_search_results_for_route(&route);
            self.handle_home_route_transition(&previous, &route);
            self.render_current_route();
            if matches!(route, Route::Home) {
                self.refresh_home_for_current_visit();
            }
            if matches!(route, Route::Playlists) {
                self.refresh_playlists_for_current_visit();
            }
        }
    }
    fn go_forward(self: &Rc<Self>) {
        let previous = self.state.routes.borrow().current().clone();
        let route = self.state.routes.borrow_mut().forward().cloned();
        if let Some(route) = route {
            debug!(?route, "navigate forward");
            self.refresh_search_results_for_route(&route);
            self.handle_home_route_transition(&previous, &route);
            self.render_current_route();
            if matches!(route, Route::Home) {
                self.refresh_home_for_current_visit();
            }
            if matches!(route, Route::Playlists) {
                self.refresh_playlists_for_current_visit();
            }
        }
    }
    fn refresh_search_results_for_route(&self, route: &Route) {
        if let Route::Search { query, .. } = route {
            self.controller.search(query.clone());
        }
    }
    fn start_folder_load(self: &Rc<Self>, path: Vec<FolderPathItem>) {
        let request_id = self.state.folder_request_generation.get().saturating_add(1);
        self.state.folder_request_generation.set(request_id);
        *self.state.folder_state.borrow_mut() = FolderRouteState {
            request_id,
            path: path.clone(),
            loading: true,
            detail: None,
            error: None,
        };
        self.controller.load_folder_for_active(request_id, path);
    }
    fn apply_folder_loaded(
        self: &Rc<Self>,
        request_id: u64,
        path: Vec<FolderPathItem>,
        detail: FolderDetail,
    ) {
        let should_render = {
            let mut state = self.state.folder_state.borrow_mut();
            if state.request_id != request_id || state.path != path {
                return;
            }
            state.loading = false;
            state.detail = Some(detail);
            state.error = None;
            matches!(
                self.state.routes.borrow().current(),
                Route::Folders { path: current_path } if current_path == &state.path
            )
        };
        if should_render {
            self.render_current_route();
        }
    }
    fn apply_folder_load_failed(
        self: &Rc<Self>,
        request_id: u64,
        path: Vec<FolderPathItem>,
        error: String,
    ) {
        warn!(%error, "folder load failed");
        let should_render = {
            let mut state = self.state.folder_state.borrow_mut();
            if state.request_id != request_id || state.path != path {
                return;
            }
            state.loading = false;
            state.detail = None;
            state.error = Some(error);
            matches!(
                self.state.routes.borrow().current(),
                Route::Folders { path: current_path } if current_path == &state.path
            )
        };
        if should_render {
            self.render_current_route();
        }
    }
    fn handle_home_route_transition(self: &Rc<Self>, previous: &Route, next: &Route) {
        let was_home = matches!(previous, Route::Home);
        let is_home = matches!(next, Route::Home);
        let was_playlists = matches!(previous, Route::Playlists);
        let is_playlists = matches!(next, Route::Playlists);

        if is_home && !was_home {
            self.state.home_refresh_started_for_visit.set(false);
            self.state.home_showcase_seed.set(next_home_showcase_seed());
            reset_home_section_pages(&mut self.state.home_section_state.borrow_mut());
            self.promote_cached_prefetched_explore();
        }
        if is_playlists && !was_playlists {
            self.state.playlist_refresh_started_for_visit.set(false);
        }
    }
    fn refresh_home_for_current_visit(self: &Rc<Self>) {
        if !matches!(self.state.routes.borrow().current(), Route::Home) {
            return;
        }
        if self.state.home_refresh_started_for_visit.replace(true) {
            return;
        }
        self.controller
            .refresh_home_sections_without_explore_for_active();
        self.controller.prefetch_explore_for_active();
    }
    fn refresh_playlists_for_current_visit(self: &Rc<Self>) {
        if !matches!(self.state.routes.borrow().current(), Route::Playlists) {
            return;
        }
        if self.state.playlist_refresh_started_for_visit.replace(true) {
            return;
        }
        self.controller.refresh_playlists_for_active();
    }
    fn refresh_home_section(self: &Rc<Self>, section_kind: HomeSectionKind) {
        if let Some(state) = self
            .state
            .home_section_state
            .borrow_mut()
            .get_mut(&section_kind)
        {
            state.page_start = 0;
        }

        if section_kind == HomeSectionKind::Explore && self.apply_prefetched_explore() {
            return;
        }

        self.controller
            .refresh_home_section_for_active(section_kind);
        if section_kind == HomeSectionKind::Explore {
            self.controller.prefetch_explore_for_active();
        }
    }
    fn apply_prefetched_explore(self: &Rc<Self>) -> bool {
        let prefetched = self.state.prefetched_explore.borrow().clone();
        let promoted = prefetched
            .map(|prefetched| self.promote_prefetched_explore(prefetched, true))
            .unwrap_or(false);
        if promoted {
            self.controller.prefetch_explore_for_active();
        }
        promoted
    }
    fn promote_cached_prefetched_explore(self: &Rc<Self>) -> bool {
        let prefetched = self.state.prefetched_explore.borrow().clone();
        prefetched
            .map(|prefetched| self.promote_prefetched_explore(prefetched, false))
            .unwrap_or(false)
    }
    fn promote_prefetched_explore(
        self: &Rc<Self>,
        prefetched: PrefetchedHomeSection,
        render_current_route: bool,
    ) -> bool {
        let Some(server_id) = self
            .state
            .library
            .borrow()
            .server
            .as_ref()
            .map(|server| server.id.clone())
        else {
            return false;
        };
        if prefetched.server_id != server_id {
            *self.state.prefetched_explore.borrow_mut() = Some(prefetched);
            return false;
        }

        let section = prefetched.section.clone();
        let mut changed = false;
        {
            let mut library = self.state.library.borrow_mut();
            let current = library
                .home_sections
                .iter()
                .find(|existing| existing.kind == section.kind);
            if current != Some(&section) {
                upsert_snapshot_home_section(&mut library.home_sections, section.clone());
                changed = true;
            }
        }
        if changed {
            reset_home_section_pages(&mut self.state.home_section_state.borrow_mut());
            self.controller
                .promote_prefetched_explore_for_active(section.clone());
        }
        if render_current_route {
            self.refresh_visible_home_section(section.kind, std::slice::from_ref(&section));
        }
        true
    }
    fn update_prefetched_explore_from_snapshot(
        &self,
        server_id: Option<rufin_core::ServerId>,
        prefetched: Option<PrefetchedHomeSection>,
        sections: &[HomeSection],
    ) {
        if prefetched.is_some() {
            *self.state.prefetched_explore.borrow_mut() = prefetched;
            return;
        }

        let keep_current = {
            let current = self.state.prefetched_explore.borrow();
            current.as_ref().is_some_and(|current| {
                server_id
                    .as_ref()
                    .is_some_and(|server_id| &current.server_id == server_id)
                    && !sections.iter().any(|section| {
                        section.kind == HomeSectionKind::Explore && section == &current.section
                    })
            })
        };
        if !keep_current {
            *self.state.prefetched_explore.borrow_mut() = None;
        }
    }
    fn update_layout(self: &Rc<Self>) -> bool {
        let width = self.layout_width().max(1);
        let settings = self.state.settings.borrow().layout.clone();
        let resolved = resolve_layout(&settings, width);
        self.apply_resolved_layout(resolved)
    }
    fn apply_resolved_layout(self: &Rc<Self>, resolved: ResolvedLayout) -> bool {
        let login_active = self.login_screen_active();
        if login_active {
            self.root_stack.set_visible_child(&self.login_host);
        } else {
            self.root_stack.set_visible_child(&self.app_root);
        }
        let previous_left = self
            .state
            .resolved_left_sidebar
            .replace(resolved.left_sidebar);
        let previous_right = self
            .state
            .resolved_right_sidebar
            .replace(resolved.right_sidebar);
        let previous_right_width = self
            .state
            .resolved_right_sidebar_width
            .replace(resolved.right_sidebar_width);
        let previous_main_width = self.state.main_content_width.replace(resolved.main_width);

        self.normal_nav
            .set_visible(!login_active && resolved.left_sidebar == LeftSidebarMode::Full);
        self.compact_nav
            .set_visible(!login_active && resolved.left_sidebar == LeftSidebarMode::Compact);
        self.right_panel_slot
            .set_visible(!login_active && resolved.right_sidebar.is_visible());
        self.right_panel_slot.set_min_content_width(0);
        self.right_panel_slot
            .set_max_content_width(resolved.right_sidebar_width);
        self.right_panel_slot.set_size_request(-1, -1);
        self.right_panel
            .set_width_request(resolved.right_sidebar_width);
        self.right_panel
            .set_visible(!login_active && resolved.right_sidebar.is_visible());
        self.player_controls.root.set_visible(!login_active);
        self.update_right_panel_button();
        self.update_lyrics_panel_button();

        let changed = previous_left != resolved.left_sidebar
            || previous_right != resolved.right_sidebar
            || previous_right_width != resolved.right_sidebar_width
            || previous_main_width != resolved.main_width;
        if changed {
            debug!(?resolved, "resolved layout changed");
            self.queue_responsive_route_render();
        }
        self.log_layout_snapshot("apply_resolved_layout");
        changed
    }
    fn layout_width(&self) -> i32 {
        self.window
            .surface()
            .map(|surface| surface.width())
            .filter(|width| *width > 1)
            .or_else(|| {
                let width = self.window.width();
                (width > 1).then_some(width)
            })
            .unwrap_or(1)
    }
    fn login_screen_active(&self) -> bool {
        self.state.library.borrow().first_run || self.state.first_run_connection_pending.get()
    }
    fn log_layout_snapshot(&self, stage: &'static str) {
        if std::env::var_os("RUFIN_DEBUG_LAYOUT").is_none() {
            return;
        }

        let route = self.state.routes.borrow().current().clone();
        info!(
            stage,
            ?route,
            login_active = self.login_screen_active(),
            first_run = self.state.library.borrow().first_run,
            first_run_connection_pending = self.state.first_run_connection_pending.get(),
            first_run_connection_ready = self.state.first_run_connection_ready.get(),
            window_width = self.layout_width(),
            root_stack_width = self.root_stack.width(),
            app_root_width = self.app_root.width(),
            login_host_width = self.login_host.width(),
            route_host_width = self.route_host.width(),
            resolved_main_width = self.state.main_content_width.get(),
            right_sidebar = ?self.state.resolved_right_sidebar.get(),
            right_panel_slot_visible = self.right_panel_slot.is_visible(),
            right_panel_slot_width = self.right_panel_slot.width(),
            right_panel_width = self.right_panel.width(),
            "layout snapshot"
        );
    }
    fn render_startup_loading_view(&self) {
        self.route_title.set_title("Rufin");
        self.set_history_buttons_sensitive(false, false);
        while let Some(child) = self.route_host.first_child() {
            self.route_host.remove(&child);
        }
        self.route_host
            .append(&route_boundary(self.startup_loading_view()));
    }
    fn startup_loading_view(&self) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 0);
        wrapper.add_css_class("startup-loading-page");
        wrapper.set_hexpand(true);
        wrapper.set_vexpand(true);
        wrapper.set_halign(gtk::Align::Center);
        wrapper.set_valign(gtk::Align::Center);

        let spinner = gtk::Spinner::new();
        spinner.start();
        wrapper.append(&spinner);
        wrapper.upcast()
    }
    fn schedule_startup_route_reveal(self: &Rc<Self>) {
        if self.state.startup_route_revealed.get() || self.login_screen_active() {
            return;
        }

        let prime_shell = Rc::clone(self);
        glib::timeout_add_local_once(
            Duration::from_millis(STARTUP_TRACK_THUMB_PRIME_DELAY_MS),
            move || {
                if !prime_shell.state.startup_route_revealed.get()
                    && !prime_shell.login_screen_active()
                {
                    prime_first_track_thumbnail_covers(&prime_shell);
                }
            },
        );

        let started_at = Instant::now();
        let shell = Rc::clone(self);
        glib::timeout_add_local(
            Duration::from_millis(STARTUP_ROUTE_REVEAL_POLL_MS),
            move || {
                if shell.state.startup_route_revealed.get() || shell.login_screen_active() {
                    return glib::ControlFlow::Break;
                }

                shell.update_layout();
                let elapsed = started_at.elapsed();
                let width_ready = shell.layout_width() > 1 && shell.route_host.width() > 1;
                let reveal_ready =
                    width_ready && elapsed >= Duration::from_millis(STARTUP_ROUTE_REVEAL_MIN_MS);
                let reveal_expired = elapsed >= Duration::from_millis(STARTUP_ROUTE_REVEAL_MAX_MS);
                if reveal_ready || reveal_expired {
                    shell.reveal_startup_route();
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            },
        );
    }
    fn reveal_startup_route(self: &Rc<Self>) {
        if self.state.startup_route_revealed.replace(true) || self.login_screen_active() {
            return;
        }

        self.update_layout();
        self.render_current_route();
    }
    fn schedule_first_run_app_reveal(self: &Rc<Self>) {
        self.log_layout_snapshot("first_run_reveal_queued");
        if let Some(generation) = self.begin_first_run_cover_prime() {
            let started_at = Instant::now();
            let shell = Rc::clone(self);
            glib::timeout_add_local(
                Duration::from_millis(FIRST_RUN_COVER_PRIME_POLL_MS),
                move || {
                    if shell.state.first_run_cover_prime_generation.get() != generation {
                        return glib::ControlFlow::Break;
                    }
                    let pending = shell.state.first_run_cover_prime_pending.borrow().len();
                    let expired = started_at.elapsed()
                        >= Duration::from_millis(FIRST_RUN_COVER_PRIME_TIMEOUT_MS);
                    if pending == 0 || expired {
                        if expired && pending > 0 {
                            debug!(
                                pending,
                                "revealing first-run route with cover prime still pending"
                            );
                        }
                        shell
                            .state
                            .first_run_cover_prime_pending
                            .borrow_mut()
                            .clear();
                        shell.finish_first_run_app_reveal();
                        glib::ControlFlow::Break
                    } else {
                        glib::ControlFlow::Continue
                    }
                },
            );
            return;
        }

        self.finish_first_run_app_reveal();
    }
    fn finish_first_run_app_reveal(self: &Rc<Self>) {
        if self.state.first_run_connection_pending.get() {
            self.state.library.borrow_mut().sync_status = tr(LIBRARY_SYNC_COMPLETE_STATUS);
            self.render_current_route();
        }

        self.state.first_run_cover_prime_generation.set(
            self.state
                .first_run_cover_prime_generation
                .get()
                .saturating_add(1),
        );
        self.state
            .first_run_cover_prime_pending
            .borrow_mut()
            .clear();

        let shell = Rc::clone(self);
        glib::idle_add_local_once(move || {
            shell.state.first_run_connection_pending.set(false);
            shell.state.first_run_connection_ready.set(false);
            shell.log_layout_snapshot("first_run_reveal_before_stack_switch");
            shell.update_layout();
            shell.window.queue_resize();
            shell.app_root.queue_resize();
            shell.route_host.queue_resize();
            shell.right_panel_slot.queue_resize();
            shell.log_layout_snapshot("first_run_reveal_after_stack_switch");

            let shell = Rc::clone(&shell);
            glib::timeout_add_local_once(
                Duration::from_millis(RESPONSIVE_RENDER_DELAY_MS),
                move || {
                    shell.log_layout_snapshot("first_run_reveal_before_render");
                    shell.state.startup_route_revealed.set(true);
                    shell.update_layout();
                    shell.render_current_route();
                    if matches!(shell.state.routes.borrow().current(), Route::Home) {
                        shell.refresh_home_for_current_visit();
                    }
                    shell.render_queue_panel();
                    shell.render_lyrics_panel();
                    shell.update_bottom_player();
                    shell.log_layout_snapshot("first_run_reveal_after_render");
                    shell.queue_post_layout_route_render();
                },
            );
        });
    }
    fn begin_first_run_cover_prime(self: &Rc<Self>) -> Option<u64> {
        let generation = self
            .state
            .first_run_cover_prime_generation
            .get()
            .saturating_add(1);
        self.state.first_run_cover_prime_generation.set(generation);
        self.state
            .first_run_cover_prime_pending
            .borrow_mut()
            .clear();

        let jobs = self.first_run_cover_prime_jobs();
        if jobs.is_empty() {
            return None;
        }

        let mut pending = HashSet::new();
        for job in jobs {
            if self.state.decoded_covers.borrow().contains_key(&job.key) {
                continue;
            }
            pending.insert(job.key.clone());
            if let Some(path) = self.controller.cached_cover_path_for_key(&job.key) {
                self.start_cover_decode_from_path(
                    job.key,
                    path,
                    job.size,
                    CoverDecodePriority::Visible,
                );
            } else {
                self.controller
                    .request_cover_for_key(job.key, job.image_ref, job.fetch_size);
            }
        }

        if pending.is_empty() {
            return None;
        }
        let pending_count = pending.len();
        *self.state.first_run_cover_prime_pending.borrow_mut() = pending;
        info!(covers = pending_count, "started first-run cover prime");
        Some(generation)
    }
    fn first_run_cover_prime_jobs(&self) -> Vec<FirstRunCoverPrimeJob> {
        let image_refs = first_run_cover_prime_refs(&self.state.library.borrow());
        let mut seen = HashSet::new();
        let mut jobs = Vec::new();
        for image_ref in image_refs {
            let Some(key) = self.cover_cache_key(&image_ref, GRID_COVER_SIZE) else {
                continue;
            };
            if !seen.insert(key.clone()) {
                continue;
            }
            jobs.push(FirstRunCoverPrimeJob {
                key,
                image_ref,
                fetch_size: GRID_COVER_SIZE,
                size: GRID_COVER_SIZE as i32,
            });
        }
        jobs
    }
    fn update_server_selector(self: &Rc<Self>) {
        source_selector::update_server_selector(self);
    }
    fn present_library_preferences_dialog(self: &Rc<Self>) {
        present_library_preferences_dialog(self);
    }
    fn rebuild_sidebar_navigation(self: &Rc<Self>) {
        rebuild_navigation(self);
        self.update_layout();
    }
    fn set_history_buttons_sensitive(&self, can_back: bool, can_forward: bool) {
        self.normal_back_button.set_sensitive(can_back);
        self.compact_back_button.set_sensitive(can_back);
        self.normal_forward_button.set_sensitive(can_forward);
        self.compact_forward_button.set_sensitive(can_forward);
    }
    fn queue_responsive_route_render(self: &Rc<Self>) {
        if !self.state.startup_route_revealed.get() && !self.login_screen_active() {
            return;
        }
        if !route_uses_responsive_cards(self.state.routes.borrow().current()) {
            return;
        }
        if self.state.responsive_render_queued.replace(true) {
            return;
        }

        let shell = Rc::clone(self);
        glib::timeout_add_local_once(
            Duration::from_millis(RESPONSIVE_RENDER_DELAY_MS),
            move || {
                if !shell.state.responsive_render_queued.replace(false) {
                    return;
                }
                shell.update_layout();
                if route_uses_responsive_cards(shell.state.routes.borrow().current()) {
                    shell.render_current_route();
                }
            },
        );
    }
    fn queue_post_layout_route_render(self: &Rc<Self>) {
        if !route_uses_responsive_cards(self.state.routes.borrow().current()) {
            return;
        }

        self.window.queue_resize();
        self.app_root.queue_resize();
        self.route_host.queue_resize();
        self.right_panel_slot.queue_resize();
        self.queue_responsive_route_render();

        let shell = Rc::clone(self);
        glib::timeout_add_local_once(
            Duration::from_millis(RESPONSIVE_RENDER_DELAY_MS * 4),
            move || {
                shell.state.responsive_render_queued.set(false);
                shell.update_layout();
                if route_uses_responsive_cards(shell.state.routes.borrow().current())
                    && !shell.login_screen_active()
                {
                    shell.render_current_route();
                }
            },
        );
    }
    fn notify_now_playing(&self, snapshot: &PlaybackSnapshot) {
        let settings = self.state.settings.borrow().clone();
        if !settings.notifications_enabled || settings.private_mode {
            return;
        }
        if !matches!(
            snapshot.state,
            PlaybackState::Playing | PlaybackState::Buffering
        ) {
            return;
        }
        let Some(entry) = snapshot.current.as_ref() else {
            return;
        };
        let notification = gio::Notification::new(&entry.title);
        notification.set_body(Some(&format!("{} - {}", entry.artist, entry.album)));
        self.application
            .send_notification(Some("now-playing"), &notification);
    }
    fn update_lyrics_highlight(self: &Rc<Self>) {
        self.cancel_scheduled_lyrics_highlight();
        self.update_lyrics_highlight_at(self.current_position_millis());
    }
    fn request_initial_lyrics_if_needed(&self) {
        let Some(track_id) = current_playback_track_id(&self.state.player.borrow()) else {
            return;
        };
        *self.state.lyrics_track_id.borrow_mut() = Some(track_id);
        self.request_auto_lyrics_if_needed();
    }
    fn request_auto_lyrics_if_needed(&self) {
        let Some(track_id) = current_playback_track_id(&self.state.player.borrow()) else {
            return;
        };
        if self.state.lyrics.borrow().is_some() {
            return;
        }
        let settings = self.state.settings.borrow();
        let request = auto_lyrics_request_for_settings(&settings, &track_id);
        drop(settings);
        let Some(request) = request else {
            return;
        };
        if !self
            .state
            .lyrics_auto_search_attempted
            .borrow_mut()
            .insert(track_id)
        {
            return;
        }
        match request {
            AutoLyricsRequest::Default => self.controller.request_lyrics_for_current(),
            AutoLyricsRequest::ServerOnly => self.controller.request_server_lyrics_for_current(),
        }
    }
    fn suppress_auto_lyrics_for_current(self: &Rc<Self>) {
        let Some(track_id) = current_playback_track_id(&self.state.player.borrow()) else {
            return;
        };
        {
            let mut attempted = self.state.lyrics_auto_search_attempted.borrow_mut();
            attempted.remove(&track_id);
        }
        {
            let mut settings = self.state.settings.borrow_mut();
            let id = track_id.as_str().to_string();
            if !settings.suppressed_auto_lyrics_track_ids.contains(&id) {
                settings.suppressed_auto_lyrics_track_ids.push(id);
                if let Err(error) = self.controller.save_settings(&settings) {
                    warn!(%error, "failed to save lyrics auto-search setting");
                }
            }
        }
        self.render_lyrics_panel();
    }
    fn lyrics_empty_status(&self) -> String {
        let settings = self.state.settings.borrow();
        if settings.private_mode {
            tr("No server lyrics for the current track. Private mode is on.")
        } else if !settings.external_lyrics_enabled {
            tr("No server lyrics for the current track. External lyric lookup is off.")
        } else {
            tr("No lyrics for the current track.")
        }
    }
    fn update_lyrics_highlight_at(self: &Rc<Self>, position_millis: u64) {
        let lyrics = self.state.lyrics.borrow();
        self.lyrics_pane
            .update_highlight(lyrics.as_ref(), position_millis);
        self.schedule_next_lyrics_highlight(position_millis);
    }
    fn current_position_millis(&self) -> u64 {
        self.state.player.borrow().position_millis
    }
    fn seek_to_lyrics_position(self: &Rc<Self>, position_millis: u64) {
        self.lyrics_pane.clear_follow_scroll_pause();
        self.controller.seek_millis(position_millis);
        self.update_lyrics_highlight_at(position_millis);
    }
}
