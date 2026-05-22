impl Shell {
    fn cancel_scheduled_lyrics_highlight(&self) {
        self.state
            .lyrics_timing_generation
            .set(self.state.lyrics_timing_generation.get().saturating_add(1));
        if let Some(source) = self.state.lyrics_timing_source.borrow_mut().take() {
            source.remove();
        }
    }
    fn schedule_next_lyrics_highlight(self: &Rc<Self>, position_millis: u64) {
        let playing = matches!(self.state.player.borrow().state, PlaybackState::Playing);
        if !playing {
            return;
        }

        let Some(next_position_millis) = self
            .state
            .lyrics
            .borrow()
            .as_ref()
            .and_then(|lyrics| next_lyrics_line_start_after(&lyrics.lines, position_millis))
        else {
            return;
        };
        let delay_millis = next_position_millis.saturating_sub(position_millis);
        let generation = self.state.lyrics_timing_generation.get().saturating_add(1);
        self.state.lyrics_timing_generation.set(generation);

        let shell = Rc::clone(self);
        let source = glib::timeout_add_local_once(Duration::from_millis(delay_millis), move || {
            if shell.state.lyrics_timing_generation.get() != generation {
                return;
            }
            let _source = shell.state.lyrics_timing_source.borrow_mut().take();
            shell.update_lyrics_highlight_at(next_position_millis);
        });
        if let Some(previous_source) = self.state.lyrics_timing_source.borrow_mut().replace(source)
        {
            previous_source.remove();
        }
    }
    fn render_current_route(self: &Rc<Self>) {
        let render_started = Instant::now();
        self.cancel_cover_warm();
        self.update_layout();
        self.state.home_section_views.borrow_mut().clear();
        if !self.state.startup_route_revealed.get() && !self.login_screen_active() {
            self.render_startup_loading_view();
            return;
        }
        if self.login_screen_active() {
            clear_favorite_controls(&self.state.favorite_controls);
            while let Some(child) = self.login_host.first_child() {
                self.login_host.remove(&child);
            }
            let route_name = "FirstRun".to_string();
            self.route_title.set_title(&tr("Connect to Music Server"));
            self.set_history_buttons_sensitive(false, false);
            let view = self.add_server_view();
            self.login_host.append(&view);
            self.observe_route_scroll(&route_name);
            self.record_perf_route_render(route_name, render_started.elapsed());
            return;
        }

        clear_favorite_controls(&self.state.favorite_controls);
        while let Some(child) = self.route_host.first_child() {
            self.route_host.remove(&child);
        }

        let route = self.state.routes.borrow().current().clone();
        let route_name = format!("{route:?}");
        self.route_title.set_title(&tr(route.title()));
        self.set_history_buttons_sensitive(
            self.state.routes.borrow().can_back(),
            self.state.routes.borrow().can_forward(),
        );
        update_navigation_selection(self.as_ref());

        let view = match route {
            Route::Home => self.home_view(),
            Route::Albums => self.library_albums_view(),
            Route::AlbumDetail(album_id) => self.album_detail_view(album_id),
            Route::Tracks => self.library_tracks_route_view(),
            Route::Favorites => self.favorites_view(),
            Route::Artists => self.library_artist_list_view(false),
            Route::ArtistDetail(artist_id) => self.artist_detail_view(artist_id),
            Route::ArtistDiscography(artist_id) => self.artist_discography_view(artist_id),
            Route::ArtistTracks(artist_id) => self.artist_tracks_view(artist_id),
            Route::AlbumArtists => self.library_artist_list_view(true),
            Route::Genres => self.library_genre_list_view(),
            Route::GenreDetail(genre_id) => self.genre_detail_view(genre_id),
            Route::Folders { path } => self.folders_view(path),
            Route::Playlists => self.library_playlists_view(),
            Route::PlaylistDetail(playlist_id) => self.playlist_detail_view(playlist_id),
            Route::Search { query, .. } => {
                let library = self.state.library.borrow().clone();
                self.search_view(&query, library)
            }
        };

        self.route_host.append(&route_boundary(view));
        self.observe_route_scroll(&route_name);
        self.record_perf_route_render(route_name, render_started.elapsed());
    }
    fn render_current_route_preserving_scroll(self: &Rc<Self>) {
        let scroll_value = self.current_route_scroll_value();
        self.render_current_route();
        if let Some(value) = scroll_value {
            self.restore_current_route_scroll(value);
        }
    }
    fn current_route_scroll_value(&self) -> Option<f64> {
        find_largest_scrolled_window(&self.route_host.clone().upcast())
            .map(|scroller| scroller.vadjustment().value())
    }
    fn restore_current_route_scroll(&self, value: f64) {
        let route_host = self.route_host.clone();
        glib::idle_add_local_once(move || {
            restore_scrolled_window_value(&route_host.clone().upcast(), value);
            glib::timeout_add_local_once(Duration::from_millis(16), move || {
                restore_scrolled_window_value(&route_host.clone().upcast(), value);
            });
        });
    }
    fn observe_route_scroll(&self, route: &str) {
        let Some(perf) = self
            .state
            .perf
            .as_ref()
            .filter(|perf| perf.options.observe_scroll)
            .cloned()
        else {
            return;
        };
        let host = if self.login_screen_active() {
            self.login_host.clone().upcast::<gtk::Widget>()
        } else {
            self.route_host.clone().upcast::<gtk::Widget>()
        };
        let route = route.to_string();
        glib::idle_add_local_once(move || {
            let Some(scroller) = find_largest_scrolled_window(&host) else {
                perf.record_scroll_note(&route, "no_scrolled_window");
                return;
            };
            let adjustment = scroller.vadjustment();
            adjustment.connect_value_changed(move |adjustment| {
                let max_adjustment = (adjustment.upper() - adjustment.page_size()).max(0.0);
                perf.record_manual_scroll_step(&route, adjustment.value(), max_adjustment);
            });
        });
    }
    fn register_favorite_button(&self, key: FavoriteControlKey, button: &gtk::Button) {
        register_favorite_control(&self.state.favorite_controls, key, button);
    }
    fn update_visible_favorite_buttons(&self, item_id: &FavoriteItemId, favorite: bool) {
        let key = favorite_control_key(item_id);
        update_favorite_controls(&self.state.favorite_controls, &key, favorite);
    }
    fn apply_favorite_changed(
        self: &Rc<Self>,
        item_id: FavoriteItemId,
        favorite: bool,
        snapshot: LibrarySnapshot,
    ) {
        let route = self.state.routes.borrow().current().clone();
        {
            let mut library = self.state.library.borrow_mut();
            merge_favorite_snapshot(
                &mut library,
                snapshot,
                &item_id,
                favorite,
                matches!(route, Route::Search { .. }),
            );
        }

        self.update_visible_favorite_buttons(&item_id, favorite);
        let track_sort_key = self.state.settings.borrow().track_table.sort_key;
        if favorite_change_needs_route_render(&route, &item_id, track_sort_key) {
            self.render_current_route();
        }
    }
    fn album_detail_view(self: &Rc<Self>, album_id: AlbumId) -> gtk::Widget {
        let detail = self
            .controller
            .cached_album_detail(&album_id)
            .ok()
            .flatten()
            .or_else(|| {
                let library = self.state.library.borrow();
                let album = library
                    .albums
                    .iter()
                    .find(|album| album.id.as_str() == album_id.as_str())
                    .cloned()?;
                let tracks = library
                    .tracks
                    .iter()
                    .filter(|track| track.album_id.as_str() == album_id.as_str())
                    .cloned()
                    .collect::<Vec<_>>();
                Some((album, tracks))
            });
        let Some((album, tracks)) = detail else {
            return self.placeholder_view("Album", "The selected cached album was not found.");
        };

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(true);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 22);
        content.add_css_class("route-content");
        content.set_margin_top(20);
        content.set_margin_bottom(36);
        content.set_margin_start(32);
        content.set_margin_end(32);

        let content_width = route_content_width(self);
        let compact = content_width < 760;
        let cover_size = if compact { 164 } else { 204 };
        let header_orientation = if compact {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        };
        let header = gtk::Box::new(header_orientation, if compact { 16 } else { 24 });
        header.add_css_class("album-detail-showcase");
        add_album_seed_gradient_class(&header, album.color_seed);
        header.set_hexpand(true);
        let cover = self.cover_tile_for(
            album.image_ref.as_ref(),
            album.color_seed,
            cover_size,
            DETAIL_COVER_SIZE,
        );
        cover.add_css_class("album-detail-cover");
        header.append(&cover);

        let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
        metadata.set_valign(gtk::Align::Center);
        metadata.set_hexpand(true);
        let kind = gtk::Label::new(Some(&tr("Album")));
        kind.add_css_class("eyebrow");
        kind.set_xalign(0.0);
        let title = gtk::Label::new(Some(&album.title));
        title.add_css_class("detail-title");
        title.set_xalign(0.0);
        title.set_wrap(true);
        let artist = gtk::Label::new(Some(&album.artist));
        artist.add_css_class("detail-artist");
        artist.set_xalign(0.0);
        artist.set_halign(gtk::Align::Start);
        artist.set_cursor_from_name(Some("pointer"));
        add_dynamic_link_hover(artist.upcast_ref(), &artist);
        if let Some(artist_id) = album.artist_id.clone() {
            let shell = Rc::clone(self);
            add_label_click(&artist, move || {
                shell.navigate(Route::ArtistDetail(artist_id.clone()))
            });
        } else if !album.artist.trim().is_empty() {
            let shell = Rc::clone(self);
            let artist_name = album.artist.clone();
            add_label_click(&artist, move || {
                shell.navigate(Route::Search {
                    query: artist_name.clone(),
                    kind: SearchKind::Artists,
                });
            });
        }
        let facts = gtk::Label::new(Some(&format!(
            "{} • {} {} • {}",
            album.year,
            album.track_count,
            tr("tracks"),
            format_duration(album.duration_seconds)
        )));
        facts.add_css_class("muted");
        facts.set_xalign(0.0);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.add_css_class("album-detail-actions");
        let play_album = icon_button("media-playback-start-symbolic", "Play");
        play_album.add_css_class("album-detail-action-button");
        play_album.add_css_class("album-detail-play-button");
        let controller = self.controller.clone();
        let album_tracks = tracks.clone();
        play_album.connect_clicked(move |_| controller.play_tracks_now(album_tracks.clone()));
        actions.append(&play_album);

        let play_next = icon_button(PLAY_NEXT_ICON, "Play next");
        play_next.add_css_class("album-detail-action-button");
        let controller = self.controller.clone();
        let next_tracks = tracks.clone();
        play_next.connect_clicked(move |_| {
            for track in next_tracks.iter().rev() {
                controller.play_next(track.clone());
            }
        });
        actions.append(&play_next);

        let favorite = favorite_icon_button("Favorite");
        favorite.add_css_class("album-detail-action-button");
        set_favorite_button_active(&favorite, album.favorite);
        self.register_favorite_button(album_favorite_key(&album.id), &favorite);
        let controller = self.controller.clone();
        let album_id = album.id.clone();
        favorite.connect_clicked(move |button| {
            controller.set_album_favorite(album_id.clone(), !favorite_button_is_active(button));
        });
        actions.append(&favorite);

        metadata.append(&kind);
        metadata.append(&title);
        metadata.append(&artist);
        metadata.append(&actions);
        metadata.append(&facts);
        header.append(&metadata);
        content.append(&header);

        let table =
            self.library_tracks_panel(tracks, LibraryListKey::AlbumDetailTracks, "album-detail");
        content.append(&table);

        scroller.set_child(Some(&content));
        scroller.upcast()
    }
    fn compact_artist_tracks_table(
        self: &Rc<Self>,
        tracks: Vec<Track>,
        context: &str,
    ) -> gtk::Widget {
        self.tracks_table_with_options(
            tracks,
            context,
            TrackTableOptions {
                paging: None,
                expand: false,
                max_visible_rows: Some(5),
                favorite_first: true,
            },
        )
    }
    fn tracks_table_with_options(
        self: &Rc<Self>,
        tracks: Vec<Track>,
        context: &str,
        options: TrackTableOptions,
    ) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 10);
        wrapper.set_vexpand(options.expand);
        let tracks = Rc::new(RefCell::new(tracks));
        let page_cursor = options.paging.map(|(offset, total)| {
            Rc::new(PagedGridCursor {
                offset: Cell::new(offset),
                total: Cell::new(total),
                loading: Cell::new(false),
            })
        });
        let server_search = page_cursor.is_some();
        let paged_query = Rc::new(RefCell::new(String::new()));

        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        toolbar.add_css_class("track-toolbar");
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(&tr("Search")));
        search.set_hexpand(true);
        toolbar.append(&search);

        let settings = self.state.settings.borrow().track_table.clone();
        let sort_button = gtk::Button::new();
        sort_button.add_css_class("flat");
        set_track_sort_button_content(&sort_button, &settings);
        toolbar.append(&sort_button);

        let configure = gtk::MenuButton::new();
        configure.add_css_class("flat");
        configure.set_icon_name("view-more-symbolic");
        configure.set_tooltip_text(Some(&tr("Configure columns")));
        toolbar.append(&configure);
        wrapper.append(&toolbar);

        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        populate_track_model_with_options(
            &model,
            &tracks.borrow(),
            &settings,
            "",
            options.favorite_first,
        );
        let selection = gtk::SingleSelection::new(Some(model.clone()));
        let table = gtk::ColumnView::new(Some(selection));
        table.add_css_class("track-table");
        table.set_vexpand(options.expand);
        table.set_hexpand(true);
        table.set_single_click_activate(false);
        set_track_table_columns(self, &table, &settings);

        let controller = self.controller.clone();
        let model_for_activate = model.clone();
        table.connect_activate(move |_, position| {
            let Some(item) = model_for_activate.item(position) else {
                return;
            };
            let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            controller.play_now(boxed.borrow::<Track>().clone());
        });

        let model_for_search = model.clone();
        let tracks_for_search = Rc::clone(&tracks);
        let shell = Rc::clone(self);
        let page_cursor_for_search = page_cursor.clone();
        let paged_query_for_search = Rc::clone(&paged_query);
        search.connect_search_changed(move |entry| {
            let settings = shell.state.settings.borrow().track_table.clone();
            if let Some(cursor) = page_cursor_for_search.as_ref() {
                let query = entry.text().trim().to_string();
                *paged_query_for_search.borrow_mut() = query.clone();
                cursor.offset.set(0);
                cursor.total.set(usize::MAX);
                cursor.loading.set(true);
                match shell
                    .controller
                    .cached_tracks_page_matching(&query, 0, TRACK_ROUTE_PAGE_SIZE)
                {
                    Ok(page) => {
                        let count = page.items.len();
                        *tracks_for_search.borrow_mut() = page.items;
                        let tracks = tracks_for_search.borrow();
                        populate_track_model_with_options(
                            &model_for_search,
                            &tracks,
                            &settings,
                            "",
                            options.favorite_first,
                        );
                        finish_grid_page(cursor, 0, count, page.total);
                    }
                    Err(error) => {
                        warn!(%error, "failed to search cached tracks page");
                        cursor.loading.set(false);
                    }
                }
            } else {
                let tracks = tracks_for_search.borrow();
                populate_track_model_with_options(
                    &model_for_search,
                    &tracks,
                    &settings,
                    entry.text().as_str(),
                    options.favorite_first,
                );
            }
        });

        let model_for_sort = model.clone();
        let tracks_for_sort = Rc::clone(&tracks);
        let shell = Rc::clone(self);
        let search_for_sort = search.clone();
        sort_button.connect_clicked(move |button| {
            let mut settings = shell.state.settings.borrow().track_table.clone();
            settings.descending = !settings.descending;
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
                options.favorite_first,
            );
            set_track_sort_button_content(button, &settings);
        });

        configure.set_popover(Some(&self.track_table_popover(
            &table,
            &model,
            Rc::clone(&tracks),
            &search,
            &sort_button,
            options.favorite_first,
            server_search,
        )));

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(options.expand);
        if let Some(max_visible_rows) = options.max_visible_rows {
            let visible_rows = tracks.borrow().len().min(max_visible_rows).max(1);
            let height = 92 + visible_rows as i32 * 58;
            scroller.set_min_content_height(height);
            scroller.set_max_content_height(height);
        }
        scroller.set_child(Some(&table));
        if let Some(cursor) = page_cursor {
            let shell = Rc::clone(self);
            let tracks_for_page = Rc::clone(&tracks);
            let model_for_page = model.clone();
            let paged_query_for_page = Rc::clone(&paged_query);
            let load_next = Rc::new(move || {
                if !shell.can_load_grid_page(&cursor, &Route::Tracks) {
                    return;
                }
                let offset = cursor.offset.get();
                let query = paged_query_for_page.borrow().clone();
                match shell.controller.cached_tracks_page_matching(
                    &query,
                    offset,
                    TRACK_ROUTE_PAGE_SIZE,
                ) {
                    Ok(page) => {
                        let count = page.items.len();
                        let mut items = page.items;
                        tracks_for_page.borrow_mut().extend(items.iter().cloned());
                        let settings = shell.state.settings.borrow().track_table.clone();
                        sort_tracks_with_options(&mut items, &settings, options.favorite_first);
                        append_tracks_to_model(&model_for_page, items);
                        finish_grid_page(&cursor, offset, count, page.total);
                    }
                    Err(error) => {
                        warn!(%error, "failed to append cached tracks page");
                        cursor.loading.set(false);
                    }
                }
            });
            connect_paged_grid_loader(&scroller, load_next);
        }
        wrapper.append(&scroller);
        wrapper.set_widget_name(context);
        wrapper.upcast()
    }
    fn new_playlist_dialog(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("New Playlist"))
            .body(tr(
                "Create a playlist. If a track is playing, it will be added.",
            ))
            .build();
        dialog.add_response("cancel", &tr("Cancel"));
        dialog.add_response("create", &tr("Create"));
        dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some(&tr("Playlist name")));
        dialog.set_extra_child(Some(&entry));
        let controller = self.controller.clone();
        let current_track = self
            .state
            .player
            .borrow()
            .current
            .as_ref()
            .and_then(|entry| {
                self.state
                    .library
                    .borrow()
                    .tracks
                    .iter()
                    .find(|track| track.id == entry.track_id)
                    .cloned()
            });
        dialog.connect_response(None, move |_, response| {
            if response == "create" {
                let name = entry.text().trim().to_string();
                if !name.is_empty() {
                    controller.create_playlist(name, current_track.clone().into_iter().collect());
                }
            }
        });
        dialog.present(Some(&self.window));
    }
    fn genre_detail_view(self: &Rc<Self>, genre_id: rufin_core::GenreId) -> gtk::Widget {
        let detail = self
            .controller
            .cached_genre_detail(&genre_id)
            .ok()
            .flatten()
            .or_else(|| {
                let library = self.state.library.borrow();
                let genre = library
                    .genres
                    .iter()
                    .find(|genre| genre.id.as_str() == genre_id.as_str())
                    .cloned()?;
                Some(CachedGenreDetail {
                    genre,
                    albums: Vec::new(),
                    tracks: Vec::new(),
                })
            });
        let Some(detail) = detail else {
            return self.placeholder_view("Genre", "The selected cached genre was not found.");
        };
        let seed = stable_seed(detail.genre.id.as_str());
        let summary = format!("{} {}", detail.genre.track_count, tr("tracks"));
        let cover_refs = grouped_cover_refs_for_items(&detail.albums, &detail.tracks);
        self.grouped_detail_view(GroupedDetailData {
            title: detail.genre.name,
            image_ref: detail.genre.image_ref,
            cover_refs,
            seed,
            summary,
            tracks: detail.tracks,
            table_context: "genre-detail",
        })
    }
    fn playlist_detail_view(self: &Rc<Self>, playlist_id: PlaylistId) -> gtk::Widget {
        let detail = self
            .controller
            .cached_playlist_detail(&playlist_id)
            .ok()
            .flatten()
            .or_else(|| {
                let library = self.state.library.borrow();
                let playlist = library
                    .playlists
                    .iter()
                    .find(|playlist| playlist.id.as_str() == playlist_id.as_str())
                    .cloned()?;
                Some(rufin_provider::PlaylistDetail {
                    playlist,
                    tracks: Vec::new(),
                    entries: Vec::new(),
                })
            });
        let Some(detail) = detail else {
            return self
                .placeholder_view("Playlist", "The selected cached playlist was not found.");
        };
        let seed = stable_seed(detail.playlist.id.as_str());
        let cover_refs = track_cover_refs_for_items(&detail.tracks);
        let summary = format!(
            "{} {} • {}",
            detail.playlist.track_count,
            tr("tracks"),
            format_duration(detail.playlist.duration_seconds)
        );
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(true);

        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 20);
        wrapper.add_css_class("route-content");
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_margin_top(28);
        wrapper.set_margin_bottom(36);
        wrapper.set_margin_start(32);
        wrapper.set_margin_end(32);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 22);
        header.append(&self.cover_group_tile_for(
            cover_refs,
            detail.playlist.image_ref.as_ref(),
            seed,
            160,
            DETAIL_COVER_SIZE,
        ));
        let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
        metadata.set_valign(gtk::Align::Center);
        let title = gtk::Label::new(Some(&detail.playlist.name));
        title.add_css_class("detail-title");
        title.set_xalign(0.0);
        title.set_wrap(true);
        let summary = gtk::Label::new(Some(&summary));
        summary.add_css_class("muted");
        summary.set_xalign(0.0);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let play = text_button("media-playback-start-symbolic", "Play");
        let controller = self.controller.clone();
        let tracks = detail.tracks.clone();
        play.connect_clicked(move |_| controller.play_tracks_now(tracks.clone()));
        actions.append(&play);
        let rename = text_button("document-edit-symbolic", "Rename");
        let shell = Rc::clone(self);
        let playlist_id_for_rename = detail.playlist.id.clone();
        let current_name = detail.playlist.name.clone();
        rename.connect_clicked(move |_| {
            shell.rename_playlist_dialog(playlist_id_for_rename.clone(), current_name.clone())
        });
        actions.append(&rename);
        let add_current = text_button("list-add-symbolic", "Add current");
        let current_track = self
            .state
            .player
            .borrow()
            .current
            .as_ref()
            .and_then(|entry| {
                self.state
                    .library
                    .borrow()
                    .tracks
                    .iter()
                    .find(|track| track.id == entry.track_id)
                    .cloned()
            });
        add_current.set_sensitive(current_track.is_some());
        let controller = self.controller.clone();
        let playlist_id_for_add = detail.playlist.id.clone();
        add_current.connect_clicked(move |_| {
            if let Some(track) = current_track.clone() {
                controller.add_tracks_to_playlist(playlist_id_for_add.clone(), vec![track]);
            }
        });
        actions.append(&add_current);
        metadata.append(&title);
        metadata.append(&summary);
        metadata.append(&actions);
        header.append(&metadata);
        wrapper.append(&header);

        if detail.entries.is_empty() {
            wrapper
                .append(&self.placeholder_view("Tracks", "No cached tracks are linked here yet."));
        } else {
            wrapper.append(&self.playlist_entries_view(&detail));
        }
        scroller.set_child(Some(&wrapper));
        scroller.upcast()
    }
    fn playlist_entries_view(
        self: &Rc<Self>,
        detail: &rufin_provider::PlaylistDetail,
    ) -> gtk::Widget {
        let entries = Rc::new(detail.entries.clone());
        let state = Rc::new(RefCell::new(PlaylistEntryListState::default()));
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 8);
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);

        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        toolbar.add_css_class("track-toolbar");
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(&tr("Search")));
        search.set_hexpand(true);
        toolbar.append(&search);

        let sort_titles = PLAYLIST_ENTRY_SORTS
            .iter()
            .map(|sort| tr(sort.title()))
            .collect::<Vec<_>>();
        let sort_refs = sort_titles.iter().map(String::as_str).collect::<Vec<_>>();
        let sort_options = gtk::StringList::new(&sort_refs);
        let sort_dropdown = gtk::DropDown::new(Some(sort_options), None::<gtk::Expression>);
        toolbar.append(&sort_dropdown);

        let direction = gtk::Button::from_icon_name("view-sort-ascending-symbolic");
        direction.add_css_class("flat");
        direction.set_tooltip_text(Some(&tr("Change sort order")));
        toolbar.append(&direction);
        wrapper.append(&toolbar);

        wrapper.append(&playlist_entries_header_row());

        let list = gtk::ListBox::new();
        list.add_css_class("track-table");
        list.add_css_class("playlist-entry-list");
        list.set_hexpand(true);
        list.set_halign(gtk::Align::Fill);
        list.set_selection_mode(gtk::SelectionMode::None);

        rebuild_playlist_entries_list(self, &list, &entries, &state.borrow(), &detail.playlist.id);

        {
            let shell = Rc::clone(self);
            let list = list.clone();
            let entries = Rc::clone(&entries);
            let state = Rc::clone(&state);
            let playlist_id = detail.playlist.id.clone();
            search.connect_search_changed(move |entry| {
                state.borrow_mut().query = entry.text().trim().to_string();
                rebuild_playlist_entries_list(
                    &shell,
                    &list,
                    &entries,
                    &state.borrow(),
                    &playlist_id,
                );
            });
        }
        {
            let shell = Rc::clone(self);
            let list = list.clone();
            let entries = Rc::clone(&entries);
            let state = Rc::clone(&state);
            let playlist_id = detail.playlist.id.clone();
            sort_dropdown.connect_selected_notify(move |dropdown| {
                let selected = PLAYLIST_ENTRY_SORTS
                    .get(dropdown.selected() as usize)
                    .copied()
                    .unwrap_or(PlaylistEntrySort::Order);
                state.borrow_mut().sort = selected;
                rebuild_playlist_entries_list(
                    &shell,
                    &list,
                    &entries,
                    &state.borrow(),
                    &playlist_id,
                );
            });
        }
        {
            let shell = Rc::clone(self);
            let list = list.clone();
            let entries = Rc::clone(&entries);
            let state = Rc::clone(&state);
            let playlist_id = detail.playlist.id.clone();
            direction.connect_clicked(move |button| {
                let descending = {
                    let mut state = state.borrow_mut();
                    state.descending = !state.descending;
                    state.descending
                };
                button.set_icon_name(if descending {
                    "view-sort-descending-symbolic"
                } else {
                    "view-sort-ascending-symbolic"
                });
                rebuild_playlist_entries_list(
                    &shell,
                    &list,
                    &entries,
                    &state.borrow(),
                    &playlist_id,
                );
            });
        }
        wrapper.append(&list);
        wrapper.upcast()
    }
}
