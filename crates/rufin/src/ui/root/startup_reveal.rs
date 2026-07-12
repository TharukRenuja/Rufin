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

pub(in crate::ui) fn main_loop_stall_delay_ms(expected: Duration, observed: Duration) -> u64 {
    observed.saturating_sub(expected).as_millis() as u64
}

pub(in crate::ui) fn install_startup_main_loop_stall_monitor(shell: &Rc<Shell>) {
    let started_at = Instant::now();
    let last_tick = Rc::new(Cell::new(started_at));
    let shell = Rc::clone(shell);
    glib::timeout_add_local(
        Duration::from_millis(STARTUP_MAIN_LOOP_STALL_MONITOR_INTERVAL_MS),
        move || {
            let now = Instant::now();
            let elapsed = now.duration_since(started_at);
            let expected = Duration::from_millis(STARTUP_MAIN_LOOP_STALL_MONITOR_INTERVAL_MS);
            let observed = now.duration_since(last_tick.get());
            last_tick.set(now);
            let delayed_ms = main_loop_stall_delay_ms(expected, observed);
            if delayed_ms >= STARTUP_MAIN_LOOP_STALL_LOG_MS {
                let startup_revealed = shell.state.startup_route_revealed.get();
                let phase = if startup_revealed {
                    "post_startup_reveal"
                } else {
                    "startup_reveal"
                };
                let cover = shell.cover_work_stats();
                if startup_revealed && delayed_ms < POST_REVEAL_MAIN_LOOP_STALL_WARN_MS {
                    debug!(
                        target: "rufin::ui::root::main_loop_stall",
                        delayed_ms,
                        observed_ms = observed.as_millis() as u64,
                        elapsed_ms = elapsed.as_millis() as u64,
                        phase,
                        route = ?shell.state.routes.borrow().current().clone(),
                        startup_revealed,
                        startup_render_pending = shell.state.startup_route_render_pending.get(),
                        startup_content_prepared = shell.state.startup_route_content_prepared.get(),
                        cover_prime_pending = cover.prime_pending,
                        cover_requests = cover.requests,
                        cover_bindings = cover.bindings,
                        "main loop stalled after startup route reveal"
                    );
                } else {
                    warn!(
                        target: "rufin::ui::root::main_loop_stall",
                        delayed_ms,
                        observed_ms = observed.as_millis() as u64,
                        elapsed_ms = elapsed.as_millis() as u64,
                        phase,
                        route = ?shell.state.routes.borrow().current().clone(),
                        startup_revealed,
                        startup_render_pending = shell.state.startup_route_render_pending.get(),
                        startup_content_prepared = shell.state.startup_route_content_prepared.get(),
                        cover_prime_pending = cover.prime_pending,
                        cover_requests = cover.requests,
                        cover_bindings = cover.bindings,
                        "main loop stalled during startup monitor window"
                    );
                }
            }
            if elapsed >= Duration::from_millis(STARTUP_MAIN_LOOP_STALL_MONITOR_WINDOW_MS) {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        },
    );
}

impl Shell {
    pub(in crate::ui) fn render_startup_loading_view(&self) {
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
        let status = self.library_loading_status();
        if let Some(status) = status {
            let label = gtk::Label::new(Some(&status));
            label.add_css_class("dim-label");
            label.add_css_class("startup-loading-status");
            label.set_wrap(true);
            wrapper.append(&label);
        }
        wrapper.upcast()
    }

    fn library_loading_status(&self) -> Option<String> {
        let load = self.state.library_load.borrow();
        match &*load {
            LibraryLoad::Switching { .. } => Some(tr("Switching library...")),
            LibraryLoad::Connecting { stage, .. } => Some(stage.clone()),
            LibraryLoad::Failed { message, .. } => Some(message.clone()),
            LibraryLoad::WaitingForFirstCommit { source_id } => self
                .state
                .source_syncs
                .borrow()
                .get(source_id)
                .map(source_sync_progress_text)
                .or_else(|| Some(library_preparing_status())),
            LibraryLoad::Ready => None,
        }
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
                let pending_covers =
                    shell.startup_cover_prime_pending_count(cover_prime_generation.get());
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
    }
    pub(in crate::ui) fn schedule_first_run_app_reveal(self: &Rc<Self>) {
        self.log_layout_snapshot("first_run_reveal_queued");
        *self.state.library_load.borrow_mut() = LibraryLoad::Ready;
        self.schedule_startup_route_reveal();
    }
}
