use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};
use rufin_core::{QueueEntry, QueueEntryId, RightSidebarMode, Route, SearchKind, format_duration};

use crate::controller::AppController;
use crate::i18n::tr;

use super::{
    FAVORITE_EMPTY_GLYPH, Shell, THUMB_COVER_SIZE, add_dynamic_link_hover,
    context_menu_playlist_label, context_menu_playlists, favorite_button_is_active,
    favorite_icon_button, set_favorite_button_active, track_from_queue_entry,
};

const QUEUE_LINK_CLICK_DELAY_MS: u64 = 250;
const QUEUE_FULLSCREEN_COLUMN_SPACING: i32 = 16;
const QUEUE_FULLSCREEN_ROW_HORIZONTAL_PADDING: i32 = 12;
const QUEUE_FULLSCREEN_INDEX_COLUMN_WIDTH: i32 = 24;
const QUEUE_FULLSCREEN_COVER_COLUMN_WIDTH: i32 = 50;
const QUEUE_FULLSCREEN_TITLE_COLUMN_WIDTH: i32 = 320;
const QUEUE_FULLSCREEN_ALBUM_COLUMN_WIDTH: i32 = 260;
const QUEUE_FULLSCREEN_TITLE_MIN_WIDTH: i32 = 160;
const QUEUE_FULLSCREEN_ALBUM_MIN_WIDTH: i32 = 140;
const QUEUE_DURATION_COLUMN_WIDTH: i32 = 82;
const QUEUE_YEAR_COLUMN_WIDTH: i32 = 64;
const QUEUE_FAVORITE_COLUMN_WIDTH: i32 = 64;
const QUEUE_ROW_HEIGHT: i32 = 58;
const QUEUE_DEFAULT_VIEWPORT_ROWS: usize = 24;
const QUEUE_OVERSCAN_ROWS: usize = 8;

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
    row_start: usize,
    row_count: usize,
    current_id: Option<QueueEntryId>,
    show_header: bool,
    empty_text: Option<String>,
    fullscreen_widths: Option<QueueFullscreenColumnWidths>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum QueuePanelLayout {
    Sidebar,
    Fullscreen,
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
        let has_filter = !queue_filter.is_empty();
        let queue_scroller =
            queue_panel_scroller(panel).unwrap_or_else(|| new_queue_scroller(self));
        let adjustment = queue_scroller.vadjustment();
        let previous_scroll = adjustment.value();
        let target_scroll = match scroll_behavior {
            QueueScrollBehavior::Preserve => previous_scroll,
            QueueScrollBehavior::Reset => 0.0,
        };
        let viewport_height = adjustment.page_size();
        let fullscreen_widths = (layout == QueuePanelLayout::Fullscreen).then(|| {
            fullscreen_queue_column_widths(fullscreen_queue_available_width(
                panel,
                self.window.width(),
            ))
        });
        let queue_snapshot = self.state.queue.borrow();
        let render_state = queue_panel_render_state(
            &queue_snapshot,
            queue_filter,
            fullscreen_widths,
            target_scroll,
            viewport_height,
        );
        let state_cell = match layout {
            QueuePanelLayout::Sidebar => &self.state.queue_sidebar_render_state,
            QueuePanelLayout::Fullscreen => &self.state.queue_fullscreen_render_state,
        };
        if scroll_behavior == QueueScrollBehavior::Preserve
            && state_cell
                .borrow()
                .as_ref()
                .is_some_and(|previous| previous.same_rows_as(&render_state))
            && update_queue_current_rows(panel, &render_state)
        {
            *state_cell.borrow_mut() = Some(render_state);
            return;
        }

        clear_queue_panel_static_children(panel);

        let queue_list = gtk::ListBox::new();
        queue_list.add_css_class("queue-list");
        queue_list.set_vexpand(true);
        queue_list.set_selection_mode(gtk::SelectionMode::None);
        let mut queue_has_entries = false;
        let mut show_header = false;
        if let Some(snapshot) = &*queue_snapshot {
            queue_has_entries = !snapshot.entries.is_empty();
            show_header = render_state.show_header;
            for index in &render_state.row_indices {
                if let Some(entry) = snapshot.entries.get(*index) {
                    queue_list.append(&self.queue_row(
                        *index,
                        entry,
                        snapshot.current_index,
                        layout,
                        fullscreen_widths,
                    ));
                }
            }
        }
        if render_state.row_count == 0 && queue_list.first_child().is_none() {
            let empty_text = if has_filter && queue_has_entries {
                tr("No queue items match the search.")
            } else {
                tr("Add music to start a queue.")
            };
            let empty = gtk::Label::new(Some(&empty_text));
            empty.add_css_class("muted");
            empty.set_wrap(true);
            empty.set_margin_top(24);
            queue_list.append(&empty);
        }
        if show_header {
            panel.prepend(&queue_header_row(layout, fullscreen_widths));
        }
        if render_state.row_count == 0 {
            queue_scroller.set_child(Some(&queue_list));
        } else {
            let virtual_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
            let top_spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
            top_spacer.set_height_request(virtual_spacer_height(render_state.row_start));
            let bottom_spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
            bottom_spacer.set_height_request(virtual_spacer_height(
                render_state
                    .row_count
                    .saturating_sub(render_state.row_start + render_state.row_ids.len()),
            ));
            virtual_box.append(&top_spacer);
            virtual_box.append(&queue_list);
            virtual_box.append(&bottom_spacer);
            queue_scroller.set_child(Some(&virtual_box));
        }
        if queue_scroller.parent().is_none() {
            panel.append(&queue_scroller);
        }
        match scroll_behavior {
            QueueScrollBehavior::Preserve => {
                restore_queue_scroll_position(&queue_scroller, previous_scroll);
            }
            QueueScrollBehavior::Reset => {
                restore_queue_scroll_position(&queue_scroller, 0.0);
            }
        }
        *state_cell.borrow_mut() = Some(render_state);
    }

    fn queue_row(
        self: &Rc<Self>,
        index: usize,
        entry: &QueueEntry,
        current_index: Option<usize>,
        layout: QueuePanelLayout,
        fullscreen_widths: Option<QueueFullscreenColumnWidths>,
    ) -> gtk::Widget {
        if layout == QueuePanelLayout::Fullscreen {
            return self.fullscreen_queue_row(
                index,
                entry,
                current_index,
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
        if current_index == Some(index) {
            row.add_css_class("queue-row-current");
        }
        let number = gtk::Label::new(Some(&(index + 1).to_string()));
        number.add_css_class("muted");
        number.set_width_chars(2);
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
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let artist = queue_link_label(&entry.artist);
        labels.append(&title);
        labels.append(&artist);
        if let Some(artist_id) = entry.artist_id.clone() {
            let shell = Rc::clone(self);
            add_queue_label_click(&artist, move || {
                shell.navigate(Route::ArtistDetail(artist_id.clone()))
            });
        } else if !entry.artist.trim().is_empty() {
            let shell = Rc::clone(self);
            let artist_name = entry.artist.clone();
            add_queue_label_click(&artist, move || {
                shell.navigate(Route::Search {
                    query: artist_name.clone(),
                    kind: SearchKind::Artists,
                });
            });
        }
        let year_text = (entry.year != 0).then(|| entry.year.to_string());
        let year = gtk::Label::new(year_text.as_deref());
        year.add_css_class("muted");
        year.set_xalign(1.0);
        year.set_width_chars(4);
        year.set_halign(gtk::Align::End);
        row.append(&number);
        row.append(&cover);
        row.append(&labels);
        row.append(&year);
        install_queue_row_activation(&row, &self.controller, entry.id.clone());
        install_queue_row_context_menu(&row, self, entry);
        row.upcast()
    }

    fn fullscreen_queue_row(
        self: &Rc<Self>,
        index: usize,
        entry: &QueueEntry,
        current_index: Option<usize>,
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
        if current_index == Some(index) {
            row.add_css_class("queue-row-current");
        }

        let columns = fullscreen_queue_row_box();
        let number = gtk::Label::new(Some(&(index + 1).to_string()));
        number.add_css_class("muted");
        number.set_xalign(1.0);
        number.set_width_request(QUEUE_FULLSCREEN_INDEX_COLUMN_WIDTH);
        number.set_halign(gtk::Align::Fill);

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

        columns.append(&number);
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
            let shell = Rc::clone(self);
            add_queue_label_click(&artist, move || {
                shell.navigate(Route::ArtistDetail(artist_id.clone()))
            });
        } else if !entry.artist.trim().is_empty() {
            let shell = Rc::clone(self);
            let artist_name = entry.artist.clone();
            add_queue_label_click(&artist, move || {
                shell.navigate(Route::Search {
                    query: artist_name.clone(),
                    kind: SearchKind::Artists,
                });
            });
        }

        install_queue_row_activation(&row, &self.controller, entry.id.clone());
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

        let controller = self.controller.clone();
        let track_id = entry.track_id.clone();
        button.connect_clicked(move |button| {
            controller.set_track_favorite(track_id.clone(), !favorite_button_is_active(button));
        });

        cell.set_center_widget(Some(&button));
        cell.upcast()
    }
}

