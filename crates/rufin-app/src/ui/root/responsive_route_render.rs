use super::*;

impl Shell {
    pub(in crate::ui) fn queue_responsive_route_render(self: &Rc<Self>) {
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
    pub(in crate::ui) fn queue_post_layout_route_render(self: &Rc<Self>) {
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
}
