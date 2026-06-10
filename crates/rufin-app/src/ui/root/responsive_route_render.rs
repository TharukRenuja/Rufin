use super::*;

impl Shell {
    pub(in crate::ui) fn queue_responsive_route_render(self: &Rc<Self>) {
        if (!self.state.startup_route_revealed.get() && !self.login_screen_active())
            || self.state.startup_route_render_pending.get()
        {
            return;
        }
        if self.state.current_route_resize_policy.get() == RouteResizePolicy::Stable {
            return;
        }
        let diagnostics = route_resize_diagnostics_enabled();
        self.state.responsive_render_queued.set(true);
        let generation = self
            .state
            .responsive_render_generation
            .get()
            .saturating_add(1);
        self.state.responsive_render_generation.set(generation);

        let shell = Rc::clone(self);
        glib::timeout_add_local_once(
            Duration::from_millis(RESPONSIVE_ROUTE_SETTLE_MS),
            move || {
                if shell.state.responsive_render_generation.get() != generation {
                    return;
                }
                shell.state.responsive_render_queued.set(false);
                if shell.state.startup_route_render_pending.get() {
                    return;
                }
                shell.update_layout();
                let policy = shell.state.current_route_resize_policy.get();
                if policy == RouteResizePolicy::Stable || shell.login_screen_active() {
                    return;
                }
                let route = shell.state.routes.borrow().current().clone();
                let signature = shell.route_resize_signature(&route, policy);
                let previous_signature = shell.state.responsive_render_signature.get();
                if previous_signature == signature {
                    return;
                }
                if diagnostics {
                    info!(
                        ?route,
                        ?policy,
                        generation,
                        previous_signature,
                        signature,
                        "responsive route resize render"
                    );
                }
                shell.render_current_route_preserving_scroll();
            },
        );
    }
    pub(in crate::ui) fn queue_post_layout_route_render(self: &Rc<Self>) {
        if self.state.current_route_resize_policy.get() == RouteResizePolicy::Stable {
            return;
        }

        self.window.queue_resize();
        self.app_root.queue_resize();
        self.route_host.queue_resize();
        self.right_panel_slot.queue_resize();
        self.queue_responsive_route_render();
    }
}
