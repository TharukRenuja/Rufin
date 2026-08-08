use std::{cell::RefCell, collections::HashMap, rc::Rc};

use ::library::Track;
use adw::prelude::*;

use crate::localization::localized_column;
use crate::routes::collection_context::install_dynamic_track_context_menu;
use crate::shell::Shell;

use super::detail_links::{DetailLinkBinding, DetailLinks};
use super::library_fields::item_at_from_item;

#[derive(Clone)]
pub(crate) struct TrackLinkCell {
    links: DetailLinkBinding,
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
    F: Fn(&Track) -> DetailLinks + 'static,
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

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.set_valign(gtk::Align::Center);
        root.set_halign(gtk::Align::Fill);
        root.set_hexpand(true);

        let label = gtk::Label::new(None);
        label.add_css_class("table-link-label");
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_halign(gtk::Align::Start);
        label.set_hexpand(false);
        label.set_width_chars(1);
        let links = DetailLinkBinding::new(&label, &setup_shell);
        root.append(&label);

        install_dynamic_track_context_menu(&root, &setup_shell, Rc::clone(&current_track));
        list_item.set_child(Some(&root));
        store_track_link_cell(
            list_item,
            TrackLinkCell {
                links,
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
        let links = value(&track);
        *cell.current_track.borrow_mut() = Some(track);
        cell.links.bind(links);
    });

    factory.connect_unbind(|_, list_item| {
        if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = track_link_cell(list_item)
        {
            cell.links.clear();
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
