use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use crate::format_duration;
use crate::routes::route::Route;
use ::library::{AcceptedTrackReplacement, MetadataItemId, RadioSeed, Track, TrackId};
use adw::prelude::*;
use artwork::ArtworkBinding;
use gtk::{gio, glib};
use playback::{OccurrenceId, QueuePage, QueuePageQuery, SequenceEntry};
use playback::{QueueHandle, QueueReorderRequest, RadioPlayRequest};

use crate::favorites::{
    FAVORITE_ADD_ICON, FAVORITE_REMOVE_ICON, favorite_button_is_active, favorite_icon_button,
    set_favorite_button_active,
};
use crate::interactions::add_dynamic_link_hover;
use crate::interactions::install_context_menu_openers;
use crate::interactions::{
    ADD_TO_PLAYLIST_ICON, ALBUM_ICON, ARTIST_ICON, ContextMenuSurface, RADIO_ICON,
    radio_context_submenu,
};
use crate::layout::width_allocation_owner;
use crate::preferences::dialogs::metadata::present_metadata_dialog;
use crate::routes::playlist_picker::{PlaylistTrackSource, install_context_menu_picker_action};
use crate::settings::ContextMenuItem;
use crate::shell::Shell;
use crate::shell::actions::{EDIT_ICON, PLAY_ICON, PLAY_LATER_ICON, PLAY_NEXT_ICON, REMOVE_ICON};
use crate::shell::cover::{ArtworkTile, THUMB_COVER_SIZE};
use localization::{msgid, tr};

const QUEUE_LINK_CLICK_DELAY_MS: u64 = 250;
const QUEUE_SEARCH_DELAY_MS: u64 = 120;
const QUEUE_FULLSCREEN_COLUMN_SPACING: i32 = 16;
const QUEUE_FULLSCREEN_ROW_HORIZONTAL_PADDING: i32 = 12;
const QUEUE_DRAG_HANDLE_WIDTH: i32 = 16;
const QUEUE_FULLSCREEN_COVER_COLUMN_WIDTH: i32 = 50;
const QUEUE_FULLSCREEN_TITLE_COLUMN_WIDTH: i32 = 320;
const QUEUE_FULLSCREEN_ALBUM_COLUMN_WIDTH: i32 = 260;
const QUEUE_FULLSCREEN_TITLE_MIN_WIDTH: i32 = 160;
const QUEUE_FULLSCREEN_ALBUM_MIN_WIDTH: i32 = 140;
const QUEUE_DURATION_COLUMN_WIDTH: i32 = 82;
const QUEUE_YEAR_COLUMN_WIDTH: i32 = 64;

pub(crate) struct QueueState {
    pub(crate) page: RefCell<Option<QueuePage>>,
    pub(crate) filter: RefCell<String>,
    pub(crate) page_request: RefCell<Option<QueuePageQuery>>,
    pub(crate) search_source: RefCell<Option<glib::SourceId>>,
    pub(crate) render_queued: Cell<bool>,
    sidebar_render_state: RefCell<Option<QueuePanelModelState>>,
    fullscreen_render_state: RefCell<Option<QueuePanelModelState>>,
}

impl QueueState {
    pub(crate) fn new(page: Option<QueuePage>) -> Self {
        Self {
            page: RefCell::new(page),
            filter: RefCell::new(String::new()),
            page_request: RefCell::new(None),
            search_source: RefCell::new(None),
            render_queued: Cell::new(false),
            sidebar_render_state: RefCell::new(None),
            fullscreen_render_state: RefCell::new(None),
        }
    }
}

const QUEUE_FAVORITE_COLUMN_WIDTH: i32 = 64;
const QUEUE_ROW_HEIGHT: i32 = 58;
const QUEUE_CURRENT_COMFORT_TOP: f64 = 0.25;
const QUEUE_CURRENT_COMFORT_BOTTOM: f64 = 0.70;
const QUEUE_CURRENT_TARGET: f64 = 0.42;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct QueueFullscreenColumnWidths {
    title: i32,
    album: i32,
    show_album: bool,
    show_year: bool,
}

struct QueueFullscreenColumnWidgets {
    title: gtk::Widget,
    album: gtk::Widget,
    year: gtk::Widget,
}

