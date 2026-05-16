use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};
use rufin_core::{QueueEntry, QueueEntryId, Route, SearchKind};

use crate::controller::AppController;
use crate::i18n::tr;

use super::{Shell, THUMB_COVER_SIZE, add_dynamic_link_hover};

const QUEUE_LINK_CLICK_DELAY_MS: u64 = 250;

impl Shell {
    pub(super) fn render_queue_panel(self: &Rc<Self>) {
        let queue_snapshot = self.state.queue.borrow().clone();
        let queue_filter = self.state.queue_filter.borrow().trim().to_lowercase();
        let has_filter = !queue_filter.is_empty();

        while let Some(child) = self.queue_panel.first_child() {
            self.queue_panel.remove(&child);
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
                if !queue_entry_matches_filter(entry, &queue_filter) {
                    continue;
                }
                if visible_entries == 0 {
                    self.queue_panel.append(&queue_header_row());
                }
                queue_list.append(&self.queue_row(index, entry, snapshot.current_index));
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
        self.queue_panel.append(&queue_scroller);
    }

    fn queue_row(
        self: &Rc<Self>,
        index: usize,
        entry: &QueueEntry,
        current_index: Option<usize>,
    ) -> gtk::Widget {
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

fn queue_header_row() -> gtk::Widget {
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
