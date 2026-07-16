use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use super::route::Route;
use super::{PreparedHomeExplore, SourceHomeSection};
use crate::format_duration_units;
use crate::interactions::{add_link_hover, add_widget_click};
use crate::layout::{configure_fill_width_clip, width_allocation_owner};
use crate::localization::{bind_label_text_with, localized_label};
use crate::shell::Shell;
use crate::shell::cover::presentation::{add_album_seed_gradient_class, next_home_showcase_seed};
use crate::shell::route::MountedRoute;
use ::library::{
    ActiveLibraryQuery, Album, HomeBlockKind, HomeGenre, HomeOverview, HomeSection,
    HomeSectionKind, Track,
};
use adw::prelude::*;
use gtk::{gio, glib};
use localization::{album_count_text, msgid, track_count_text};
use playback::{RadioPlayRequest, RadioSeed};

use super::cards::album_cover_overlay;
use super::collection_routes::{MountedRefreshLoader, MountedRouteRefresh};
use super::collections::{home_album_row, home_track_row, library_route_inset};
use super::detail_links::album_artist_route;
use super::detail_showcase::{DetailSummaryProjection, detail_radio_button};
use super::home_layout::{
    HomeShowcaseMode, home_section_header, home_showcase_cover_size, home_showcase_is_compact,
    home_showcase_mode, home_showcase_spacing,
};
use super::route_layout::{
    ROUTE_TOP_MARGIN, home_album_content_width, home_album_page_size, route_scroller_widget,
};

pub(crate) const HOME_GENRE_LIMIT: usize = 12;

#[derive(Clone, Copy, Eq, PartialEq)]
enum HomeSectionContent {
    Albums,
    Tracks,
}

fn upsert_home_section(sections: &mut Vec<HomeSection>, section: HomeSection) {
    if let Some(current) = sections
        .iter_mut()
        .find(|current| current.kind == section.kind)
    {
        *current = section;
    } else {
        sections.push(section);
    }
}

fn replace_home_section_projection(
    sections: &mut Vec<HomeSection>,
    kind: HomeSectionKind,
    section: Option<HomeSection>,
) {
    match section {
        Some(section) => upsert_home_section(sections, section),
        None => sections.retain(|section| section.kind != kind),
    }
}

fn overlay_source_home_section(
    source_id: &::library::SourceId,
    sections: &mut Vec<HomeSection>,
    pending: Option<&SourceHomeSection>,
) {
    if let Some(pending) = pending.filter(|pending| &pending.source_id == source_id) {
        upsert_home_section(sections, pending.section.clone());
    }
}

fn home_projection_overlay_invalidated_by(delta: &::library::LibraryDelta) -> bool {
    delta.reset.is_some() || delta.home_changed
}

fn home_promotion_matches(
    pending: &SourceHomeSection,
    source_id: &::library::SourceId,
    section: &HomeSection,
) -> bool {
    &pending.source_id == source_id && &pending.section == section
}

fn add_card_label_link(
    shell: &Rc<Shell>,
    target: &gtk::Widget,
    label: &gtk::Label,
    text: &str,
    route: Option<Route>,
) {
    let Some(route) = route else {
        return;
    };
    target.set_cursor_from_name(Some("pointer"));
    label.set_cursor_from_name(Some("pointer"));
    add_link_hover(target, label, text);
    let shell = Rc::clone(shell);
    add_widget_click(target, move || shell.navigate(route.clone()));
}