impl QueueFullscreenColumnWidgets {
    fn apply(&self, widths: QueueFullscreenColumnWidths) {
        self.title.set_width_request(widths.title);
        self.album.set_width_request(widths.album.max(1));
        self.album.set_visible(widths.show_album);
        self.year.set_visible(widths.show_year);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QueuePanelRenderState {
    query: Option<QueuePageQuery>,
    row_ids: Vec<OccurrenceId>,
    row_artwork: Vec<String>,
    row_tracks: Vec<Track>,
    row_indices: Vec<usize>,
    row_count: usize,
    current_row: Option<usize>,
    show_header: bool,
    empty_text: Option<String>,
    covered_height: i32,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum QueuePanelLayout {
    Sidebar,
    Fullscreen,
}

#[derive(Clone)]
enum QueuePanelItem {
    Entry {
        index: usize,
        entry: Box<SequenceEntry>,
        current: bool,
        reorderable: bool,
        layout: QueuePanelLayout,
    },
    Empty {
        text: String,
    },
    Covered {
        height: i32,
    },
}

struct QueuePanelModelState {
    render: Option<QueuePanelRenderState>,
    model: gio::ListStore,
}

#[derive(Clone)]
struct QueueSidebarBinding {
    index: usize,
    entry: SequenceEntry,
    reorderable: bool,
}

struct QueueSidebarRowSlot {
    stack: gtk::Stack,
    row: gtk::Box,
    drag: gtk::Image,
    cover: ArtworkTile,
    title: gtk::Label,
    artist: gtk::Label,
    year: gtk::Label,
    empty: gtk::Label,
    covered: gtk::Box,
    binding: Rc<RefCell<Option<QueueSidebarBinding>>>,
}

impl QueueSidebarRowSlot {
    fn new(shell: &Rc<Shell>) -> Self {
        let binding = Rc::new(RefCell::new(None::<QueueSidebarBinding>));
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_halign(gtk::Align::Fill);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("queue-row");
        row.set_height_request(QUEUE_ROW_HEIGHT);
        row.set_hexpand(true);
        row.set_halign(gtk::Align::Fill);
        row.set_valign(gtk::Align::Center);
        row.set_focusable(true);

        let drag = reusable_queue_drag_handle(&binding);
        row.append(&drag);

        let cover = ArtworkTile::new(50, 0);
        row.append(&cover.widget());

        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        labels.set_hexpand(true);
        labels.set_valign(gtk::Align::Center);
        let title = gtk::Label::new(None);
        title.add_css_class("queue-title");
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let artist = queue_link_label("");
        labels.append(&title);
        labels.append(&artist);
        row.append(&labels);

        let year = gtk::Label::new(None);
        year.add_css_class("muted");
        year.set_xalign(1.0);
        year.set_width_chars(4);
        year.set_halign(gtk::Align::End);
        row.append(&year);

        add_queue_label_link_style(&artist);
        let artist_shell = Rc::clone(shell);
        let artist_binding = Rc::clone(&binding);
        add_queue_label_click(&artist, move || {
            let route = artist_binding
                .borrow()
                .as_ref()
                .and_then(|binding| queue_artist_route(&binding.entry));
            if let Some(route) = route {
                artist_shell.navigate(route);
            }
        });

        install_reusable_queue_row_drop(&row, &shell.products.playback.queue, Rc::clone(&binding));
        install_reusable_queue_row_context_menu(&row, shell, Rc::clone(&binding));

        let empty = gtk::Label::new(None);
        empty.add_css_class("muted");
        empty.set_wrap(true);
        empty.set_margin_top(24);

        let covered = gtk::Box::new(gtk::Orientation::Vertical, 0);
        stack.add_named(&row, Some("entry"));
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&covered, Some("covered"));

        Self {
            stack,
            row,
            drag,
            cover,
            title,
            artist,
            year,
            empty,
            covered,
            binding,
        }
    }

    fn widget(&self) -> gtk::Widget {
        self.stack.clone().upcast()
    }

    fn bind(&self, shell: &Rc<Shell>, item: QueuePanelItem) {
        match item {
            QueuePanelItem::Entry {
                index,
                entry,
                current,
                reorderable,
                layout: QueuePanelLayout::Sidebar,
            } => {
                self.stack.set_visible_child_name("entry");
                if current {
                    self.row.add_css_class("queue-row-current");
                } else {
                    self.row.remove_css_class("queue-row-current");
                }
                self.drag.set_visible(reorderable);
                self.title.set_text(&entry.track.title);
                self.artist.set_text(&entry.track.artist);
                self.artist
                    .set_cursor_from_name(entry.track.primary_artist_id().map(|_| "pointer"));
                self.year.set_text(
                    &(entry.track.year != 0)
                        .then(|| entry.track.year.to_string())
                        .unwrap_or_default(),
                );
                let accessible_label = format!("{} {}", entry.track.title, entry.track.artist);
                self.row
                    .update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
                shell.bind_artwork_tile(
                    &self.cover,
                    ArtworkBinding::track(&entry.track),
                    index as u32 * 7 + entry.track.duration_seconds,
                    50,
                    THUMB_COVER_SIZE,
                );
                *self.binding.borrow_mut() = Some(QueueSidebarBinding {
                    index,
                    entry: *entry,
                    reorderable,
                });
            }
            QueuePanelItem::Empty { text } => {
                self.clear(shell);
                self.empty.set_text(&text);
                self.stack.set_visible_child_name("empty");
            }
            QueuePanelItem::Covered { height } => {
                self.clear(shell);
                self.covered.set_height_request(height.max(0));
                self.stack.set_visible_child_name("covered");
            }
            QueuePanelItem::Entry { .. } => {}
        }
    }

    fn clear(&self, shell: &Rc<Shell>) {
        shell.clear_artwork_tile(&self.cover);
        self.row.remove_css_class("queue-row-current");
        self.drag.set_visible(false);
        self.title.set_text("");
        self.artist.set_text("");
        self.artist.set_cursor_from_name(None);
        self.year.set_text("");
        self.empty.set_text("");
        self.covered.set_height_request(0);
        *self.binding.borrow_mut() = None;
    }
}

impl QueuePanelModelState {
    fn new() -> Self {
        Self {
            render: None,
            model: gio::ListStore::new::<glib::BoxedAnyObject>(),
        }
    }

    fn invalidate_render(&mut self) {
        self.render = None;
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum QueueScrollBehavior {
    Preserve,
    Start,
}

impl Shell {
    pub(crate) fn schedule_queue_panel_render(self: &Rc<Self>) {
        if self.queue.render_queued.replace(true) {
            return;
        }
        let shell = Rc::clone(self);
        glib::idle_add_local_once(move || {
            shell.queue.render_queued.set(false);
            shell.render_queue_panel();
        });
    }

    pub(crate) fn render_queue_panel(self: &Rc<Self>) {
        self.render_queue_panel_into(&self.right_panel.queue_panel, QueuePanelLayout::Sidebar);
        if self.fullscreen_player_visible() {
            self.render_queue_panel_into(
                &self.player_view.fullscreen_player.queue_panel,
                QueuePanelLayout::Fullscreen,
            );
        }
    }

    pub(crate) fn position_startup_queue_for_reveal(&self) -> bool {
        if !self.right_panel.root.is_visible() {
            return false;
        }
        let Some(scroller) = queue_panel_scroller(&self.right_panel.queue_panel) else {
            return false;
        };
        let current_row = self
            .queue
            .sidebar_render_state
            .borrow()
            .as_ref()
            .and_then(|state| state.render.as_ref())
            .and_then(|render| render.current_row);
        reveal_queue_current_row(&scroller, current_row)
    }

    pub(crate) fn invalidate_queue_panel_render_state(&self) {
        if let Some(state) = self.queue.sidebar_render_state.borrow_mut().as_mut() {
            state.invalidate_render();
        }
        if let Some(state) = self.queue.fullscreen_render_state.borrow_mut().as_mut() {
            state.invalidate_render();
        }
    }

    fn render_queue_panel_into(self: &Rc<Self>, panel: &gtk::Box, layout: QueuePanelLayout) {
        let queue_scroller = queue_panel_scroller(panel).unwrap_or_else(new_queue_scroller);
        let adjustment = queue_scroller.vadjustment();
        let previous_scroll = adjustment.value();
        let current_occurrence = self
            .playback
            .player
            .borrow()
            .as_ref()
            .and_then(|player| player.queue.current_occurrence.clone());
        let queue_page = self.queue.page.borrow();
        let covered_height = if layout == QueuePanelLayout::Sidebar
            && self.lyrics.panel_visible.get()
            && queue_page
                .as_ref()
                .is_some_and(|page| !page.rows.is_empty())
        {
            self.right_panel.lyrics_surface.height_request().max(0)
        } else {
            0
        };
        let render_state = queue_panel_render_state(
            queue_page.as_ref(),
            current_occurrence.as_ref(),
            covered_height,
        );
        let state_cell = match layout {
            QueuePanelLayout::Sidebar => &self.queue.sidebar_render_state,
            QueuePanelLayout::Fullscreen => &self.queue.fullscreen_render_state,
        };
        let model = {
            let mut state = state_cell.borrow_mut();
            state
                .get_or_insert_with(QueuePanelModelState::new)
                .model
                .clone()
        };
        ensure_queue_panel_view(self, panel, &queue_scroller, &model, layout);
        let scroll_behavior = state_cell
            .borrow()
            .as_ref()
            .and_then(|state| state.render.as_ref())
            .map_or(QueueScrollBehavior::Preserve, |previous| {
                queue_page_scroll_behavior(previous, &render_state)
            });

        if scroll_behavior == QueueScrollBehavior::Preserve {
            let updated_current = {
                let mut state = state_cell.borrow_mut();
                if let Some(state) = state.as_mut() {
                    if let Some(previous) = state.render.clone()
                        && previous.same_rows_as(&render_state)
                    {
                        update_queue_rows(
                            &state.model,
                            queue_page.as_ref(),
                            &previous,
                            &render_state,
                            layout,
                        );
                        if previous.current_row != render_state.current_row {
                            reveal_queue_current_row_later(
                                &queue_scroller,
                                render_state.current_row,
                            );
                        }
                        state.render = Some(render_state.clone());
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            };
            if updated_current {
                return;
            }
        }

        let rows = queue_panel_items(queue_page.as_ref(), &render_state, layout);
        replace_queue_model(&model, rows);
        clear_queue_panel_static_children(panel);
        if render_state.show_header {
            panel.prepend(&queue_header_row(layout));
        }
        if queue_scroller.parent().is_none() {
            panel.append(&queue_scroller);
        }
        match scroll_behavior {
            QueueScrollBehavior::Preserve => {
                restore_queue_scroll_position(&queue_scroller, previous_scroll);
                reveal_queue_current_row_later(&queue_scroller, render_state.current_row);
            }
            QueueScrollBehavior::Start => {
                restore_queue_scroll_position(&queue_scroller, 0.0);
            }
        }
        if let Some(state) = state_cell.borrow_mut().as_mut() {
            state.render = Some(render_state);
        }
    }

    fn queue_item_widget(self: &Rc<Self>, item: QueuePanelItem) -> gtk::Widget {
        match item {
            QueuePanelItem::Entry {
                index,
                entry,
                current,
                reorderable,
                layout,
            } => self.queue_row(index, &entry, current, reorderable, layout),
            QueuePanelItem::Empty { text } => queue_empty_row(&text),
            QueuePanelItem::Covered { height } => {
                let covered = gtk::Box::new(gtk::Orientation::Vertical, 0);
                covered.set_height_request(height.max(0));
                covered.upcast()
            }
        }
    }

    fn queue_row(
        self: &Rc<Self>,
        index: usize,
        entry: &SequenceEntry,
        current: bool,
        reorderable: bool,
        layout: QueuePanelLayout,
    ) -> gtk::Widget {
        if layout == QueuePanelLayout::Fullscreen {
            return self.fullscreen_queue_row(index, entry, current, reorderable);
        }

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("queue-row");
        row.set_height_request(QUEUE_ROW_HEIGHT);
        row.set_valign(gtk::Align::Center);
        row.set_focusable(true);
        let accessible_label = format!("{} {}", entry.track.title, entry.track.artist);
        row.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
        if current {
            row.add_css_class("queue-row-current");
        }
        if reorderable {
            row.append(&queue_drag_handle(&entry.occurrence));
        }
        let cover = self.cover_tile_for_candidates(
            ArtworkBinding::track(&entry.track),
            index as u32 * 7 + entry.track.duration_seconds,
            50,
            THUMB_COVER_SIZE,
        );
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        labels.set_hexpand(true);
        labels.set_valign(gtk::Align::Center);
        let title = gtk::Label::new(Some(&entry.track.title));
        title.add_css_class("queue-title");
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let artist = queue_link_label(&entry.track.artist);
        labels.append(&title);
        labels.append(&artist);
        if let Some(artist_id) = entry.track.primary_artist_id().cloned() {
            add_queue_label_link_style(&artist);
            let shell = Rc::clone(self);
            add_queue_label_click(&artist, move || {
                shell.navigate(Route::ArtistDetail(artist_id.clone()))
            });
        }
        let year_text = (entry.track.year != 0).then(|| entry.track.year.to_string());
        let year = gtk::Label::new(year_text.as_deref());
        year.add_css_class("muted");
        year.set_xalign(1.0);
        year.set_width_chars(4);
        year.set_halign(gtk::Align::End);
        row.append(&cover);
        row.append(&labels);
        row.append(&year);
        if reorderable {
            install_queue_row_drop(
                &row,
                &self.products.playback.queue,
                entry.occurrence.clone(),
                index,
            );
        }
        install_queue_row_context_menu(&row, self, entry);
        row.upcast()
    }

    fn fullscreen_queue_row(
        self: &Rc<Self>,
        index: usize,
        entry: &SequenceEntry,
        current: bool,
        reorderable: bool,
    ) -> gtk::Widget {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.add_css_class("queue-row");
        row.set_height_request(QUEUE_ROW_HEIGHT);
        row.set_hexpand(true);
        row.set_halign(gtk::Align::Fill);
        row.set_valign(gtk::Align::Center);
        row.set_focusable(true);
        let accessible_label = format!("{} {}", entry.track.title, entry.track.artist);
        row.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
        if current {
            row.add_css_class("queue-row-current");
        }

        let columns = fullscreen_queue_row_box();
        if reorderable {
            columns.append(&queue_drag_handle(&entry.occurrence));
        } else {
            columns.append(&fullscreen_queue_fixed_spacer(QUEUE_DRAG_HANDLE_WIDTH));
        }
        let cover = self.cover_tile_for_candidates(
            ArtworkBinding::track(&entry.track),
            index as u32 * 7 + entry.track.duration_seconds,
            QUEUE_FULLSCREEN_COVER_COLUMN_WIDTH,
            THUMB_COVER_SIZE,
        );

        let (labels, artist) = queue_identity_cell(entry);
        let album = fullscreen_queue_text_cell(&entry.track.album);
        let duration = fullscreen_queue_fixed_cell(
            &format_duration(entry.track.duration_seconds),
            QUEUE_DURATION_COLUMN_WIDTH,
        );
        let year_text = (entry.track.year != 0).then(|| entry.track.year.to_string());
        let year = fullscreen_queue_fixed_cell(
            year_text.as_deref().unwrap_or(""),
            QUEUE_YEAR_COLUMN_WIDTH,
        );

        columns.append(&cover);
        columns.append(&labels);
        columns.append(&album);
        columns.append(&duration);
        columns.append(&year);
        columns.append(&self.queue_favorite_cell(entry));
        row.append(&columns);
        let allocation_owner = fullscreen_queue_column_owner(
            &row,
            QueueFullscreenColumnWidgets {
                title: labels.clone().upcast(),
                album: album.upcast(),
                year: year.upcast(),
            },
        );

        if let Some(artist_id) = entry.track.primary_artist_id().cloned() {
            add_queue_label_link_style(&artist);
            let shell = Rc::clone(self);
            add_queue_label_click(&artist, move || {
                shell.navigate(Route::ArtistDetail(artist_id.clone()))
            });
        }

        if reorderable {
            install_queue_row_drop(
                &row,
                &self.products.playback.queue,
                entry.occurrence.clone(),
                index,
            );
        }
        install_queue_row_context_menu(&row, self, entry);
        allocation_owner
    }

    fn queue_favorite_cell(self: &Rc<Self>, entry: &SequenceEntry) -> gtk::Widget {
        let cell = gtk::CenterBox::new();
        cell.add_css_class("queue-favorite-cell");
        cell.set_width_request(QUEUE_FAVORITE_COLUMN_WIDTH);
        cell.set_halign(gtk::Align::Fill);

        let button = favorite_icon_button("Favorite");
        button.add_css_class("queue-favorite-button");
        set_favorite_button_active(
            &button,
            self.projected_track_favorite(&entry.track.id, entry.track.favorite),
        );

        let shell = Rc::clone(self);
        let track_id = entry.track.id.clone();
        button.connect_clicked(move |button| {
            let favorite = !favorite_button_is_active(button);
            shell.set_favorite_with_feedback(
                library::FavoriteItemId::Track(track_id.clone()),
                favorite,
                Some(button),
            );
        });

        cell.set_center_widget(Some(&button));
        cell.upcast()
    }
}

impl Shell {
    pub(crate) fn request_queue_page(self: &Rc<Self>, query: QueuePageQuery) {
        if self.queue.page_request.borrow().as_ref() == Some(&query) {
            return;
        }
        *self.queue.page_request.borrow_mut() = Some(query.clone());
        let Some(page) = self.products.playback.queue.request_page(query) else {
            self.queue.page_request.borrow_mut().take();
            return;
        };
        if self.apply_queue_page_projection(page) {
            Shell::schedule_queue_panel_render(self);
        }
    }

    fn queue_page_matches_filter(&self, page: &QueuePage) -> bool {
        let filter = self.queue.filter.borrow();
        if filter.trim().is_empty() {
            !page.query.is_filtered()
        } else {
            page.query == QueuePageQuery::search(&filter)
        }
    }

    fn request_queue_filter_page(self: &Rc<Self>) {
        if self.queue.search_source.borrow().is_some() {
            return;
        }
        let filter = self.queue.filter.borrow().clone();
        self.request_queue_page(QueuePageQuery::search(&filter));
    }

    fn accept_queue_page(&self, page: QueuePage) {
        self.queue.page_request.borrow_mut().take();
        *self.queue.page.borrow_mut() = Some(page);
    }

    pub(crate) fn apply_queue_page_projection(self: &Rc<Self>, page: QueuePage) -> bool {
        if !self.queue_page_matches_filter(&page) {
            self.request_queue_filter_page();
            return false;
        }
        self.accept_queue_page(page);
        true
    }

    pub(crate) fn apply_queue_track_replacements(
        self: &Rc<Self>,
        replacements: &[AcceptedTrackReplacement],
    ) {
        if replacements.is_empty() {
            return;
        }
        let revision = self
            .playback
            .player
            .borrow()
            .as_ref()
            .map(|player| player.queue.revision);
        let changed = self.queue.page.borrow_mut().as_mut().is_some_and(|page| {
            replace_queue_page_tracks(page, replacements, revision.unwrap_or(page.revision))
        });
        if changed {
            self.schedule_queue_panel_render();
        }
    }
}

impl QueuePanelRenderState {
    fn same_rows_as(&self, next: &Self) -> bool {
        self.query == next.query
            && self.row_ids == next.row_ids
            && self.row_artwork == next.row_artwork
            && self.row_indices == next.row_indices
            && self.row_count == next.row_count
            && self.show_header == next.show_header
            && self.empty_text == next.empty_text
    }
}

fn queue_panel_render_state(
    queue_page: Option<&QueuePage>,
    current_occurrence: Option<&OccurrenceId>,
    covered_height: i32,
) -> QueuePanelRenderState {
    let filter = queue_page
        .and_then(|page| page.query.search_text())
        .unwrap_or_default();
    let has_filter = !filter.is_empty();
    let mut queue_has_entries = false;
    let mut row_ids = Vec::new();
    let mut row_artwork = Vec::new();
    let mut row_tracks = Vec::new();
    let mut row_indices = Vec::new();
    let mut row_count = 0usize;
    let mut current_row = None;
    if let Some(page) = queue_page {
        queue_has_entries = page.total != 0;
        row_count = page.rows.len();
        for (row_position, row) in page.rows.iter().enumerate() {
            let entry = row.entry.as_ref();
            if current_occurrence == Some(&entry.occurrence) {
                current_row = Some(row_position);
            }
            row_ids.push(entry.occurrence.clone());
            row_artwork.push(ArtworkBinding::track(&entry.track).to_string());
            row_tracks.push(entry.track.clone());
            row_indices.push(row.absolute_index);
        }
    }
    let empty_text = (row_count == 0).then(|| {
        if has_filter && queue_has_entries {
            tr(r"No results ¯\_(°╭╮°)_/¯")
        } else {
            tr("Add music to start a queue.")
        }
    });
    QueuePanelRenderState {
        query: queue_page.map(|page| page.query.clone()),
        show_header: row_count != 0,
        row_ids,
        row_artwork,
        row_tracks,
        row_indices,
        row_count,
        current_row,
        empty_text,
        covered_height,
    }
}

fn queue_page_scroll_behavior(
    previous: &QueuePanelRenderState,
    next: &QueuePanelRenderState,
) -> QueueScrollBehavior {
    if previous.query == next.query
        || next
            .query
            .as_ref()
            .is_some_and(QueuePageQuery::follows_current)
    {
        QueueScrollBehavior::Preserve
    } else {
        QueueScrollBehavior::Start
    }
}

fn update_queue_rows(
    model: &gio::ListStore,
    queue_page: Option<&QueuePage>,
    previous: &QueuePanelRenderState,
    next: &QueuePanelRenderState,
    layout: QueuePanelLayout,
) {
    for position in queue_row_update_positions(previous, next) {
        let Some(row) = queue_panel_entry(queue_page, next, layout, position) else {
            continue;
        };
        model.splice(position as u32, 1, &[glib::BoxedAnyObject::new(row)]);
    }
    match (previous.covered_height > 0, next.covered_height > 0) {
        (true, true) => model.splice(
            next.row_count as u32,
            1,
            &[glib::BoxedAnyObject::new(QueuePanelItem::Covered {
                height: next.covered_height,
            })],
        ),
        (false, true) => model.append(&glib::BoxedAnyObject::new(QueuePanelItem::Covered {
            height: next.covered_height,
        })),
        (true, false) => model.remove(previous.row_count as u32),
        (false, false) => {}
    }
}

fn queue_row_update_positions(
    previous: &QueuePanelRenderState,
    next: &QueuePanelRenderState,
) -> Vec<usize> {
    let mut positions = Vec::new();
    for (position, (previous_track, next_track)) in
        previous.row_tracks.iter().zip(&next.row_tracks).enumerate()
    {
        if previous_track != next_track {
            positions.push(position);
        }
    }
    if previous.current_row != next.current_row {
        if let Some(position) = previous.current_row
            && !positions.contains(&position)
        {
            positions.push(position);
        }
        if let Some(position) = next.current_row
            && !positions.contains(&position)
        {
            positions.push(position);
        }
    }
    positions.sort_unstable();
    positions
}

fn replace_queue_page_tracks(
    page: &mut QueuePage,
    replacements: &[AcceptedTrackReplacement],
    revision: u64,
) -> bool {
    let replacements = replacements
        .iter()
        .filter_map(|replacement| {
            replacement
                .track
                .as_ref()
                .map(|track| (&replacement.id, track))
        })
        .collect::<std::collections::HashMap<&TrackId, &Track>>();
    let mut changed = false;
    for row in &mut page.rows {
        let Some(track) = replacements.get(&row.entry.track.id) else {
            continue;
        };
        if &row.entry.track == *track {
            continue;
        }
        let mut entry = row.entry.as_ref().clone();
        entry.track = (*track).clone();
        row.entry = std::sync::Arc::new(entry);
        changed = true;
    }
    if changed {
        page.revision = revision;
    }
    changed
}

fn queue_panel_items(
    queue_page: Option<&QueuePage>,
    state: &QueuePanelRenderState,
    layout: QueuePanelLayout,
) -> Vec<QueuePanelItem> {
    let mut items = Vec::new();
    for position in 0..state.row_count {
        if let Some(item) = queue_panel_entry(queue_page, state, layout, position) {
            items.push(item);
        }
    }
    if items.is_empty()
        && let Some(text) = state.empty_text.clone()
    {
        items.push(QueuePanelItem::Empty { text });
    } else if state.covered_height > 0 {
        items.push(QueuePanelItem::Covered {
            height: state.covered_height,
        });
    }
    items
}

fn queue_panel_entry(
    queue_page: Option<&QueuePage>,
    state: &QueuePanelRenderState,
    layout: QueuePanelLayout,
    row_position: usize,
) -> Option<QueuePanelItem> {
    let absolute_index = *state.row_indices.get(row_position)?;
    let row = queue_page?
        .rows
        .iter()
        .find(|row| row.absolute_index == absolute_index)?;
    Some(QueuePanelItem::Entry {
        index: absolute_index,
        entry: Box::new(row.entry.as_ref().clone()),
        current: state.current_row == Some(row_position),
        reorderable: state
            .query
            .as_ref()
            .is_some_and(|query| !query.is_filtered()),
        layout,
    })
}

fn replace_queue_model(model: &gio::ListStore, rows: Vec<QueuePanelItem>) {
    let additions = rows
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}

fn ensure_queue_panel_view(
    shell: &Rc<Shell>,
    panel: &gtk::Box,
    scroller: &gtk::ScrolledWindow,
    model: &gio::ListStore,
    layout: QueuePanelLayout,
) {
    let has_list_view = scroller
        .child()
        .is_some_and(|child| child.is::<gtk::ListView>());
    if !has_list_view {
        scroller.set_child(Some(&queue_list_view(shell, model, layout)));
    }
    if scroller.parent().is_none() {
        panel.append(scroller);
    }
}

fn queue_list_view(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    layout: QueuePanelLayout,
) -> gtk::ListView {
    let selection = gtk::NoSelection::new(Some(model.clone()));
    let factory = match layout {
        QueuePanelLayout::Sidebar => reusable_sidebar_queue_factory(shell),
        QueuePanelLayout::Fullscreen => rebuilding_queue_factory(shell),
    };
    let controller = shell.products.playback.queue.clone();

    let list = gtk::ListView::new(Some(selection), Some(factory));
    list.add_css_class("queue-list");
    list.set_vscroll_policy(gtk::ScrollablePolicy::Minimum);
    list.set_vexpand(true);
    let model = model.clone();
    list.connect_activate(move |_, position| {
        let Some(QueuePanelItem::Entry { entry, .. }) = queue_model_item_at(&model, position)
        else {
            return;
        };
        let controller = controller.clone();
        glib::idle_add_local_once(move || {
            controller.activate(entry.occurrence.clone());
        });
    });
    list
}

fn reusable_sidebar_queue_factory(shell: &Rc<Shell>) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let slots = Rc::new(RefCell::new(HashMap::<usize, QueueSidebarRowSlot>::new()));

    let setup_slots = Rc::clone(&slots);
    let setup_shell = Rc::clone(shell);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let slot = QueueSidebarRowSlot::new(&setup_shell);
        item.set_child(Some(&slot.widget()));
        setup_slots
            .borrow_mut()
            .insert(item.as_ptr() as usize, slot);
    });

    let bind_slots = Rc::clone(&slots);
    let bind_shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = queue_model_item_from_list_item(item) else {
            return;
        };
        if let Some(slot) = bind_slots.borrow().get(&(item.as_ptr() as usize)) {
            slot.bind(&bind_shell, row);
        }
    });

    let unbind_slots = Rc::clone(&slots);
    let unbind_shell = Rc::clone(shell);
    factory.connect_unbind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        if let Some(slot) = unbind_slots.borrow().get(&(item.as_ptr() as usize)) {
            slot.clear(&unbind_shell);
        }
    });

    let teardown_slots = Rc::clone(&slots);
    let teardown_shell = Rc::clone(shell);
    factory.connect_teardown(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        if let Some(slot) = teardown_slots
            .borrow_mut()
            .remove(&(item.as_ptr() as usize))
        {
            slot.clear(&teardown_shell);
        }
        item.set_child(None::<&gtk::Widget>);
    });

    factory
}

