use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use crate::format_duration;
use crate::routes::route::Route;
use adw::prelude::*;
use artwork::ArtworkBinding;
use gtk::{gio, glib};
use playback::{OccurrenceId, QueuePage, QueuePageQuery, SequenceEntry};
use playback::{QueueHandle, QueueReorderRequest, RadioPlayRequest, RadioSeed};

use crate::favorites::{
    FAVORITE_ADD_ICON, FAVORITE_REMOVE_ICON, favorite_button_is_active, favorite_icon_button,
    set_favorite_button_active,
};
use crate::interactions::add_dynamic_link_hover;
use crate::interactions::install_context_menu_openers;
use crate::interactions::{
    ADD_TO_PLAYLIST_ICON, ALBUM_ICON, ARTIST_ICON, ContextMenuSurface, RADIO_ICON,
    context_menu_action, context_menu_box, context_menu_submenu_action, radio_context_submenu,
};
use crate::layout::width_allocation_owner;
use crate::routes::playlist_picker::{
    context_menu_can_add_to_playlist, context_menu_picker_button,
};
use crate::shell::Shell;
use crate::shell::actions::{PLAY_ICON, PLAY_LATER_ICON, PLAY_NEXT_ICON, REMOVE_ICON};
use crate::shell::cover::THUMB_COVER_SIZE;
use localization::{msgid, tr};

const QUEUE_LINK_CLICK_DELAY_MS: u64 = 250;
const QUEUE_SEARCH_DELAY_MS: u64 = 120;
const QUEUE_SCROLL_EDGE_TOLERANCE: f64 = 1.0;
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

pub(crate) type PendingQueueSelection = Rc<dyn Fn(&QueuePage)>;

pub(crate) struct QueueState {
    pub(crate) page: RefCell<Option<QueuePage>>,
    pub(crate) pending_playlist_entry_selection: RefCell<Option<PendingQueueSelection>>,
    pub(crate) filter: RefCell<String>,
    pub(crate) page_request: RefCell<Option<QueuePageQuery>>,
    pub(crate) search_source: RefCell<Option<glib::SourceId>>,
    pub(crate) scroll_programmatic: Cell<bool>,
    pub(crate) render_queued: Cell<bool>,
    sidebar_render_state: RefCell<Option<QueuePanelModelState>>,
    fullscreen_render_state: RefCell<Option<QueuePanelModelState>>,
}

impl QueueState {
    pub(crate) fn new(page: Option<QueuePage>) -> Self {
        Self {
            page: RefCell::new(page),
            pending_playlist_entry_selection: RefCell::new(None),
            filter: RefCell::new(String::new()),
            page_request: RefCell::new(None),
            search_source: RefCell::new(None),
            scroll_programmatic: Cell::new(false),
            render_queued: Cell::new(false),
            sidebar_render_state: RefCell::new(None),
            fullscreen_render_state: RefCell::new(None),
        }
    }
}

impl Shell {
    pub(crate) fn arm_playlist_entry_selection(&self, select: PendingQueueSelection) {
        *self.queue.pending_playlist_entry_selection.borrow_mut() = Some(select);
    }

