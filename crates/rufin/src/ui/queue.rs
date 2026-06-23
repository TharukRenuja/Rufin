use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use domain::{QueueEntry, QueueEntryId, RightSidebarMode, Route, format_duration};
use gtk::{gio, glib};

use crate::controller::AppController;
use crate::i18n::{msgid, tr};

use super::{
    ADD_TO_PLAYLIST_ICON, ALBUM_ICON, ARTIST_ICON, ContextMenuSurface, FAVORITE_ADD_ICON,
    FAVORITE_EMPTY_GLYPH, FAVORITE_REMOVE_ICON, PLAY_LATER_ICON, PLAY_NEXT_ICON, RADIO_ICON, Shell,
    THUMB_COVER_SIZE, add_dynamic_link_hover, context_menu_action, context_menu_box,
    context_menu_can_add_to_playlist, context_menu_picker_button, context_menu_submenu_action,
    favorite_button_is_active, favorite_icon_button, install_context_menu_openers,
    radio_context_submenu, set_favorite_button_active, track_from_queue_entry,
};

const QUEUE_LINK_CLICK_DELAY_MS: u64 = 250;
const QUEUE_FULLSCREEN_HORIZONTAL_MARGIN: i32 = 72;
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
const QUEUE_FAVORITE_COLUMN_WIDTH: i32 = 64;
const QUEUE_ROW_HEIGHT: i32 = 58;
const QUEUE_CURRENT_COMFORT_TOP: f64 = 0.25;
const QUEUE_CURRENT_COMFORT_BOTTOM: f64 = 0.70;
const QUEUE_CURRENT_TARGET: f64 = 0.42;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui::root) struct QueueFullscreenColumnWidths {
    title: i32,
    album: i32,
    show_album: bool,
    show_year: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::ui::root) struct QueuePanelRenderState {
    filter: String,
    row_ids: Vec<QueueEntryId>,
    row_indices: Vec<usize>,
    row_count: usize,
    current_row: Option<usize>,
    show_header: bool,
    empty_text: Option<String>,
    fullscreen_widths: Option<QueueFullscreenColumnWidths>,
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
        entry: Box<QueueEntry>,
        current: bool,
        reorderable: bool,
        layout: QueuePanelLayout,
        fullscreen_widths: Option<QueueFullscreenColumnWidths>,
    },
    Empty {
        text: String,
    },
}

pub(in crate::ui::root) struct QueuePanelModelState {
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
    Reset,
}

impl Shell {
    pub(super) fn schedule_queue_panel_render(self: &Rc<Self>) {
        if self.state.resolved_right_sidebar.get() == RightSidebarMode::Hidden
            && !self.state.fullscreen_player_visible.get()
        {
            return;
        }
        if self.state.queue_render_queued.replace(true) {
            return;
        }
        let shell = Rc::clone(self);
        glib::idle_add_local_once(move || {
            shell.state.queue_render_queued.set(false);
            shell.render_queue_panel();
        });
    }

    pub(super) fn render_queue_panel(self: &Rc<Self>) {
        self.render_queue_panel_with_scroll(QueueScrollBehavior::Preserve);
    }

    pub(super) fn invalidate_queue_panel_render_state(&self) {
        self.state.queue_sidebar_render_state.borrow_mut().take();
        self.state.queue_fullscreen_render_state.borrow_mut().take();
    }

    fn render_queue_panel_reset_scroll(self: &Rc<Self>) {
        self.render_queue_panel_with_scroll(QueueScrollBehavior::Reset);
    }

    fn render_queue_panel_with_scroll(self: &Rc<Self>, scroll_behavior: QueueScrollBehavior) {
        let queue_filter = self.state.queue_filter.borrow().trim().to_lowercase();
        if self.state.resolved_right_sidebar.get() != RightSidebarMode::Hidden {
            self.render_queue_panel_into(
                &self.queue_panel,
                &queue_filter,
                QueuePanelLayout::Sidebar,
                scroll_behavior,
            );
        }
        if self.state.fullscreen_player_visible.get() {
            self.render_queue_panel_into(
                &self.fullscreen_player.queue_panel,
                "",
                QueuePanelLayout::Fullscreen,
                scroll_behavior,
            );
        }
    }

