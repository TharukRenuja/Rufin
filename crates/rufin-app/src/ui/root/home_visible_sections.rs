impl Shell {
    fn register_home_section_view(
        &self,
        section_kind: HomeSectionKind,
        root: &gtk::Box,
        row: &gtk::Box,
        previous: &gtk::Button,
        next: &gtk::Button,
    ) {
        if !matches!(self.state.routes.borrow().current(), Route::Home) {
            return;
        }

        self.state.home_section_views.borrow_mut().insert(
            section_kind,
            HomeSectionView {
                root: root.clone().upcast::<gtk::Widget>(),
                row: row.clone(),
                previous: previous.clone(),
                next: next.clone(),
            },
        );
    }
    fn refresh_visible_home_section(
        self: &Rc<Self>,
        section_kind: HomeSectionKind,
        sections: &[HomeSection],
    ) {
        if !matches!(self.state.routes.borrow().current(), Route::Home) {
            return;
        }

        if let Some(section) = sections.iter().find(|section| section.kind == section_kind) {
            self.render_visible_home_section(section);
        } else {
            self.hide_visible_home_section(section_kind);
        }
    }
    fn refresh_visible_home_sections(
        self: &Rc<Self>,
        sections: &[HomeSection],
        include_explore: bool,
    ) {
        if !matches!(self.state.routes.borrow().current(), Route::Home) {
            return;
        }

        let section_kinds = self
            .state
            .home_section_views
            .borrow()
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for section_kind in section_kinds {
            if !include_explore && section_kind == HomeSectionKind::Explore {
                continue;
            }
            self.refresh_visible_home_section(section_kind, sections);
        }
    }
    fn render_visible_home_section(self: &Rc<Self>, section: &HomeSection) -> bool {
        let view = self
            .state
            .home_section_views
            .borrow()
            .get(&section.kind)
            .cloned();
        let Some(view) = view else {
            return false;
        };

        view.root.set_visible(true);
        if !section.tracks.is_empty() {
            cards::render_home_track_page(
                self,
                &view.row,
                &view.previous,
                &view.next,
                section.kind,
                &section.tracks,
            );
        } else {
            cards::render_home_album_page(
                self,
                &view.row,
                &view.previous,
                &view.next,
                section.kind,
                &section.albums,
            );
        }
        true
    }
    fn hide_visible_home_section(&self, section_kind: HomeSectionKind) -> bool {
        let view = self
            .state
            .home_section_views
            .borrow()
            .get(&section_kind)
            .cloned();
        let Some(view) = view else {
            return false;
        };
        view.root.set_visible(false);
        true
    }
}
