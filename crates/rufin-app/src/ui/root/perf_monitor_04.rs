impl Shell {
    fn drain_cover_decode_queue(self: &Rc<Self>) {
        loop {
            if self.state.cover_decodes.borrow().len() >= COVER_DECODE_MAX_IN_FLIGHT {
                break;
            }
            let Some(job) = self.state.cover_decode_queue.borrow_mut().pop_front() else {
                break;
            };
            if job.priority == CoverDecodePriority::Warm && self.cover_warm_is_paused() {
                self.state.cover_decode_queue.borrow_mut().push_front(job);
                break;
            }
            if self.apply_decoded_cover_if_available(&job.key, job.size) {
                continue;
            }
            if !self
                .state
                .cover_decodes
                .borrow_mut()
                .insert(job.key.clone())
            {
                continue;
            }
            self.spawn_cover_decode_job(job);
        }
    }
    fn spawn_cover_decode_job(self: &Rc<Self>, job: CoverDecodeJob) {
        let shell = Rc::clone(self);
        glib::spawn_future_local(async move {
            let CoverDecodeJob {
                key,
                path,
                size,
                priority,
            } = job;
            shell.record_perf_cover_decode_start(&key);
            match load_cover_pixbuf(path.clone(), size, priority.glib_priority()).await {
                Ok(pixbuf) => {
                    shell.finish_cover_decode(&key);
                    shell.record_perf_cover_decode_ok(&key);
                    let pixbuf = shell.remember_decoded_cover(key.clone(), pixbuf);
                    let bindings = shell.take_live_cover_bindings(&key);
                    apply_pixbuf_to_bindings(bindings, pixbuf);
                }
                Err(error) => {
                    shell.finish_cover_decode(&key);
                    shell.record_perf_cover_decode_error(&key);
                    warn!(%error, path = %path.display(), "failed to load cached cover");
                    for binding in shell.take_live_cover_bindings(&key) {
                        if !binding.tile.clear_image_if_current(binding.generation) {
                            shell.record_perf_cover_stale_ignored();
                        }
                    }
                }
            }
            shell.drain_cover_decode_queue();
        });
    }
    fn finish_cover_decode(&self, key: &str) {
        self.state.cover_decodes.borrow_mut().remove(key);
        self.state
            .startup_cover_prime_pending
            .borrow_mut()
            .remove(key);
        self.state
            .first_run_cover_prime_pending
            .borrow_mut()
            .remove(key);
    }
    fn cancel_queued_warm_cover_decodes(&self) {
        self.state
            .cover_decode_queue
            .borrow_mut()
            .retain(|job| job.priority != CoverDecodePriority::Warm);
    }
    fn pending_cover_size(&self, key: &str) -> Option<i32> {
        self.state
            .cover_bindings
            .borrow()
            .get(key)
            .and_then(|bindings| bindings.first())
            .map(|binding| binding.tile.size())
            .or_else(|| cover_size_from_cache_key(key))
    }
    fn take_live_cover_bindings(&self, key: &str) -> Vec<CoverBinding> {
        let Some(bindings) = self.state.cover_bindings.borrow_mut().remove(key) else {
            return Vec::new();
        };
        self.live_cover_bindings(key, bindings)
    }
    fn live_cover_bindings(&self, key: &str, bindings: Vec<CoverBinding>) -> Vec<CoverBinding> {
        let mut live = Vec::with_capacity(bindings.len());
        let mut stale = 0_usize;
        for binding in bindings {
            if binding.tile.is_live_generation(binding.generation) {
                live.push(binding);
            } else {
                stale = stale.saturating_add(1);
            }
        }
        if stale > 0 {
            self.record_perf_cover_stale_ignored_by(stale);
        }
        if live.is_empty() {
            self.record_perf_cover_stale_key(key);
        }
        live
    }
    fn remember_decoded_cover(&self, key: String, pixbuf: Pixbuf) -> Pixbuf {
        let size = pixbuf.width().min(pixbuf.height()).max(1);
        let mut covers = self.state.decoded_covers.borrow_mut();
        if let Some(existing) = covers.get(&key)
            && existing.size >= size
        {
            return existing.pixbuf.clone();
        }
        if !covers.contains_key(&key) {
            self.state
                .decoded_cover_order
                .borrow_mut()
                .push_back(key.clone());
        }
        covers.insert(
            key,
            DecodedCover {
                pixbuf: pixbuf.clone(),
                size,
            },
        );
        let mut order = self.state.decoded_cover_order.borrow_mut();
        while covers.len() > DECODED_COVER_CACHE_LIMIT {
            let Some(oldest) = order.pop_front() else {
                break;
            };
            covers.remove(&oldest);
        }
        pixbuf
    }
    fn decoded_cover_has_min_size(&self, key: &str, min_size: i32) -> bool {
        self.state
            .decoded_covers
            .borrow()
            .get(key)
            .is_some_and(|cover| cover.size >= min_size)
    }
    fn record_perf_route_render(&self, route: String, elapsed: Duration) {
        if let Some(perf) = &self.state.perf {
            perf.record_route_render(route, elapsed);
        }
    }
    fn record_perf_cover_bind_request(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_bind_request(key);
        }
    }
    fn record_perf_coverless_tile(&self) {
        if let Some(perf) = &self.state.perf {
            perf.record_coverless_tile();
        }
    }
    fn record_perf_cover_cache_hit(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_cache_hit(key);
        }
    }
    fn record_perf_cover_ready(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_ready(key);
        }
    }
    fn record_perf_cover_path_ready(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_path_ready(key);
        }
    }
    fn record_perf_cover_decode_start(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_decode_start(key);
        }
    }
    fn record_perf_cover_decode_ok(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_decode_ok(key);
        }
    }
    fn record_perf_cover_decode_error(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_decode_error(key);
        }
    }
    fn record_perf_cover_stale_ignored(&self) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_stale_ignored();
        }
    }
    fn record_perf_cover_stale_ignored_by(&self, count: usize) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_stale_ignored_by(count);
        }
    }
    fn record_perf_cover_stale_key(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_stale_key(key);
        }
    }
    fn record_perf_track_row_bind(&self, column: &'static str, elapsed: Duration) {
        if let Some(perf) = &self.state.perf {
            perf.record_track_row_bind(column, elapsed);
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn track_table_popover(
        self: &Rc<Self>,
        table: &gtk::ColumnView,
        model: &gio::ListStore,
        tracks: Rc<RefCell<Vec<Track>>>,
        search: &gtk::SearchEntry,
        sort_button: &gtk::Button,
        favorite_first: bool,
        server_search: bool,
    ) -> gtk::Popover {
        let popover = gtk::Popover::new();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
        content.set_margin_top(12);
        content.set_margin_bottom(12);
        content.set_margin_start(12);
        content.set_margin_end(12);

        let sort_label = gtk::Label::new(Some(&tr("Sort by")));
        sort_label.add_css_class("muted");
        sort_label.set_xalign(0.0);
        content.append(&sort_label);

        let sort_titles = TrackSortKey::all()
            .iter()
            .map(|key| tr(key.title()))
            .collect::<Vec<_>>();
        let sort_title_refs = sort_titles.iter().map(String::as_str).collect::<Vec<_>>();
        let sort_options = gtk::StringList::new(&sort_title_refs);
        let sort_dropdown = gtk::DropDown::new(Some(sort_options), None::<gtk::Expression>);
        let current_sort = self.state.settings.borrow().track_table.sort_key;
        sort_dropdown.set_selected(track_sort_index(current_sort));
        let shell = Rc::clone(self);
        let model_for_sort = model.clone();
        let tracks_for_sort = Rc::clone(&tracks);
        let search_for_sort = search.clone();
        let sort_button_for_sort = sort_button.clone();
        let popover_for_sort = popover.clone();
        sort_dropdown.connect_selected_notify(move |dropdown| {
            let sort_key = track_sort_from_index(dropdown.selected());
            let mut settings = shell.state.settings.borrow().track_table.clone();
            if settings.sort_key == sort_key {
                return;
            }
            settings.sort_key = sort_key;
            shell.update_track_table_settings(|stored| *stored = settings.clone());
            let tracks = tracks_for_sort.borrow();
            let search_text = search_for_sort.text();
            let query = if server_search {
                ""
            } else {
                search_text.as_str()
            };
            populate_track_model_with_options(
                &model_for_sort,
                &tracks,
                &settings,
                query,
                favorite_first,
            );
            set_track_sort_button_content(&sort_button_for_sort, &settings);
            let popover = popover_for_sort.clone();
            glib::idle_add_local_once(move || popover.popdown());
        });
        content.append(&sort_dropdown);

        let columns_label = gtk::Label::new(Some(&tr("Columns")));
        columns_label.add_css_class("muted");
        columns_label.set_xalign(0.0);
        content.append(&columns_label);

        let visible = self
            .state
            .settings
            .borrow()
            .track_table
            .visible_columns
            .clone();
        let column_checks = Rc::new(RefCell::new(Vec::new()));
        let syncing_column_checks = Rc::new(Cell::new(false));
        for column in TrackTableColumn::all() {
            let check = gtk::CheckButton::with_label(&tr(track_table_column_config_title(column)));
            check.set_active(visible.contains(&column));
            column_checks.borrow_mut().push((column, check.clone()));
            let shell = Rc::clone(self);
            let table_for_column = table.clone();
            let column_checks_for_column = Rc::clone(&column_checks);
            let syncing_column_checks_for_column = Rc::clone(&syncing_column_checks);
            check.connect_toggled(move |check| {
                if syncing_column_checks_for_column.get() {
                    return;
                }
                shell.update_track_table_settings(|settings| {
                    if check.is_active() {
                        if !settings.visible_columns.contains(&column) {
                            settings.visible_columns.push(column);
                        }
                    } else {
                        settings.visible_columns.retain(|stored| *stored != column);
                        if settings.visible_columns.is_empty() {
                            settings.visible_columns.push(TrackTableColumn::Title);
                        }
                    }
                });
                let settings = shell.state.settings.borrow().track_table.clone();
                sync_track_column_checks(
                    &column_checks_for_column,
                    &settings,
                    &syncing_column_checks_for_column,
                );
                set_track_table_columns(&shell, &table_for_column, &settings);
            });
            content.append(&check);
        }

        popover.set_child(Some(&content));
        popover
    }
}
fn cover_size_from_cache_key(key: &str) -> Option<i32> {
    key.rsplit('/')
        .next()
        .and_then(|value| value.parse::<i32>().ok())
        .filter(|size| *size > 0)
}
