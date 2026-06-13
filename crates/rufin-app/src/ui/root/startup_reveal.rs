use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum StartupRevealAction {
    Wait,
    RevealReady,
    RevealExpired,
}

pub(in crate::ui) fn startup_route_reveal_action(
    width_ready: bool,
    pending_covers: usize,
    elapsed: Duration,
) -> StartupRevealAction {
    if elapsed >= Duration::from_millis(STARTUP_ROUTE_REVEAL_MAX_MS) {
        return StartupRevealAction::RevealExpired;
    }
    if width_ready && pending_covers == 0 {
        return StartupRevealAction::RevealReady;
    }
    StartupRevealAction::Wait
}

pub(in crate::ui) fn startup_prime_action(
    pending_covers: usize,
    elapsed: Duration,
) -> StartupRevealAction {
    if pending_covers == 0 {
        StartupRevealAction::RevealReady
    } else if elapsed >= Duration::from_millis(PRIME_TIMEOUT_MS) {
        StartupRevealAction::RevealExpired
    } else {
        StartupRevealAction::Wait
    }
}

pub(in crate::ui) fn cover_warm_delay() -> u64 {
    WARM_SETTLE_MS
}

pub(in crate::ui) fn startup_stall_delay_ms(expected: Duration, observed: Duration) -> u64 {
    observed.saturating_sub(expected).as_millis() as u64
}

