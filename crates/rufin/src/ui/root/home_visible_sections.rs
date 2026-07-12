use super::*;

impl Shell {
    pub(in crate::ui) fn register_home_section_view(
        &self,
        section_kind: HomeSectionKind,
        root: &gtk::Box,
        model: &gio::ListStore,
        content: HomeSectionContent,
        previous: &gtk::Button,
        next: &gtk::Button,
        page_size: usize,
    ) {
        if !matches!(self.state.routes.borrow().current(), Route::Home) {
            return;
        }

        self.state.home_section_views.borrow_mut().insert(
            section_kind,
            HomeSectionView {
                root: root.clone().upcast::<gtk::Widget>(),
                model: model.clone(),
                content,
                previous: previous.clone(),
                next: next.clone(),
                page_start: Rc::new(Cell::new(0)),
                page_size,
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
        update_home_section_page_model(&view, section);
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
    pub(in crate::ui) fn reset_visible_home_section_page(&self, section_kind: HomeSectionKind) {
        let view = self
            .state
            .home_section_views
            .borrow()
            .get(&section_kind)
            .cloned();
        if let Some(view) = view {
            view.page_start.set(0);
        }
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

        let view = self
            .state
            .home_section_views
            .borrow()
            .get(&section_kind)
            .cloned();
        let Some(view) = view else {
            return;
        };
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
        let item_count = home_section_item_count(view.content, &section);
        if item_count == 0 {
            return;
        }

        let page_size = view.page_size.max(1);
        match direction {
            HomeSectionPageDirection::Previous => {
                view.page_start
                    .set(view.page_start.get().saturating_sub(page_size));
            }
            HomeSectionPageDirection::Next => {
                let next_page = view.page_start.get().saturating_add(page_size);
                if next_page < item_count {
                    view.page_start.set(next_page);
                }
            }
        }
        update_home_section_page_model(&view, &section);
    }
}

#[derive(Clone, Copy)]
enum HomeSectionPageDirection {
    Previous,
    Next,
}

pub(in crate::ui) fn render_home_section_page_model(
    model: &gio::ListStore,
    content: HomeSectionContent,
    section: &HomeSection,
    page_start: usize,
    page_size: usize,
) -> (usize, usize) {
    let item_count = home_section_item_count(content, section);
    let page_size = page_size.max(1);
    let page_start = clamped_home_section_page_start(page_start, page_size, item_count);
    let page_end = page_start.saturating_add(page_size).min(item_count);
    match content {
        HomeSectionContent::Albums => {
            replace_home_section_page_model(
                model,
                section.albums[page_start..page_end].iter().cloned(),
            );
        }
        HomeSectionContent::Tracks => {
            replace_home_section_page_model(
                model,
                section.tracks[page_start..page_end].iter().cloned(),
            );
        }
    }
    (page_start, page_end)
}

fn replace_home_section_page_model<T: 'static>(
    model: &gio::ListStore,
    items: impl IntoIterator<Item = T>,
) {
    let additions = items
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}

fn update_home_section_page_model(view: &HomeSectionView, section: &HomeSection) {
    let (page_start, page_end) = render_home_section_page_model(
        &view.model,
        view.content,
        section,
        view.page_start.get(),
        view.page_size,
    );
    view.page_start.set(page_start);
    let item_count = home_section_item_count(view.content, section);
    view.previous.set_sensitive(page_start > 0);
    view.next.set_sensitive(page_end < item_count);
}

fn home_section_item_count(content: HomeSectionContent, section: &HomeSection) -> usize {
    match content {
        HomeSectionContent::Albums => section.albums.len(),
        HomeSectionContent::Tracks => section.tracks.len(),
    }
}

fn clamped_home_section_page_start(
    page_start: usize,
    page_size: usize,
    item_count: usize,
) -> usize {
    if item_count == 0 {
        return 0;
    }
    let page_size = page_size.max(1);
    let last_page_start = ((item_count - 1) / page_size) * page_size;
    page_start.min(last_page_start)
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