fn rebuilding_queue_factory(shell: &Rc<Shell>) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = queue_model_item_from_list_item(item) else {
            return;
        };
        let content = shell.queue_item_widget(row);
        content.set_hexpand(true);
        content.set_halign(gtk::Align::Fill);
        item.set_child(Some(&content));
    });
    factory.connect_unbind(clear_queue_list_item_child);
    factory
}

fn queue_model_item_at(model: &gio::ListStore, position: u32) -> Option<QueuePanelItem> {
    model
        .item(position)
        .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        .map(|boxed| boxed.borrow::<QueuePanelItem>().clone())
}

fn queue_model_item_from_list_item(item: &gtk::ListItem) -> Option<QueuePanelItem> {
    item.item()
        .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        .map(|boxed| boxed.borrow::<QueuePanelItem>().clone())
}

fn clear_queue_list_item_child(_: &gtk::SignalListItemFactory, item: &glib::Object) {
    if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
        item.set_child(None::<&gtk::Widget>);
    }
}

fn clear_queue_panel_static_children(panel: &gtk::Box) {
    let mut child = panel.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if !widget.is::<gtk::ScrolledWindow>() {
            panel.remove(&widget);
        }
    }
}

fn queue_empty_row(text: &str) -> gtk::Widget {
    let empty = gtk::Label::new(Some(text));
    empty.add_css_class("muted");
    empty.set_wrap(true);
    empty.set_margin_top(24);
    empty.upcast()
}