impl QueuePanelRenderState {
    fn same_rows_as(&self, next: &Self) -> bool {
        self.filter == next.filter
            && self.row_ids == next.row_ids
            && self.row_start == next.row_start
            && self.row_count == next.row_count
            && self.show_header == next.show_header
            && self.empty_text == next.empty_text
            && self.fullscreen_widths == next.fullscreen_widths
    }
}

fn queue_panel_render_state(
    queue_snapshot: &Option<rufin_core::QueueSnapshot>,
    queue_filter: &str,
    fullscreen_widths: Option<QueueFullscreenColumnWidths>,
    scroll_value: f64,
    viewport_height: f64,
) -> QueuePanelRenderState {
    let has_filter = !queue_filter.is_empty();
    let mut queue_has_entries = false;
    let mut row_ids = Vec::new();
    let mut row_indices = Vec::new();
    let mut row_count = 0usize;
    let mut current_id = None;
    if let Some(snapshot) = queue_snapshot {
        queue_has_entries = !snapshot.entries.is_empty();
        current_id = snapshot
            .current_index
            .and_then(|index| snapshot.entries.get(index))
            .map(|entry| entry.id.clone());
        if has_filter {
            let (window_start, window_end) =
                queue_virtual_window(usize::MAX, scroll_value, viewport_height);
            for (entry_index, entry) in snapshot.entries.iter().enumerate() {
                if !queue_entry_matches_filter(entry, queue_filter) {
                    continue;
                }
                if row_count >= window_start && row_count < window_end {
                    row_ids.push(entry.id.clone());
                    row_indices.push(entry_index);
                }
                row_count += 1;
            }
        } else {
            row_count = snapshot.entries.len();
            let (window_start, window_end) =
                queue_virtual_window(row_count, scroll_value, viewport_height);
            for entry_index in window_start..window_end {
                if let Some(entry) = snapshot.entries.get(entry_index) {
                    row_ids.push(entry.id.clone());
                    row_indices.push(entry_index);
                }
            }
        }
    }
    let row_start = queue_virtual_window(row_count, scroll_value, viewport_height).0;
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
        row_start,
        row_count,
        current_id,
        empty_text,
        fullscreen_widths,
    }
}

