use super::*;

pub(super) type CollectionPageLoader<T> =
    Rc<dyn Fn(&Rc<Shell>, usize, usize) -> Result<source::PagedResponse<T>, String>>;
pub(super) type CollectionSearchPageLoader<T> =
    Rc<dyn Fn(&Rc<Shell>, &str, usize, usize) -> Result<source::PagedResponse<T>, String>>;
pub(super) type CollectionSearchMatcher<T> = Rc<dyn Fn(&T, &str) -> bool>;
pub(super) type CollectionSorter<T> = Rc<dyn Fn(&mut [T], &LibraryListSettings)>;
pub(super) type CollectionModelPopulator<T> =
    Rc<dyn Fn(&gio::ListStore, &[T], &LibraryListSettings)>;
pub(super) type CollectionModelAppender<T> = Rc<dyn Fn(&gio::ListStore, Vec<T>)>;
pub(super) type CollectionWarmer<T> = Rc<dyn Fn(&Rc<Shell>, &[T], &LibraryListSettings)>;
pub(super) type CollectionContentBuilder = Rc<dyn Fn(&Rc<Shell>, gio::ListStore) -> gtk::Widget>;
pub(super) type CollectionScrollerConfigurator =
    Rc<dyn Fn(&Rc<Shell>, &gtk::ScrolledWindow, &gio::ListStore, &LibraryListSettings)>;
pub(super) type CollectionReplaceHook<T> = Rc<dyn Fn(&Rc<Shell>, &[T])>;

pub(super) struct CollectionRouteSpec<T: Clone + 'static> {
    pub(super) key: LibraryListKey,
    pub(super) route: Route,
    pub(super) page_name: &'static str,
    pub(super) empty_body: &'static str,
    pub(super) initial_page: Rc<dyn Fn(&Rc<Shell>) -> source::PagedResponse<T>>,
    pub(super) load_page: Option<CollectionPageLoader<T>>,
    pub(super) load_matching_page: Option<CollectionSearchPageLoader<T>>,
    pub(super) matches_query: CollectionSearchMatcher<T>,
    pub(super) sort_items: CollectionSorter<T>,
    pub(super) populate_model: CollectionModelPopulator<T>,
    pub(super) append_model: CollectionModelAppender<T>,
    pub(super) warm_items: CollectionWarmer<T>,
    pub(super) build_content: CollectionContentBuilder,
    pub(super) configure_scroller: CollectionScrollerConfigurator,
    pub(super) after_replace: Option<CollectionReplaceHook<T>>,
}