pub(crate) fn connect_queue_panel_controls(shell: &Rc<Shell>) {
    let filter_shell = Rc::clone(shell);
    shell
        .right_panel
        .queue_search
        .connect_search_changed(move |entry| {
            let text = entry.text().trim().to_string();
            *filter_shell.queue.filter.borrow_mut() = text.clone();
            if let Some(source) = filter_shell.queue.search_source.borrow_mut().take() {
                source.remove();
            }
            if text.is_empty() {
                filter_shell.request_queue_page(QueuePageQuery::current());
                return;
            }
            let search_shell = Rc::clone(&filter_shell);
            let source = glib::timeout_add_local_once(
                Duration::from_millis(QUEUE_SEARCH_DELAY_MS),
                move || {
                    search_shell.queue.search_source.borrow_mut().take();
                    let text = search_shell.queue.filter.borrow().clone();
                    search_shell.request_queue_page(QueuePageQuery::search(&text));
                },
            );
            *filter_shell.queue.search_source.borrow_mut() = Some(source);
        });

    let controller = shell.products.playback.queue.clone();
    shell
        .right_panel
        .queue_clear_button
        .connect_clicked(move |_| controller.clear());
}

fn new_queue_scroller() -> gtk::ScrolledWindow {
    let scroller = gtk::ScrolledWindow::new();
    scroller.add_css_class("queue-scroller");
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_vexpand(true);
    scroller
}

