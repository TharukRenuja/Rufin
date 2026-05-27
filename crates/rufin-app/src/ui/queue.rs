use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};
use rufin_core::{QueueEntry, QueueEntryId, Route, SearchKind, format_duration};

use crate::controller::AppController;
use crate::i18n::tr;

use super::{
    FAVORITE_EMPTY_GLYPH, Shell, THUMB_COVER_SIZE, add_dynamic_link_hover,
    favorite_button_is_active, favorite_icon_button, set_favorite_button_active,
};

const QUEUE_LINK_CLICK_DELAY_MS: u64 = 250;
const QUEUE_FULLSCREEN_COLUMN_SPACING: i32 = 16;
const QUEUE_FULLSCREEN_INDEX_COLUMN_WIDTH: i32 = 24;
const QUEUE_FULLSCREEN_COVER_COLUMN_WIDTH: i32 = 50;
const QUEUE_FULLSCREEN_TITLE_COLUMN_WIDTH: i32 = 190;
const QUEUE_FULLSCREEN_ALBUM_COLUMN_WIDTH: i32 = 270;
const QUEUE_FULLSCREEN_TEXT_MIN_WIDTH: i32 = 1;
const QUEUE_FULLSCREEN_TITLE_WIDTH_CHARS: i32 = 24;
const QUEUE_FULLSCREEN_ALBUM_WIDTH_CHARS: i32 = 34;
const QUEUE_DURATION_COLUMN_WIDTH: i32 = 82;
const QUEUE_YEAR_COLUMN_WIDTH: i32 = 64;
const QUEUE_FAVORITE_COLUMN_WIDTH: i32 = 64;

#[derive(Clone, Copy, Eq, PartialEq)]
enum QueuePanelLayout {
    Sidebar,
    Fullscreen,
}

impl Shell {
    pub(super) fn render_queue_panel(self: &Rc<Self>) {
        let queue_filter = self.state.queue_filter.borrow().trim().to_lowercase();
        self.render_queue_panel_into(&self.queue_panel, &queue_filter, QueuePanelLayout::Sidebar);
        self.render_queue_panel_into(
            &self.fullscreen_player.queue_panel,
            "",
            QueuePanelLayout::Fullscreen,
        );
    }