    fn render_queue_panel_into(
        self: &Rc<Self>,
        panel: &gtk::Box,
        queue_filter: &str,
        layout: QueuePanelLayout,
        scroll_behavior: QueueScrollBehavior,
    ) {
        let queue_scroller = queue_panel_scroller(panel).unwrap_or_else(new_queue_scroller);
        let adjustment = queue_scroller.vadjustment();
        let previous_scroll = adjustment.value();
        let fullscreen_widths = (layout == QueuePanelLayout::Fullscreen).then(|| {
            fullscreen_queue_column_widths(fullscreen_queue_available_width(
                panel,
                self.window.width(),
            ))
        });
        let queue_snapshot = self.state.queue.borrow();
        let render_state =
            queue_panel_render_state(&queue_snapshot, queue_filter, fullscreen_widths);
        let state_cell = match layout {
            QueuePanelLayout::Sidebar => &self.state.queue_sidebar_render_state,
            QueuePanelLayout::Fullscreen => &self.state.queue_fullscreen_render_state,
        };
        let model = {
            let mut state = state_cell.borrow_mut();
            state
                .get_or_insert_with(QueuePanelModelState::new)
                .model
                .clone()
        };
        ensure_queue_panel_view(self, panel, &queue_scroller, &model);

        if scroll_behavior == QueueScrollBehavior::Preserve {
            let updated_current = {
                let mut state = state_cell.borrow_mut();
                if let Some(state) = state.as_mut() {
                    if let Some(previous) = state.render.clone()
                        && previous.same_rows_as(&render_state)
                    {
                        update_queue_current_rows(&state.model, &previous, &render_state);
                        reveal_queue_current_row_later(&queue_scroller, render_state.current_row);
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

        let rows = queue_panel_items(&queue_snapshot, &render_state, layout, fullscreen_widths);
        replace_queue_model(&model, rows);
        clear_queue_panel_static_children(panel);
        if render_state.show_header {
            panel.prepend(&queue_header_row(layout, fullscreen_widths));
        }
        if queue_scroller.parent().is_none() {
            panel.append(&queue_scroller);
        }
        match scroll_behavior {
            QueueScrollBehavior::Preserve => {
                restore_queue_scroll_position(&queue_scroller, previous_scroll);
                reveal_queue_current_row_later(&queue_scroller, render_state.current_row);
            }
            QueueScrollBehavior::Reset => {
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
                fullscreen_widths,
            } => self.queue_row(
                index,
                &entry,
                current,
                reorderable,
                layout,
                fullscreen_widths,
            ),
            QueuePanelItem::Empty { text } => queue_empty_row(&text),
        }
    }

    fn queue_row(
        self: &Rc<Self>,
        index: usize,
        entry: &QueueEntry,
        current: bool,
        reorderable: bool,
        layout: QueuePanelLayout,
        fullscreen_widths: Option<QueueFullscreenColumnWidths>,
    ) -> gtk::Widget {
        if layout == QueuePanelLayout::Fullscreen {
            return self.fullscreen_queue_row(
                index,
                entry,
                current,
                reorderable,
                fullscreen_widths.unwrap_or_else(|| fullscreen_queue_column_widths(1)),
            );
        }

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("queue-row");
        row.set_height_request(QUEUE_ROW_HEIGHT);
        row.set_valign(gtk::Align::Center);
        row.set_focusable(true);
        let accessible_label = format!("{} {}", entry.title, entry.artist);
        row.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
        if current {
            row.add_css_class("queue-row-current");
        }
        if reorderable {
            row.append(&queue_drag_handle(&entry.id));
        }
        let cover = self.cover_tile_for(
            entry.image_ref.as_ref(),
            index as u32 * 7 + entry.duration_seconds,
            50,
            THUMB_COVER_SIZE,
        );
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        labels.set_hexpand(true);
        labels.set_valign(gtk::Align::Center);
        let title = gtk::Label::new(Some(&entry.title));
        title.add_css_class("queue-title");
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let artist = queue_link_label(&entry.artist);
        labels.append(&title);
        labels.append(&artist);
        if let Some(artist_id) = entry.artist_id.clone() {
            add_queue_label_link_style(&artist);
            let shell = Rc::clone(self);
            add_queue_label_click(&artist, move || {
                shell.navigate(Route::ArtistDetail(artist_id.clone()))
            });
        }
        let year_text = (entry.year != 0).then(|| entry.year.to_string());
        let year = gtk::Label::new(year_text.as_deref());
        year.add_css_class("muted");
        year.set_xalign(1.0);
        year.set_width_chars(4);
        year.set_halign(gtk::Align::End);
        row.append(&cover);
        row.append(&labels);
        row.append(&year);
        if reorderable {
            install_queue_row_drop(&row, &self.controller, entry.id.clone(), index);
        }
        install_queue_row_context_menu(&row, self, entry);
        row.upcast()
    }

    fn fullscreen_queue_row(
        self: &Rc<Self>,
        index: usize,
        entry: &QueueEntry,
        current: bool,
        reorderable: bool,
        widths: QueueFullscreenColumnWidths,
    ) -> gtk::Widget {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.add_css_class("queue-row");
        row.set_height_request(QUEUE_ROW_HEIGHT);
        row.set_hexpand(true);
        row.set_halign(gtk::Align::Fill);
        row.set_valign(gtk::Align::Center);
        row.set_focusable(true);
        let accessible_label = format!("{} {}", entry.title, entry.artist);
        row.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
        if current {
            row.add_css_class("queue-row-current");
        }

        let columns = fullscreen_queue_row_box();
        if reorderable {
            columns.append(&queue_drag_handle(&entry.id));
        }
        let cover = self.cover_tile_for(
            entry.image_ref.as_ref(),
            index as u32 * 7 + entry.duration_seconds,
            QUEUE_FULLSCREEN_COVER_COLUMN_WIDTH,
            THUMB_COVER_SIZE,
        );

        let (labels, artist) = queue_identity_cell(entry, widths.title);
        let duration = fullscreen_queue_fixed_cell(
            &format_duration(entry.duration_seconds),
            QUEUE_DURATION_COLUMN_WIDTH,
        );

        columns.append(&cover);
        columns.append(&labels);
        if widths.show_album {
            columns.append(&fullscreen_queue_text_cell(&entry.album, widths.album));
        }
        columns.append(&duration);
        if widths.show_year {
            let year_text = (entry.year != 0).then(|| entry.year.to_string());
            columns.append(&fullscreen_queue_fixed_cell(
                year_text.as_deref().unwrap_or(""),
                QUEUE_YEAR_COLUMN_WIDTH,
            ));
        }
        columns.append(&self.queue_favorite_cell(entry));
        row.append(&columns);

        if let Some(artist_id) = entry.artist_id.clone() {
            add_queue_label_link_style(&artist);
            let shell = Rc::clone(self);
            add_queue_label_click(&artist, move || {
                shell.navigate(Route::ArtistDetail(artist_id.clone()))
            });
        }

        if reorderable {
            install_queue_row_drop(&row, &self.controller, entry.id.clone(), index);
        }
        install_queue_row_context_menu(&row, self, entry);
        row.upcast()
    }

    fn queue_favorite_cell(self: &Rc<Self>, entry: &QueueEntry) -> gtk::Widget {
        let cell = gtk::CenterBox::new();
        cell.add_css_class("queue-favorite-cell");
        cell.set_width_request(QUEUE_FAVORITE_COLUMN_WIDTH);
        cell.set_halign(gtk::Align::Fill);

        let button = favorite_icon_button("Favorite");
        button.add_css_class("queue-favorite-button");
        set_favorite_button_active(&button, entry.favorite);

        let shell = Rc::clone(self);
        let track_id = entry.track_id.clone();
        button.connect_clicked(move |button| {
            let favorite = !favorite_button_is_active(button);
            shell.set_favorite_with_feedback(
                source::FavoriteItemId::Track(track_id.clone()),
                favorite,
                Some(button),
            );
        });

        cell.set_center_widget(Some(&button));
        cell.upcast()
    }
}

impl QueuePanelRenderState {
    fn same_rows_as(&self, next: &Self) -> bool {
        self.filter == next.filter
            && self.row_ids == next.row_ids
            && self.row_count == next.row_count
            && self.show_header == next.show_header
            && self.empty_text == next.empty_text
            && self.fullscreen_widths == next.fullscreen_widths
    }
}

fn queue_panel_render_state(
    queue_snapshot: &Option<domain::QueueSnapshot>,
    queue_filter: &str,
    fullscreen_widths: Option<QueueFullscreenColumnWidths>,
) -> QueuePanelRenderState {
    let has_filter = !queue_filter.is_empty();
    let mut queue_has_entries = false;
    let mut row_ids = Vec::new();
    let mut row_indices = Vec::new();
    let mut row_count = 0usize;
    let mut current_row = None;
    if let Some(snapshot) = queue_snapshot {
        queue_has_entries = !snapshot.entries.is_empty();
        let current_id = snapshot
            .current_index
            .and_then(|index| snapshot.entries.get(index))
            .map(|entry| entry.id.clone());
        if has_filter {
            for (entry_index, entry) in snapshot.entries.iter().enumerate() {
                if !queue_entry_matches_filter(entry, queue_filter) {
                    continue;
                }
                if current_id.as_ref() == Some(&entry.id) {
                    current_row = Some(row_count);
                }
                row_ids.push(entry.id.clone());
                row_indices.push(entry_index);
                row_count += 1;
            }
        } else {
            row_count = snapshot.entries.len();
            for (entry_index, entry) in snapshot.entries.iter().enumerate() {
                if current_id.as_ref() == Some(&entry.id) {
                    current_row = Some(entry_index);
                }
                row_ids.push(entry.id.clone());
                row_indices.push(entry_index);
            }
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
        filter: queue_filter.to_string(),
        show_header: row_count != 0,
        row_ids,
        row_indices,
        row_count,
        current_row,
        empty_text,
        fullscreen_widths,
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
    queue_snapshot: &Option<domain::QueueSnapshot>,
    state: &QueuePanelRenderState,
    layout: QueuePanelLayout,
    fullscreen_widths: Option<QueueFullscreenColumnWidths>,
) -> Vec<QueuePanelItem> {
    let mut items = Vec::new();
    if let Some(snapshot) = queue_snapshot {
        for (row_position, entry_index) in state.row_indices.iter().enumerate() {
            if let Some(entry) = snapshot.entries.get(*entry_index) {
                items.push(QueuePanelItem::Entry {
                    index: *entry_index,
                    entry: Box::new(entry.clone()),
                    current: state.current_row == Some(row_position),
                    reorderable: state.filter.is_empty(),
                    layout,
                    fullscreen_widths,
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
    let controller = shell.controller.clone();
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
            controller.activate_queue_entry(entry.id.clone());
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

pub(super) fn connect_queue_panel_controls(shell: &Rc<Shell>) {
    let filter_shell = Rc::clone(shell);
    shell.queue_search.connect_search_changed(move |entry| {
        *filter_shell.state.queue_filter.borrow_mut() = entry.text().trim().to_string();
        filter_shell.render_queue_panel_reset_scroll();
    });

    let controller = shell.controller.clone();
    shell
        .queue_clear_button
        .connect_clicked(move |_| controller.clear_queue());
}

fn queue_entry_matches_filter(entry: &QueueEntry, filter: &str) -> bool {
    filter.is_empty()
        || entry.title.to_lowercase().contains(filter)
        || entry.artist.to_lowercase().contains(filter)
        || entry.album.to_lowercase().contains(filter)
        || (entry.year != 0 && entry.year.to_string().contains(filter))
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
    glib::idle_add_local_once(move || reveal_queue_current_row(&idle_scroller, current_row));

    let settled_scroller = scroller.clone();
    glib::timeout_add_local_once(Duration::from_millis(80), move || {
        reveal_queue_current_row(&settled_scroller, current_row)
    });
}

fn reveal_queue_current_row(scroller: &gtk::ScrolledWindow, current_row: Option<usize>) {
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
    restore_queue_scroll_position(scroller, target);
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

fn queue_header_row(
    layout: QueuePanelLayout,
    fullscreen_widths: Option<QueueFullscreenColumnWidths>,
) -> gtk::Widget {
    if layout == QueuePanelLayout::Fullscreen {
        return fullscreen_queue_header_row(
            fullscreen_widths.unwrap_or_else(|| fullscreen_queue_column_widths(1)),
        );
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

fn fullscreen_queue_header_row(widths: QueueFullscreenColumnWidths) -> gtk::Widget {
    let header = fullscreen_queue_row_box();
    header.add_css_class("queue-header");

    let drag = fullscreen_queue_fixed_spacer(QUEUE_DRAG_HANDLE_WIDTH);
    let cover = fullscreen_queue_fixed_spacer(QUEUE_FULLSCREEN_COVER_COLUMN_WIDTH);
    let title = queue_header_text_label(&tr("Title").to_uppercase(), widths.title, 0.0);
    let duration = queue_duration_header_icon();
    let favorite = gtk::Label::new(Some(FAVORITE_EMPTY_GLYPH));
    favorite.add_css_class("muted");
    favorite.set_width_request(QUEUE_FAVORITE_COLUMN_WIDTH);
    favorite.set_halign(gtk::Align::Center);

    header.append(&drag);
    header.append(&cover);
    header.append(&title);
    if widths.show_album {
        header.append(&queue_header_text_label(
            &tr("Album").to_uppercase(),
            widths.album,
            0.0,
        ));
    }
    header.append(&duration);
    if widths.show_year {
        header.append(&queue_header_fixed_label(
            &tr("Year").to_uppercase(),
            QUEUE_YEAR_COLUMN_WIDTH,
        ));
    }
    header.append(&favorite);

    header.upcast()
}

fn queue_header_text_label(text: &str, width: i32, xalign: f32) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("muted");
    label.set_xalign(xalign);
    label.set_width_request(width);
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

fn fullscreen_queue_text_cell(text: &str, width: i32) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("muted");
    label.set_xalign(0.0);
    label.set_width_request(width);
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

fn queue_identity_cell(entry: &QueueEntry, width: i32) -> (gtk::Box, gtk::Label) {
    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_width_request(width);
    labels.set_hexpand(false);
    labels.set_halign(gtk::Align::Fill);
    labels.set_valign(gtk::Align::Center);

    let title = gtk::Label::new(Some(&entry.title));
    title.add_css_class("queue-title");
    title.set_xalign(0.0);
    title.set_width_request(width);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let artist = queue_link_label(&entry.artist);
    artist.set_width_request(width);
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

fn fullscreen_queue_available_width(panel: &gtk::Box, window_width: i32) -> i32 {
    let window_width = window_width
        .saturating_sub(QUEUE_FULLSCREEN_HORIZONTAL_MARGIN)
        .max(1);
    let panel_width = panel.width();
    if panel_width > 1 {
        panel_width.min(window_width).max(1)
    } else {
        window_width
    }
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

fn queue_drag_handle(entry_id: &QueueEntryId) -> gtk::Widget {
    let drag = gtk::Image::from_icon_name("list-drag-handle-symbolic");
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
    controller: &AppController,
    entry_id: QueueEntryId,
    target_index: usize,
) {
    let target_id = entry_id;
    let controller = controller.clone();
    let target = target_row.clone();
    let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
    drop_target.connect_drop(move |_, value, _, y| {
        let Ok(drag_id) = value.get::<String>() else {
            return false;
        };
        let drag_id = QueueEntryId::new(drag_id);
        if drag_id == target_id {
            return false;
        }
        let after = y > f64::from(target.height()) / 2.0;
        controller.reorder_queue_entry(drag_id, target_index, after);
        true
    });
    target_row.add_controller(drop_target);
}

fn install_queue_row_context_menu(row: &gtk::Box, shell: &Rc<Shell>, entry: &QueueEntry) {
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
    entry: &QueueEntry,
    pointing_to: Option<gtk::gdk::Rectangle>,
) {
    let main_menu = context_menu_box();
    let track = track_from_queue_entry(entry);
    main_menu.append(&context_menu_action(
        "Remove from Queue",
        "queue.remove",
        "remove-minus",
    ));
    main_menu.append(&context_menu_action(
        "Play",
        "queue.play-now",
        "media-playback-start-symbolic",
    ));
    main_menu.append(&context_menu_action(
        "Play Next",
        "queue.play-next",
        PLAY_NEXT_ICON,
    ));

    if track.is_some() {
        main_menu.append(&context_menu_action(
            "Play Later",
            "queue.play-last",
            PLAY_LATER_ICON,
        ));
    }
    if track.is_some() {
        main_menu.append(&context_menu_submenu_action(
            msgid("Track radio"),
            "queue.play-radio",
            RADIO_ICON,
            &radio_context_submenu("queue"),
        ));
    }
    if let Some(track) = track.as_ref()
        && context_menu_can_add_to_playlist(shell)
    {
        let track_source: Rc<dyn Fn() -> Vec<domain::Track>> = Rc::new({
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

    if entry.favorite {
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
    let album_route = entry.album_id.clone().map(Route::AlbumDetail);
    if album_route.is_some() {
        main_menu.append(&context_menu_action(
            "Go to Album",
            "queue.go-album",
            ALBUM_ICON,
        ));
    }

    let surface = ContextMenuSurface::new(row, "queue", "queue-context-menu", None, &main_menu);
    surface.popover().set_pointing_to(pointing_to.as_ref());

    let controller = shell.controller.clone();
    let entry_id = entry.id.clone();

    surface.add_action("remove", {
        let remove_controller = controller.clone();
        let remove_id = entry_id.clone();
        move || {
            remove_controller.remove_from_queue(remove_id.clone());
        }
    });

    surface.add_action("play-now", {
        let play_now_controller = controller.clone();
        let play_now_id = entry_id.clone();
        move || {
            play_now_controller.activate_queue_entry(play_now_id.clone());
        }
    });

    surface.add_action("play-next", {
        let play_next_controller = controller.clone();
        move || {
            play_next_controller.move_queue_entry_after_current(entry_id.clone());
        }
    });

    if let Some(track) = track.clone() {
        surface.add_action("play-last", {
            let last_controller = controller.clone();
            let track = track.clone();
            move || {
                last_controller.play_last(vec![track.clone()]);
            }
        });

        surface.add_action("play-radio", {
            let radio_controller = controller.clone();
            let track = track.clone();
            move || {
                radio_controller.play_track_radio(track.clone());
            }
        });

        surface.add_action("play-radio-next", {
            let radio_controller = controller.clone();
            let track = track.clone();
            move || {
                radio_controller.play_track_radio_next(track.clone());
            }
        });

        surface.add_action("play-radio-last", {
            let radio_controller = controller.clone();
            move || {
                radio_controller.play_track_radio_last(track.clone());
            }
        });
    }

    surface.add_action("favorite", {
        let favorite_shell = Rc::clone(shell);
        let favorite_track_id = entry.track_id.clone();
        let favorite_value = !entry.favorite;
        move || {
            favorite_shell.set_favorite_with_feedback(
                source::FavoriteItemId::Track(favorite_track_id.clone()),
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

    surface.popup();
}

fn queue_artist_route(entry: &QueueEntry) -> Option<Route> {
    entry.artist_id.clone().map(Route::ArtistDetail)
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
    use super::super::layout::MIN_APP_WINDOW_WIDTH;
    use super::*;
    use domain::{QueueSnapshot, RepeatMode, ServerId, ShuffleState, TrackId};

    fn queue_entry(number: usize, title: &str) -> QueueEntry {
        QueueEntry {
            id: QueueEntryId::new(format!("queue-{number}")),
            track_id: TrackId::fake(number),
            album_id: None,
            title: title.to_string(),
            artist: "Artist".to_string(),
            artist_id: None,
            album: "Album".to_string(),
            year: 2026,
            duration_seconds: 180,
            favorite: false,
            image_ref: None,
            local_path: None,
            source_format: None,
            origin: None,
        }
    }

    fn queue_snapshot(entries: Vec<QueueEntry>) -> Option<QueueSnapshot> {
        Some(QueueSnapshot {
            server_id: ServerId::fake(1),
            entries,
            current_index: Some(0),
            repeat_mode: RepeatMode::All,
            shuffle: ShuffleState::default(),
            shuffle_order: Vec::new(),
            progress_seconds: 0,
        })
    }

    #[test]
    fn queue_render_state_keeps_all_row_ids() {
        let entries = (0..1_000)
            .map(|number| queue_entry(number, &format!("Track {number}")))
            .collect::<Vec<_>>();
        let snapshot = queue_snapshot(entries);

        let state = queue_panel_render_state(&snapshot, "", None);

        assert_eq!(state.row_count, 1_000);
        assert_eq!(state.row_ids.len(), 1_000);
        assert_eq!(state.row_indices.first().copied(), Some(0));
        assert_eq!(state.row_indices.last().copied(), Some(999));
        assert_eq!(state.current_row, Some(0));
    }

    #[test]
    fn filtered_queue_render_state_keeps_all_matching_row_ids() {
        let entries = (0..1_000)
            .map(|number| {
                let title = if number % 100 == 0 {
                    format!("Needle {number}")
                } else {
                    format!("Track {number}")
                };
                queue_entry(number, &title)
            })
            .collect::<Vec<_>>();
        let snapshot = queue_snapshot(entries);

        let state = queue_panel_render_state(&snapshot, "needle", None);

        assert_eq!(state.row_count, 10);
        assert_eq!(state.row_ids.len(), 10);
        assert_eq!(state.row_indices.first().copied(), Some(0));
        assert_eq!(state.row_indices.last().copied(), Some(900));
        assert_eq!(state.current_row, Some(0));
    }

    #[test]
    fn queue_current_change_replaces_only_old_and_new_model_rows() {
        let entries = (0..1_000)
            .map(|number| queue_entry(number, &format!("Track {number}")))
            .collect::<Vec<_>>();
        let mut previous_snapshot = queue_snapshot(entries.clone());
        let mut next_snapshot = queue_snapshot(entries);
        previous_snapshot.as_mut().expect("snapshot").current_index = Some(10);
        next_snapshot.as_mut().expect("snapshot").current_index = Some(900);

        let previous = queue_panel_render_state(&previous_snapshot, "", None);
        let next = queue_panel_render_state(&next_snapshot, "", None);

        assert_eq!(
            queue_current_update_positions(&previous, &next),
            vec![10, 900]
        );
    }

    #[test]
    fn fullscreen_queue_widths_fit_tiny_window() {
        let available = MIN_APP_WINDOW_WIDTH - QUEUE_FULLSCREEN_HORIZONTAL_MARGIN;
        let widths = fullscreen_queue_column_widths(available);
        let total = fullscreen_queue_fixed_width(widths.show_album, widths.show_year)
            + widths.title
            + widths.album;

        assert!(!widths.show_album);
        assert!(!widths.show_year);
        assert!(total <= available);
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
    fn fullscreen_queue_text_columns_fill_available_width() {
        let available = 900;
        let widths = fullscreen_queue_column_widths(available);
        let fixed = fullscreen_queue_fixed_width(widths.show_album, widths.show_year);

        assert_eq!(widths.title + widths.album + fixed, available);
        assert!(widths.show_album);
        assert!(widths.show_year);
        assert!(widths.title > widths.album);
        assert!(widths.title >= QUEUE_FULLSCREEN_TITLE_MIN_WIDTH);
        assert!(widths.album >= QUEUE_FULLSCREEN_ALBUM_MIN_WIDTH);
    }

    #[test]
    fn fullscreen_queue_drops_secondary_columns_when_narrow() {
        let available = 542;
        let widths = fullscreen_queue_column_widths(available);
        let fixed = fullscreen_queue_fixed_width(widths.show_album, widths.show_year);

        assert!(!widths.show_album);
        assert!(!widths.show_year);
        assert_eq!(widths.album, 0);
        assert_eq!(widths.title + fixed, available);
    }
}