fn queue_panel_scroller(panel: &gtk::Box) -> Option<gtk::ScrolledWindow> {
    let mut child = panel.first_child();
    while let Some(widget) = child {
        if let Ok(scroller) = widget.clone().downcast::<gtk::ScrolledWindow>() {
            return Some(scroller);
        }
        child = widget.next_sibling();
    }
    None
}

fn restore_queue_scroll_position(scroller: &gtk::ScrolledWindow, value: f64) {
    let adjustment = scroller.vadjustment();
    let lower = adjustment.lower();
    let upper = (adjustment.upper() - adjustment.page_size()).max(lower);
    adjustment.set_value(value.clamp(lower, upper));
}

fn reveal_queue_current_row_later(scroller: &gtk::ScrolledWindow, current_row: Option<usize>) {
    let idle_scroller = scroller.clone();
    glib::idle_add_local_once(move || {
        reveal_queue_current_row(&idle_scroller, current_row);
    });

    let settled_scroller = scroller.clone();
    glib::timeout_add_local_once(Duration::from_millis(80), move || {
        reveal_queue_current_row(&settled_scroller, current_row);
    });
}

fn reveal_queue_current_row(scroller: &gtk::ScrolledWindow, current_row: Option<usize>) -> bool {
    let Some(current_row) = current_row else {
        return false;
    };
    let adjustment = scroller.vadjustment();
    let page_size = adjustment.page_size();
    if page_size <= 1.0 {
        return false;
    }
    let Some(target) = queue_current_row_scroll_target(current_row, adjustment.value(), page_size)
    else {
        return false;
    };
    let previous = adjustment.value();
    restore_queue_scroll_position(scroller, target);
    adjustment.value() != previous
}

fn queue_current_row_scroll_target(
    current_row: usize,
    scroll_value: f64,
    page_size: f64,
) -> Option<f64> {
    let row_height = f64::from(QUEUE_ROW_HEIGHT);
    let row_top = current_row as f64 * row_height;
    let row_bottom = row_top + row_height;
    let comfort_top = scroll_value + page_size * QUEUE_CURRENT_COMFORT_TOP;
    let comfort_bottom = scroll_value + page_size * QUEUE_CURRENT_COMFORT_BOTTOM;
    if row_top >= comfort_top && row_bottom <= comfort_bottom {
        return None;
    }
    let target_offset = if page_size >= row_height * 3.0 {
        page_size * QUEUE_CURRENT_TARGET
    } else {
        (page_size - row_height).max(0.0) / 2.0
    };
    Some(row_top - target_offset)
}

fn queue_header_row(layout: QueuePanelLayout) -> gtk::Widget {
    if layout == QueuePanelLayout::Fullscreen {
        return fullscreen_queue_header_row();
    }

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("queue-header");
    header.set_valign(gtk::Align::Center);

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_width_request(QUEUE_DRAG_HANDLE_WIDTH);
    header.append(&spacer);

    let title = gtk::Label::new(Some(&tr("Title").to_uppercase()));
    title.add_css_class("muted");
    title.set_xalign(0.0);
    title.set_hexpand(true);
    header.append(&title);

    let year = gtk::Label::new(Some(&tr("Year").to_uppercase()));
    year.add_css_class("muted");
    year.set_xalign(1.0);
    year.set_width_chars(4);
    header.append(&year);

    header.upcast()
}

fn fullscreen_queue_header_row() -> gtk::Widget {
    let header = fullscreen_queue_row_box();
    header.add_css_class("queue-header");

    let drag = fullscreen_queue_fixed_spacer(QUEUE_DRAG_HANDLE_WIDTH);
    let cover = fullscreen_queue_fixed_spacer(QUEUE_FULLSCREEN_COVER_COLUMN_WIDTH);
    let title = queue_header_text_label(&tr("Title").to_uppercase(), 0.0);
    let album = queue_header_text_label(&tr("Album").to_uppercase(), 0.0);
    let duration = queue_duration_header_icon();
    let year = queue_header_fixed_label(&tr("Year").to_uppercase(), QUEUE_YEAR_COLUMN_WIDTH);
    let favorite = gtk::Image::from_icon_name(FAVORITE_ADD_ICON);
    favorite.add_css_class("muted");
    favorite.set_pixel_size(14);
    favorite.set_width_request(QUEUE_FAVORITE_COLUMN_WIDTH);
    favorite.set_halign(gtk::Align::Center);

    header.append(&drag);
    header.append(&cover);
    header.append(&title);
    header.append(&album);
    header.append(&duration);
    header.append(&year);
    header.append(&favorite);
    let allocation_owner = fullscreen_queue_column_owner(
        &header,
        QueueFullscreenColumnWidgets {
            title: title.upcast(),
            album: album.upcast(),
            year: year.upcast(),
        },
    );

    allocation_owner
}