pub(in crate::ui) fn install_startup_stall_monitor(shell: &Rc<Shell>) {
    let started_at = Instant::now();
    let last_tick = Rc::new(Cell::new(started_at));
    let shell = Rc::clone(shell);
    glib::timeout_add_local(
        Duration::from_millis(STARTUP_STALL_MONITOR_INTERVAL_MS),
        move || {
            let now = Instant::now();
            let elapsed = now.duration_since(started_at);
            let expected = Duration::from_millis(STARTUP_STALL_MONITOR_INTERVAL_MS);
            let observed = now.duration_since(last_tick.get());
            last_tick.set(now);
            let delayed_ms = startup_stall_delay_ms(expected, observed);
            if delayed_ms >= STARTUP_STALL_WARN_MS {
                warn!(
                    delayed_ms,
                    observed_ms = observed.as_millis() as u64,
                    elapsed_ms = elapsed.as_millis() as u64,
                    route = ?shell.state.routes.borrow().current().clone(),
                    startup_revealed = shell.state.startup_route_revealed.get(),
                    startup_render_pending = shell.state.startup_route_render_pending.get(),
                    startup_content_prepared = shell.state.startup_route_content_prepared.get(),
                    cover_prime_pending = shell.state.startup_cover_prime_pending.borrow().len(),
                    route_cover_prime_pending = shell.state.route_cover_prime_pending.borrow().len(),
                    cover_path_lookups = shell.state.cover_path_lookups.borrow().len(),
                    cover_fetches = shell.state.cover_fetches.borrow().len(),
                    cover_visible_requests = shell.state.cover_visible_requests.borrow().len(),
                    cover_bindings = shell.state.cover_bindings.borrow().len(),
                    cover_decode_queue = shell.state.cover_decode_queue.borrow().len(),
                    cover_decodes = shell.state.cover_decodes.borrow().len(),
                    decoded_covers = shell.state.decoded_covers.borrow().len(),
                    cover_warm_pending = shell.state.cover_warm_pending.borrow().is_some(),
                    cover_warm_started = shell.state.cover_warm_started.borrow().is_some(),
                    "startup UI thread stalled"
                );
            }
            if elapsed >= Duration::from_millis(STARTUP_STALL_MONITOR_WINDOW_MS) {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        },
    );
}

pub(in crate::ui) fn take_pending_warm(
    pending: &mut Option<(ServerId, u64)>,
    server_id: &ServerId,
    token: u64,
) -> bool {
    if pending
        .as_ref()
        .is_some_and(|(pending_server_id, pending_token)| {
            pending_server_id == server_id && *pending_token == token
        })
    {
        pending.take();
        true
    } else {
        false
    }
}

impl Shell {
    pub(in crate::ui) fn render_startup_loading_view(&self) {
        self.route_title.set_title("Rufin");
        self.set_history_buttons_sensitive(false, false);
        self.root_stack
            .set_visible_child(&self.startup_loading_host);
        while let Some(child) = self.startup_loading_host.first_child() {
            self.startup_loading_host.remove(&child);
        }
        self.startup_loading_host
            .append(&self.startup_loading_view());
    }
    pub(in crate::ui) fn startup_loading_view(&self) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 0);
        wrapper.add_css_class("startup-loading-page");
        wrapper.set_hexpand(true);
        wrapper.set_vexpand(true);
        wrapper.set_halign(gtk::Align::Center);
        wrapper.set_valign(gtk::Align::Center);

        let spinner = gtk::Spinner::new();
        spinner.start();
        wrapper.append(&spinner);
        let status = if self.state.source_switch_preparing.get() {
            Some(tr("Switching library…"))
        } else {
            startup_loading_status_label(self.state.library.borrow().sync_status.as_str())
        };
        if let Some(status) = status {
            let (title, detail) = startup_loading_status_parts(&status);
            let label = gtk::Label::new(Some(&title));
            label.add_css_class("dim-label");
            label.add_css_class("startup-loading-status");
            label.set_wrap(true);
            wrapper.append(&label);
            if let Some(detail) = detail {
                let detail_label = gtk::Label::new(Some(&detail));
                detail_label.add_css_class("dim-label");
                detail_label.add_css_class("startup-loading-status-detail");
                detail_label.set_wrap(true);
                wrapper.append(&detail_label);
            }
        }
        wrapper.upcast()
    }
    pub(in crate::ui) fn schedule_startup_route_reveal(self: &Rc<Self>) {
        if self.state.startup_route_revealed.get() || self.login_screen_active() {
            return;
        }

        let cover_prime_generation = Rc::new(Cell::new(None::<u64>));
        {
            let shell = Rc::clone(self);
            let cover_prime_generation = Rc::clone(&cover_prime_generation);
            glib::timeout_add_local_once(
                Duration::from_millis(STARTUP_ROUTE_REVEAL_POLL_MS),
                move || {
                    if shell.state.startup_route_revealed.get() || shell.login_screen_active() {
                        return;
                    }
                    shell.prepare_startup_route_content();
                    cover_prime_generation.set(Some(shell.begin_startup_cover_prime()));
                },
            );
        }
        let started_at = Instant::now();
        let timeout_logged = Rc::new(Cell::new(false));
        let shell = Rc::clone(self);
        glib::timeout_add_local(
            Duration::from_millis(STARTUP_ROUTE_REVEAL_POLL_MS),
            move || {
                if shell.state.startup_route_revealed.get() || shell.login_screen_active() {
                    return glib::ControlFlow::Break;
                }

                shell.update_layout();
                let elapsed = started_at.elapsed();
                let width_ready = shell.layout_width() > 1 && shell.root_stack.width() > 1;
                shell.reconcile_startup_cover_prime_pending();
                let pending_covers = cover_prime_generation
                    .get()
                    .filter(|generation| {
                        shell.state.startup_cover_prime_generation.get() == *generation
                    })
                    .map(|_| shell.state.startup_cover_prime_pending.borrow().len())
                    .unwrap_or(usize::from(cover_prime_generation.get().is_none()));
                match startup_route_reveal_action(width_ready, pending_covers, elapsed) {
                    StartupRevealAction::RevealReady => {
                        shell.finish_startup_cover_prime_gate();
                        shell.reveal_startup_route();
                        glib::ControlFlow::Break
                    }
                    StartupRevealAction::RevealExpired => {
                        if pending_covers > 0 && !timeout_logged.replace(true) {
                            warn!(
                                pending_covers,
                                elapsed_ms = elapsed.as_millis() as u64,
                                "startup route cover prime expired"
                            );
                        }
                        shell.finish_startup_cover_prime_gate();
                        shell.reveal_startup_route();
                        glib::ControlFlow::Break
                    }
                    StartupRevealAction::Wait => glib::ControlFlow::Continue,
                }
            },
        );
    }
    pub(in crate::ui) fn prepare_startup_route_content(self: &Rc<Self>) {
        if self.state.startup_route_content_prepared.get()
            || self.state.startup_route_revealed.get()
            || self.login_screen_active()
        {
            return;
        }

        self.state.startup_route_render_pending.set(true);
        self.update_layout();
        self.log_layout_snapshot("startup_prepare_before_hidden_render");
        self.state.home_section_views.borrow_mut().clear();
        if matches!(self.state.routes.borrow().current(), Route::Home) {
            self.prepare_cached_home_entry();
        }
        self.render_current_route_content();
        self.render_queue_panel();
        self.render_lyrics_panel();
        self.update_bottom_player();
        self.state.startup_route_render_pending.set(false);
        self.state.startup_route_content_prepared.set(true);
        self.log_layout_snapshot("startup_prepare_after_hidden_render");
    }
    pub(in crate::ui) fn begin_startup_cover_prime(self: &Rc<Self>) -> u64 {
        let generation = self
            .state
            .startup_cover_prime_generation
            .get()
            .saturating_add(1);
        self.state.startup_cover_prime_generation.set(generation);
        self.state.startup_cover_prime_pending.borrow_mut().clear();

        let jobs = startup_cover_prime_jobs(self);
        let mut pending = HashSet::new();
        for job in jobs {
            if self
                .decoded_cover_for_ref(&job.image_ref, job.fetch_size, job.size)
                .is_some()
                || self.state.cover_unavailable.borrow().contains(&job.key)
            {
                continue;
            }
            pending.insert(job.key.clone());
            self.start_cached_cover_path_lookup(CoverPathLookupRequest {
                key: job.key,
                image_ref: job.image_ref,
                fetch_size: job.fetch_size,
                size: job.size,
                intent: CoverPathLookupIntent::StartupPrime,
            });
        }

        let pending_count = pending.len();
        *self.state.startup_cover_prime_pending.borrow_mut() = pending;
        if pending_count > 0 {
            info!(covers = pending_count, "started startup cover prime");
        }
        generation
    }
    fn finish_startup_cover_prime_gate(&self) {
        self.state.startup_cover_prime_generation.set(
            self.state
                .startup_cover_prime_generation
                .get()
                .saturating_add(1),
        );
        self.state.startup_cover_prime_pending.borrow_mut().clear();
    }
    pub(in crate::ui) fn reveal_startup_route(self: &Rc<Self>) {
        if self.login_screen_active() || self.state.startup_route_revealed.get() {
            return;
        }

        if !self.state.startup_route_content_prepared.get() {
            self.prepare_startup_route_content();
        }
        self.state.startup_route_revealed.set(true);

        self.log_layout_snapshot("startup_reveal_before_stack_switch");
        self.update_layout();
        restore_queue_lyrics_split_for_current_height(self);
        self.window.queue_resize();
        self.app_root.queue_resize();
        self.route_host.queue_resize();
        self.right_panel_slot.queue_resize();
        self.log_layout_snapshot("startup_reveal_after_stack_switch");
        if std::env::var_os("RUFIN_DEBUG_LAYOUT").is_some() {
            let shell = Rc::clone(self);
            glib::timeout_add_local_once(
                Duration::from_millis(RESPONSIVE_RENDER_DELAY_MS),
                move || shell.log_layout_snapshot("startup_reveal_after_allocation_tick"),
            );
        }
        self.queue_settled_warm();
    }
    pub(in crate::ui) fn schedule_first_run_app_reveal(self: &Rc<Self>) {
        self.log_layout_snapshot("first_run_reveal_queued");
        if let Some(generation) = self.begin_first_run_cover_prime() {
            let started_at = Instant::now();
            let timeout_logged = Rc::new(Cell::new(false));
            let shell = Rc::clone(self);
            glib::timeout_add_local(Duration::from_millis(PRIME_POLL_MS), move || {
                if shell.state.first_run_cover_prime_generation.get() != generation {
                    return glib::ControlFlow::Break;
                }
                shell.reconcile_prime_pending();
                let pending_covers = shell.state.first_run_cover_prime_pending.borrow().len();
                match startup_prime_action(pending_covers, started_at.elapsed()) {
                    StartupRevealAction::RevealReady => {
                        shell.finish_first_run_app_reveal();
                        glib::ControlFlow::Break
                    }
                    StartupRevealAction::RevealExpired => {
                        if pending_covers > 0 && !timeout_logged.replace(true) {
                            debug!(pending = pending_covers, "first-run cover prime expired");
                        }
                        shell.finish_first_run_app_reveal();
                        glib::ControlFlow::Break
                    }
                    StartupRevealAction::Wait => glib::ControlFlow::Continue,
                }
            });
            return;
        }

        self.finish_first_run_app_reveal();
    }
    pub(in crate::ui) fn finish_first_run_app_reveal(self: &Rc<Self>) {
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
            restore_queue_lyrics_split_for_current_height(&shell);
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
                    shell.state.startup_route_render_pending.set(false);
                    shell.state.startup_route_revealed.set(true);
                    shell.update_layout();
                    shell.render_current_route();
                    shell.state.startup_route_content_prepared.set(true);
                    shell.render_queue_panel();
                    shell.render_lyrics_panel();
                    shell.update_bottom_player();
                    shell.log_layout_snapshot("first_run_reveal_after_render");
                    shell.queue_post_layout_route_render();
                    shell.queue_settled_warm();
                },
            );
        });
    }
    pub(in crate::ui) fn queue_settled_warm(self: &Rc<Self>) {
        let Some(server_id) = self
            .state
            .library
            .borrow()
            .server
            .as_ref()
            .map(|server| server.id.clone())
        else {
            return;
        };
        if self
            .state
            .cover_warm_pending
            .borrow()
            .as_ref()
            .is_some_and(|(pending_server_id, _)| pending_server_id == &server_id)
        {
            return;
        }
        if self
            .state
            .cover_warm_started
            .borrow()
            .as_ref()
            .is_some_and(|started_server_id| started_server_id == &server_id)
        {
            return;
        }

        let token = self.state.cover_warm_token.get().saturating_add(1);
        self.state.cover_warm_token.set(token);
        *self.state.cover_warm_pending.borrow_mut() = Some((server_id.clone(), token));

        let shell = Rc::clone(self);
        glib::timeout_add_local_once(Duration::from_millis(cover_warm_delay()), move || {
            if !take_pending_warm(
                &mut shell.state.cover_warm_pending.borrow_mut(),
                &server_id,
                token,
            ) {
                return;
            }
            let active_server_id = shell
                .state
                .library
                .borrow()
                .server
                .as_ref()
                .map(|server| server.id.clone());
            if active_server_id.as_ref() != Some(&server_id)
                || !shell.state.startup_route_revealed.get()
                || shell.state.startup_route_render_pending.get()
            {
                return;
            }

            let smart_playlists = if sidebar_route_visible(
                &shell.state.settings.borrow(),
                SidebarRouteItem::SmartPlaylists,
            ) {
                Some(
                        shell
                            .controller
                            .cached_smart_playlists_page(0, 1_000)
                            .map(|page| page.items)
                            .unwrap_or_else(|error| {
                                warn!(%error, "failed to load cached smart playlists for source cover warm");
                                Vec::new()
                            }),
                    )
            } else {
                None
            };
            let active_server_id = shell
                .state
                .library
                .borrow()
                .server
                .as_ref()
                .map(|server| server.id.clone());
            if active_server_id.as_ref() != Some(&server_id) {
                return;
            }
            if let Some(smart_playlists) = smart_playlists.as_ref() {
                *shell.state.smart_playlists.borrow_mut() = smart_playlists.clone();
                shell.state.smart_playlists_loaded.set(true);
            }

            let targets = source_warm_targets(
                &shell.state.library.borrow(),
                smart_playlists.as_deref().unwrap_or_default(),
                &shell.state.settings.borrow(),
                shell.source_route_initial_cover_metrics(),
            );
            let target_count = targets.len();
            let queued = shell.schedule_warm_targets(targets);
            *shell.state.cover_warm_started.borrow_mut() = Some(server_id.clone());
            if target_count > 0 {
                debug!(
                    targets = target_count,
                    queued, "started source route cover warm"
                );
            }
        });
    }
    pub(in crate::ui) fn source_route_initial_cover_metrics(&self) -> InitialRouteCoverMetrics {
        let (grid_columns, grid_card_size) = self.responsive_card_grid_metrics();
        InitialRouteCoverMetrics {
            route_height: self.route_host.height(),
            app_height: self.app_root.height(),
            grid_columns,
            grid_card_size,
            home_showcase_seed: self.state.home_showcase_seed.get(),
        }
    }
    pub(in crate::ui) fn begin_first_run_cover_prime(self: &Rc<Self>) -> Option<u64> {
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
            if self
                .decoded_cover_for_ref(&job.image_ref, job.fetch_size, job.size)
                .is_some()
                || self.state.cover_unavailable.borrow().contains(&job.key)
            {
                continue;
            }
            pending.insert(job.key.clone());
            self.start_cached_cover_path_lookup(CoverPathLookupRequest {
                key: job.key,
                image_ref: job.image_ref,
                fetch_size: job.fetch_size,
                size: job.size,
                intent: CoverPathLookupIntent::StartupPrime,
            });
        }

        if pending.is_empty() {
            return None;
        }
        let pending_count = pending.len();
        *self.state.first_run_cover_prime_pending.borrow_mut() = pending;
        info!(covers = pending_count, "started first-run cover prime");
        Some(generation)
    }
    pub(in crate::ui) fn first_run_cover_prime_jobs(&self) -> Vec<FirstRunCoverPrimeJob> {
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

pub(in crate::ui) fn startup_loading_status_label(sync_status: &str) -> Option<String> {
    let status = sync_status.trim();
    if status.is_empty()
        || status == LIBRARY_SYNC_COMPLETE_STATUS
        || status == "Cached library ready"
        || status.starts_with("Library cache ready for ")
        || status == tr(LIBRARY_SYNC_COMPLETE_STATUS)
    {
        None
    } else {
        Some(status.to_string())
    }
}

pub(in crate::ui) fn startup_loading_status_parts(status: &str) -> (String, Option<String>) {
    const DETAIL_MARKER: &str = " This may take some time. ";
    let Some((title, detail)) = status.split_once(DETAIL_MARKER) else {
        return (status.to_string(), None);
    };
    (
        format!("{title} This may take some time."),
        (!detail.trim().is_empty()).then(|| detail.trim().to_string()),
    )
}

pub(in crate::ui) fn connection_progress_status_label(sync_status: &str) -> Option<String> {
    let Some(status) = startup_loading_status_label(sync_status) else {
        return Some(tr(LIBRARY_PREPARING_STATUS));
    };
    let (_title, detail) = startup_loading_status_parts(&status);
    Some(detail.unwrap_or_else(|| {
        if connection_progress_status_is_cache_headline(&status) {
            tr(LIBRARY_PREPARING_STATUS)
        } else {
            status
        }
    }))
}

fn connection_progress_status_is_cache_headline(status: &str) -> bool {
    status == "Caching library…"
        || status == "Caching local library…"
        || status == tr("Caching local library…")
}
