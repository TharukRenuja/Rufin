use super::*;

impl Shell {
    pub(in crate::ui) fn update_layout(self: &Rc<Self>) -> bool {
        let width = self.layout_width().max(1);
        let settings = self.state.settings.borrow().layout.clone();
        let resolved = resolve_layout_with_sidebar_widths(&settings, width, self.sidebar_widths());
        self.apply_resolved_layout(resolved)
    }
    pub(in crate::ui) fn apply_resolved_layout(self: &Rc<Self>, resolved: ResolvedLayout) -> bool {
        let login_active = self.login_screen_active();
        let startup_loading_active =
            startup_loading_screen_active(login_active, self.state.startup_route_revealed.get());
        if login_active {
            self.root_stack.set_visible_child(&self.login_host);
            self.state.fullscreen_player_visible.set(false);
            self.app_content_stack.set_visible_child_name("main");
            if let Some(tick) = self.fullscreen_player.animation_tick.borrow_mut().take() {
                tick.remove();
            }
            self.fullscreen_player.root.set_margin_top(0);
            self.fullscreen_player.root.set_opacity(0.0);
            self.fullscreen_player.root.set_can_target(false);
            self.fullscreen_player.root.set_sensitive(false);
            self.fullscreen_player.root.set_visible(false);
        } else if startup_loading_active {
            self.root_stack
                .set_visible_child(&self.startup_loading_host);
            self.state.fullscreen_player_visible.set(false);
            self.app_content_stack.set_visible_child_name("main");
            if let Some(tick) = self.fullscreen_player.animation_tick.borrow_mut().take() {
                tick.remove();
            }
            self.fullscreen_player.root.set_margin_top(0);
            self.fullscreen_player.root.set_opacity(0.0);
            self.fullscreen_player.root.set_can_target(false);
            self.fullscreen_player.root.set_sensitive(false);
            self.fullscreen_player.root.set_visible(false);
        } else {
            self.root_stack.set_visible_child(&self.app_root_overlay);
            self.app_content_stack.set_visible_child_name("main");
            let fullscreen_visible = self.state.fullscreen_player_visible.get();
            if self.fullscreen_player.animation_tick.borrow().is_none() {
                self.fullscreen_player.root.set_margin_top(0);
                self.fullscreen_player
                    .root
                    .set_opacity(if fullscreen_visible { 1.0 } else { 0.0 });
                self.fullscreen_player
                    .root
                    .set_can_target(fullscreen_visible);
                self.fullscreen_player
                    .root
                    .set_sensitive(fullscreen_visible);
                self.fullscreen_player.root.set_visible(true);
            }
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

        let app_active = !login_active && !startup_loading_active;
        let full_sidebar = resolved.left_sidebar == ResolvedLeftSidebarMode::Full;
        let hidden_sidebar = resolved.left_sidebar == ResolvedLeftSidebarMode::Hidden;
        self.split_view.set_collapsed(!app_active || !full_sidebar);
        if app_active && full_sidebar {
            self.split_view.set_show_sidebar(true);
        } else {
            self.split_view.set_show_sidebar(false);
        }
        self.normal_nav_slot.set_visible(app_active);
        self.compact_nav_slot
            .set_visible(app_active && resolved.left_sidebar == ResolvedLeftSidebarMode::Compact);
        self.tiny_nav_button
            .set_visible(app_active && hidden_sidebar);
        self.right_panel_slot.set_visible(
            !login_active && !startup_loading_active && resolved.right_sidebar.is_visible(),
        );
        set_scrolled_window_exact_content_width(
            &self.right_panel_slot,
            resolved.right_sidebar_width,
        );
        self.right_panel
            .set_width_request(resolved.right_sidebar_width);
        self.right_panel.set_visible(
            !login_active && !startup_loading_active && resolved.right_sidebar.is_visible(),
        );
        self.player_controls
            .root
            .set_visible(!login_active && !startup_loading_active);
        self.sync_current_route_boundary_width(resolved.main_width);
        self.update_right_panel_button();
        self.update_lyrics_panel_button();
        self.apply_bottom_player_width(self.layout_width());
        if self.state.fullscreen_player_visible.get() {
            self.refresh_fullscreen_player_layout();
        }

        let changed = previous_left != resolved.left_sidebar
            || previous_right != resolved.right_sidebar
            || previous_right_width != resolved.right_sidebar_width
            || previous_main_width != resolved.main_width;
        if changed {
            debug!(?resolved, "resolved layout changed");
            self.refit_route_column_views();
            self.queue_responsive_route_render();
        }
        if previous_right == RightSidebarMode::Hidden
            && resolved.right_sidebar.is_visible()
            && !login_active
            && !startup_loading_active
        {
            self.schedule_queue_panel_render();
        }
        self.log_layout_snapshot("apply_resolved_layout");
        changed
    }
    pub(in crate::ui) fn layout_width(&self) -> i32 {
        let root_width = self.root_stack.width();
        if root_width > 1 {
            return root_width;
        }

        let window_width = self.window.width();
        if window_width > 1 {
            return window_width;
        }

        self.window
            .surface()
            .map(|surface| surface.width())
            .filter(|width| *width > 1)
            .unwrap_or(1)
    }
    pub(in crate::ui) fn login_screen_active(&self) -> bool {
        self.state.library.borrow().first_run || self.state.first_run_connection_pending.get()
    }
    pub(in crate::ui) fn log_layout_snapshot(&self, stage: &'static str) {
        if std::env::var_os("RUFIN_DEBUG_LAYOUT").is_none() {
            return;
        }

        let route = self.state.routes.borrow().current().clone();
        let route_chain = widget_width_chain(&self.route_host.clone().upcast());
        info!(
            stage,
            ?route,
            login_active = self.login_screen_active(),
            startup_loading_active = startup_loading_screen_active(
                self.login_screen_active(),
                self.state.startup_route_revealed.get(),
            ),
            first_run = self.state.library.borrow().first_run,
            first_run_connection_pending = self.state.first_run_connection_pending.get(),
            first_run_connection_ready = self.state.first_run_connection_ready.get(),
            window_width = self.layout_width(),
            root_stack_width = self.root_stack.width(),
            app_root_width = self.app_root.width(),
            login_host_width = self.login_host.width(),
            startup_loading_host_width = self.startup_loading_host.width(),
            route_host_width = self.route_host.width(),
            resolved_main_width = self.state.main_content_width.get(),
            left_sidebar = ?self.state.resolved_left_sidebar.get(),
            split_collapsed = self.split_view.is_collapsed(),
            split_show_sidebar = self.split_view.shows_sidebar(),
            right_sidebar = ?self.state.resolved_right_sidebar.get(),
            right_panel_slot_visible = self.right_panel_slot.is_visible(),
            right_panel_slot_width = self.right_panel_slot.width(),
            right_panel_width = self.right_panel.width(),
            %route_chain,
            "layout snapshot"
        );
    }
}

impl Shell {
    fn sync_current_route_boundary_width(&self, width: i32) {
        if let Some(boundary) = self.state.current_route_boundary.borrow().as_ref() {
            apply_route_boundary_width(boundary, width);
            boundary.queue_resize();
            self.route_host.queue_resize();
        }
    }

    fn refit_route_column_views(self: &Rc<Self>) {
        library::refit_column_view_width_fits(&mut self.state.column_view_width_fits.borrow_mut());
        let shell = Rc::clone(self);
        glib::idle_add_local_once(move || {
            library::refit_column_view_width_fits(
                &mut shell.state.column_view_width_fits.borrow_mut(),
            );
        });
    }

    fn sidebar_widths(&self) -> SidebarWidths {
        SidebarWidths::default()
    }
}

fn widget_width_chain(widget: &gtk::Widget) -> String {
    let mut parts = Vec::new();
    let mut current = Some(widget.clone());
    while let Some(widget) = current {
        parts.push(format!("{}:{}", widget.type_().name(), widget.width()));
        current = widget.parent();
    }
    parts.join(" <- ")
}

fn set_scrolled_window_exact_content_width(scroller: &gtk::ScrolledWindow, width: i32) {
    scroller.set_width_request(width);
    scroller.set_max_content_width(-1);
    scroller.set_min_content_width(width);
    scroller.set_max_content_width(width);
}

pub(in crate::ui) fn startup_loading_screen_active(
    login_active: bool,
    startup_route_revealed: bool,
) -> bool {
    !login_active && !startup_route_revealed
}