fn queue_header_text_label(text: &str, xalign: f32) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("muted");
    label.set_xalign(xalign);
    label.set_width_request(1);
    label.set_hexpand(false);
    label.set_halign(gtk::Align::Fill);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label
}

fn queue_header_fixed_label(text: &str, width: i32) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("muted");
    label.set_xalign(0.5);
    label.set_width_request(width);
    label.set_halign(gtk::Align::Fill);
    label
}

fn queue_duration_header_icon() -> gtk::Image {
    let image = gtk::Image::from_icon_name("appointment-soon-symbolic");
    let label = tr("Duration");
    image.add_css_class("muted");
    image.set_width_request(QUEUE_DURATION_COLUMN_WIDTH);
    image.set_halign(gtk::Align::Fill);
    image.set_tooltip_text(Some(&label));
    image.update_property(&[gtk::accessible::Property::Label(&label)]);
    image
}

fn fullscreen_queue_text_cell(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("muted");
    label.set_xalign(0.0);
    label.set_width_request(1);
    label.set_hexpand(false);
    label.set_halign(gtk::Align::Fill);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label
}

fn fullscreen_queue_fixed_cell(text: &str, width: i32) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("muted");
    label.set_xalign(0.5);
    label.set_width_request(width);
    label.set_halign(gtk::Align::Fill);
    label
}

fn queue_identity_cell(entry: &SequenceEntry) -> (gtk::Box, gtk::Label) {
    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_width_request(1);
    labels.set_hexpand(false);
    labels.set_halign(gtk::Align::Fill);
    labels.set_valign(gtk::Align::Center);

    let title = gtk::Label::new(Some(&entry.track.title));
    title.add_css_class("queue-title");
    title.set_xalign(0.0);
    title.set_width_request(1);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let artist = queue_link_label(&entry.track.artist);
    artist.set_width_request(1);
    labels.append(&title);
    labels.append(&artist);
    (labels, artist)
}

fn fullscreen_queue_row_box() -> gtk::Box {
    let row = gtk::Box::new(
        gtk::Orientation::Horizontal,
        QUEUE_FULLSCREEN_COLUMN_SPACING,
    );
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);
    row.set_valign(gtk::Align::Center);
    row
}

fn fullscreen_queue_fixed_spacer(width: i32) -> gtk::Box {
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_width_request(width);
    spacer
}

fn fullscreen_queue_column_owner(
    root: &gtk::Box,
    columns: QueueFullscreenColumnWidgets,
) -> gtk::Widget {
    let initial = fullscreen_queue_column_widths(1);
    columns.apply(initial);
    let last = Cell::new(initial);
    width_allocation_owner(root, move |width| {
        let widths = fullscreen_queue_column_widths(width.max(1));
        if last.replace(widths) != widths {
            columns.apply(widths);
        }
    })
    .upcast()
}

fn fullscreen_queue_column_widths(available_width: i32) -> QueueFullscreenColumnWidths {
    let full_fixed = fullscreen_queue_fixed_width(true, true);
    let full_variable = available_width.saturating_sub(full_fixed);
    if full_variable >= QUEUE_FULLSCREEN_TITLE_MIN_WIDTH + QUEUE_FULLSCREEN_ALBUM_MIN_WIDTH {
        return split_queue_text_width(full_variable, true);
    }

    let compact_fixed = fullscreen_queue_fixed_width(true, false);
    let compact_variable = available_width.saturating_sub(compact_fixed);
    if compact_variable >= QUEUE_FULLSCREEN_TITLE_MIN_WIDTH + QUEUE_FULLSCREEN_ALBUM_MIN_WIDTH {
        return split_queue_text_width(compact_variable, false);
    }

    let title_fixed = fullscreen_queue_fixed_width(false, false);
    QueueFullscreenColumnWidths {
        title: available_width.saturating_sub(title_fixed).max(1),
        album: 0,
        show_album: false,
        show_year: false,
    }
}

fn fullscreen_queue_fixed_width(show_album: bool, show_year: bool) -> i32 {
    let columns = 5 + i32::from(show_album) + i32::from(show_year);
    QUEUE_FULLSCREEN_ROW_HORIZONTAL_PADDING
        + QUEUE_DRAG_HANDLE_WIDTH
        + QUEUE_FULLSCREEN_COVER_COLUMN_WIDTH
        + QUEUE_DURATION_COLUMN_WIDTH
        + QUEUE_FAVORITE_COLUMN_WIDTH
        + if show_year {
            QUEUE_YEAR_COLUMN_WIDTH
        } else {
            0
        }
        + (columns - 1) * QUEUE_FULLSCREEN_COLUMN_SPACING
}

fn split_queue_text_width(width: i32, show_year: bool) -> QueueFullscreenColumnWidths {
    let min_total = QUEUE_FULLSCREEN_TITLE_MIN_WIDTH + QUEUE_FULLSCREEN_ALBUM_MIN_WIDTH;
    if width <= min_total {
        return QueueFullscreenColumnWidths {
            title: QUEUE_FULLSCREEN_TITLE_MIN_WIDTH,
            album: QUEUE_FULLSCREEN_ALBUM_MIN_WIDTH,
            show_album: true,
            show_year,
        };
    }

    let base_total = QUEUE_FULLSCREEN_TITLE_COLUMN_WIDTH + QUEUE_FULLSCREEN_ALBUM_COLUMN_WIDTH;
    if width <= base_total {
        let title = ((i64::from(width) * i64::from(QUEUE_FULLSCREEN_TITLE_COLUMN_WIDTH))
            / i64::from(base_total)) as i32;
        return QueueFullscreenColumnWidths {
            title: title.max(QUEUE_FULLSCREEN_TITLE_MIN_WIDTH),
            album: (width - title).max(QUEUE_FULLSCREEN_ALBUM_MIN_WIDTH),
            show_album: true,
            show_year,
        };
    }

    let extra = width - base_total;
    QueueFullscreenColumnWidths {
        title: QUEUE_FULLSCREEN_TITLE_COLUMN_WIDTH + extra / 2,
        album: QUEUE_FULLSCREEN_ALBUM_COLUMN_WIDTH + extra - extra / 2,
        show_album: true,
        show_year,
    }
}

fn queue_drag_handle(entry_id: &OccurrenceId) -> gtk::Widget {
    let drag = gtk::Image::from_icon_name("rufin-list-drag-handle-symbolic");
    drag.add_css_class("dim-label");
    drag.set_tooltip_text(Some(&tr("Drag to reorder")));
    drag.set_width_request(QUEUE_DRAG_HANDLE_WIDTH);
    drag.set_halign(gtk::Align::Center);
    let source = gtk::DragSource::builder()
        .actions(gtk::gdk::DragAction::MOVE)
        .build();
    let drag_id = entry_id.as_str().to_string();
    source.connect_prepare(move |_, _, _| {
        Some(gtk::gdk::ContentProvider::for_value(&drag_id.to_value()))
    });
    drag.add_controller(source);
    drag.upcast()
}

fn reusable_queue_drag_handle(binding: &Rc<RefCell<Option<QueueSidebarBinding>>>) -> gtk::Image {
    let drag = gtk::Image::from_icon_name("rufin-list-drag-handle-symbolic");
    drag.add_css_class("dim-label");
    drag.set_tooltip_text(Some(&tr("Drag to reorder")));
    drag.set_width_request(QUEUE_DRAG_HANDLE_WIDTH);
    drag.set_halign(gtk::Align::Center);
    let source = gtk::DragSource::builder()
        .actions(gtk::gdk::DragAction::MOVE)
        .build();
    let binding = Rc::clone(binding);
    source.connect_prepare(move |_, _, _| {
        let binding = binding.borrow();
        let binding = binding.as_ref().filter(|binding| binding.reorderable)?;
        Some(gtk::gdk::ContentProvider::for_value(
            &binding.entry.occurrence.as_str().to_value(),
        ))
    });
    drag.add_controller(source);
    drag
}

