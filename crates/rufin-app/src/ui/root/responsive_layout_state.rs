use super::*;

impl Shell {
    pub(in crate::ui) fn update_layout(self: &Rc<Self>) -> bool {
        let width = self.layout_width().max(1);
        let settings = self.state.settings.borrow().layout.clone();
        let resolved = resolve_layout(&settings, width);
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
        } else if startup_loading_active {
            self.root_stack
                .set_visible_child(&self.startup_loading_host);
            self.state.fullscreen_player_visible.set(false);
            self.app_content_stack.set_visible_child_name("main");
        } else {
            self.root_stack.set_visible_child(&self.app_root);
            let content_view = if self.state.fullscreen_player_visible.get() {
                "fullscreen-player"
            } else {
                "main"
            };
            self.app_content_stack.set_visible_child_name(content_view);
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

        self.normal_nav_slot.set_visible(
            !login_active
                && !startup_loading_active
                && resolved.left_sidebar == LeftSidebarMode::Full,
        );
        self.compact_nav_slot.set_visible(
            !login_active
                && !startup_loading_active
                && resolved.left_sidebar == LeftSidebarMode::Compact,
        );
        self.right_panel_slot.set_visible(
            !login_active && !startup_loading_active && resolved.right_sidebar.is_visible(),
        );
        self.right_panel_slot
            .set_width_request(resolved.right_sidebar_width);
        self.right_panel_slot
            .set_min_content_width(resolved.right_sidebar_width);
        self.right_panel_slot
            .set_max_content_width(resolved.right_sidebar_width);
        self.right_panel
            .set_width_request(resolved.right_sidebar_width);
        self.right_panel.set_visible(
            !login_active && !startup_loading_active && resolved.right_sidebar.is_visible(),
        );
        self.player_controls
            .root
            .set_visible(!login_active && !startup_loading_active);
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
    pub(in crate::ui) fn layout_width(&self) -> i32 {
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
    pub(in crate::ui) fn login_screen_active(&self) -> bool {
        self.state.library.borrow().first_run || self.state.first_run_connection_pending.get()
    }
    pub(in crate::ui) fn log_layout_snapshot(&self, stage: &'static str) {
        if std::env::var_os("RUFIN_DEBUG_LAYOUT").is_none() {
            return;
        }

        let route = self.state.routes.borrow().current().clone();
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
            right_sidebar = ?self.state.resolved_right_sidebar.get(),
            right_panel_slot_visible = self.right_panel_slot.is_visible(),
            right_panel_slot_width = self.right_panel_slot.width(),
            right_panel_width = self.right_panel.width(),
            "layout snapshot"
        );
    }
}

pub(in crate::ui) fn startup_loading_screen_active(
    login_active: bool,
    startup_route_revealed: bool,
) -> bool {
    !login_active && !startup_route_revealed
}
