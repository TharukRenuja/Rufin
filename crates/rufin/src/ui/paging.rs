use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use domain::Route;
use gtk::glib;

use super::Shell;

pub(super) struct PagedGridCursor {
    pub(super) offset: Cell<usize>,
    pub(super) total: Cell<usize>,
    pub(super) loading: Cell<bool>,
}

impl Shell {
    pub(super) fn can_load_grid_page(&self, cursor: &PagedGridCursor, route: &Route) -> bool {
        if cursor.loading.get() || cursor.offset.get() >= cursor.total.get() {
            return false;
        }
        if self.state.routes.borrow().current() != route {
            return false;
        }
        cursor.loading.set(true);
        true
    }
}

pub(super) fn finish_grid_page(
    cursor: &PagedGridCursor,
    previous_offset: usize,
    count: usize,
    total: usize,
) {
    let next_offset = previous_offset.saturating_add(count);
    cursor.offset.set(next_offset);
    cursor.total.set(if count == 0 {
        next_offset
    } else {
        total.max(next_offset)
    });
    cursor.loading.set(false);
}

pub(super) fn connect_paged_grid_loader(scroller: &gtk::ScrolledWindow, load_next: Rc<dyn Fn()>) {
    let load_for_edge = Rc::clone(&load_next);
    scroller.connect_edge_reached(move |_, position| {
        if position == gtk::PositionType::Bottom {
            load_for_edge();
        }
    });

    let scroller_for_idle = scroller.clone();
    glib::idle_add_local_once(move || {
        if scroller_needs_more_items(&scroller_for_idle) {
            load_next();
        }
    });
}

fn scroller_needs_more_items(scroller: &gtk::ScrolledWindow) -> bool {
    let adjustment = scroller.vadjustment();
    adjustment.upper() <= adjustment.page_size() + 1.0
}