fn install_queue_row_drop(
    target_row: &gtk::Box,
    controller: &QueueHandle,
    entry_id: OccurrenceId,
    target_index: usize,
) {
    let target_id = entry_id;
    let controller = controller.clone();
    let target = target_row.downgrade();
    let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
    drop_target.connect_drop(move |_, value, _, y| {
        let Ok(drag_id) = value.get::<String>() else {
            return false;
        };
        let drag_id = OccurrenceId::new(drag_id);
        if drag_id == target_id {
            return false;
        }
        let Some(target) = target.upgrade() else {
            return false;
        };
        let after = y > f64::from(target.height()) / 2.0;
        controller.reorder(QueueReorderRequest {
            occurrence: drag_id,
            target_index,
            after,
        });
        true
    });
    target_row.add_controller(drop_target);
}

fn install_reusable_queue_row_drop(
    target_row: &gtk::Box,
    controller: &QueueHandle,
    binding: Rc<RefCell<Option<QueueSidebarBinding>>>,
) {
    let controller = controller.clone();
    let target = target_row.downgrade();
    let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
    drop_target.connect_drop(move |_, value, _, y| {
        let Ok(drag_id) = value.get::<String>() else {
            return false;
        };
        let drag_id = OccurrenceId::new(drag_id);
        let Some(binding) = binding
            .borrow()
            .clone()
            .filter(|binding| binding.reorderable)
        else {
            return false;
        };
        if drag_id == binding.entry.occurrence {
            return false;
        }
        let Some(target) = target.upgrade() else {
            return false;
        };
        let after = y > f64::from(target.height()) / 2.0;
        controller.reorder(QueueReorderRequest {
            occurrence: drag_id,
            target_index: binding.index,
            after,
        });
        true
    });
    target_row.add_controller(drop_target);
}

fn install_queue_row_context_menu(row: &gtk::Box, shell: &Rc<Shell>, entry: &SequenceEntry) {
    let shell = Rc::clone(shell);
    let entry = entry.clone();
    install_context_menu_openers(
        row,
        Rc::new(move |target, position| {
            let pointing_to =
                position.map(|(x, y)| gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1));
            show_queue_row_context_menu(target, &shell, &entry, pointing_to);
        }),
    );
}

fn install_reusable_queue_row_context_menu(
    row: &gtk::Box,
    shell: &Rc<Shell>,
    binding: Rc<RefCell<Option<QueueSidebarBinding>>>,
) {
    let shell = Rc::clone(shell);
    install_context_menu_openers(
        row,
        Rc::new(move |target, position| {
            let Some(entry) = binding
                .borrow()
                .as_ref()
                .map(|binding| binding.entry.clone())
            else {
                return;
            };
            let pointing_to =
                position.map(|(x, y)| gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1));
            show_queue_row_context_menu(target, &shell, &entry, pointing_to);
        }),
    );
}

fn show_queue_row_context_menu(
    row: &gtk::Widget,
    shell: &Rc<Shell>,
    entry: &SequenceEntry,
    pointing_to: Option<gtk::gdk::Rectangle>,
) {
    let metadata_editable =
        shell.metadata_editing_available(MetadataItemId::Track(entry.track.id.clone()));
    show_resolved_queue_row_context_menu(row, shell, entry, pointing_to, metadata_editable);
}

fn show_resolved_queue_row_context_menu(
    row: &gtk::Widget,
    shell: &Rc<Shell>,
    entry: &SequenceEntry,
    pointing_to: Option<gtk::gdk::Rectangle>,
    metadata_editable: bool,
) {
    let track = entry.track.clone();
    let surface = ContextMenuSurface::new(row, "queue", None);
    surface.append_fixed_action(msgid("Remove from Queue"), "remove", REMOVE_ICON);
    surface.append_configurable_action(ContextMenuItem::Play, msgid("Play"), "play-now", PLAY_ICON);
    surface.append_configurable_action(
        ContextMenuItem::PlayNext,
        msgid("Play Next"),
        "play-next",
        PLAY_NEXT_ICON,
    );
    surface.append_configurable_action(
        ContextMenuItem::PlayLater,
        msgid("Play Later"),
        "play-last",
        PLAY_LATER_ICON,
    );
    surface.append_configurable_submenu(
        ContextMenuItem::PlayRadio,
        msgid("Track radio"),
        &radio_context_submenu("queue"),
        RADIO_ICON,
    );
    let playlist_source = shell.library.selected.borrow().as_ref().map(|selected| {
        PlaylistTrackSource::ready(
            selected,
            downloads::DownloadSubject::Track(track.id.clone()),
            vec![track.id.clone()].into(),
        )
    });
    if playlist_source.is_some() {
        surface.append_configurable_action(
            ContextMenuItem::AddToPlaylist,
            msgid("Add to Playlist"),
            "add-to-playlist",
            ADD_TO_PLAYLIST_ICON,
        );
    }

    if entry.track.favorite {
        surface.append_configurable_action(
            ContextMenuItem::Favorites,
            msgid("Remove from Favorites"),
            "favorite",
            FAVORITE_REMOVE_ICON,
        );
    } else {
        surface.append_configurable_action(
            ContextMenuItem::Favorites,
            msgid("Add to Favorites"),
            "favorite",
            FAVORITE_ADD_ICON,
        );
    }
    if metadata_editable {
        surface.append_configurable_action(
            ContextMenuItem::EditMetadata,
            msgid("Edit metadata"),
            "edit-metadata",
            EDIT_ICON,
        );
    }
    let artist_route = queue_artist_route(entry);
    if artist_route.is_some() {
        surface.append_configurable_action(
            ContextMenuItem::GoToArtist,
            msgid("Go to Artist"),
            "go-artist",
            ARTIST_ICON,
        );
    }
    let album_route = entry.track.album_id.clone().map(Route::AlbumDetail);
    if album_route.is_some() {
        surface.append_configurable_action(
            ContextMenuItem::GoToAlbum,
            msgid("Go to Album"),
            "go-album",
            ALBUM_ICON,
        );
    }

    surface.popover().set_pointing_to(pointing_to.as_ref());
    if let Some(playlist_source) = playlist_source {
        install_context_menu_picker_action(&surface, shell, playlist_source);
    }

    let controller = shell.products.playback.queue.clone();
    let radio = shell.products.playback.radio.clone();
    let entry_id = entry.occurrence.clone();
    let play_last_request =
        shell.library.selected.borrow().as_ref().map(|selected| {
            selected.one_track(entry.track.clone(), playback::QueuePlacement::Last)
        });

    surface.add_action("remove", {
        let remove_controller = controller.clone();
        let remove_id = entry_id.clone();
        move || {
            remove_controller.remove(remove_id.clone());
        }
    });
    if metadata_editable {
        surface.add_action("edit-metadata", {
            let shell = Rc::clone(shell);
            let track = track.clone();
            move || {
                present_metadata_dialog(&shell, MetadataItemId::Track(track.id.clone()));
            }
        });
    }

    surface.add_action("play-now", {
        let play_now_controller = controller.clone();
        let play_now_id = entry_id.clone();
        move || {
            play_now_controller.activate(play_now_id.clone());
        }
    });

    surface.add_action("play-next", {
        let play_next_controller = controller.clone();
        move || {
            play_next_controller.move_after_current(entry_id.clone());
        }
    });

    surface.add_action("play-last", {
        let last_controller = controller.clone();
        move || {
            if let Some(request) = play_last_request.clone() {
                last_controller.play_loaded(request);
            }
        }
    });

    surface.add_action("play-radio", {
        let radio_controller = radio.clone();
        let track = track.clone();
        move || {
            radio_controller.play_radio(RadioPlayRequest::now(RadioSeed::Track(track.id.clone())));
        }
    });

    surface.add_action("play-radio-next", {
        let radio_controller = radio.clone();
        let track = track.clone();
        move || {
            radio_controller.play_radio(RadioPlayRequest::next(RadioSeed::Track(track.id.clone())));
        }
    });

    surface.add_action("play-radio-last", {
        let radio_controller = radio;
        move || {
            radio_controller.play_radio(RadioPlayRequest::last(RadioSeed::Track(track.id.clone())));
        }
    });

    surface.add_action("favorite", {
        let favorite_shell = Rc::clone(shell);
        let favorite_track_id = entry.track.id.clone();
        let favorite_value = !entry.track.favorite;
        move || {
            favorite_shell.set_favorite_with_feedback(
                library::FavoriteItemId::Track(favorite_track_id.clone()),
                favorite_value,
                None,
            );
        }
    });

    if let Some(artist_route) = artist_route {
        surface.add_action("go-artist", {
            let action_shell = Rc::clone(shell);
            move || {
                let shell = Rc::clone(&action_shell);
                let route = artist_route.clone();
                glib::idle_add_local_once(move || shell.navigate(route));
            }
        });
    }
    if let Some(album_route) = album_route {
        surface.add_action("go-album", {
            let action_shell = Rc::clone(shell);
            move || {
                let shell = Rc::clone(&action_shell);
                let route = album_route.clone();
                glib::idle_add_local_once(move || shell.navigate(route));
            }
        });
    }

    surface.popup(&shell.settings.current.borrow().context_menu);
}

