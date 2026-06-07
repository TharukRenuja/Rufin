use super::*;

impl Shell {
    pub(in crate::ui) fn queue_responsive_route_render(self: &Rc<Self>) {
        if (!self.state.startup_route_revealed.get() && !self.login_screen_active())
            || self.state.startup_route_render_pending.get()
        {
            return;
        }
        if self.state.current_route_resize_policy.get() == RouteResizePolicy::StableOnWidthChange {
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
                if shell.state.startup_route_render_pending.get() {
                    return;
                }
                shell.update_layout();
                if shell.state.current_route_resize_policy.get()
                    == RouteResizePolicy::RerenderOnWidthChange
                {
                    let width = route_content_width(shell.as_ref());
                    if shell.state.width_sensitive_render_width.get() == width {
                        return;
                    }
                    shell.render_current_route_preserving_scroll();
                }
            },
        );
    }
    pub(in crate::ui) fn queue_post_layout_route_render(self: &Rc<Self>) {
        if self.state.current_route_resize_policy.get() == RouteResizePolicy::StableOnWidthChange {
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
                if shell.state.startup_route_render_pending.get() {
                    return;
                }
                shell.update_layout();
                if shell.state.current_route_resize_policy.get()
                    == RouteResizePolicy::RerenderOnWidthChange
                    && !shell.login_screen_active()
                {
                    let width = route_content_width(shell.as_ref());
                    if shell.state.width_sensitive_render_width.get() != width {
                        shell.render_current_route_preserving_scroll();
                    }
                }
            },
        );
    }
}
