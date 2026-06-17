use super::*;

impl Shell {
    pub(in crate::ui) fn reset_cover_pipeline(&self) {
        self.state.cover_bindings.borrow_mut().clear();
        self.state.cover_unavailable.borrow_mut().clear();
        self.state.cover_path_lookups.borrow_mut().clear();
        self.state.cover_fetches.borrow_mut().clear();
        self.state.cover_visible_requests.borrow_mut().clear();
        self.state.cover_decode_queue.borrow_mut().clear();
        self.state.startup_cover_prime_pending.borrow_mut().clear();
        self.state
            .first_run_cover_prime_pending
            .borrow_mut()
            .clear();
        self.state.route_cover_prime_pending.borrow_mut().clear();
        self.state.cover_warm_pending.borrow_mut().take();
        self.state.cover_warm_started.borrow_mut().take();
        self.state.route_track_refs.borrow_mut().clear();
        self.state.smart_playlists.borrow_mut().clear();
        self.state.smart_playlists_loaded.set(false);
        self.cancel_cover_warm();
    }

    pub(in crate::ui) fn prepare_cover_retry(&self) {
        self.state.cover_unavailable.borrow_mut().clear();
        self.player_controls.cover_key.borrow_mut().take();
        self.fullscreen_player.cover_key.borrow_mut().take();
    }

    pub(in crate::ui) fn prepare_home_route(self: &Rc<Self>) {
        self.reset_cover_pipeline();
        let previous = self.state.routes.borrow().current().clone();
        self.state.routes.borrow_mut().navigate(Route::Home);
        self.handle_home_route_transition(&previous, &Route::Home);
    }

    pub(in crate::ui) fn navigate(self: &Rc<Self>, route: Route) {
        debug!(?route, "navigate");
        self.close_fullscreen_player();
        let previous = self.state.routes.borrow().current().clone();
        self.pause_cover_warm_for_nav();
        self.refresh_search_results_for_route(&route);
        self.state.routes.borrow_mut().navigate(route.clone());
        self.handle_home_route_transition(&previous, &route);
        self.request_current_route_render();
    }
    pub(in crate::ui) fn go_back(self: &Rc<Self>) {
        let previous = self.state.routes.borrow().current().clone();
        let route = self.state.routes.borrow_mut().back().cloned();
        if let Some(route) = route {
            debug!(?route, "navigate back");
            self.pause_cover_warm_for_nav();
            self.refresh_search_results_for_route(&route);
            self.handle_home_route_transition(&previous, &route);
            self.request_current_route_render();
        }
    }
    pub(in crate::ui) fn go_forward(self: &Rc<Self>) {
        let previous = self.state.routes.borrow().current().clone();
        let route = self.state.routes.borrow_mut().forward().cloned();
        if let Some(route) = route {
            debug!(?route, "navigate forward");
            self.pause_cover_warm_for_nav();
            self.refresh_search_results_for_route(&route);
            self.handle_home_route_transition(&previous, &route);
            self.request_current_route_render();
        }
    }
    pub(in crate::ui) fn refresh_search_results_for_route(self: &Rc<Self>, route: &Route) {
        self.start_search_for_route(route);
    }
}
