use std::{cell::RefCell, collections::HashMap, rc::Rc};

use ::library::Track;
use adw::prelude::*;

use crate::interactions::add_stateful_link_hover;
use crate::localization::localized_column;
use crate::routes::collection_context::install_dynamic_track_context_menu;
use crate::shell::Shell;

use super::library_fields::item_at_from_item;
use super::route::Route;

#[derive(Clone)]
pub(crate) struct TrackLinkCell {
    button: gtk::Button,
    button_label: gtk::Label,
    label: gtk::Label,
    route: Rc<RefCell<Option<Route>>>,
    hover_text: Rc<RefCell<String>>,
    current_track: Rc<RefCell<Option<Track>>>,
}

thread_local! {
    static TRACK_LINK_CELLS: RefCell<HashMap<usize, TrackLinkCell>> = RefCell::new(HashMap::new());
}

pub(crate) fn list_item_storage_key(list_item: &gtk::ListItem) -> usize {
    list_item.as_ptr() as usize
}

fn store_track_link_cell(list_item: &gtk::ListItem, cell: TrackLinkCell) {
    let key = list_item_storage_key(list_item);
    TRACK_LINK_CELLS.with(|cells| {
        cells.borrow_mut().insert(key, cell);
    });
}

fn track_link_cell(list_item: &gtk::ListItem) -> Option<TrackLinkCell> {
    let key = list_item_storage_key(list_item);
    TRACK_LINK_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

fn remove_track_link_cell(list_item: &gtk::ListItem) {
    let key = list_item_storage_key(list_item);
    TRACK_LINK_CELLS.with(|cells| {
        cells.borrow_mut().remove(&key);
    });
}

pub(crate) fn track_link_column<F>(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
    value: F,
) -> gtk::ColumnViewColumn
where
    F: Fn(&Track) -> (String, Option<Route>) + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let value = Rc::new(value);
    let shell = Rc::clone(shell);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let current_track = Rc::new(RefCell::new(None::<Track>));
        let route = Rc::new(RefCell::new(None::<Route>));
        let hover_text = Rc::new(RefCell::new(String::new()));

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.set_valign(gtk::Align::Center);
        root.set_halign(gtk::Align::Fill);
        root.set_hexpand(true);

        let button_label = gtk::Label::new(None);
        button_label.add_css_class("table-link-label");
        button_label.set_xalign(0.0);
        button_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        button_label.set_halign(gtk::Align::Start);
        button_label.set_hexpand(false);
        button_label.set_width_chars(1);

        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.add_css_class("table-link");
        button.set_halign(gtk::Align::Start);
        button.set_hexpand(false);
        button.set_cursor_from_name(Some("pointer"));
        add_stateful_link_hover(button.upcast_ref(), &button_label, Rc::clone(&hover_text));
        button.set_child(Some(&button_label));
        button.set_visible(false);
        root.append(&button);

        let label = gtk::Label::new(None);
        label.add_css_class("table-link-label");
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_halign(gtk::Align::Start);
        label.set_hexpand(false);
        label.set_width_chars(1);
        label.set_visible(false);
        root.append(&label);

        let click_shell = Rc::clone(&setup_shell);
        let route_for_click = Rc::clone(&route);
        button.connect_clicked(move |_| {
            let route = route_for_click.borrow().clone();
            if let Some(route) = route {
                click_shell.navigate(route);
            }
        });

        install_dynamic_track_context_menu(&root, &setup_shell, Rc::clone(&current_track));
        list_item.set_child(Some(&root));
        store_track_link_cell(
            list_item,
            TrackLinkCell {
                button,
                button_label,
                label,
                route,
                hover_text,
                current_track,
            },
        );
    });

    factory.connect_bind(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(cell) = track_link_cell(list_item) else {
            return;
        };
        let Some(track) = item_at_from_item::<Track>(list_item) else {
            return;
        };
        let (text, route) = value(&track);
        *cell.current_track.borrow_mut() = Some(track);
        if let Some(route) = route {
            *cell.route.borrow_mut() = Some(route);
            *cell.hover_text.borrow_mut() = text.clone();
            cell.button_label.set_text(&text);
            cell.button.set_visible(true);
            cell.label.set_visible(false);
        } else {
            *cell.route.borrow_mut() = None;
            cell.hover_text.borrow_mut().clear();
            cell.label.set_text(&text);
            cell.button.set_visible(false);
            cell.label.set_visible(true);
        }
    });

    factory.connect_unbind(|_, list_item| {
        if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = track_link_cell(list_item)
        {
            cell.button_label.set_text("");
            cell.label.set_text("");
            cell.button.set_visible(false);
            cell.label.set_visible(false);
            cell.hover_text.borrow_mut().clear();
            *cell.route.borrow_mut() = None;
            *cell.current_track.borrow_mut() = None;
        }
    });

    factory.connect_teardown(|_, list_item| {
        if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() {
            remove_track_link_cell(list_item);
        }
    });

    let column = localized_column(title, &factory);
    column.set_fixed_width(width);
    column
}
