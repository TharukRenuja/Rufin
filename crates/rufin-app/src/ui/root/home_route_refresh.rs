use super::*;

impl Shell {
    pub(in crate::ui) fn handle_home_route_transition(
        self: &Rc<Self>,
        previous: &Route,
        next: &Route,
    ) {
        let was_home = matches!(previous, Route::Home);
        let is_home = matches!(next, Route::Home);
        let was_playlists = matches!(previous, Route::Playlists);
        let is_playlists = matches!(next, Route::Playlists);

        if is_home && !was_home {
            self.state.home_showcase_seed.set(next_home_showcase_seed());
            reset_home_section_pages(&mut self.state.home_section_state.borrow_mut());
            self.prepare_cached_home_entry();
        }
        if is_playlists && !was_playlists {
            self.state.playlist_refresh_started_for_visit.set(false);
        }
    }
    pub(in crate::ui) fn prepare_cached_home_entry(&self) {
        if self.apply_prefetched_explore_for_home_entry() {
            return;
        }
        self.rotate_cached_explore_for_home_entry();
    }
    fn apply_prefetched_explore_for_home_entry(&self) -> bool {
        let Some(prefetched) = self.state.prefetched_explore.borrow_mut().take() else {
            return false;
        };
        let Some(server_id) = self
            .state
            .library
            .borrow()
            .server
            .as_ref()
            .map(|server| server.id.clone())
        else {
            *self.state.prefetched_explore.borrow_mut() = Some(prefetched);
            return false;
        };
        if prefetched.server_id != server_id {
            *self.state.prefetched_explore.borrow_mut() = Some(prefetched);
            return false;
        }

        upsert_snapshot_home_section(
            &mut self.state.library.borrow_mut().home_sections,
            prefetched.section,
        );
        true
    }
    fn rotate_cached_explore_for_home_entry(&self) {
        let seed = self.state.home_showcase_seed.get();
        let section = {
            let library = self.state.library.borrow();
            let Some(section) = cached_explore_section(&library, seed) else {
                return;
            };
            section
        };
        upsert_snapshot_home_section(&mut self.state.library.borrow_mut().home_sections, section);
    }
    pub(in crate::ui) fn refresh_playlists_for_current_visit(self: &Rc<Self>) {
        if !matches!(self.state.routes.borrow().current(), Route::Playlists) {
            return;
        }
        if self.state.playlist_refresh_started_for_visit.replace(true) {
            return;
        }
        self.controller.refresh_playlists_for_active();
    }
    pub(in crate::ui) fn refresh_home_section(self: &Rc<Self>, section_kind: HomeSectionKind) {
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
    pub(in crate::ui) fn apply_prefetched_explore(self: &Rc<Self>) -> bool {
        let prefetched = self.state.prefetched_explore.borrow().clone();
        let promoted = prefetched
            .map(|prefetched| self.promote_prefetched_explore(prefetched, true))
            .unwrap_or(false);
        if promoted {
            self.controller.prefetch_explore_for_active();
        }
        promoted
    }
    pub(in crate::ui) fn promote_cached_prefetched_explore(self: &Rc<Self>) -> bool {
        let prefetched = self.state.prefetched_explore.borrow().clone();
        prefetched
            .map(|prefetched| self.promote_prefetched_explore(prefetched, false))
            .unwrap_or(false)
    }
    pub(in crate::ui) fn promote_prefetched_explore(
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
    pub(in crate::ui) fn update_prefetched_explore_from_snapshot(
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

fn cached_explore_section(library: &LibrarySnapshot, seed: u64) -> Option<HomeSection> {
    if let Some(section) = library
        .home_sections
        .iter()
        .find(|section| section.kind == HomeSectionKind::Explore)
    {
        let mut section = section.clone();
        rotate_home_section(&mut section, seed);
        return Some(section);
    }

    let mut albums = Vec::new();
    for album in library
        .home_sections
        .iter()
        .filter(|section| section.kind != HomeSectionKind::Explore)
        .flat_map(|section| section.albums.iter())
    {
        if !albums
            .iter()
            .any(|existing: &Album| existing.id == album.id)
        {
            albums.push(album.clone());
        }
    }
    if !albums.is_empty() {
        rotate_items(&mut albums, seed);
        return Some(HomeSection {
            kind: HomeSectionKind::Explore,
            albums,
            tracks: Vec::new(),
        });
    }

    let mut tracks = Vec::new();
    for track in library
        .home_sections
        .iter()
        .filter(|section| section.kind != HomeSectionKind::Explore)
        .flat_map(|section| section.tracks.iter())
    {
        if !tracks
            .iter()
            .any(|existing: &Track| existing.id == track.id)
        {
            tracks.push(track.clone());
        }
    }
    if !tracks.is_empty() {
        rotate_items(&mut tracks, seed);
        return Some(HomeSection {
            kind: HomeSectionKind::Explore,
            albums: Vec::new(),
            tracks,
        });
    }

    super::home::showcase_album(library, seed).map(|album| HomeSection {
        kind: HomeSectionKind::Explore,
        albums: vec![album],
        tracks: Vec::new(),
    })
}

fn rotate_home_section(section: &mut HomeSection, seed: u64) {
    rotate_items(&mut section.albums, seed);
    rotate_items(&mut section.tracks, seed);
}

fn rotate_items<T>(items: &mut [T], seed: u64) {
    if items.len() > 1 {
        items.rotate_left((seed as usize) % items.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_explore_rotation_keeps_existing_items() {
        let mut section =
            super::super::shell_tests::test_home_album_section(HomeSectionKind::Explore, 1);
        let second =
            super::super::shell_tests::test_home_album_section(HomeSectionKind::Explore, 2);
        let third = super::super::shell_tests::test_home_album_section(HomeSectionKind::Explore, 3);
        section.albums.extend(second.albums);
        section.albums.extend(third.albums);

        let mut library = super::super::shell_tests::test_library_snapshot();
        library.home_sections = vec![section];

        let rotated = cached_explore_section(&library, 1).expect("cached explore section");

        assert_eq!(rotated.kind, HomeSectionKind::Explore);
        assert_eq!(rotated.albums.len(), 3);
        assert_eq!(rotated.albums[0].id, AlbumId::fake(2));
    }
}