fn render_home_section_page_model(
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
        HomeSectionContent::Albums => replace_home_section_page_model(
            model,
            section.albums[page_start..page_end].iter().cloned(),
        ),
        HomeSectionContent::Tracks => replace_home_section_page_model(
            model,
            section.tracks[page_start..page_end].iter().cloned(),
        ),
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

#[derive(Clone)]
struct MountedHomeSection {
    root: gtk::Box,
    presentation: MountedHomeSectionPresentation,
}

#[derive(Clone)]
struct MountedHomeSectionPresentation {
    shell: std::rc::Weak<Shell>,
    query: ActiveLibraryQuery,
    row_slot: gtk::Box,
    row: Rc<RefCell<Option<MountedHomeSectionRow>>>,
    previous: glib::WeakRef<gtk::Button>,
    next: glib::WeakRef<gtk::Button>,
    page_start: Rc<Cell<usize>>,
    page_size: Rc<Cell<usize>>,
    enabled: Rc<Cell<bool>>,
}

#[derive(Clone)]
enum MountedHomeSectionRow {
    Albums {
        model: gio::ListStore,
        row: super::grid_cells::FixedPageCollectionRow,
    },
    Tracks {
        model: gio::ListStore,
        row: super::grid_cells::FixedPageCollectionRow,
    },
}

impl MountedHomeSectionRow {
    fn content(&self) -> HomeSectionContent {
        match self {
            Self::Albums { .. } => HomeSectionContent::Albums,
            Self::Tracks { .. } => HomeSectionContent::Tracks,
        }
    }

    fn model(&self) -> &gio::ListStore {
        match self {
            Self::Albums { model, .. } | Self::Tracks { model, .. } => model,
        }
    }

    fn set_page_size(&self, page_size: usize) {
        match self {
            Self::Albums { row, .. } | Self::Tracks { row, .. } => row.set_page_size(page_size),
        }
    }
}

impl MountedHomeSection {
    fn replace(&self, section: Option<&HomeSection>) {
        self.presentation.enabled.set(section.is_some());
        self.root.set_visible(section.is_some());
        self.presentation.replace(section);
    }
}

impl MountedHomeSectionPresentation {
    fn ensure_row(&self, content: HomeSectionContent) -> Option<MountedHomeSectionRow> {
        if let Some(row) = self
            .row
            .borrow()
            .as_ref()
            .filter(|row| row.content() == content)
        {
            return Some(row.clone());
        }

        let shell = self.shell.upgrade()?;
        while let Some(child) = self.row_slot.first_child() {
            self.row_slot.remove(&child);
        }
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let row = match content {
            HomeSectionContent::Albums => {
                let row = home_album_row(
                    &shell,
                    model.clone(),
                    self.page_size.get(),
                    self.query.clone(),
                );
                self.row_slot.append(&row.widget());
                MountedHomeSectionRow::Albums { model, row }
            }
            HomeSectionContent::Tracks => {
                let row = home_track_row(&shell, model.clone(), self.page_size.get());
                self.row_slot.append(&row.widget());
                MountedHomeSectionRow::Tracks { model, row }
            }
        };
        self.page_start.set(0);
        self.row.replace(Some(row.clone()));
        Some(row)
    }

    fn replace(&self, section: Option<&HomeSection>) {
        let Some(section) = section else {
            if let Some(row) = self.row.borrow().as_ref() {
                row.model().remove_all();
            }
            self.page_start.set(0);
            if let Some(previous) = self.previous.upgrade() {
                previous.set_sensitive(false);
            }
            if let Some(next) = self.next.upgrade() {
                next.set_sensitive(false);
            }
            return;
        };
        let content = if section.tracks.is_empty() {
            HomeSectionContent::Albums
        } else {
            HomeSectionContent::Tracks
        };
        let Some(row) = self.ensure_row(content) else {
            return;
        };
        let (page_start, page_end) = render_home_section_page_model(
            row.model(),
            content,
            section,
            self.page_start.get(),
            self.page_size.get(),
        );
        self.page_start.set(page_start);
        let item_count = match content {
            HomeSectionContent::Albums => section.albums.len(),
            HomeSectionContent::Tracks => section.tracks.len(),
        };
        if let Some(previous) = self.previous.upgrade() {
            previous.set_sensitive(page_start > 0);
        }
        if let Some(next) = self.next.upgrade() {
            next.set_sensitive(page_end < item_count);
        }
    }

    fn shift(&self, section: &HomeSection, direction: HomeSectionPageDirection) {
        let item_count = if section.tracks.is_empty() {
            section.albums.len()
        } else {
            section.tracks.len()
        };
        if item_count == 0 {
            return;
        }
        let page_size = self.page_size.get().max(1);
        match direction {
            HomeSectionPageDirection::Previous => self
                .page_start
                .set(self.page_start.get().saturating_sub(page_size)),
            HomeSectionPageDirection::Next => {
                let next = self.page_start.get().saturating_add(page_size);
                if next < item_count {
                    self.page_start.set(next);
                }
            }
        }
        self.replace(Some(section));
    }

    fn fit_width(&self, width: i32, section: Option<&HomeSection>) {
        if width <= 1 {
            return;
        }
        let page_size = home_album_page_size(width, Some(self.page_size.get()));
        if self.page_size.replace(page_size) == page_size {
            return;
        }
        if let Some(row) = self.row.borrow().as_ref() {
            row.set_page_size(page_size);
        }
        self.replace(self.enabled.get().then_some(section).flatten());
    }

    fn reset_page(&self) {
        self.page_start.set(0);
    }
}

#[derive(Clone, Copy)]
enum HomeSectionPageDirection {
    Previous,
    Next,
}

#[derive(Clone)]
struct HomeRouteProjection {
    shell: Rc<Shell>,
    query: ActiveLibraryQuery,
    sections: Rc<RefCell<Vec<HomeSection>>>,
    genres: Rc<RefCell<Vec<HomeGenre>>>,
    showcase_fallback: Rc<RefCell<Option<Album>>>,
    section_views: Rc<RefCell<HashMap<HomeSectionKind, MountedHomeSection>>>,
    section_slots: Rc<HashMap<HomeSectionKind, gtk::Box>>,
    content: gtk::Box,
    block_roots: Rc<HashMap<HomeBlockKind, gtk::Widget>>,
    showcase_slot: Option<gtk::Box>,
    genres_slot: Option<gtk::Box>,
    empty: gtk::Widget,
    applied_blocks: Rc<RefCell<Vec<HomeBlockKind>>>,
}

impl HomeRouteProjection {
    fn replace(&self, data: HomeOverview) {
        if *self.sections.borrow() == data.sections
            && *self.genres.borrow() == data.genres
            && *self.showcase_fallback.borrow() == data.showcase_fallback
        {
            self.prepare_next_hidden_projection();
            return;
        }
        *self.sections.borrow_mut() = data.sections;
        *self.genres.borrow_mut() = data.genres;
        *self.showcase_fallback.borrow_mut() = data.showcase_fallback;
        self.update_mounted_models();
        self.apply_block_settings();
        self.prepare_next_hidden_projection();
    }

    fn prepare_next_hidden_projection(&self) {
        self.shell
            .prepare_next_home_explore_rotation(self.query.source_id(), &self.sections.borrow());
    }

    fn update_mounted_models(&self) {
        let sections = self.sections.borrow();
        let blocks = self.shell.settings.current.borrow().home_blocks.clone();
        let mut visible = false;
        let mut section_views = self.section_views.borrow_mut();
        for (kind, slot) in self.section_slots.iter() {
            let enabled = HomeBlockKind::all()
                .into_iter()
                .find(|block| block.section_kind() == Some(*kind))
                .is_some_and(|block| blocks.contains(&block));
            let section = enabled
                .then(|| sections.iter().find(|section| section.kind == *kind))
                .flatten();
            if let Some(section) = section {
                let view = section_views.entry(*kind).or_insert_with(|| {
                    let view = self.shell.mounted_home_section(
                        *kind,
                        None,
                        &self.query,
                        Rc::clone(&self.sections),
                    );
                    slot.append(&view.root);
                    view
                });
                view.replace(Some(section));
            } else if let Some(view) = section_views.remove(kind) {
                view.replace(None);
                slot.remove(&view.root);
            }
            visible |= section.is_some();
        }
        drop(section_views);
        visible |= self.update_showcase(&blocks);
        if let Some(slot) = &self.genres_slot {
            while let Some(child) = slot.first_child() {
                slot.remove(&child);
            }
            let genres = blocks
                .contains(&HomeBlockKind::Genres)
                .then(|| self.shell.home_genres_block(&self.genres.borrow()));
            if let Some(genres) = genres.flatten() {
                slot.append(&genres);
                slot.set_visible(true);
                visible = true;
            } else {
                slot.set_visible(false);
            }
        }
        self.empty.set_visible(!visible);
    }

    fn update_showcase(&self, blocks: &[HomeBlockKind]) -> bool {
        if let Some(slot) = &self.showcase_slot {
            while let Some(child) = slot.first_child() {
                slot.remove(&child);
            }
            let sections = self.sections.borrow();
            let showcase = blocks.contains(&HomeBlockKind::Showcase).then(|| {
                self.shell.home_showcase_block(
                    &sections,
                    self.showcase_fallback.borrow().as_ref(),
                    self.shell.library.home_showcase_seed.get(),
                    &self.query,
                )
            });
            if let Some(showcase) = showcase.flatten() {
                slot.append(&showcase);
                slot.set_visible(true);
                return true;
            } else {
                slot.set_visible(false);
            }
        }
        false
    }

    fn replace_section(
        &self,
        kind: HomeSectionKind,
        section: Option<HomeSection>,
        showcase_fallback: Option<Album>,
    ) {
        {
            let mut sections = self.sections.borrow_mut();
            replace_home_section_projection(&mut sections, kind, section);
        }
        *self.showcase_fallback.borrow_mut() = showcase_fallback;

        let blocks = self.shell.settings.current.borrow().home_blocks.clone();
        if let Some(slot) = self.section_slots.get(&kind) {
            let enabled = HomeBlockKind::all()
                .into_iter()
                .find(|block| block.section_kind() == Some(kind))
                .is_some_and(|block| blocks.contains(&block));
            let sections = self.sections.borrow();
            let section = enabled
                .then(|| sections.iter().find(|section| section.kind == kind))
                .flatten();
            let mut section_views = self.section_views.borrow_mut();
            if let Some(section) = section {
                let view = section_views.entry(kind).or_insert_with(|| {
                    let view = self.shell.mounted_home_section(
                        kind,
                        None,
                        &self.query,
                        Rc::clone(&self.sections),
                    );
                    slot.append(&view.root);
                    view
                });
                view.replace(Some(section));
            } else if let Some(view) = section_views.remove(&kind) {
                view.replace(None);
                slot.remove(&view.root);
            }
        }
        self.update_showcase(&blocks);
        self.apply_block_settings();
        self.prepare_next_hidden_projection();
    }

    fn apply_block_settings(&self) {
        let blocks = self.shell.settings.current.borrow().home_blocks.clone();
        let mut previous = None::<gtk::Widget>;
        for block in &blocks {
            let Some(root) = self.block_roots.get(block) else {
                continue;
            };
            self.content.reorder_child_after(root, previous.as_ref());
            previous = Some(root.clone());
        }
        for (block, root) in self.block_roots.iter() {
            root.set_visible(blocks.contains(block) && self.block_available(*block));
        }
        self.content
            .reorder_child_after(&self.empty, previous.as_ref());
        self.empty.set_visible(
            !blocks
                .iter()
                .filter_map(|block| self.block_roots.get(block))
                .any(gtk::Widget::is_visible),
        );
        *self.applied_blocks.borrow_mut() = blocks;
    }

    fn reconcile_block_settings(&self) {
        let blocks = self.shell.settings.current.borrow().home_blocks.clone();
        if *self.applied_blocks.borrow() == blocks {
            return;
        }
        self.update_mounted_models();
        self.apply_block_settings();
    }

    fn block_available(&self, block: HomeBlockKind) -> bool {
        match block {
            HomeBlockKind::Showcase => self
                .showcase_slot
                .as_ref()
                .is_some_and(|slot| slot.first_child().is_some()),
            HomeBlockKind::Genres => !self.genres.borrow().is_empty(),
            _ => block.section_kind().is_some_and(|kind| {
                self.sections
                    .borrow()
                    .iter()
                    .any(|section| section.kind == kind)
            }),
        }
    }
}

pub(crate) fn showcase_album(
    home_sections: &[HomeSection],
    fallback: Option<&Album>,
    seed: u64,
) -> Option<Album> {
    let mut seen = HashSet::new();
    let section_candidates = home_sections
        .iter()
        .filter(|section| section.kind != HomeSectionKind::Explore)
        .flat_map(|section| section.albums.iter())
        .filter(|album| seen.insert(album.id.clone()))
        .collect::<Vec<_>>();

    if !section_candidates.is_empty() {
        return section_candidates
            .get((seed as usize) % section_candidates.len())
            .map(|album| (*album).clone());
    }

    home_sections
        .iter()
        .find(|section| section.kind == HomeSectionKind::Explore)
        .and_then(|section| section.albums.first())
        .cloned()
        .or_else(|| fallback.cloned())
}

impl Shell {
    pub(crate) fn home_route_from_prepared(
        self: &Rc<Self>,
        library_query: ActiveLibraryQuery,
        mut home_data: HomeOverview,
    ) -> MountedRoute {
        self.overlay_pending_home_explore(library_query.source_id(), &mut home_data.sections);
        let scroller = gtk::ScrolledWindow::new();
        configure_fill_width_clip(&scroller, gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.add_css_class("route-content");
        content.set_hexpand(true);
        content.set_halign(gtk::Align::Fill);
        content.set_width_request(1);
        content.set_margin_top(ROUTE_TOP_MARGIN);
        content.set_margin_bottom(36);

        let sections = Rc::new(RefCell::new(home_data.sections));
        let genres = Rc::new(RefCell::new(home_data.genres));
        let showcase_fallback = Rc::new(RefCell::new(home_data.showcase_fallback));
        let section_views = HashMap::new();
        let mut section_slots = HashMap::new();
        let mut showcase_slot = None;
        let mut genres_slot = None;
        let mut block_roots = HashMap::new();
        for block in HomeBlockKind::all() {
            let slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
            slot.set_hexpand(true);
            content.append(&slot);
            block_roots.insert(block, slot.clone().upcast());
            match block {
                HomeBlockKind::Showcase => {
                    showcase_slot = Some(slot);
                }
                HomeBlockKind::Genres => {
                    genres_slot = Some(slot);
                }
                _ => {
                    let Some(kind) = block.section_kind() else {
                        continue;
                    };
                    section_slots.insert(kind, slot);
                }
            }
        }

        let empty =
            self.route_empty_view(msgid("Cached entries will appear here after sync finishes"));
        content.append(&empty);

        scroller.set_child(Some(&library_route_inset(content.clone().upcast())));
        let projection = HomeRouteProjection {
            shell: Rc::clone(self),
            query: library_query.clone(),
            sections,
            genres,
            showcase_fallback,
            section_views: Rc::new(RefCell::new(section_views)),
            section_slots: Rc::new(section_slots),
            content,
            block_roots: Rc::new(block_roots),
            showcase_slot,
            genres_slot,
            empty,
            applied_blocks: Rc::new(RefCell::new(Vec::new())),
        };
        projection.update_mounted_models();
        projection.apply_block_settings();
        projection.prepare_next_hidden_projection();
        let widget = route_scroller_widget(scroller);
        let apply_loaded: Rc<dyn Fn(Result<HomeOverview, String>)> = {
            let shell = Rc::clone(self);
            let apply_projection = projection.clone();
            let source_id = library_query.source_id().clone();
            Rc::new(move |result| {
                if apply_projection.content.root().is_none()
                    || !matches!(shell.navigation.routes.borrow().current(), Route::Home)
                    || shell
                        .library
                        .query
                        .borrow()
                        .as_ref()
                        .is_none_or(|query| query.source_id() != &source_id)
                {
                    return;
                }
                let mut data = match result {
                    Ok(data) => data,
                    Err(error) => {
                        tracing::warn!(%error, "failed to refresh mounted Home projection");
                        return;
                    }
                };
                shell.overlay_pending_home_explore(&source_id, &mut data.sections);
                apply_projection.replace(data);
            })
        };
        let load_query = library_query;
        let load: MountedRefreshLoader<Result<HomeOverview, String>> =
            Arc::new(move || load_query.home_overview(HOME_GENRE_LIMIT));
        let refresh = MountedRouteRefresh::new(Rc::downgrade(&apply_loaded), load, "mounted Home");
        let affected_by = Rc::new(home_projection_overlay_invalidated_by);
        let apply_delta = {
            let apply_loaded = Rc::clone(&apply_loaded);
            let refresh = Rc::clone(&refresh);
            Rc::new(move |_: &::library::LibraryDelta| {
                let _ = &apply_loaded;
                refresh.request();
            })
        };
        let resume_projection = projection.clone();
        let resume = Rc::new(move || {
            resume_projection.reconcile_block_settings();
        });
        let apply_home_section = {
            let projection = projection.clone();
            Rc::new(move |kind, section, showcase_fallback| {
                projection.replace_section(kind, section, showcase_fallback)
            })
        };
        MountedRoute::new(widget, affected_by, apply_delta, resume)
            .with_home_section_applier(apply_home_section)
    }

    fn home_showcase_block(
        self: &Rc<Self>,
        home_sections: &[HomeSection],
        showcase_fallback: Option<&Album>,
        seed: u64,
        query: &ActiveLibraryQuery,
    ) -> Option<gtk::Widget> {
        let width = home_album_content_width(self);
        let mode = home_showcase_mode(width);
        let cover_size = home_showcase_cover_size(width);
        let album = showcase_album(home_sections, showcase_fallback, seed)?;

        let section = gtk::Box::new(gtk::Orientation::Vertical, 10);
        section.set_hexpand(true);

        let body = gtk::Box::new(gtk::Orientation::Horizontal, home_showcase_spacing(width));
        body.add_css_class("home-showcase");
        add_album_seed_gradient_class(&body, album.color_seed);
        body.set_hexpand(true);
        body.set_halign(gtk::Align::Fill);
        body.set_valign(gtk::Align::Start);
        body.set_width_request(1);
        body.set_overflow(gtk::Overflow::Hidden);
        let cover = album_cover_overlay(
            self,
            &album,
            cover_size,
            &self.products.playback.queue,
            query,
        );
        cover.widget().add_css_class("home-showcase-cover");
        let cover_column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        cover_column.set_width_request(cover_size);
        cover_column.set_halign(gtk::Align::Start);
        cover_column.append(&cover.widget());
        body.append(&cover_column);

        let facts = DetailSummaryProjection::new(&[
            ("x-office-calendar-symbolic", album.year.to_string()),
            (
                "rufin-route-tracks-symbolic",
                track_count_text(album.track_count.into()),
            ),
            (
                "appointment-soon-symbolic",
                format_duration_units(album.duration_seconds),
            ),
        ]);
        let showcase_track_count = u64::from(album.track_count);
        facts.bind_text_with(1, move || track_count_text(showcase_track_count));
        let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
        metadata.set_hexpand(true);
        metadata.set_halign(gtk::Align::Fill);
        metadata.set_valign(gtk::Align::Center);
        metadata.set_width_request(1);
        metadata.set_visible(mode != HomeShowcaseMode::CoverOnly);

        metadata.append(&self.home_showcase_kind_row(&album));

        let title = gtk::Label::new(Some(&album.title));
        title.add_css_class("home-showcase-title");
        if home_showcase_is_compact(width) {
            title.add_css_class("home-showcase-title-compact");
        }
        title.set_xalign(0.0);
        title.set_wrap(true);
        title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        title.set_width_chars(1);
        metadata.append(&title);

        let artist = gtk::Label::new(Some(&album.artist));
        artist.add_css_class("muted");
        artist.set_xalign(0.0);
        artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
        artist.set_width_chars(1);
        add_card_label_link(
            self,
            artist.upcast_ref(),
            &artist,
            &album.artist,
            album_artist_route(&album),
        );
        metadata.append(&artist);
        metadata.append(&facts.widget());

        body.append(&metadata);
        section.append(&body);
        let allocated_width = Rc::new(Cell::new(width));
        let resize_body = body;
        let resize_metadata = metadata;
        let resize_title = title;
        let resize_cover_column = cover_column;
        let resize_cover = cover;
        // Resolve the showcase state before GTK measures its height for this
        // width. Applying it only during allocation leaves the parent with a
        // height measured for the previous cover/mode.
        let owner = width_allocation_owner(&section, move |width| {
            if width <= 1 || allocated_width.replace(width) == width {
                return;
            }
            let mode = home_showcase_mode(width);
            resize_body.set_spacing(home_showcase_spacing(width));
            resize_metadata.set_visible(mode != HomeShowcaseMode::CoverOnly);
            if home_showcase_is_compact(width) {
                resize_title.add_css_class("home-showcase-title-compact");
            } else {
                resize_title.remove_css_class("home-showcase-title-compact");
            }
            let cover_size = home_showcase_cover_size(width);
            resize_cover_column.set_width_request(cover_size);
            resize_cover.resize(cover_size);
        });
        Some(owner.upcast())
    }

    fn home_showcase_kind_row(self: &Rc<Self>, album: &Album) -> gtk::Box {
        let label = localized_label("Showcase");
        label.add_css_class("eyebrow");
        label.set_xalign(0.0);
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        label.set_margin_end(6);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        row.add_css_class("album-detail-kind-row");
        row.add_css_class("album-detail-genre-row");
        row.add_css_class("home-showcase-kind-row");
        row.set_valign(gtk::Align::Center);
        row.set_halign(gtk::Align::Start);
        row.append(&label);

        let radio = detail_radio_button();
        let controller = self.products.playback.radio.clone();
        let album = album.clone();
        radio.connect_clicked(move |_| {
            controller.play_radio(RadioPlayRequest::now(RadioSeed::Album(album.clone())));
        });
        row.append(&radio);
        row
    }

    fn home_genres_block(self: &Rc<Self>, genres: &[HomeGenre]) -> Option<gtk::Widget> {
        if genres.is_empty() {
            return None;
        }

        let section = gtk::Box::new(gtk::Orientation::Vertical, 10);
        section.set_hexpand(true);

        let heading = localized_label(HomeBlockKind::Genres.title());
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);
        section.append(&heading);

        let flow = gtk::FlowBox::new();
        flow.add_css_class("home-genre-flow");
        flow.set_column_spacing(8);
        flow.set_row_spacing(8);
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_max_children_per_line(6);
        flow.set_min_children_per_line(2);

        for genre in genres.iter().take(12) {
            flow.insert(&self.home_genre_chip(genre), -1);
        }

        section.append(&flow);
        Some(section.upcast())
    }

    fn home_genre_chip(self: &Rc<Self>, genre: &HomeGenre) -> gtk::Widget {
        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.add_css_class("home-genre-chip");
        button.set_hexpand(true);
        button.set_halign(gtk::Align::Fill);

        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        labels.set_margin_top(8);
        labels.set_margin_bottom(8);
        labels.set_margin_start(10);
        labels.set_margin_end(10);

        let name = gtk::Label::new(Some(&genre.name));
        name.add_css_class("album-title");
        name.set_xalign(0.0);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        labels.append(&name);

        let counts = gtk::Label::new(None);
        let album_count = u64::from(genre.album_count);
        let track_count = u64::from(genre.track_count);
        bind_label_text_with(&counts, move || {
            format!(
                "{} • {}",
                album_count_text(album_count),
                track_count_text(track_count)
            )
        });
        counts.add_css_class("muted");
        counts.set_xalign(0.0);
        counts.set_ellipsize(gtk::pango::EllipsizeMode::End);
        labels.append(&counts);

        button.set_child(Some(&labels));
        let shell = Rc::clone(self);
        let genre_id = genre.id.clone();
        button.connect_clicked(move |_| shell.navigate(Route::GenreDetail(genre_id.clone())));
        button.upcast()
    }

    fn mounted_home_section(
        self: &Rc<Self>,
        section_kind: HomeSectionKind,
        section_data: Option<&HomeSection>,
        query: &ActiveLibraryQuery,
        sections: Rc<RefCell<Vec<HomeSection>>>,
    ) -> MountedHomeSection {
        let section = gtk::Box::new(gtk::Orientation::Vertical, 10);
        section.set_hexpand(true);

        let header = home_section_header(section_kind.title());
        let previous = header.previous.clone();
        let next = header.next.clone();
        let refresh = header.refresh.clone();
        section.append(&header.root);

        let content_width = home_album_content_width(self);
        let page_size = home_album_page_size(content_width, None);
        let row_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
        row_slot.set_hexpand(true);
        let presentation = MountedHomeSectionPresentation {
            shell: Rc::downgrade(self),
            query: query.clone(),
            row_slot: row_slot.clone(),
            row: Rc::new(RefCell::new(None)),
            previous: previous.downgrade(),
            next: next.downgrade(),
            page_start: Rc::new(Cell::new(0)),
            page_size: Rc::new(Cell::new(page_size)),
            enabled: Rc::new(Cell::new(false)),
        };
        let fit_presentation = presentation.clone();
        let fit_sections = Rc::clone(&sections);
        // The number of cards determines the FlowBox height. Resolve it before
        // GTK's vertical height-for-width measurement so allocation and
        // measurement describe the same row.
        let width_owner = width_allocation_owner(&row_slot, move |width| {
            let sections = fit_sections.borrow();
            fit_presentation.fit_width(
                width,
                sections.iter().find(|section| section.kind == section_kind),
            );
        });
        section.append(&width_owner);
        let view = MountedHomeSection {
            root: section,
            presentation: presentation.clone(),
        };
        view.replace(section_data);

        let previous_view = presentation.clone();
        let previous_sections = Rc::clone(&sections);
        previous.connect_clicked(move |_| {
            if let Some(section) = previous_sections
                .borrow()
                .iter()
                .find(|section| section.kind == section_kind)
            {
                previous_view.shift(section, HomeSectionPageDirection::Previous);
            }
        });

        let next_view = presentation.clone();
        let next_sections = Rc::clone(&sections);
        next.connect_clicked(move |_| {
            if let Some(section) = next_sections
                .borrow()
                .iter()
                .find(|section| section.kind == section_kind)
            {
                next_view.shift(section, HomeSectionPageDirection::Next);
            }
        });

        let refresh_view = presentation;
        let library = self.products.library.clone();
        let source_id = query.source_id().clone();
        refresh.connect_clicked(move |_| {
            refresh_view.reset_page();
            library.refresh_home_section(source_id.clone(), section_kind);
        });
        view
    }
}

impl Shell {
    pub(crate) fn handle_home_route_transition(self: &Rc<Self>, previous: &Route, next: &Route) {
        if home_transition_prepares_hidden_projection(previous, next) {
            if self.home_explore_promotion_pending() {
                return;
            }
            self.library
                .home_showcase_seed
                .set(self.library.next_home_showcase_seed.get());
            self.library
                .next_home_showcase_seed
                .set(next_home_showcase_seed());
            self.prepare_cached_home_projection();
        }
    }

    pub(crate) fn prepare_cached_home_projection(self: &Rc<Self>) {
        let Some(query) = self.library.query.borrow().clone() else {
            return;
        };
        if self.home_explore_promotion_pending() {
            return;
        }
        let prepared = self.library.prepared_home_explore.borrow_mut().take();
        let Some(prepared) = prepared else {
            return;
        };
        let projection = prepared.projection();
        if &projection.source_id != query.source_id() {
            return;
        }
        let section = projection.section.clone();
        self.library
            .pending_home_explore
            .replace(Some(projection.clone()));
        self.products
            .library
            .save_explore_projection(projection.source_id.clone(), section);
    }

    fn home_explore_promotion_pending(&self) -> bool {
        let Some(query) = self.library.query.borrow().clone() else {
            return false;
        };
        let mut pending = self.library.pending_home_explore.borrow_mut();
        if pending
            .as_ref()
            .is_some_and(|pending| &pending.source_id == query.source_id())
        {
            return true;
        }
        pending.take();
        false
    }

    fn prepare_next_home_explore_rotation(
        &self,
        source_id: &::library::SourceId,
        sections: &[HomeSection],
    ) {
        let mut prepared = self.library.prepared_home_explore.borrow_mut();
        if prepared.as_ref().is_some_and(|prepared| {
            matches!(prepared, PreparedHomeExplore::Prefetched(_))
                && &prepared.projection().source_id == source_id
        }) {
            return;
        }
        *prepared = cached_explore_section(sections, self.library.next_home_showcase_seed.get())
            .map(|section| {
                PreparedHomeExplore::Rotation(SourceHomeSection {
                    source_id: source_id.clone(),
                    section,
                })
            });
    }

    pub(crate) fn remember_prefetched_home_explore(
        &self,
        source_id: ::library::SourceId,
        section: HomeSection,
    ) {
        if section.kind != HomeSectionKind::Explore
            || self
                .library
                .query
                .borrow()
                .as_ref()
                .is_none_or(|query| query.source_id() != &source_id)
        {
            return;
        }
        self.library
            .prepared_home_explore
            .replace(Some(PreparedHomeExplore::Prefetched(SourceHomeSection {
                source_id,
                section,
            })));
    }

    pub(crate) fn finish_home_explore_promotion(
        &self,
        source_id: &::library::SourceId,
        section: &HomeSection,
    ) {
        let mut pending = self.library.pending_home_explore.borrow_mut();
        if pending
            .as_ref()
            .is_some_and(|pending| home_promotion_matches(pending, source_id, section))
        {
            pending.take();
        }
    }

    fn overlay_pending_home_explore(
        &self,
        source_id: &::library::SourceId,
        sections: &mut Vec<HomeSection>,
    ) {
        overlay_source_home_section(
            source_id,
            sections,
            self.library.pending_home_explore.borrow().as_ref(),
        );
    }

    pub(crate) fn clear_home_projection_state(&self) {
        self.library.prepared_home_explore.borrow_mut().take();
        self.library.pending_home_explore.borrow_mut().take();
        self.library
            .home_showcase_seed
            .set(next_home_showcase_seed());
        self.library
            .next_home_showcase_seed
            .set(next_home_showcase_seed());
    }

    pub(crate) fn invalidate_home_projection_overlay_for(&self, delta: &::library::LibraryDelta) {
        if home_projection_overlay_invalidated_by(delta) {
            self.library.prepared_home_explore.borrow_mut().take();
            self.library.pending_home_explore.borrow_mut().take();
        }
    }
}

fn home_transition_prepares_hidden_projection(previous: &Route, next: &Route) -> bool {
    matches!(previous, Route::Home) && !matches!(next, Route::Home)
}

fn cached_explore_section(home_sections: &[HomeSection], seed: u64) -> Option<HomeSection> {
    if let Some(section) = home_sections
        .iter()
        .find(|section| section.kind == HomeSectionKind::Explore)
    {
        let mut section = section.clone();
        rotate_home_section(&mut section, seed);
        return Some(section);
    }

    let mut section_albums = Vec::new();
    for album in home_sections
        .iter()
        .filter(|section| section.kind != HomeSectionKind::Explore)
        .flat_map(|section| section.albums.iter())
    {
        if !section_albums
            .iter()
            .any(|existing: &Album| existing.id == album.id)
        {
            section_albums.push(album.clone());
        }
    }
    if !section_albums.is_empty() {
        rotate_items(&mut section_albums, seed);
        return Some(HomeSection {
            kind: HomeSectionKind::Explore,
            albums: section_albums,
            tracks: Vec::new(),
        });
    }

    let mut tracks = Vec::new();
    for track in home_sections
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

    showcase_album(home_sections, None, seed).map(|album| HomeSection {
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
    use ::library::{
        Album, AlbumId, EntityDelta, HomeSection, HomeSectionKind, LibraryDelta, LibraryReset,
        SourceId,
    };

    use super::{
        home_projection_overlay_invalidated_by, home_promotion_matches,
        home_transition_prepares_hidden_projection, overlay_source_home_section,
        replace_home_section_projection, showcase_album,
    };
    use crate::routes::SourceHomeSection;
    use crate::routes::route::Route;

    #[test]
    fn home_preparation_belongs_to_the_hidden_transition() {
        assert!(home_transition_prepares_hidden_projection(
            &Route::Home,
            &Route::Albums
        ));
        assert!(!home_transition_prepares_hidden_projection(
            &Route::Albums,
            &Route::Home
        ));
        assert!(!home_transition_prepares_hidden_projection(
            &Route::Home,
            &Route::Home
        ));
        assert!(!home_transition_prepares_hidden_projection(
            &Route::Albums,
            &Route::Tracks
        ));
    }

    #[test]
    fn pending_explore_overlays_stale_store_projection_before_reveal() {
        let source_id = SourceId::new("source:current");
        let stale = HomeSection {
            kind: HomeSectionKind::Explore,
            albums: vec![album(1)],
            tracks: Vec::new(),
        };
        let promoted = HomeSection {
            kind: HomeSectionKind::Explore,
            albums: vec![album(2)],
            tracks: Vec::new(),
        };
        let pending = SourceHomeSection {
            source_id: source_id.clone(),
            section: promoted.clone(),
        };
        let mut sections = vec![stale.clone()];

        overlay_source_home_section(&source_id, &mut sections, Some(&pending));

        assert_eq!(sections, vec![promoted]);
        let mut other_source_sections = vec![stale.clone()];
        overlay_source_home_section(
            &SourceId::new("source:other"),
            &mut other_source_sections,
            Some(&pending),
        );
        assert_eq!(other_source_sections, vec![stale]);
    }

    #[test]
    fn home_projection_overlay_is_invalidated_only_by_home_truth() {
        assert!(!home_projection_overlay_invalidated_by(&LibraryDelta {
            albums: EntityDelta {
                fields: vec![AlbumId::fake(1)],
                ..EntityDelta::default()
            },
            ..LibraryDelta::default()
        }));
        assert!(home_projection_overlay_invalidated_by(&LibraryDelta {
            home_changed: true,
            ..LibraryDelta::default()
        }));
        assert!(home_projection_overlay_invalidated_by(&LibraryDelta {
            reset: Some(LibraryReset::Source),
            ..LibraryDelta::default()
        }));
    }

    #[test]
    fn promotion_completion_only_releases_the_matching_pending_projection() {
        let source_id = SourceId::new("source:current");
        let section = HomeSection {
            kind: HomeSectionKind::Explore,
            albums: vec![album(1)],
            tracks: Vec::new(),
        };
        let pending = SourceHomeSection {
            source_id: source_id.clone(),
            section: section.clone(),
        };

        assert!(home_promotion_matches(&pending, &source_id, &section));
        assert!(!home_promotion_matches(
            &pending,
            &SourceId::new("source:other"),
            &section
        ));
        assert!(!home_promotion_matches(
            &pending,
            &source_id,
            &HomeSection {
                kind: HomeSectionKind::Explore,
                albums: vec![album(2)],
                tracks: Vec::new(),
            }
        ));
    }

    #[test]
    fn home_use_candidate() {
        let sections = vec![HomeSection {
            kind: HomeSectionKind::NewlyAdded,
            albums: vec![album(1), album(2), album(3)],
            tracks: Vec::new(),
        }];

        let first = showcase_album(&sections, None, 0).expect("first showcase album");
        let second = showcase_album(&sections, None, 1).expect("second showcase album");

        assert_eq!(first.id, AlbumId::fake(1));
        assert_eq!(second.id, AlbumId::fake(2));
    }

    #[test]
    fn home_showcase_possible() {
        let sections = vec![HomeSection {
            kind: HomeSectionKind::Explore,
            albums: vec![album(1)],
            tracks: Vec::new(),
        }];

        let selected = showcase_album(&sections, None, 0).expect("showcase album");

        assert_eq!(selected.id, AlbumId::fake(1));
    }

    #[test]
    fn sparse_home_uses_its_fallback_showcase() {
        let fallback = album(7);
        let selected = showcase_album(&[], Some(&fallback), 0).expect("showcase fallback");

        assert_eq!(selected.id, fallback.id);
    }

    #[test]
    fn explore_refresh_does_not_rotate_showcase() {
        let sections = vec![
            HomeSection {
                kind: HomeSectionKind::NewlyAdded,
                albums: vec![album(1), album(2), album(3)],
                tracks: Vec::new(),
            },
            HomeSection {
                kind: HomeSectionKind::Explore,
                albums: vec![album(1)],
                tracks: Vec::new(),
            },
        ];
        let before = showcase_album(&sections, None, 1).expect("showcase before refresh");
        let mut refreshed = sections;
        refreshed[1].albums = vec![album(2)];
        let after = showcase_album(&refreshed, None, 1).expect("showcase after refresh");

        assert_eq!(before.id, AlbumId::fake(2));
        assert_eq!(after.id, before.id);
    }

    #[test]
    fn exact_home_projection_preserves_other_sections() {
        let mut sections = vec![
            HomeSection {
                kind: HomeSectionKind::Explore,
                albums: vec![album(1)],
                tracks: Vec::new(),
            },
            HomeSection {
                kind: HomeSectionKind::NewlyAdded,
                albums: vec![album(2)],
                tracks: Vec::new(),
            },
        ];

        replace_home_section_projection(
            &mut sections,
            HomeSectionKind::Explore,
            Some(HomeSection {
                kind: HomeSectionKind::Explore,
                albums: vec![album(3)],
                tracks: Vec::new(),
            }),
        );

        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].albums[0].id, AlbumId::fake(3));
        assert_eq!(sections[1].kind, HomeSectionKind::NewlyAdded);
        assert_eq!(sections[1].albums[0].id, AlbumId::fake(2));
    }

    fn album(number: u32) -> Album {
        Album {
            id: AlbumId::fake(number),
            title: format!("Album {number}"),
            artist: "Artist".to_string(),
            artist_id: None,
            album_artist_credits: Vec::new(),
            artist_credits: Vec::new(),
            year: 2026,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            track_count: 1,
            duration_seconds: 60,
            favorite: false,
            color_seed: number,
            image_ref: None,
            genres: Vec::new(),
            release_types: Vec::new(),
            is_compilation: None,
            musicbrainz_album_id: None,
            musicbrainz_release_group_id: None,
        }
    }
}
