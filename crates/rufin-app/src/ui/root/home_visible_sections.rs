use super::*;

impl Shell {
    pub(in crate::ui) fn register_home_section_view(
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
    pub(in crate::ui) fn refresh_visible_home_section(
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
    pub(in crate::ui) fn refresh_changed_visible_home_sections(
        self: &Rc<Self>,
        previous_sections: &[HomeSection],
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
        for section_kind in changed_visible_home_section_kinds(
            section_kinds,
            previous_sections,
            sections,
            include_explore,
        ) {
            self.refresh_visible_home_section(section_kind, sections);
        }
    }
    pub(in crate::ui) fn render_visible_home_section(
        self: &Rc<Self>,
        section: &HomeSection,
    ) -> bool {
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
    pub(in crate::ui) fn hide_visible_home_section(&self, section_kind: HomeSectionKind) -> bool {
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
    pub(in crate::ui) fn show_previous_home_section_page(
        self: &Rc<Self>,
        section_kind: HomeSectionKind,
    ) {
        self.shift_visible_home_section_page(section_kind, HomeSectionPageDirection::Previous);
    }
    pub(in crate::ui) fn show_next_home_section_page(
        self: &Rc<Self>,
        section_kind: HomeSectionKind,
    ) {
        self.shift_visible_home_section_page(section_kind, HomeSectionPageDirection::Next);
    }
    fn shift_visible_home_section_page(
        self: &Rc<Self>,
        section_kind: HomeSectionKind,
        direction: HomeSectionPageDirection,
    ) {
        if !matches!(self.state.routes.borrow().current(), Route::Home) {
            return;
        }

        let section = self
            .state
            .library
            .borrow()
            .home_sections
            .iter()
            .find(|section| section.kind == section_kind)
            .cloned();
        let Some(section) = section else {
            return;
        };
        let item_count = if section.tracks.is_empty() {
            section.albums.len()
        } else {
            section.tracks.len()
        };
        if item_count == 0 {
            return;
        }

        {
            let mut states = self.state.home_section_state.borrow_mut();
            let state = states.entry(section_kind).or_insert(HomeSectionState {
                page_start: 0,
                page_size: 2,
            });
            match direction {
                HomeSectionPageDirection::Previous => {
                    state.page_start = state.page_start.saturating_sub(state.page_size);
                }
                HomeSectionPageDirection::Next => {
                    let next_page = state.page_start.saturating_add(state.page_size);
                    if next_page < item_count {
                        state.page_start = next_page;
                    }
                }
            }
        }

        self.render_visible_home_section(&section);
    }
}

#[derive(Clone, Copy)]
enum HomeSectionPageDirection {
    Previous,
    Next,
}

pub(in crate::ui) fn changed_visible_home_section_kinds(
    visible_kinds: impl IntoIterator<Item = HomeSectionKind>,
    previous_sections: &[HomeSection],
    sections: &[HomeSection],
    include_explore: bool,
) -> Vec<HomeSectionKind> {
    visible_kinds
        .into_iter()
        .filter(|section_kind| include_explore || *section_kind != HomeSectionKind::Explore)
        .filter(|section_kind| {
            let previous = home_section_by_kind(previous_sections, *section_kind);
            let next = home_section_by_kind(sections, *section_kind);
            previous != next
        })
        .collect()
}

fn home_section_by_kind(
    sections: &[HomeSection],
    section_kind: HomeSectionKind,
) -> Option<&HomeSection> {
    sections.iter().find(|section| section.kind == section_kind)
}