impl<T: Clone + 'static> CollectionRouteSpec<T> {
    pub(super) fn view(self, shell: &Rc<Shell>) -> gtk::Widget {
        let settings = shell.library_settings(self.key);
        let mut page = (self.initial_page)(shell);
        if let Some(load_page) = self.load_page.as_ref().cloned() {
            page = complete_cached_page(
                page,
                library_layout_loads_complete_page(self.key, &settings),
                |limit| load_page(shell, 0, limit),
                self.page_name,
            );
        }
        Rc::new(self).view_from_page(shell, page)
    }

    fn view_from_page(
        self: Rc<Self>,
        shell: &Rc<Shell>,
        page: source::PagedResponse<T>,
    ) -> gtk::Widget {
        let settings = shell.library_settings(self.key);
        let page_total = page.total;
        let complete_page = page.items.len() >= page_total || self.load_matching_page.is_none();
        let source_items = Rc::new(page.items.clone());
        let items = Rc::new(RefCell::new(page.items));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        self.publish_replace(shell, &items.borrow());
        (self.warm_items)(shell, &items.borrow(), &settings);
        (self.populate_model)(&model, &items.borrow(), &settings);

        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(&tr("Search")));
        search.set_hexpand(true);
        let cursor = Rc::new(PagedGridCursor {
            offset: Cell::new(items.borrow().len()),
            total: Cell::new(page_total),
            loading: Cell::new(false),
        });
        let query = Rc::new(RefCell::new(String::new()));

        {
            let spec = Rc::clone(&self);
            let shell = Rc::clone(shell);
            let model = model.clone();
            let source_items = Rc::clone(&source_items);
            let items = Rc::clone(&items);
            let cursor = Rc::clone(&cursor);
            let query = Rc::clone(&query);
            search.connect_search_changed(move |entry| {
                let text = entry.text().trim().to_string();
                *query.borrow_mut() = text.clone();
                if complete_page {
                    let query = text.to_lowercase();
                    let values = source_items
                        .iter()
                        .filter(|item| query.is_empty() || (spec.matches_query)(item, &query))
                        .cloned()
                        .collect::<Vec<_>>();
                    let count = values.len();
                    *items.borrow_mut() = values;
                    let settings = shell.library_settings(spec.key);
                    spec.publish_replace(&shell, &items.borrow());
                    (spec.warm_items)(&shell, &items.borrow(), &settings);
                    (spec.populate_model)(&model, &items.borrow(), &settings);
                    cursor.offset.set(count);
                    cursor.total.set(count);
                    cursor.loading.set(false);
                    return;
                }

                let Some(load_matching_page) = spec.load_matching_page.as_ref().cloned() else {
                    cursor.loading.set(false);
                    return;
                };
                cursor.offset.set(0);
                cursor.total.set(usize::MAX);
                cursor.loading.set(true);
                let total_started = Instant::now();
                let load_started = Instant::now();
                match load_matching_page(&shell, &text, 0, GRID_ROUTE_PAGE_SIZE) {
                    Ok(page) => {
                        let load_ms = load_started.elapsed().as_millis() as u64;
                        let apply_started = Instant::now();
                        let settings = shell.library_settings(spec.key);
                        let page = complete_cached_page(
                            page,
                            library_layout_loads_complete_page(spec.key, &settings),
                            |limit| load_matching_page(&shell, &text, 0, limit),
                            &format!("{} search", spec.page_name),
                        );
                        let count = page.items.len();
                        let total = page.total;
                        *items.borrow_mut() = page.items;
                        spec.publish_replace(&shell, &items.borrow());
                        (spec.warm_items)(&shell, &items.borrow(), &settings);
                        (spec.populate_model)(&model, &items.borrow(), &settings);
                        finish_grid_page(&cursor, 0, count, total);
                        log_route_page_timing(RoutePageTiming {
                            route: &spec.route,
                            action: "search",
                            offset: 0,
                            count,
                            total,
                            load_ms,
                            apply_ms: apply_started.elapsed().as_millis() as u64,
                            total_ms: total_started.elapsed().as_millis() as u64,
                        });
                    }
                    Err(error) => {
                        warn!(
                            %error,
                            page = spec.page_name,
                            "failed to search cached collection page"
                        );
                        cursor.loading.set(false);
                    }
                }
            });
        }

        let load_next = if complete_page {
            None
        } else {
            self.load_matching_page
                .as_ref()
                .cloned()
                .map(|load_matching_page| {
                    let spec = Rc::clone(&self);
                    let shell = Rc::clone(shell);
                    let model = model.clone();
                    let items = Rc::clone(&items);
                    let cursor = Rc::clone(&cursor);
                    let query = Rc::clone(&query);
                    Rc::new(move || {
                        if !shell.can_load_grid_page(&cursor, &spec.route) {
                            return;
                        }
                        let total_started = Instant::now();
                        let offset = cursor.offset.get();
                        let text = query.borrow().clone();
                        let load_started = Instant::now();
                        match load_matching_page(&shell, &text, offset, GRID_ROUTE_PAGE_SIZE) {
                            Ok(page) => {
                                let load_ms = load_started.elapsed().as_millis() as u64;
                                let apply_started = Instant::now();
                                let count = page.items.len();
                                let total = page.total;
                                let mut loaded = page.items;
                                let settings = shell.library_settings(spec.key);
                                (spec.sort_items)(&mut loaded, &settings);
                                (spec.warm_items)(&shell, &loaded, &settings);
                                items.borrow_mut().extend(loaded.iter().cloned());
                                (spec.append_model)(&model, loaded);
                                finish_grid_page(&cursor, offset, count, total);
                                log_route_page_timing(RoutePageTiming {
                                    route: &spec.route,
                                    action: "append",
                                    offset,
                                    count,
                                    total,
                                    load_ms,
                                    apply_ms: apply_started.elapsed().as_millis() as u64,
                                    total_ms: total_started.elapsed().as_millis() as u64,
                                });
                            }
                            Err(error) => {
                                warn!(
                                    %error,
                                    page = spec.page_name,
                                    "failed to append cached collection page"
                                );
                                cursor.loading.set(false);
                            }
                        }
                    }) as Rc<dyn Fn()>
                })
        };
        let configure_scroller = {
            let spec = Rc::clone(&self);
            let shell = Rc::clone(shell);
            let model = model.clone();
            let settings = settings.clone();
            Rc::new(move |scroller: &gtk::ScrolledWindow| {
                (spec.configure_scroller)(&shell, scroller, &model, &settings);
            }) as Rc<dyn Fn(&gtk::ScrolledWindow)>
        };

        shell.library_page_shell(LibraryPageShellOptions {
            key: self.key,
            empty: items.borrow().is_empty(),
            empty_body: self.empty_body,
            search,
            content: (self.build_content)(shell, model),
            load_next,
            configure_scroller: Some(configure_scroller),
        })
    }

    fn publish_replace(&self, shell: &Rc<Shell>, items: &[T]) {
        if let Some(after_replace) = self.after_replace.as_ref() {
            after_replace(shell, items);
        }
    }
}