fn queue_artist_route(entry: &SequenceEntry) -> Option<Route> {
    entry
        .track
        .primary_artist_id()
        .cloned()
        .map(Route::ArtistDetail)
}

fn queue_link_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("queue-link");
    label.add_css_class("muted");
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label
}

fn add_queue_label_link_style(label: &gtk::Label) {
    label.set_cursor_from_name(Some("pointer"));
    add_dynamic_link_hover(label.upcast_ref(), label);
}

fn add_queue_label_click(label: &gtk::Label, callback: impl Fn() + 'static) {
    let click = gtk::GestureClick::new();
    let callback: Rc<dyn Fn()> = Rc::new(callback);
    let generation = Rc::new(Cell::new(0_u64));
    let cancel_generation = Rc::clone(&generation);
    click.connect_pressed(move |_, press_count, _, _| {
        if press_count > 1 {
            cancel_generation.set(cancel_generation.get().saturating_add(1));
        }
    });
    click.connect_released(move |_, press_count, _, _| {
        let next_generation = generation.get().saturating_add(1);
        generation.set(next_generation);
        if press_count != 1 {
            return;
        }

        let callback = Rc::clone(&callback);
        let generation = Rc::clone(&generation);
        glib::timeout_add_local_once(
            Duration::from_millis(QUEUE_LINK_CLICK_DELAY_MS),
            move || {
                if generation.get() == next_generation {
                    callback();
                }
            },
        );
    });
    label.add_controller(click);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use adw::prelude::ListModelExt;
    use gtk::{gio, glib};
    use library::{AcceptedTrackReplacement, TrackId};
    use playback::{Provenance, QueuePage, QueuePageRow, SequenceEntry};

    use super::{
        OccurrenceId, QUEUE_CURRENT_TARGET, QUEUE_ROW_HEIGHT, QueuePageQuery, QueuePanelItem,
        QueuePanelLayout, QueuePanelModelState, QueuePanelRenderState,
        fullscreen_queue_column_widths, fullscreen_queue_fixed_width,
        queue_current_row_scroll_target, queue_model_item_at, queue_panel_items,
        queue_row_update_positions, replace_queue_model, replace_queue_page_tracks,
        update_queue_rows,
    };
    use crate::test_support::track;

    fn render_state(current_row: usize) -> QueuePanelRenderState {
        QueuePanelRenderState {
            query: Some(QueuePageQuery::current()),
            row_ids: (400..500)
                .map(|number| OccurrenceId::new(format!("queue-{number}")))
                .collect(),
            row_artwork: vec![String::new(); 100],
            row_tracks: (400..500)
                .map(|number| track(number, format!("Track {number}")))
                .collect(),
            row_indices: (400..500).collect(),
            row_count: 100,
            current_row: Some(current_row),
            show_header: true,
            empty_text: None,
            covered_height: 0,
        }
    }

    #[test]
    fn queue_current_change_replaces_only_old_and_new_model_rows() {
        let previous = render_state(10);
        let next = render_state(90);

        assert!(previous.same_rows_as(&next));
        assert_eq!(queue_row_update_positions(&previous, &next), vec![10, 90]);
    }

    #[test]
    fn queue_track_change_replaces_only_matching_model_rows() {
        let previous = render_state(10);
        let mut next = previous.clone();
        next.row_tracks[42].favorite = true;

        assert!(previous.same_rows_as(&next));
        assert_eq!(queue_row_update_positions(&previous, &next), vec![42]);
    }

    #[test]
    fn queue_track_change_preserves_the_complete_view() {
        let original = track(1, "Original");
        let mut replacement = original.clone();
        replacement.favorite = true;
        let query = QueuePageQuery::current();
        let mut page = QueuePage {
            revision: 7,
            query: query.clone(),
            total: 900,
            current_absolute_index: Some(450),
            rows: vec![QueuePageRow {
                absolute_index: 400,
                entry: Arc::new(SequenceEntry {
                    occurrence: OccurrenceId::new("queue-400"),
                    track: original,
                    provenance: Provenance::Manual,
                }),
            }],
        };

        assert!(replace_queue_page_tracks(
            &mut page,
            &[AcceptedTrackReplacement {
                id: TrackId::fake(1),
                track: Some(replacement),
            }],
            8,
        ));
        assert_eq!(page.revision, 8);
        assert_eq!(page.query, query);
        assert_eq!(page.total, 900);
        assert_eq!(page.current_absolute_index, Some(450));
        assert!(page.rows[0].entry.track.favorite);
    }

    #[test]
    fn queue_render_invalidation_keeps_the_model_bound_to_the_visible_list() {
        let mut state = QueuePanelModelState::new();
        let visible_model = state.model.clone();
        state.render = Some(render_state(10));

        state.invalidate_render();
        replace_queue_model(
            &state.model,
            vec![QueuePanelItem::Empty {
                text: "one visible row".to_string(),
            }],
        );

        assert!(state.render.is_none());
        assert_eq!(visible_model, state.model);
        assert_eq!(visible_model.n_items(), 1);
    }

    #[test]
    fn lyrics_overlap_adds_one_queue_scroll_extent() {
        let entry = Arc::new(SequenceEntry {
            occurrence: OccurrenceId::new("queue-400"),
            track: track(400, "Covered track"),
            provenance: Provenance::Manual,
        });
        let page = QueuePage {
            revision: 1,
            query: QueuePageQuery::current(),
            total: 1,
            current_absolute_index: Some(400),
            rows: vec![QueuePageRow {
                absolute_index: 400,
                entry,
            }],
        };
        let mut closed = render_state(0);
        closed.row_ids.truncate(1);
        closed.row_artwork.truncate(1);
        closed.row_tracks.truncate(1);
        closed.row_indices.truncate(1);
        closed.row_count = 1;
        let mut open = closed.clone();
        open.covered_height = 300;

        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        replace_queue_model(
            &model,
            queue_panel_items(Some(&page), &closed, QueuePanelLayout::Sidebar),
        );
        update_queue_rows(
            &model,
            Some(&page),
            &closed,
            &open,
            QueuePanelLayout::Sidebar,
        );

        assert_eq!(model.n_items(), 2);
        assert!(matches!(
            queue_model_item_at(&model, 1),
            Some(QueuePanelItem::Covered { height: 300 })
        ));

        update_queue_rows(
            &model,
            Some(&page),
            &open,
            &closed,
            QueuePanelLayout::Sidebar,
        );
        assert_eq!(model.n_items(), 1);
    }

    #[test]
    fn queue_current_reveal_prefers_comfort_band_when_possible() {
        let row = 10usize;
        let row_top = row as f64 * f64::from(QUEUE_ROW_HEIGHT);

        assert_eq!(
            queue_current_row_scroll_target(row, 0.0, 400.0),
            Some(row_top - 400.0 * QUEUE_CURRENT_TARGET)
        );
        assert_eq!(
            queue_current_row_scroll_target(row, row_top - 140.0, 400.0),
            None
        );
        assert_eq!(
            queue_current_row_scroll_target(row, 0.0, 80.0),
            Some(row_top - 11.0)
        );
    }

    #[test]
    fn fullscreen_queue_columns_never_outgrow_their_allocation() {
        for available in [320, 542, 900] {
            let widths = fullscreen_queue_column_widths(available);
            let fixed = fullscreen_queue_fixed_width(widths.show_album, widths.show_year);

            assert!(widths.title + widths.album + fixed <= available);
        }
    }
}