fn update_queue_current_rows(panel: &gtk::Box, state: &QueuePanelRenderState) -> bool {
    let Some(list) = queue_panel_list(panel) else {
        return false;
    };
    let mut row_position = 0usize;
    let mut child = list.first_child();
    while let Some(widget) = child {
        if row_position >= state.row_ids.len() {
            return false;
        }
        let Some(row) = queue_content_row(&widget) else {
            return false;
        };
        if state.current_id.as_ref() == Some(&state.row_ids[row_position]) {
            row.add_css_class("queue-row-current");
        } else {
            row.remove_css_class("queue-row-current");
        }
        row_position += 1;
        child = widget.next_sibling();
    }
    row_position == state.row_ids.len()
}

fn queue_panel_list(panel: &gtk::Box) -> Option<gtk::ListBox> {
    let scroller = queue_panel_scroller(panel)?;
    let child = scroller.child()?;
    if let Ok(list) = child.clone().downcast::<gtk::ListBox>() {
        return Some(list);
    }
    let container = child.downcast::<gtk::Box>().ok()?;
    let mut child = container.first_child();
    while let Some(widget) = child {
        if let Ok(list) = widget.clone().downcast::<gtk::ListBox>() {
            return Some(list);
        }
        child = widget.next_sibling();
    }
    None
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

fn queue_content_row(widget: &gtk::Widget) -> Option<gtk::Widget> {
    if widget.has_css_class("queue-row") {
        return Some(widget.clone());
    }
    widget
        .clone()
        .downcast::<gtk::ListBoxRow>()
        .ok()
        .and_then(|row| row.child())
        .filter(|child| child.has_css_class("queue-row"))
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

fn new_queue_scroller(shell: &Rc<Shell>) -> gtk::ScrolledWindow {
    let scroller = gtk::ScrolledWindow::new();
    scroller.add_css_class("queue-scroller");
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_vexpand(true);
    let shell = Rc::clone(shell);
    scroller
        .vadjustment()
        .connect_value_changed(move |_| shell.schedule_queue_panel_render());
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

fn queue_virtual_window(
    row_count: usize,
    scroll_value: f64,
    viewport_height: f64,
) -> (usize, usize) {
    if row_count == 0 {
        return (0, 0);
    }
    let row_height = f64::from(QUEUE_ROW_HEIGHT);
    let overscan_height = row_height * QUEUE_OVERSCAN_ROWS as f64;
    let viewport_height = if viewport_height > 0.0 {
        viewport_height
    } else {
        row_height * QUEUE_DEFAULT_VIEWPORT_ROWS as f64
    };
    let top = (scroll_value - overscan_height).max(0.0);
    let bottom = scroll_value + viewport_height + overscan_height;
    let start = (top / row_height).floor().max(0.0) as usize;
    let end = (bottom / row_height).ceil().max(start as f64) as usize;
    (
        start.min(row_count),
        end.min(row_count).max(start.min(row_count)),
    )
}

fn virtual_spacer_height(row_count: usize) -> i32 {
    let rows = i32::try_from(row_count).unwrap_or(i32::MAX / QUEUE_ROW_HEIGHT);
    rows.saturating_mul(QUEUE_ROW_HEIGHT)
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

    let number = fullscreen_queue_fixed_spacer(QUEUE_FULLSCREEN_INDEX_COLUMN_WIDTH);
    let cover = fullscreen_queue_fixed_spacer(QUEUE_FULLSCREEN_COVER_COLUMN_WIDTH);
    let title = queue_header_text_label(&tr("Title").to_uppercase(), widths.title, 0.0);
    let duration = queue_duration_header_icon();
    let favorite = gtk::Label::new(Some(FAVORITE_EMPTY_GLYPH));
    favorite.add_css_class("muted");
    favorite.set_width_request(QUEUE_FAVORITE_COLUMN_WIDTH);
    favorite.set_halign(gtk::Align::Center);

    header.append(&number);
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
    let window_width = window_width.saturating_sub(72).max(1);
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
        title: available_width
            .saturating_sub(title_fixed)
            .max(QUEUE_FULLSCREEN_TITLE_MIN_WIDTH),
        album: 0,
        show_album: false,
        show_year: false,
    }
}

fn fullscreen_queue_fixed_width(show_album: bool, show_year: bool) -> i32 {
    let columns = 5 + i32::from(show_album) + i32::from(show_year);
    QUEUE_FULLSCREEN_ROW_HORIZONTAL_PADDING
        + QUEUE_FULLSCREEN_INDEX_COLUMN_WIDTH
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

fn install_queue_row_activation(
    row: &gtk::Box,
    controller: &AppController,
    entry_id: QueueEntryId,
) {
    let controller = controller.clone();
    let click = gtk::GestureClick::new();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.set_button(1);
    click.connect_released(move |gesture, press_count, _, _| {
        if press_count == 2 {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let controller = controller.clone();
            let entry_id = entry_id.clone();
            glib::idle_add_local_once(move || {
                controller.activate_queue_entry(entry_id);
            });
        }
    });
    row.add_controller(click);
}

fn install_queue_row_context_menu(row: &gtk::Box, shell: &Rc<Shell>, entry: &QueueEntry) {
    let shell = Rc::clone(shell);
    let entry = entry.clone();

    let click_shell = Rc::clone(&shell);
    let click_entry = entry.clone();
    let click_row = row.downgrade();
    let click = gtk::GestureClick::new();
    click.set_button(3);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed(move |click, _, x, y| {
        click.set_state(gtk::EventSequenceState::Claimed);
        if let Some(row) = click_row.upgrade() {
            show_queue_row_context_menu(
                &row,
                &click_shell,
                &click_entry,
                Some(gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)),
            );
        }
    });
    row.add_controller(click);

    let key_shell = Rc::clone(&shell);
    let key_entry = entry;
    let key_row = row.downgrade();
    let key_controller = gtk::EventControllerKey::new();
    key_controller.connect_key_pressed(move |_, key, _, state| {
        let opens_menu = key == gtk::gdk::Key::Menu
            || (key == gtk::gdk::Key::F10 && state.contains(gtk::gdk::ModifierType::SHIFT_MASK));
        if opens_menu {
            if let Some(row) = key_row.upgrade() {
                show_queue_row_context_menu(&row, &key_shell, &key_entry, None);
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    row.add_controller(key_controller);
}

fn show_queue_row_context_menu(
    row: &gtk::Box,
    shell: &Rc<Shell>,
    entry: &QueueEntry,
    pointing_to: Option<gtk::gdk::Rectangle>,
) {
    let playlists = context_menu_playlists(shell);
    let menu = gio::Menu::new();
    menu.append(Some(&tr("Remove from Queue")), Some("queue.remove"));
    menu.append(Some(&tr("Play Now")), Some("queue.play-now"));
    menu.append(Some(&tr("Play Next")), Some("queue.play-next"));

    let track = track_from_queue_entry(entry);
    if track.is_some() && !playlists.is_empty() {
        let playlist_menu = gio::Menu::new();
        for (index, playlist) in playlists.iter().enumerate() {
            let label = context_menu_playlist_label(&playlist.name);
            playlist_menu.append(
                Some(&label),
                Some(&format!("queue.add-to-playlist-{index}")),
            );
        }
        menu.append_submenu(Some(&tr("Add to Playlist")), &playlist_menu);
    }

    menu.append(
        Some(&tr(if entry.favorite {
            "Remove from Favorites"
        } else {
            "Add to Favorites"
        })),
        Some("queue.favorite"),
    );
    let artist_route = queue_artist_route(entry);
    if artist_route.is_some() {
        menu.append(Some(&tr("Go to Artist")), Some("queue.go-artist"));
    }
    let album_route = entry.album_id.clone().map(Route::AlbumDetail);
    if album_route.is_some() {
        menu.append(Some(&tr("Go to Album")), Some("queue.go-album"));
    }

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.add_css_class("queue-context-menu");
    popover.set_parent(row);
    popover.set_pointing_to(pointing_to.as_ref());
    popover.connect_closed(|popover| popover.unparent());

    let actions = gio::SimpleActionGroup::new();
    let controller = shell.controller.clone();
    let entry_id = entry.id.clone();

    let remove = gio::SimpleAction::new("remove", None);
    let remove_controller = controller.clone();
    let remove_id = entry_id.clone();
    let remove_popover = popover.downgrade();
    remove.connect_activate(move |_, _| {
        if let Some(popover) = remove_popover.upgrade() {
            popover.popdown();
        }
        remove_controller.remove_from_queue(remove_id.clone());
    });
    actions.add_action(&remove);

    let play_now = gio::SimpleAction::new("play-now", None);
    let play_now_controller = controller.clone();
    let play_now_id = entry_id.clone();
    let play_now_popover = popover.downgrade();
    play_now.connect_activate(move |_, _| {
        if let Some(popover) = play_now_popover.upgrade() {
            popover.popdown();
        }
        play_now_controller.activate_queue_entry(play_now_id.clone());
    });
    actions.add_action(&play_now);

    let play_next = gio::SimpleAction::new("play-next", None);
    let play_next_controller = controller.clone();
    let play_next_popover = popover.downgrade();
    play_next.connect_activate(move |_, _| {
        if let Some(popover) = play_next_popover.upgrade() {
            popover.popdown();
        }
        play_next_controller.move_queue_entry_after_current(entry_id.clone());
    });
    actions.add_action(&play_next);

    if let Some(track) = track {
        for (index, playlist) in playlists.iter().enumerate() {
            let action_name = format!("add-to-playlist-{index}");
            let add = gio::SimpleAction::new(&action_name, None);
            let add_controller = controller.clone();
            let playlist_id = playlist.id.clone();
            let action_track = track.clone();
            let add_popover = popover.downgrade();
            add.connect_activate(move |_, _| {
                if let Some(popover) = add_popover.upgrade() {
                    popover.popdown();
                }
                add_controller
                    .add_tracks_to_playlist(playlist_id.clone(), vec![action_track.clone()]);
            });
            actions.add_action(&add);
        }
    }

    let favorite = gio::SimpleAction::new("favorite", None);
    let favorite_controller = controller.clone();
    let favorite_track_id = entry.track_id.clone();
    let favorite_value = !entry.favorite;
    let favorite_popover = popover.downgrade();
    favorite.connect_activate(move |_, _| {
        if let Some(popover) = favorite_popover.upgrade() {
            popover.popdown();
        }
        favorite_controller.set_track_favorite(favorite_track_id.clone(), favorite_value);
    });
    actions.add_action(&favorite);

    if let Some(artist_route) = artist_route {
        let go_artist = gio::SimpleAction::new("go-artist", None);
        let action_shell = Rc::clone(shell);
        let go_artist_popover = popover.downgrade();
        go_artist.connect_activate(move |_, _| {
            if let Some(popover) = go_artist_popover.upgrade() {
                popover.popdown();
            }
            let shell = Rc::clone(&action_shell);
            let route = artist_route.clone();
            glib::idle_add_local_once(move || shell.navigate(route));
        });
        actions.add_action(&go_artist);
    }
    if let Some(album_route) = album_route {
        let go_album = gio::SimpleAction::new("go-album", None);
        let action_shell = Rc::clone(shell);
        let go_album_popover = popover.downgrade();
        go_album.connect_activate(move |_, _| {
            if let Some(popover) = go_album_popover.upgrade() {
                popover.popdown();
            }
            let shell = Rc::clone(&action_shell);
            let route = album_route.clone();
            glib::idle_add_local_once(move || shell.navigate(route));
        });
        actions.add_action(&go_album);
    }

    row.insert_action_group("queue", Some(&actions));
    popover.popup();
}

fn queue_artist_route(entry: &QueueEntry) -> Option<Route> {
    if let Some(artist_id) = entry.artist_id.clone() {
        Some(Route::ArtistDetail(artist_id))
    } else if !entry.artist.trim().is_empty() {
        Some(Route::Search {
            query: entry.artist.clone(),
            kind: SearchKind::Artists,
        })
    } else {
        None
    }
}

fn queue_link_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("queue-link");
    label.add_css_class("muted");
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_cursor_from_name(Some("pointer"));
    add_dynamic_link_hover(label.upcast_ref(), &label);
    label
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
    use super::*;
    use rufin_core::{QueueSnapshot, RepeatMode, ServerId, ShuffleState, TrackId};

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
    fn queue_render_state_keeps_only_virtual_window_ids() {
        let entries = (0..10_000)
            .map(|number| queue_entry(number, &format!("Track {number}")))
            .collect::<Vec<_>>();
        let snapshot = queue_snapshot(entries);

        let state =
            queue_panel_render_state(&snapshot, "", None, 0.0, f64::from(QUEUE_ROW_HEIGHT * 10));

        assert_eq!(state.row_count, 10_000);
        assert!(state.row_ids.len() <= QUEUE_DEFAULT_VIEWPORT_ROWS);
        assert_eq!(state.row_indices.first().copied(), Some(0));
    }

    #[test]
    fn filtered_queue_render_state_keeps_only_virtual_window_ids() {
        let entries = (0..10_000)
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

        let state = queue_panel_render_state(
            &snapshot,
            "needle",
            None,
            0.0,
            f64::from(QUEUE_ROW_HEIGHT * 10),
        );

        assert_eq!(state.row_count, 100);
        assert!(state.row_ids.len() <= QUEUE_DEFAULT_VIEWPORT_ROWS);
        assert_eq!(state.row_indices.first().copied(), Some(0));
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
