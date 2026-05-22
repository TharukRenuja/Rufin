impl Shell {
    fn rename_playlist_dialog(self: &Rc<Self>, playlist_id: PlaylistId, current_name: String) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("Rename Playlist"))
            .body(tr("Enter a new playlist name."))
            .build();
        dialog.add_response("cancel", &tr("Cancel"));
        dialog.add_response("rename", &tr("Rename"));
        dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
        let entry = gtk::Entry::new();
        entry.set_text(&current_name);
        dialog.set_extra_child(Some(&entry));
        let controller = self.controller.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "rename" {
                let name = entry.text().trim().to_string();
                if !name.is_empty() {
                    controller.rename_playlist(playlist_id.clone(), name);
                }
            }
        });
        dialog.present(Some(&self.window));
    }
    fn grouped_detail_view(self: &Rc<Self>, data: GroupedDetailData) -> gtk::Widget {
        let GroupedDetailData {
            title,
            image_ref,
            cover_refs,
            seed,
            summary,
            tracks,
            table_context,
        } = data;
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(true);

        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 20);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(28);
        wrapper.set_margin_bottom(36);
        wrapper.set_margin_start(32);
        wrapper.set_margin_end(32);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 22);
        header.append(&self.cover_group_tile_for(
            cover_refs,
            image_ref.as_ref(),
            seed,
            160,
            DETAIL_COVER_SIZE,
        ));
        let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
        metadata.set_valign(gtk::Align::Center);
        let title_label = gtk::Label::new(Some(&title));
        title_label.add_css_class("detail-title");
        title_label.set_xalign(0.0);
        title_label.set_wrap(true);
        let summary_label = gtk::Label::new(Some(&summary));
        summary_label.add_css_class("muted");
        summary_label.set_xalign(0.0);
        metadata.append(&title_label);
        metadata.append(&summary_label);
        header.append(&metadata);
        wrapper.append(&header);

        if tracks.is_empty() {
            wrapper
                .append(&self.placeholder_view("Tracks", "No cached tracks are linked here yet."));
        } else {
            let key = if table_context == "genre-detail" {
                LibraryListKey::GenreTracks
            } else {
                LibraryListKey::Tracks
            };
            wrapper.append(&self.library_tracks_panel(tracks, key, table_context));
        }
        scroller.set_child(Some(&wrapper));
        scroller.upcast()
    }
    fn search_view(self: &Rc<Self>, _query: &str, library: LibrarySnapshot) -> gtk::Widget {
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(true);

        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(24);
        wrapper.set_margin_bottom(28);
        wrapper.set_margin_start(PRIMARY_ROUTE_MARGIN_START);
        wrapper.set_margin_end(PRIMARY_ROUTE_MARGIN_END);
        wrapper.set_vexpand(true);

        let has_albums = !library.search.albums.is_empty();
        let has_tracks = !library.search.tracks.is_empty();
        let has_artists = !library.search.artists.is_empty();
        let has_playlists = !library.search.playlists.is_empty();
        let albums = library.search.albums;
        if !albums.is_empty() {
            let section = HomeSection {
                kind: rufin_core::HomeSectionKind::Explore,
                albums,
                tracks: Vec::new(),
            };
            wrapper.append(&self.home_album_section(&section));
        }

        if has_tracks {
            wrapper.append(&self.library_tracks_panel(
                library.search.tracks,
                LibraryListKey::Tracks,
                "search",
            ));
        } else if !has_albums && !has_artists && !has_playlists {
            wrapper.append(&self.route_empty_view("No cached results found."));
        }

        scroller.set_child(Some(&wrapper));
        scroller.upcast()
    }
    fn render_lyrics_panel(self: &Rc<Self>) {
        let settings = self.state.settings.borrow();
        let current_track_id = current_playback_track_id(&self.state.player.borrow());
        let has_current_track = current_track_id.is_some();
        let (search_label, search_enabled) = if settings.private_mode {
            (tr("Private mode is on"), false)
        } else if has_current_track {
            (tr("Search lyrics"), true)
        } else {
            (tr("No track playing"), false)
        };
        let lyrics = self.state.lyrics.borrow();
        let clear_auto_search_enabled =
            auto_lyrics_skip_action_enabled(&settings, current_track_id.as_ref(), lyrics.as_ref());
        drop(settings);
        self.lyrics_pane
            .set_search_action(&search_label, search_enabled);
        self.lyrics_pane.set_clear_auto_search_action(
            &tr("Disable automatic lyric search for this track"),
            clear_auto_search_enabled,
        );
        let empty_status = self.lyrics_empty_status();
        let seek_shell = Rc::clone(self);
        let seek: Rc<dyn Fn(u64)> = Rc::new(move |position_millis| {
            seek_shell.seek_to_lyrics_position(position_millis);
        });
        self.lyrics_pane
            .set_content(lyrics.as_ref(), empty_status, seek);
        drop(lyrics);
        self.update_lyrics_highlight();
        self.request_auto_lyrics_if_needed();
    }
    fn present_lyrics_search_dialog(self: &Rc<Self>) {
        if let Some(dialog) = self.state.lyrics_search_dialog.borrow().as_ref() {
            dialog.dialog.present(Some(&self.window));
            dialog.title_entry.grab_focus();
            return;
        }

        let Some(current) = self.state.player.borrow().current.clone() else {
            return;
        };
        if self.state.settings.borrow().private_mode {
            return;
        }

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(16);
        content.set_margin_bottom(16);
        content.set_margin_start(16);
        content.set_margin_end(16);
        content.set_width_request(420);
        content.set_height_request(500);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.set_valign(gtk::Align::Center);
        let title = gtk::Label::new(Some(&tr("Search Lyrics")));
        title.add_css_class("title");
        title.set_xalign(0.0);
        title.set_hexpand(true);
        header.append(&title);
        let close_button = icon_button("window-close-symbolic", "Close");
        header.append(&close_button);
        content.append(&header);

        let artist_entry = gtk::Entry::new();
        artist_entry.set_placeholder_text(Some(&tr("Artist")));
        artist_entry.set_text(&current.artist);
        artist_entry.set_hexpand(true);
        content.append(&artist_entry);

        let title_entry = gtk::Entry::new();
        title_entry.set_placeholder_text(Some(&tr("Song")));
        title_entry.set_text(&current.title);
        title_entry.set_hexpand(true);
        content.append(&title_entry);

        let search_button = text_button("system-search-symbolic", "Search");
        search_button.set_halign(gtk::Align::End);
        content.append(&search_button);

        let status = gtk::Label::new(Some(&tr("Ready")));
        status.add_css_class("muted");
        status.set_xalign(0.0);
        status.set_wrap(true);
        content.append(&status);

        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&list));
        content.append(&scroller);

        let dialog = adw::Dialog::builder()
            .content_width(520)
            .content_height(560)
            .child(&content)
            .build();
        let search_dialog = LyricsSearchDialog {
            dialog: dialog.clone(),
            track_id: current.track_id,
            artist_entry: artist_entry.clone(),
            title_entry: title_entry.clone(),
            search_button: search_button.clone(),
            list,
            status,
        };
        *self.state.lyrics_search_dialog.borrow_mut() = Some(search_dialog.clone());

        let close_shell = Rc::clone(self);
        dialog.connect_closed(move |_| {
            close_shell.state.lyrics_search_dialog.borrow_mut().take();
        });

        let close_dialog = dialog.clone();
        close_button.connect_clicked(move |_| {
            close_dialog.close();
        });

        let search_shell = Rc::clone(self);
        search_button.connect_clicked(move |_| submit_lyrics_search(&search_shell));

        let search_shell = Rc::clone(self);
        artist_entry.connect_activate(move |_| submit_lyrics_search(&search_shell));

        let search_shell = Rc::clone(self);
        title_entry.connect_activate(move |_| submit_lyrics_search(&search_shell));

        dialog.present(Some(&self.window));
        search_dialog.title_entry.grab_focus();
        submit_lyrics_search(self);
    }
    fn apply_lyrics_search_results(
        self: &Rc<Self>,
        track_id: rufin_core::TrackId,
        _artist_name: String,
        _track_name: String,
        results: Vec<LyricsSearchResult>,
    ) {
        let Some(dialog) = self.state.lyrics_search_dialog.borrow().clone() else {
            return;
        };
        if dialog.track_id != track_id {
            return;
        }
        dialog.search_button.set_sensitive(true);
        clear_list_box(&dialog.list);
        if results.is_empty() {
            dialog.status.set_text(&tr("No lyrics found."));
            return;
        }

        dialog
            .status
            .set_text(&format!("{} {}", results.len(), tr("results")));
        for result in results {
            let title = format!("{} - {}", result.artist_name, result.track_name);
            let subtitle = lyrics_result_subtitle(&result);
            let row = adw::ActionRow::builder()
                .title(title)
                .subtitle(subtitle)
                .build();
            let button = gtk::Button::with_label(&tr("Save"));
            button.set_valign(gtk::Align::Center);
            button.add_css_class("suggested-action");
            button.set_sensitive(lyrics_search_result_has_content(&result));
            row.add_suffix(&button);
            row.set_activatable_widget(Some(&button));

            let save_shell = Rc::clone(self);
            let save_track_id = track_id.clone();
            button.connect_clicked(move |_| {
                if save_shell.state.settings.borrow().ask_lyrics_save_path {
                    let shell = Rc::clone(&save_shell);
                    let track_id = save_track_id.clone();
                    let result = result.clone();
                    gtk::glib::spawn_future_local(async move {
                        let dialog = gtk::FileDialog::builder().title(tr("Save Lyrics")).build();
                        let Ok(file) = dialog.save_future(Some(&shell.window)).await else {
                            return;
                        };
                        let Some(path) = file.path() else {
                            return;
                        };
                        shell
                            .controller
                            .save_lyrics_search_result(track_id, result, Some(path));
                    });
                } else {
                    save_shell.controller.save_lyrics_search_result(
                        save_track_id.clone(),
                        result.clone(),
                        None,
                    );
                }
            });
            dialog.list.append(&row);
        }
    }
    fn apply_lyrics_saved(self: &Rc<Self>, path: PathBuf, lyrics: Lyrics) {
        let track_id = lyrics.track_id.clone();
        *self.state.lyrics.borrow_mut() = Some(lyrics);
        self.render_lyrics_panel();
        if let Some(dialog) = self.state.lyrics_search_dialog.borrow().as_ref()
            && dialog.track_id == track_id
        {
            dialog
                .status
                .set_text(&format!("{} {}", tr("Saved to"), path.display()));
        }
    }
    fn placeholder_view(&self, title: &str, body: &str) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 12);
        wrapper.add_css_class("empty-state");
        wrapper.set_vexpand(true);
        wrapper.set_hexpand(true);
        wrapper.set_valign(gtk::Align::Center);
        wrapper.set_halign(gtk::Align::Center);

        let heading = gtk::Label::new(Some(&tr(title)));
        heading.add_css_class("section-heading");
        let label = gtk::Label::new(Some(&tr(body)));
        label.add_css_class("muted");
        label.set_wrap(true);
        label.set_justify(gtk::Justification::Center);
        wrapper.append(&heading);
        wrapper.append(&label);
        wrapper.upcast()
    }
    fn route_empty_view(&self, body: &str) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 12);
        wrapper.add_css_class("empty-state");
        wrapper.set_vexpand(true);
        wrapper.set_hexpand(true);
        wrapper.set_valign(gtk::Align::Center);
        wrapper.set_halign(gtk::Align::Center);

        let label = gtk::Label::new(Some(&tr(body)));
        label.add_css_class("muted");
        label.set_wrap(true);
        label.set_justify(gtk::Justification::Center);
        wrapper.append(&label);
        wrapper.upcast()
    }
    fn cover_tile_for(
        self: &Rc<Self>,
        image_ref: Option<&ImageRef>,
        seed: u32,
        size: i32,
        fetch_size: u32,
    ) -> gtk::Widget {
        self.cover_tile_for_dimensions(image_ref, seed, size, size, fetch_size)
    }
    fn cover_tile_for_dimensions(
        self: &Rc<Self>,
        image_ref: Option<&ImageRef>,
        seed: u32,
        width: i32,
        height: i32,
        fetch_size: u32,
    ) -> gtk::Widget {
        let tile = ArtworkTile::new_sized(width, height, seed);
        let widget = tile.widget();
        let decode_size = width.max(height);

        if let Some(image_ref) = image_ref
            && let Some(key) = self.cover_cache_key(image_ref, fetch_size)
        {
            if let Some((cache_key, pixbuf)) = self.decoded_cover_for_ref(image_ref, fetch_size) {
                self.record_perf_cover_cache_hit(&cache_key);
                tile.set_pixbuf_if_current(tile.generation(), pixbuf);
            } else {
                let shell = Rc::clone(self);
                let tile_for_map = tile.clone();
                let image_ref = image_ref.clone();
                let started = Rc::new(Cell::new(false));
                widget.connect_map(move |_| {
                    if started.replace(true) {
                        return;
                    }
                    shell.request_cover_for_tile(
                        &tile_for_map,
                        key.clone(),
                        image_ref.clone(),
                        decode_size,
                        fetch_size,
                    );
                });
            }
        } else if image_ref.is_none() {
            self.record_perf_coverless_tile();
        }
        widget
    }
    fn cover_group_tile_for(
        self: &Rc<Self>,
        image_refs: Vec<ImageRef>,
        fallback_image_ref: Option<&ImageRef>,
        seed: u32,
        size: i32,
        fetch_size: u32,
    ) -> gtk::Widget {
        let image_refs = unique_cover_refs(image_refs);
        match image_refs.len() {
            0 => self.cover_tile_for(fallback_image_ref, seed, size, fetch_size),
            1 => self.cover_tile_for(image_refs.first(), seed, size, fetch_size),
            _ => {
                let grid = gtk::Grid::new();
                grid.add_css_class("cover-tile");
                grid.add_css_class("card");
                grid.set_size_request(size, size);
                grid.set_width_request(size);
                grid.set_height_request(size);
                grid.set_row_homogeneous(true);
                grid.set_column_homogeneous(true);
                grid.set_hexpand(false);
                grid.set_vexpand(false);
                grid.set_halign(gtk::Align::Start);
                grid.set_valign(gtk::Align::Start);

                let cell_size = (size / 2).max(1);
                if image_refs.len() == 3 {
                    let tall = self.cover_tile_for_dimensions(
                        image_refs.first(),
                        seed,
                        cell_size,
                        size,
                        fetch_size,
                    );
                    let top = self.cover_tile_for(
                        image_refs.get(1),
                        seed.wrapping_add(0x9e37_79b9),
                        cell_size,
                        fetch_size,
                    );
                    let bottom = self.cover_tile_for(
                        image_refs.get(2),
                        seed.wrapping_add(0x3c6e_f372),
                        cell_size,
                        fetch_size,
                    );
                    grid.attach(&tall, 0, 0, 1, 2);
                    grid.attach(&top, 1, 0, 1, 1);
                    grid.attach(&bottom, 1, 1, 1, 1);
                } else {
                    for index in 0..4 {
                        let image_ref = image_refs.get(index % image_refs.len());
                        let child = self.cover_tile_for(
                            image_ref,
                            seed.wrapping_add((index as u32).wrapping_mul(0x9e37_79b9)),
                            cell_size,
                            fetch_size,
                        );
                        grid.attach(&child, (index % 2) as i32, (index / 2) as i32, 1, 1);
                    }
                }
                grid.upcast()
            }
        }
    }
    fn request_cover_for_tile(
        self: &Rc<Self>,
        tile: &ArtworkTile,
        key: String,
        image_ref: ImageRef,
        size: i32,
        fetch_size: u32,
    ) {
        if let Some((cache_key, pixbuf)) = self.decoded_cover_for_ref(&image_ref, fetch_size) {
            self.record_perf_cover_cache_hit(&cache_key);
            tile.set_pixbuf_if_current(tile.generation(), pixbuf);
            return;
        }

        self.record_perf_cover_bind_request(&key);
        let generation = tile.generation();
        {
            self.state
                .cover_bindings
                .borrow_mut()
                .entry(key.clone())
                .or_default()
                .push(CoverBinding {
                    tile: tile.clone(),
                    generation,
                });
        }
        if let Some(path) = self.controller.cached_cover_path(&image_ref, fetch_size) {
            let shell = Rc::clone(self);
            glib::idle_add_local_once(move || {
                shell.record_perf_cover_ready(&key);
                shell.start_cover_decode_from_path(key, path, size, CoverDecodePriority::Visible);
            });
        } else {
            self.controller
                .request_cover_for_key(key, image_ref, fetch_size);
        }
    }
    fn warm_cover_refs(self: &Rc<Self>, image_refs: Vec<ImageRef>, fetch_size: u32, size: i32) {
        let generation = self.next_cover_warm_generation();
        let mut seen = HashSet::new();
        let mut jobs = VecDeque::new();

        for image_ref in image_refs {
            let Some(key) = self.cover_cache_key(&image_ref, fetch_size) else {
                continue;
            };
            if !seen.insert(key.clone())
                || self.decoded_cover_for_ref(&image_ref, fetch_size).is_some()
            {
                continue;
            }
            jobs.push_back((key, image_ref));
        }

        if jobs.is_empty() {
            return;
        }

        self.schedule_cover_warm_jobs(Rc::new(RefCell::new(jobs)), fetch_size, size, generation);
    }
    fn schedule_startup_cover_warm(self: &Rc<Self>) {
        let generation = self
            .state
            .startup_cover_warm_generation
            .get()
            .saturating_add(1);
        self.state.startup_cover_warm_generation.set(generation);

        let jobs = self.startup_cover_warm_jobs();
        if jobs.is_empty() {
            return;
        }

        info!(covers = jobs.len(), "scheduled startup cover warm");
        let jobs = Rc::new(RefCell::new(jobs));
        let shell = Rc::clone(self);
        glib::timeout_add_local_once(
            Duration::from_millis(STARTUP_COVER_WARM_DELAY_MS),
            move || {
                if shell.state.startup_cover_warm_generation.get() == generation {
                    shell.start_startup_cover_warm_jobs(jobs, generation);
                }
            },
        );
    }
    fn startup_cover_warm_jobs(&self) -> VecDeque<StartupCoverWarmJob> {
        let image_refs = startup_library_cover_refs(&self.state.library.borrow());
        let mut seen = HashSet::new();
        let mut jobs = VecDeque::new();

        for image_ref in image_refs {
            let fetch_size = GRID_COVER_SIZE;
            let Some(key) = self.cover_cache_key(&image_ref, fetch_size) else {
                continue;
            };
            if !seen.insert(key.clone())
                || self.decoded_cover_for_ref(&image_ref, fetch_size).is_some()
            {
                continue;
            }
            jobs.push_back(StartupCoverWarmJob {
                key,
                image_ref,
                fetch_size,
                size: GRID_COVER_SIZE as i32,
            });
        }

        jobs
    }
    fn start_startup_cover_warm_jobs(
        self: &Rc<Self>,
        jobs: Rc<RefCell<VecDeque<StartupCoverWarmJob>>>,
        generation: u64,
    ) {
        let shell = Rc::clone(self);
        glib::timeout_add_local(
            Duration::from_millis(STARTUP_COVER_WARM_INTERVAL_MS),
            move || {
                if shell.state.startup_cover_warm_generation.get() != generation {
                    return glib::ControlFlow::Break;
                }
                if jobs.borrow().is_empty() {
                    return glib::ControlFlow::Break;
                }

                let in_flight = shell.state.cover_decodes.borrow().len();
                if in_flight >= COVER_WARM_MAX_IN_FLIGHT {
                    return glib::ControlFlow::Continue;
                }

                let capacity = COVER_WARM_MAX_IN_FLIGHT.saturating_sub(in_flight);
                let mut processed = 0;
                while processed < STARTUP_COVER_WARM_BATCH_SIZE.min(capacity) {
                    let Some(job) = jobs.borrow_mut().pop_front() else {
                        break;
                    };
                    processed += 1;
                    if shell
                        .decoded_cover_for_ref(&job.image_ref, job.fetch_size)
                        .is_some()
                        || shell.state.cover_decodes.borrow().contains(&job.key)
                    {
                        continue;
                    }
                    if let Some(path) = shell
                        .controller
                        .cached_cover_path(&job.image_ref, job.fetch_size)
                    {
                        shell.start_cover_decode_from_path(
                            job.key,
                            path,
                            job.size,
                            CoverDecodePriority::Warm,
                        );
                    }
                }

                if jobs.borrow().is_empty() {
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            },
        );
    }
    fn next_cover_warm_generation(&self) -> u64 {
        let generation = self.state.cover_warm_generation.get().saturating_add(1);
        self.state.cover_warm_generation.set(generation);
        generation
    }
    fn cancel_cover_warm(&self) {
        self.state
            .cover_warm_generation
            .set(self.state.cover_warm_generation.get().saturating_add(1));
    }
    fn schedule_cover_warm_jobs(
        self: &Rc<Self>,
        jobs: Rc<RefCell<VecDeque<(String, ImageRef)>>>,
        fetch_size: u32,
        size: i32,
        generation: u64,
    ) {
        let shell = Rc::clone(self);
        glib::timeout_add_local_once(
            Duration::from_millis(COVER_WARM_INITIAL_DELAY_MS),
            move || {
                if shell.state.cover_warm_generation.get() == generation {
                    shell.start_cover_warm_jobs(jobs, fetch_size, size, generation);
                }
            },
        );
    }
    fn start_cover_warm_jobs(
        self: &Rc<Self>,
        jobs: Rc<RefCell<VecDeque<(String, ImageRef)>>>,
        fetch_size: u32,
        size: i32,
        generation: u64,
    ) {
        let shell = Rc::clone(self);
        glib::timeout_add_local(Duration::from_millis(COVER_WARM_INTERVAL_MS), move || {
            if shell.state.cover_warm_generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            if jobs.borrow().is_empty() {
                return glib::ControlFlow::Break;
            }

            let in_flight = shell.state.cover_decodes.borrow().len();
            if in_flight >= COVER_WARM_MAX_IN_FLIGHT {
                return glib::ControlFlow::Continue;
            }

            let capacity = COVER_WARM_MAX_IN_FLIGHT.saturating_sub(in_flight);
            let mut processed = 0;
            while processed < COVER_WARM_BATCH_SIZE.min(capacity) {
                let Some((key, image_ref)) = jobs.borrow_mut().pop_front() else {
                    break;
                };
                processed += 1;
                if shell.state.decoded_covers.borrow().contains_key(&key)
                    || shell.state.cover_decodes.borrow().contains(&key)
                {
                    continue;
                }
                if let Some(path) = shell.controller.cached_cover_path(&image_ref, fetch_size) {
                    shell.start_cover_decode_from_path(key, path, size, CoverDecodePriority::Warm);
                }
            }

            if jobs.borrow().is_empty() {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }
    fn cover_cache_key(&self, image_ref: &ImageRef, size: u32) -> Option<String> {
        let server = self.state.library.borrow().server.clone()?;
        if server.provider == "fake" {
            return None;
        }
        if external_metadata::is_external_image_ref(image_ref)
            && !external_metadata::enabled(&self.state.settings.borrow())
        {
            return None;
        }
        Some(image_cache_key(
            &server.id,
            &image_ref.item_id,
            image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED),
            size,
        ))
    }
    fn decoded_cover_for_ref(
        &self,
        image_ref: &ImageRef,
        preferred_size: u32,
    ) -> Option<(String, Pixbuf)> {
        for size in decoded_cover_candidate_sizes(preferred_size) {
            let Some(key) = self.cover_cache_key(image_ref, size) else {
                continue;
            };
            if let Some(pixbuf) = self.state.decoded_covers.borrow().get(&key).cloned() {
                return Some((key, pixbuf));
            }
        }
        None
    }
    fn apply_cover_ready(self: &Rc<Self>, key: &str, path: &Path) {
        self.record_perf_cover_ready(key);
        let size = self
            .pending_cover_size(key)
            .unwrap_or(GRID_COVER_SIZE as i32);
        if let Some(pixbuf) = self.state.decoded_covers.borrow().get(key).cloned() {
            let bindings = self.take_live_cover_bindings(key);
            apply_pixbuf_to_bindings(bindings, pixbuf);
            return;
        }
        self.start_cover_decode_from_path(
            key.to_string(),
            path.to_path_buf(),
            size,
            CoverDecodePriority::Visible,
        );
    }
    fn start_cover_decode_from_path(
        self: &Rc<Self>,
        key: String,
        path: PathBuf,
        size: i32,
        priority: CoverDecodePriority,
    ) {
        if self.apply_decoded_cover_if_available(&key) {
            return;
        }

        if self.state.cover_decodes.borrow().contains(&key) {
            return;
        }

        {
            let mut queue = self.state.cover_decode_queue.borrow_mut();
            if let Some(position) = queue.iter().position(|job| job.key == key) {
                let Some(mut job) = queue.remove(position) else {
                    return;
                };
                job.size = job.size.max(size);
                job.priority = if job.priority == CoverDecodePriority::Visible
                    || priority == CoverDecodePriority::Visible
                {
                    CoverDecodePriority::Visible
                } else {
                    CoverDecodePriority::Warm
                };
                if job.priority == CoverDecodePriority::Visible {
                    queue.push_front(job);
                } else {
                    queue.push_back(job);
                }
                drop(queue);
                self.drain_cover_decode_queue();
                return;
            }

            let job = CoverDecodeJob {
                key,
                path,
                size,
                priority,
            };
            if priority == CoverDecodePriority::Visible {
                queue.push_front(job);
            } else {
                queue.push_back(job);
            }
        }

        self.drain_cover_decode_queue();
    }
    fn apply_decoded_cover_if_available(&self, key: &str) -> bool {
        let Some(pixbuf) = self.state.decoded_covers.borrow().get(key).cloned() else {
            return false;
        };
        self.state
            .first_run_cover_prime_pending
            .borrow_mut()
            .remove(key);
        let bindings = self.take_live_cover_bindings(key);
        apply_pixbuf_to_bindings(bindings, pixbuf);
        true
    }
}
