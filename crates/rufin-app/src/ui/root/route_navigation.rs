use super::*;

impl Shell {
    pub(in crate::ui) fn prepare_home_route_for_source_change(self: &Rc<Self>) {
        let previous = self.state.routes.borrow().current().clone();
        self.state.routes.borrow_mut().navigate(Route::Home);
        self.handle_home_route_transition(&previous, &Route::Home);
    }

    pub(in crate::ui) fn navigate(self: &Rc<Self>, route: Route) {
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
    pub(in crate::ui) fn go_back(self: &Rc<Self>) {
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
    pub(in crate::ui) fn go_forward(self: &Rc<Self>) {
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
    pub(in crate::ui) fn refresh_search_results_for_route(&self, route: &Route) {
        if let Route::Search { query, .. } = route {
            self.controller.search(query.clone());
        }
    }
}