    pub(crate) fn apply_pending_playlist_entry_selection(&self, queue: Option<&QueuePage>) {
        let Some(select) = self
            .queue
            .pending_playlist_entry_selection
            .borrow_mut()
            .take()
        else {
            return;
        };
        if let Some(queue) = queue {
            select(queue);
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
    page_start: usize,
    row_ids: Vec<OccurrenceId>,
    row_artwork: Vec<String>,
    row_indices: Vec<usize>,
    row_count: usize,
    current_row: Option<usize>,
    show_header: bool,
    empty_text: Option<String>,
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
}

struct QueuePanelModelState {
    render: Option<QueuePanelRenderState>,
    model: gio::ListStore,
}

impl QueuePanelModelState {
    fn new() -> Self {
        Self {
            render: None,
            model: gio::ListStore::new::<glib::BoxedAnyObject>(),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum QueueScrollBehavior {
    Preserve,
    Start,
    End,
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

    pub(crate) fn invalidate_queue_panel_render_state(&self) {
        self.queue.sidebar_render_state.borrow_mut().take();
        self.queue.fullscreen_render_state.borrow_mut().take();
    }

    fn render_queue_panel_into(self: &Rc<Self>, panel: &gtk::Box, layout: QueuePanelLayout) {
        let queue_scroller =
            queue_panel_scroller(panel).unwrap_or_else(|| new_queue_scroller(self));
        let adjustment = queue_scroller.vadjustment();
        let previous_scroll = adjustment.value();
        let current_occurrence = self
            .playback
            .player
            .borrow()
            .as_ref()
            .and_then(|player| player.queue.current_occurrence.clone());
        let queue_page = self.queue.page.borrow();
        let render_state =
            queue_panel_render_state(queue_page.as_ref(), current_occurrence.as_ref());
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
        ensure_queue_panel_view(self, panel, &queue_scroller, &model);
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
                        update_queue_current_rows(&state.model, &previous, &render_state);
                        reveal_queue_current_row_later(
                            self,
                            &queue_scroller,
                            render_state.current_row,
                        );
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
                restore_queue_scroll_position(self, &queue_scroller, previous_scroll);
                reveal_queue_current_row_later(self, &queue_scroller, render_state.current_row);
            }
            QueueScrollBehavior::Start => {
                restore_queue_scroll_position(self, &queue_scroller, 0.0);
            }
            QueueScrollBehavior::End => {
                restore_queue_scroll_end_later(self, &queue_scroller);
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
        if let Some(artist_id) = entry.track.artist_id.clone() {
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

        if let Some(artist_id) = entry.track.artist_id.clone() {
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
        self.apply_pending_playlist_entry_selection(Some(&page));
        self.accept_queue_page(page);
        true
    }
}

impl QueuePanelRenderState {
    fn same_rows_as(&self, next: &Self) -> bool {
        self.query == next.query
            && self.page_start == next.page_start
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
) -> QueuePanelRenderState {
    let filter = queue_page
        .and_then(|page| page.query.search_text())
        .unwrap_or_default();
    let has_filter = !filter.is_empty();
    let mut queue_has_entries = false;
    let mut row_ids = Vec::new();
    let mut row_artwork = Vec::new();
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
            row_indices.push(row.absolute_index);
        }
    }
    let empty_text = (row_count == 0).then(|| {
        if has_filter && queue_has_entries {
            tr("No queue items match the search.")
        } else {
            tr("Add music to start a queue.")
        }
    });
    QueuePanelRenderState {
        query: queue_page.map(|page| page.query.clone()),
        page_start: queue_page.map_or(0, |page| page.start),
        show_header: row_count != 0,
        row_ids,
        row_artwork,
        row_indices,
        row_count,
        current_row,
        empty_text,
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
    } else if next
        .query
        .as_ref()
        .is_some_and(|query| !query.is_filtered())
        && previous
            .query
            .as_ref()
            .is_some_and(|query| !query.is_filtered())
        && next.page_start < previous.page_start
    {
        QueueScrollBehavior::End
    } else {
        QueueScrollBehavior::Start
    }
}

fn update_queue_current_rows(
    model: &gio::ListStore,
    previous: &QueuePanelRenderState,
    next: &QueuePanelRenderState,
) {
    for position in queue_current_update_positions(previous, next) {
        replace_queue_model_item_current(
            model,
            position as u32,
            next.current_row == Some(position),
        );
    }
}

fn queue_current_update_positions(
    previous: &QueuePanelRenderState,
    next: &QueuePanelRenderState,
) -> Vec<usize> {
    let mut positions = Vec::new();
    if let Some(position) = previous.current_row {
        positions.push(position);
    }
    if let Some(position) = next.current_row
        && Some(position) != previous.current_row
    {
        positions.push(position);
    }
    positions
}

fn replace_queue_model_item_current(model: &gio::ListStore, position: u32, current: bool) {
    let Some(mut item) = queue_model_item_at(model, position) else {
        return;
    };
    let QueuePanelItem::Entry {
        current: item_current,
        ..
    } = &mut item
    else {
        return;
    };
    if *item_current == current {
        return;
    }
    *item_current = current;
    let replacement = glib::BoxedAnyObject::new(item);
    model.splice(position, 1, &[replacement]);
}

fn queue_panel_items(
    queue_page: Option<&QueuePage>,
    state: &QueuePanelRenderState,
    layout: QueuePanelLayout,
) -> Vec<QueuePanelItem> {
    let mut items = Vec::new();
    if let Some(page) = queue_page {
        for (row_position, absolute_index) in state.row_indices.iter().enumerate() {
            if let Some(row) = page
                .rows
                .iter()
                .find(|row| row.absolute_index == *absolute_index)
            {
                items.push(QueuePanelItem::Entry {
                    index: *absolute_index,
                    entry: Box::new(row.entry.as_ref().clone()),
                    current: state.current_row == Some(row_position),
                    reorderable: state
                        .query
                        .as_ref()
                        .is_some_and(|query| !query.is_filtered()),
                    layout,
                });
            }
        }
    }
    if items.is_empty()
        && let Some(text) = state.empty_text.clone()
    {
        items.push(QueuePanelItem::Empty { text });
    }
    items
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
) {
    let has_list_view = scroller
        .child()
        .is_some_and(|child| child.is::<gtk::ListView>());
    if !has_list_view {
        scroller.set_child(Some(&queue_list_view(shell, model)));
    }
    if scroller.parent().is_none() {
        panel.append(scroller);
    }
}

fn queue_list_view(shell: &Rc<Shell>, model: &gio::ListStore) -> gtk::ListView {
    let selection = gtk::NoSelection::new(Some(model.clone()));
    let factory = gtk::SignalListItemFactory::new();
    let controller = shell.products.playback.queue.clone();
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

fn new_queue_scroller(shell: &Rc<Shell>) -> gtk::ScrolledWindow {
    let scroller = gtk::ScrolledWindow::new();
    scroller.add_css_class("queue-scroller");
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_vexpand(true);
    connect_queue_page_edges(shell, &scroller);
    scroller
}

fn connect_queue_page_edges(shell: &Rc<Shell>, scroller: &gtk::ScrolledWindow) {
    let adjustment = scroller.vadjustment();
    let last_value = Cell::new(adjustment.value());
    let shell = Rc::clone(shell);
    adjustment.connect_value_changed(move |adjustment| {
        let value = adjustment.value();
        let previous_value = last_value.replace(value);
        if shell.queue.scroll_programmatic.get() || value == previous_value {
            return;
        }
        let lower = adjustment.lower();
        let upper = (adjustment.upper() - adjustment.page_size()).max(lower);
        let query = {
            let queue = shell.queue.page.borrow();
            let Some(page) = queue.as_ref() else {
                return;
            };
            if value > previous_value && upper - value <= QUEUE_SCROLL_EDGE_TOLERANCE {
                page.next_query()
            } else if value < previous_value && value - lower <= QUEUE_SCROLL_EDGE_TOLERANCE {
                page.previous_query()
            } else {
                None
            }
        };
        if let Some(query) = query {
            shell.request_queue_page(query);
        }
    });
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

fn restore_queue_scroll_position(shell: &Shell, scroller: &gtk::ScrolledWindow, value: f64) {
    let adjustment = scroller.vadjustment();
    let lower = adjustment.lower();
    let upper = (adjustment.upper() - adjustment.page_size()).max(lower);
    shell.queue.scroll_programmatic.set(true);
    adjustment.set_value(value.clamp(lower, upper));
    shell.queue.scroll_programmatic.set(false);
}

fn restore_queue_scroll_end_later(shell: &Rc<Shell>, scroller: &gtk::ScrolledWindow) {
    let shell = Rc::clone(shell);
    let scroller = scroller.clone();
    glib::idle_add_local_once(move || {
        restore_queue_scroll_position(&shell, &scroller, f64::MAX);
    });
}

fn reveal_queue_current_row_later(
    shell: &Rc<Shell>,
    scroller: &gtk::ScrolledWindow,
    current_row: Option<usize>,
) {
    let idle_shell = Rc::clone(shell);
    let idle_scroller = scroller.clone();
    glib::idle_add_local_once(move || {
        reveal_queue_current_row(&idle_shell, &idle_scroller, current_row);
    });

    let settled_shell = Rc::clone(shell);
    let settled_scroller = scroller.clone();
    glib::timeout_add_local_once(Duration::from_millis(80), move || {
        reveal_queue_current_row(&settled_shell, &settled_scroller, current_row)
    });
}

fn reveal_queue_current_row(
    shell: &Shell,
    scroller: &gtk::ScrolledWindow,
    current_row: Option<usize>,
) {
    let Some(current_row) = current_row else {
        return;
    };
    let adjustment = scroller.vadjustment();
    let page_size = adjustment.page_size();
    if page_size <= 1.0 {
        return;
    }
    let Some(target) = queue_current_row_scroll_target(current_row, adjustment.value(), page_size)
    else {
        return;
    };
    restore_queue_scroll_position(shell, scroller, target);
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

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header.add_css_class("queue-header");
    header.set_valign(gtk::Align::Center);

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_width_request(70);
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

fn show_queue_row_context_menu(
    row: &gtk::Widget,
    shell: &Rc<Shell>,
    entry: &SequenceEntry,
    pointing_to: Option<gtk::gdk::Rectangle>,
) {
    let main_menu = context_menu_box();
    let track = entry.track.clone();
    main_menu.append(&context_menu_action(
        "Remove from Queue",
        "queue.remove",
        REMOVE_ICON,
    ));
    main_menu.append(&context_menu_action("Play", "queue.play-now", PLAY_ICON));
    main_menu.append(&context_menu_action(
        "Play Next",
        "queue.play-next",
        PLAY_NEXT_ICON,
    ));

    main_menu.append(&context_menu_action(
        "Play Later",
        "queue.play-last",
        PLAY_LATER_ICON,
    ));
    main_menu.append(&context_menu_submenu_action(
        msgid("Track radio"),
        "queue.play-radio",
        RADIO_ICON,
        &radio_context_submenu("queue"),
    ));
    if context_menu_can_add_to_playlist(shell) {
        let track_source: Rc<dyn Fn() -> Vec<library::Track>> = Rc::new({
            let track = track.clone();
            move || vec![track.clone()]
        });
        main_menu.append(&context_menu_picker_button(
            "Add to Playlist",
            ADD_TO_PLAYLIST_ICON,
            shell,
            track_source,
        ));
    }

    if entry.track.favorite {
        main_menu.append(&context_menu_action(
            "Remove from Favorites",
            "queue.favorite",
            FAVORITE_REMOVE_ICON,
        ));
    } else {
        main_menu.append(&context_menu_action(
            "Add to Favorites",
            "queue.favorite",
            FAVORITE_ADD_ICON,
        ));
    }
    let artist_route = queue_artist_route(entry);
    if artist_route.is_some() {
        main_menu.append(&context_menu_action(
            "Go to Artist",
            "queue.go-artist",
            ARTIST_ICON,
        ));
    }
    let album_route = Route::AlbumDetail(entry.track.album_id.clone());
    main_menu.append(&context_menu_action(
        "Go to Album",
        "queue.go-album",
        ALBUM_ICON,
    ));

    let surface = ContextMenuSurface::new(row, "queue", "queue-context-menu", None, &main_menu);
    surface.popover().set_pointing_to(pointing_to.as_ref());

    let controller = shell.products.playback.queue.clone();
    let radio = shell.products.playback.radio.clone();
    let entry_id = entry.occurrence.clone();

    surface.add_action("remove", {
        let remove_controller = controller.clone();
        let remove_id = entry_id.clone();
        move || {
            remove_controller.remove(remove_id.clone());
        }
    });

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
        let track = track.clone();
        move || {
            last_controller.play_last(vec![track.clone()]);
        }
    });

    surface.add_action("play-radio", {
        let radio_controller = radio.clone();
        let track = track.clone();
        move || {
            radio_controller.play_radio(RadioPlayRequest::now(RadioSeed::Track(track.clone())));
        }
    });

    surface.add_action("play-radio-next", {
        let radio_controller = radio.clone();
        let track = track.clone();
        move || {
            radio_controller.play_radio(RadioPlayRequest::next(RadioSeed::Track(track.clone())));
        }
    });

    surface.add_action("play-radio-last", {
        let radio_controller = radio;
        move || {
            radio_controller.play_radio(RadioPlayRequest::last(RadioSeed::Track(track.clone())));
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
    surface.add_action("go-album", {
        let action_shell = Rc::clone(shell);
        move || {
            let shell = Rc::clone(&action_shell);
            let route = album_route.clone();
            glib::idle_add_local_once(move || shell.navigate(route));
        }
    });

    surface.popup();
}

fn queue_artist_route(entry: &SequenceEntry) -> Option<Route> {
    entry.track.artist_id.clone().map(Route::ArtistDetail)
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
    use super::{
        OccurrenceId, QUEUE_CURRENT_TARGET, QUEUE_ROW_HEIGHT, QueuePageQuery,
        QueuePanelRenderState, fullscreen_queue_column_widths, fullscreen_queue_fixed_width,
        queue_current_row_scroll_target, queue_current_update_positions,
    };

    fn render_state(current_row: usize) -> QueuePanelRenderState {
        QueuePanelRenderState {
            query: Some(QueuePageQuery::at(400)),
            page_start: 400,
            row_ids: (400..500)
                .map(|number| OccurrenceId::new(format!("queue-{number}")))
                .collect(),
            row_artwork: vec![String::new(); 100],
            row_indices: (400..500).collect(),
            row_count: 100,
            current_row: Some(current_row),
            show_header: true,
            empty_text: None,
        }
    }

    #[test]
    fn queue_current_change_replaces_only_old_and_new_model_rows() {
        let previous = render_state(10);
        let next = render_state(90);

        assert!(previous.same_rows_as(&next));
        assert_eq!(
            queue_current_update_positions(&previous, &next),
            vec![10, 90]
        );
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
