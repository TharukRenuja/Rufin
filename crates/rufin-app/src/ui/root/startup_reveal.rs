impl Shell {
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

        let cover_prime_generation = Rc::new(Cell::new(None::<u64>));
        {
            let shell = Rc::clone(self);
            let cover_prime_generation = Rc::clone(&cover_prime_generation);
            glib::idle_add_local_once(move || {
                if shell.state.startup_route_revealed.get() || shell.login_screen_active() {
                    return;
                }
                cover_prime_generation.set(shell.begin_startup_cover_prime());
            });
        }
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
                let pending_covers = cover_prime_generation
                    .get()
                    .filter(|generation| {
                        shell.state.startup_cover_prime_generation.get() == *generation
                    })
                    .map(|_| shell.state.startup_cover_prime_pending.borrow().len())
                    .unwrap_or(usize::from(cover_prime_generation.get().is_none()));
                let reveal_ready =
                    width_ready
                        && pending_covers == 0
                        && elapsed >= Duration::from_millis(STARTUP_ROUTE_REVEAL_MIN_MS);
                let reveal_expired = elapsed >= Duration::from_millis(STARTUP_ROUTE_REVEAL_MAX_MS);
                if reveal_ready || reveal_expired {
                    if reveal_expired && pending_covers > 0 {
                        warn!(
                            pending_covers,
                            elapsed_ms = elapsed.as_millis() as u64,
                            "revealing startup route with cached cover prime still pending"
                        );
                    }
                    shell.reveal_startup_route();
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            },
        );
    }
    fn begin_startup_cover_prime(self: &Rc<Self>) -> Option<u64> {
        let generation = self
            .state
            .startup_cover_prime_generation
            .get()
            .saturating_add(1);
        self.state.startup_cover_prime_generation.set(generation);
        self.state.startup_cover_prime_pending.borrow_mut().clear();

        let jobs = startup_cover_prime_jobs(self);
        let mut pending_count = 0_usize;
        for job in jobs {
            if self
                .decoded_cover_for_ref(&job.image_ref, job.fetch_size, job.size)
                .is_some()
            {
                continue;
            }
            let Some((key, path)) =
                self.cached_cover_path_for_startup_prime(&job.image_ref, job.fetch_size)
            else {
                continue;
            };
            self.state
                .startup_cover_prime_pending
                .borrow_mut()
                .insert(key.clone());
            pending_count = pending_count.saturating_add(1);
            self.start_cover_decode_from_path(key, path, job.size, CoverDecodePriority::Warm);
        }

        if pending_count == 0 {
            None
        } else {
            info!(covers = pending_count, "started startup cached cover prime");
            Some(generation)
        }
    }
    fn cached_cover_path_for_startup_prime(
        &self,
        image_ref: &ImageRef,
        preferred_size: u32,
    ) -> Option<(String, PathBuf)> {
        for size in decoded_cover_candidate_sizes(preferred_size) {
            let key = self.cover_cache_key(image_ref, size)?;
            if let Some(path) = self.controller.cached_cover_path_for_key(&key) {
                return Some((key, path));
            }
        }
        None
    }
    fn reveal_startup_route(self: &Rc<Self>) {
        if self.state.startup_route_revealed.replace(true) || self.login_screen_active() {
            return;
        }

        self.update_layout();
        self.prewarm_startup_route_widgets();
        self.render_current_route();
    }
    fn prewarm_startup_route_widgets(self: &Rc<Self>) {
        let settings = self.state.settings.borrow().clone();
        self.prewarm_startup_artist_route(false);
        if sidebar_route_visible(&settings, SidebarRouteItem::AlbumArtists) {
            self.prewarm_startup_artist_route(true);
        }
    }
    fn prewarm_startup_artist_route(self: &Rc<Self>, album_artist: bool) {
        let route_name = if album_artist {
            "AlbumArtists"
        } else {
            "Artists"
        };
        let started = Instant::now();
        let view = route_boundary(self.library_artist_list_view(album_artist));
        view.set_visible(false);
        view.set_can_target(false);
        self.route_host.append(&view);
        self.route_host.remove(&view);
        if self.state.perf.is_some() {
            println!(
                "RUFIN_PERF_ROUTE_PREWARM route={} elapsed_ms={}",
                route_name,
                started.elapsed().as_millis() as u64
            );
        }
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
            if self.decoded_cover_has_min_size(&job.key, job.size) {
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
}
