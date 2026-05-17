use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use rufin_core::Route;
use tracing::warn;

use super::{GRID_ROUTE_PAGE_SIZE, Shell};
use crate::controller::AppController;
use crate::i18n::tr;

pub(super) struct PagedGridCursor {
    pub(super) offset: Cell<usize>,
    pub(super) total: Cell<usize>,
    pub(super) loading: Cell<bool>,
}

pub(super) struct PagedGridConfig {
    pub(super) route: Route,
    pub(super) offset: usize,
    pub(super) total: usize,
    pub(super) page_name: &'static str,
}

impl Shell {
    pub(super) fn searchable_grid_controls<T>(
        self: &Rc<Self>,
        model: gio::ListStore,
        items: Rc<RefCell<Vec<T>>>,
        config: PagedGridConfig,
        load_page: impl Fn(
            &AppController,
            &str,
            usize,
            usize,
        ) -> Result<rufin_provider::PagedResponse<T>, String>
        + 'static,
        replace_model: impl Fn(&gio::ListStore, Vec<T>) + 'static,
        append_model: impl Fn(&gio::ListStore, Vec<T>) + 'static,
    ) -> (gtk::SearchEntry, Rc<dyn Fn()>)
    where
        T: Clone + 'static,
    {
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(&tr("Search")));
        search.set_hexpand(true);

        let cursor = Rc::new(PagedGridCursor {
            offset: Cell::new(config.offset),
            total: Cell::new(config.total),
            loading: Cell::new(false),
        });
        let query = Rc::new(RefCell::new(String::new()));
        let load_page: Rc<
            dyn Fn(
                &AppController,
                &str,
                usize,
                usize,
            ) -> Result<rufin_provider::PagedResponse<T>, String>,
        > = Rc::new(load_page);
        let replace_model: Rc<dyn Fn(&gio::ListStore, Vec<T>)> = Rc::new(replace_model);
        let append_model: Rc<dyn Fn(&gio::ListStore, Vec<T>)> = Rc::new(append_model);
        let route = config.route;
        let page_name = config.page_name;

        let shell = Rc::clone(self);
        let model_for_page = model.clone();
        let items_for_page = Rc::clone(&items);
        let cursor_for_page = Rc::clone(&cursor);
        let query_for_page = Rc::clone(&query);
        let load_page_for_page = Rc::clone(&load_page);
        let append_model_for_page = Rc::clone(&append_model);
        let load_next = Rc::new(move || {
            if !shell.can_load_grid_page(&cursor_for_page, &route) {
                return;
            }
            let offset = cursor_for_page.offset.get();
            let query = query_for_page.borrow().clone();
            match load_page_for_page(&shell.controller, &query, offset, GRID_ROUTE_PAGE_SIZE) {
                Ok(page) => {
                    let count = page.items.len();
                    items_for_page
                        .borrow_mut()
                        .extend(page.items.iter().cloned());
                    append_model_for_page(&model_for_page, page.items);
                    finish_grid_page(&cursor_for_page, offset, count, page.total);
                }
                Err(error) => {
                    warn!(%error, page = page_name, "failed to append cached grid page");
                    cursor_for_page.loading.set(false);
                }
            }
        });

        let shell = Rc::clone(self);
        let model_for_search = model;
        let items_for_search = items;
        let cursor_for_search = cursor;
        let query_for_search = query;
        search.connect_search_changed(move |entry| {
            let query = entry.text().trim().to_string();
            *query_for_search.borrow_mut() = query.clone();
            cursor_for_search.offset.set(0);
            cursor_for_search.total.set(usize::MAX);
            cursor_for_search.loading.set(true);
            match load_page(&shell.controller, &query, 0, GRID_ROUTE_PAGE_SIZE) {
                Ok(page) => {
                    let count = page.items.len();
                    *items_for_search.borrow_mut() = page.items.clone();
                    replace_model(&model_for_search, page.items);
                    finish_grid_page(&cursor_for_search, 0, count, page.total);
                }
                Err(error) => {
                    warn!(%error, page = page_name, "failed to search cached grid page");
                    cursor_for_search.loading.set(false);
                }
            }
        });

        (search, load_next)
    }

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