    fn render_queue_panel_into(
        self: &Rc<Self>,
        panel: &gtk::Box,
        queue_filter: &str,
        layout: QueuePanelLayout,
    ) {
        let queue_snapshot = self.state.queue.borrow().clone();
        let has_filter = !queue_filter.is_empty();

        while let Some(child) = panel.first_child() {
            panel.remove(&child);
        }

        let queue_scroller = gtk::ScrolledWindow::new();
        queue_scroller.add_css_class("queue-scroller");
        queue_scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        queue_scroller.set_vexpand(true);

        let queue_list = gtk::ListBox::new();
        queue_list.add_css_class("queue-list");
        queue_list.set_vexpand(true);
        queue_list.set_selection_mode(gtk::SelectionMode::None);
        let mut queue_has_entries = false;
        if let Some(snapshot) = &queue_snapshot {
            queue_has_entries = !snapshot.entries.is_empty();
            let mut visible_entries = 0;
            for (index, entry) in snapshot.entries.iter().enumerate() {
                if !queue_entry_matches_filter(entry, queue_filter) {
                    continue;
                }
                if visible_entries == 0 {
                    panel.append(&queue_header_row(layout));
                }
                queue_list.append(&self.queue_row(index, entry, snapshot.current_index, layout));
                visible_entries += 1;
            }
        }
        if queue_list.first_child().is_none() {
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
        queue_scroller.set_child(Some(&queue_list));
        panel.append(&queue_scroller);
    }

    fn queue_row(
        self: &Rc<Self>,
        index: usize,
        entry: &QueueEntry,
        current_index: Option<usize>,
        layout: QueuePanelLayout,
    ) -> gtk::Widget {
        if layout == QueuePanelLayout::Fullscreen {
            return self.fullscreen_queue_row(index, entry, current_index);
        }

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("queue-row");
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
        install_queue_row_context_menu(&row, &self.controller, entry.id.clone());
        row.upcast()
    }

    fn fullscreen_queue_row(
        self: &Rc<Self>,
        index: usize,
        entry: &QueueEntry,
        current_index: Option<usize>,
    ) -> gtk::Widget {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.add_css_class("queue-row");
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

        let (labels, artist) = queue_identity_cell(entry);
        let album = fullscreen_queue_text_cell(
            &entry.album,
            QUEUE_FULLSCREEN_ALBUM_COLUMN_WIDTH,
            QUEUE_FULLSCREEN_ALBUM_WIDTH_CHARS,
        );
        let duration = fullscreen_queue_fixed_cell(
            &format_duration(entry.duration_seconds),
            QUEUE_DURATION_COLUMN_WIDTH,
        );
        let year_text = (entry.year != 0).then(|| entry.year.to_string());
        let year = fullscreen_queue_fixed_cell(
            year_text.as_deref().unwrap_or(""),
            QUEUE_YEAR_COLUMN_WIDTH,
        );

        columns.append(&number);
        columns.append(&cover);
        columns.append(&labels);
        columns.append(&album);
        columns.append(&fullscreen_queue_expanding_spacer());
        columns.append(&duration);
        columns.append(&year);
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
        install_queue_row_context_menu(&row, &self.controller, entry.id.clone());
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

pub(super) fn connect_queue_panel_controls(shell: &Rc<Shell>) {
    let filter_shell = Rc::clone(shell);
    shell.queue_search.connect_search_changed(move |entry| {
        *filter_shell.state.queue_filter.borrow_mut() = entry.text().trim().to_string();
        filter_shell.render_queue_panel();
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

    let number = fullscreen_queue_fixed_spacer(QUEUE_FULLSCREEN_INDEX_COLUMN_WIDTH);
    let cover = fullscreen_queue_fixed_spacer(QUEUE_FULLSCREEN_COVER_COLUMN_WIDTH);
    let title = queue_header_text_label(
        &tr("Title").to_uppercase(),
        QUEUE_FULLSCREEN_TITLE_COLUMN_WIDTH,
        QUEUE_FULLSCREEN_TITLE_WIDTH_CHARS,
        0.0,
    );
    let album = queue_header_text_label(
        &tr("Album").to_uppercase(),
        QUEUE_FULLSCREEN_ALBUM_COLUMN_WIDTH,
        QUEUE_FULLSCREEN_ALBUM_WIDTH_CHARS,
        0.5,
    );
    let duration = queue_duration_header_icon();
    let year = queue_header_fixed_label(&tr("Year").to_uppercase(), QUEUE_YEAR_COLUMN_WIDTH);
    let favorite = gtk::Label::new(Some(FAVORITE_EMPTY_GLYPH));
    favorite.add_css_class("muted");
    favorite.set_width_request(QUEUE_FAVORITE_COLUMN_WIDTH);
    favorite.set_halign(gtk::Align::Center);

    header.append(&number);
    header.append(&cover);
    header.append(&title);
    header.append(&album);
    header.append(&fullscreen_queue_expanding_spacer());
    header.append(&duration);
    header.append(&year);
    header.append(&favorite);

    header.upcast()
}

fn queue_header_text_label(text: &str, _width: i32, width_chars: i32, xalign: f32) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("muted");
    label.set_xalign(xalign);
    label.set_width_request(QUEUE_FULLSCREEN_TEXT_MIN_WIDTH);
    label.set_max_width_chars(width_chars);
    label.set_hexpand(true);
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

fn fullscreen_queue_text_cell(text: &str, _width: i32, width_chars: i32) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("muted");
    label.set_xalign(0.5);
    label.set_width_request(QUEUE_FULLSCREEN_TEXT_MIN_WIDTH);
    label.set_max_width_chars(width_chars);
    label.set_hexpand(true);
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

fn queue_identity_cell(entry: &QueueEntry) -> (gtk::Box, gtk::Label) {
    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_width_request(QUEUE_FULLSCREEN_TEXT_MIN_WIDTH);
    labels.set_hexpand(true);
    labels.set_halign(gtk::Align::Fill);
    labels.set_valign(gtk::Align::Center);

    let title = gtk::Label::new(Some(&entry.title));
    title.set_xalign(0.0);
    title.set_max_width_chars(QUEUE_FULLSCREEN_TITLE_WIDTH_CHARS);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let artist = queue_link_label(&entry.artist);
    artist.set_max_width_chars(QUEUE_FULLSCREEN_TITLE_WIDTH_CHARS);
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

fn fullscreen_queue_expanding_spacer() -> gtk::Box {
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    spacer
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

fn install_queue_row_context_menu(
    row: &gtk::Box,
    controller: &AppController,
    entry_id: QueueEntryId,
) {
    let menu = gio::Menu::new();
    menu.append(Some(&tr("Remove from Queue")), Some("queue.remove"));
    menu.append(Some(&tr("Play Now")), Some("queue.play-now"));
    menu.append(Some(&tr("Play Next")), Some("queue.play-next"));

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.add_css_class("queue-context-menu");
    popover.set_parent(row);

    let actions = gio::SimpleActionGroup::new();

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

    row.insert_action_group("queue", Some(&actions));

    let click_popover = popover.downgrade();
    let click = gtk::GestureClick::new();
    click.set_button(3);
    click.connect_pressed(move |_, _, x, y| {
        if let Some(popover) = click_popover.upgrade() {
            let rect = gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
            popover.set_pointing_to(Some(&rect));
            popover.popup();
        }
    });
    row.add_controller(click);

    let key_popover = popover.downgrade();
    let key_controller = gtk::EventControllerKey::new();
    key_controller.connect_key_pressed(move |_, key, _, state| {
        let opens_menu = key == gtk::gdk::Key::Menu
            || (key == gtk::gdk::Key::F10 && state.contains(gtk::gdk::ModifierType::SHIFT_MASK));
        if opens_menu {
            if let Some(popover) = key_popover.upgrade() {
                popover.set_pointing_to(None);
                popover.popup();
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    row.add_controller(key_controller);
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
