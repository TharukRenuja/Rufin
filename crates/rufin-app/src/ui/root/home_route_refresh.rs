impl Shell {
    fn handle_home_route_transition(self: &Rc<Self>, previous: &Route, next: &Route) {
        let was_home = matches!(previous, Route::Home);
        let is_home = matches!(next, Route::Home);
        let was_playlists = matches!(previous, Route::Playlists);
        let is_playlists = matches!(next, Route::Playlists);

        if is_home && !was_home {
            self.state.home_refresh_started_for_visit.set(false);
            self.state.home_showcase_seed.set(next_home_showcase_seed());
            reset_home_section_pages(&mut self.state.home_section_state.borrow_mut());
            self.promote_cached_prefetched_explore();
        }
        if is_playlists && !was_playlists {
            self.state.playlist_refresh_started_for_visit.set(false);
        }
    }
    fn refresh_home_for_current_visit(self: &Rc<Self>) {
        if !matches!(self.state.routes.borrow().current(), Route::Home) {
            return;
        }
        if self.state.home_refresh_started_for_visit.replace(true) {
            return;
        }
        self.controller
            .refresh_home_sections_without_explore_for_active();
        self.controller.prefetch_explore_for_active();
    }
    fn refresh_playlists_for_current_visit(self: &Rc<Self>) {
        if !matches!(self.state.routes.borrow().current(), Route::Playlists) {
            return;
        }
        if self.state.playlist_refresh_started_for_visit.replace(true) {
            return;
        }
        self.controller.refresh_playlists_for_active();
    }
    fn refresh_home_section(self: &Rc<Self>, section_kind: HomeSectionKind) {
        if let Some(state) = self
            .state
            .home_section_state
            .borrow_mut()
            .get_mut(&section_kind)
        {
            state.page_start = 0;
        }

        if section_kind == HomeSectionKind::Explore && self.apply_prefetched_explore() {
            return;
        }

        self.controller
            .refresh_home_section_for_active(section_kind);
        if section_kind == HomeSectionKind::Explore {
            self.controller.prefetch_explore_for_active();
        }
    }
    fn apply_prefetched_explore(self: &Rc<Self>) -> bool {
        let prefetched = self.state.prefetched_explore.borrow().clone();
        let promoted = prefetched
            .map(|prefetched| self.promote_prefetched_explore(prefetched, true))
            .unwrap_or(false);
        if promoted {
            self.controller.prefetch_explore_for_active();
        }
        promoted
    }
    fn promote_cached_prefetched_explore(self: &Rc<Self>) -> bool {
        let prefetched = self.state.prefetched_explore.borrow().clone();
        prefetched
            .map(|prefetched| self.promote_prefetched_explore(prefetched, false))
            .unwrap_or(false)
    }
    fn promote_prefetched_explore(
        self: &Rc<Self>,
        prefetched: PrefetchedHomeSection,
        render_current_route: bool,
    ) -> bool {
        let Some(server_id) = self
            .state
            .library
            .borrow()
            .server
            .as_ref()
            .map(|server| server.id.clone())
        else {
            return false;
        };
        if prefetched.server_id != server_id {
            *self.state.prefetched_explore.borrow_mut() = Some(prefetched);
            return false;
        }

        let section = prefetched.section.clone();
        let mut changed = false;
        {
            let mut library = self.state.library.borrow_mut();
            let current = library
                .home_sections
                .iter()
                .find(|existing| existing.kind == section.kind);
            if current != Some(&section) {
                upsert_snapshot_home_section(&mut library.home_sections, section.clone());
                changed = true;
            }
        }
        if changed {
            reset_home_section_pages(&mut self.state.home_section_state.borrow_mut());
            self.controller
                .promote_prefetched_explore_for_active(section.clone());
        }
        if render_current_route {
            self.refresh_visible_home_section(section.kind, std::slice::from_ref(&section));
        }
        true
    }
    fn update_prefetched_explore_from_snapshot(
        &self,
        server_id: Option<rufin_core::ServerId>,
        prefetched: Option<PrefetchedHomeSection>,
        sections: &[HomeSection],
    ) {
        if prefetched.is_some() {
            *self.state.prefetched_explore.borrow_mut() = prefetched;
            return;
        }

        let keep_current = {
            let current = self.state.prefetched_explore.borrow();
            current.as_ref().is_some_and(|current| {
                server_id
                    .as_ref()
                    .is_some_and(|server_id| &current.server_id == server_id)
                    && !sections.iter().any(|section| {
                        section.kind == HomeSectionKind::Explore && section == &current.section
                    })
            })
        };
        if !keep_current {
            *self.state.prefetched_explore.borrow_mut() = None;
        }
    }
}
