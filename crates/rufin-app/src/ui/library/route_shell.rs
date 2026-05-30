use super::*;

pub(in crate::ui) type LibraryRouteLoader = Rc<dyn Fn()>;
pub(in crate::ui) type LibraryRouteScrollerConfigurator = Rc<dyn Fn(&gtk::ScrolledWindow)>;

pub(in crate::ui) struct LibraryPageShellOptions {
    pub(in crate::ui) key: LibraryListKey,
    pub(in crate::ui) empty: bool,
    pub(in crate::ui) empty_body: &'static str,
    pub(in crate::ui) search: gtk::SearchEntry,
    pub(in crate::ui) content: gtk::Widget,
    pub(in crate::ui) load_next: Option<LibraryRouteLoader>,
    pub(in crate::ui) configure_scroller: Option<LibraryRouteScrollerConfigurator>,
}

impl Shell {
    pub(in crate::ui) fn library_album_collection_panel(
        self: &Rc<Self>,
        albums: &[Album],
        key: LibraryListKey,
        context: &str,
    ) -> gtk::Widget {
        let source_albums = Rc::new(albums.to_vec());
        let settings = self.library_settings(key);
        let album_tracks = self.album_tracks_for_layout(&source_albums, &settings);
        warm_album_covers_for_settings(self, &source_albums, &settings);
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        populate_album_collection_model(&model, &source_albums, &settings, &album_tracks);

        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(&tr("Search")));
        search.set_hexpand(true);
        {
            let shell = Rc::clone(self);
            let model = model.clone();
            let source_albums = Rc::clone(&source_albums);
            search.connect_search_changed(move |entry| {
                let query = entry.text().trim().to_lowercase();
                let albums = source_albums
                    .iter()
                    .filter(|album| query.is_empty() || album_matches_query(album, &query))
                    .cloned()
                    .collect::<Vec<_>>();
                let settings = shell.library_settings(key);
                let album_tracks = shell.album_tracks_for_layout(&albums, &settings);
                warm_album_covers_for_settings(&shell, &albums, &settings);
                populate_album_collection_model(&model, &albums, &settings, &album_tracks);
            });
        }

        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 10);
        wrapper.set_widget_name(context);
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.append(&self.library_toolbar(key, search));
        wrapper.append(&album_collection_widget(self, model, key));
        wrapper.upcast()
    }
    pub(in crate::ui) fn library_tracks_page(
        self: &Rc<Self>,
        tracks: Vec<Track>,
        total: usize,
    ) -> gtk::Widget {
        let started = Instant::now();
        let settings = self.library_settings(LibraryListKey::Tracks);
        let complete_page = library_layout_loads_complete_page(LibraryListKey::Tracks, &settings);

        let tracks = Rc::new(RefCell::new(tracks));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let populate_started = Instant::now();
        populate_track_model_for_settings(&model, &tracks.borrow(), &settings, "", false);
        let populate_ms = populate_started.elapsed().as_millis();
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(&tr("Search")));
        search.set_hexpand(true);
        let cursor = Rc::new(super::PagedGridCursor {
            offset: std::cell::Cell::new(tracks.borrow().len()),
            total: std::cell::Cell::new(total),
            loading: std::cell::Cell::new(false),
        });
        let query = Rc::new(RefCell::new(String::new()));
        {
            let shell = Rc::clone(self);
            let model = model.clone();
            let tracks = Rc::clone(&tracks);
            let cursor = Rc::clone(&cursor);
            let query = Rc::clone(&query);
            search.connect_search_changed(move |entry| {
                let text = entry.text().trim().to_string();
                *query.borrow_mut() = text.clone();
                if complete_page {
                    let settings = shell.library_settings(LibraryListKey::Tracks);
                    let visible_count = populate_track_model_for_settings(
                        &model,
                        &tracks.borrow(),
                        &settings,
                        &text,
                        false,
                    );
                    warm_track_covers_for_settings(&shell, &tracks.borrow(), &settings);
                    cursor.offset.set(visible_count);
                    cursor.total.set(visible_count);
                    cursor.loading.set(false);
                    return;
                }

                cursor.offset.set(0);
                cursor.total.set(usize::MAX);
                cursor.loading.set(true);
                match shell
                    .controller
                    .cached_tracks_page_matching(&text, 0, TRACK_ROUTE_PAGE_SIZE)
                {
                    Ok(page) => {
                        let settings = shell.library_settings(LibraryListKey::Tracks);
                        let page = complete_cached_page(
                            page,
                            library_layout_loads_complete_page(LibraryListKey::Tracks, &settings),
                            |limit| {
                                shell
                                    .controller
                                    .cached_tracks_page_matching(&text, 0, limit)
                            },
                            "tracks search",
                        );
                        let count = page.items.len();
                        *tracks.borrow_mut() = page.items;
                        populate_track_model_for_settings(
                            &model,
                            &tracks.borrow(),
                            &settings,
                            "",
                            false,
                        );
                        warm_track_covers_for_settings(&shell, &tracks.borrow(), &settings);
                        finish_grid_page(&cursor, 0, count, page.total);
                    }
                    Err(error) => {
                        warn!(%error, "failed to search cached tracks page");
                        cursor.loading.set(false);
                    }
                }
            });
        }
        let load_next = {
            let shell = Rc::clone(self);
            let model = model.clone();
            let tracks = Rc::clone(&tracks);
            let cursor = Rc::clone(&cursor);
            let query = Rc::clone(&query);
            Rc::new(move || {
                if !shell.can_load_grid_page(&cursor, &Route::Tracks) {
                    return;
                }
                let offset = cursor.offset.get();
                let text = query.borrow().clone();
                match shell.controller.cached_tracks_page_matching(
                    &text,
                    offset,
                    TRACK_ROUTE_PAGE_SIZE,
                ) {
                    Ok(page) => {
                        let count = page.items.len();
                        let mut items = page.items;
                        let settings = shell.library_settings(LibraryListKey::Tracks);
                        sort_tracks(&mut items, &settings, false);
                        warm_track_covers_for_settings(&shell, &items, &settings);
                        tracks.borrow_mut().extend(items.iter().cloned());
                        append_tracks_to_model(&model, items);
                        finish_grid_page(&cursor, offset, count, page.total);
                    }
                    Err(error) => {
                        warn!(%error, "failed to append cached tracks page");
                        cursor.loading.set(false);
                    }
                }
            }) as Rc<dyn Fn()>
        };
        let track_viewport_warm = {
            let shell = Rc::clone(self);
            let model = model.clone();
            let settings = settings.clone();
            Rc::new(move |scroller: &gtk::ScrolledWindow| {
                connect_track_viewport_cover_warm(&shell, scroller, &model, &settings);
                connect_track_row_contract_observer(&shell, scroller, &model, &settings);
            }) as Rc<dyn Fn(&gtk::ScrolledWindow)>
        };
        let shell_started = Instant::now();
        let view = self.library_page_shell(LibraryPageShellOptions {
            key: LibraryListKey::Tracks,
            empty: tracks.borrow().is_empty(),
            empty_body: "Cached tracks will appear here after the background sync finishes.",
            search,
            content: track_collection_widget(self, model, LibraryListKey::Tracks),
            load_next: if complete_page { None } else { Some(load_next) },
            configure_scroller: Some(track_viewport_warm),
        });
        if self.state.perf.is_some() {
            println!(
                "RUFIN_PERF_TRACKS_PAGE tracks={} total={} complete={} populate_ms={} shell_ms={} total_ms={}",
                tracks.borrow().len(),
                total,
                complete_page,
                populate_ms,
                shell_started.elapsed().as_millis(),
                started.elapsed().as_millis()
            );
        }
        view
    }
    pub(in crate::ui) fn library_page_shell(
        self: &Rc<Self>,
        options: LibraryPageShellOptions,
    ) -> gtk::Widget {
        let LibraryPageShellOptions {
            key,
            empty,
            empty_body,
            search,
            content,
            load_next,
            configure_scroller,
        } = options;
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 14);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(24);
        wrapper.set_margin_bottom(LIBRARY_ROUTE_BOTTOM_MARGIN);
        wrapper.set_hexpand(true);
        wrapper.set_vexpand(true);
        wrapper.append(&library_route_inset(self.library_toolbar(key, search)));

        if empty {
            wrapper.append(&library_route_inset(self.route_empty_view(empty_body)));
        } else {
            let scroller = gtk::ScrolledWindow::new();
            configure_library_route_scroller(self, &scroller);
            scroller.set_child(Some(&library_route_inset(content)));
            if let Some(configure_scroller) = configure_scroller {
                configure_scroller(&scroller);
            }
            if let Some(load_next) = load_next {
                connect_paged_grid_loader(&scroller, load_next);
            }
            wrapper.append(&scroller);
        }

        wrapper.upcast()
    }
    pub(in crate::ui) fn library_toolbar(
        self: &Rc<Self>,
        key: LibraryListKey,
        search: gtk::SearchEntry,
    ) -> gtk::Widget {
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        toolbar.add_css_class("track-toolbar");
        toolbar.append(&search);

        match key {
            LibraryListKey::Playlists => {
                let create = text_button("list-add-symbolic", "New Playlist");
                let shell = Rc::clone(self);
                create.connect_clicked(move |_| shell.new_playlist_dialog());
                toolbar.append(&create);
            }
            LibraryListKey::SmartPlaylists => {
                let create = text_button("list-add-symbolic", "New Playlist");
                let shell = Rc::clone(self);
                create.connect_clicked(move |_| shell.new_smart_playlist_dialog());
                toolbar.append(&create);
            }
            _ => {}
        }

        let settings = self.library_settings(key);
        let sort_titles = available_sort_fields(key)
            .iter()
            .map(|field| tr(field.title()))
            .collect::<Vec<_>>();
        let sort_refs = sort_titles.iter().map(String::as_str).collect::<Vec<_>>();
        let sort_options = gtk::StringList::new(&sort_refs);
        let sort_dropdown = gtk::DropDown::new(Some(sort_options), None::<gtk::Expression>);
        sort_dropdown.set_selected(
            available_sort_fields(key)
                .iter()
                .position(|field| *field == settings.sort_key)
                .unwrap_or(0) as u32,
        );
        {
            let shell = Rc::clone(self);
            sort_dropdown.connect_selected_notify(move |dropdown| {
                let sort_key = available_sort_fields(key)
                    .get(dropdown.selected() as usize)
                    .copied()
                    .unwrap_or(LibraryField::Title);
                shell.update_library_list_settings(key, |settings| settings.sort_key = sort_key);
                shell.render_current_route_preserving_scroll();
            });
        }
        toolbar.append(&sort_dropdown);

        let direction = gtk::Button::from_icon_name(if settings.descending {
            "view-sort-descending-symbolic"
        } else {
            "view-sort-ascending-symbolic"
        });
        direction.add_css_class("flat");
        direction.set_tooltip_text(Some(&tr("Change sort order")));
        {
            let shell = Rc::clone(self);
            direction.connect_clicked(move |_| {
                shell.update_library_list_settings(key, |settings| {
                    settings.descending = !settings.descending;
                });
                shell.render_current_route_preserving_scroll();
            });
        }
        toolbar.append(&direction);

        let layout = gtk::Button::from_icon_name(layout_icon(settings.layout));
        layout.add_css_class("flat");
        layout.set_tooltip_text(Some(&format!(
            "{}: {}",
            tr("Layout"),
            tr(layout_title(settings.layout))
        )));
        {
            let shell = Rc::clone(self);
            layout.connect_clicked(move |_| {
                shell.update_library_list_settings(key, |settings| {
                    settings.layout = next_layout(key, settings.layout);
                });
                shell.render_current_route_preserving_scroll();
            });
        }
        toolbar.append(&layout);

        let configure = gtk::Button::from_icon_name("view-more-symbolic");
        configure.add_css_class("flat");
        configure.set_tooltip_text(Some(&tr("Customize display")));
        {
            let shell = Rc::clone(self);
            configure.connect_clicked(move |_| {
                shell.present_library_config_dialog(key);
            });
        }
        toolbar.append(&configure);
        toolbar.upcast()
    }
    pub(in crate::ui) fn present_library_config_dialog(self: &Rc<Self>, key: LibraryListKey) {
        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        let title = adw::WindowTitle::new(&tr("Customize display"), &tr(key.title()));
        header.set_title_widget(Some(&title));
        let reset = icon_button("view-refresh-symbolic", "Reset display");
        header.pack_end(&reset);
        toolbar.add_top_bar(&header);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.set_margin_top(18);
        content.set_margin_bottom(18);
        content.set_margin_start(18);
        content.set_margin_end(18);

        let layout_group = adw::PreferencesGroup::builder()
            .title(tr("Layout"))
            .description(tr("Choose the current page layout."))
            .build();
        let layout_row = adw::ActionRow::builder().title(tr("View")).build();
        let layout_buttons = Rc::new(RefCell::new(
            Vec::<(LibraryLayout, gtk::ToggleButton)>::new(),
        ));
        let layout_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        layout_box.add_css_class("linked");
        let mut first_button: Option<gtk::ToggleButton> = None;
        for layout in supported_layouts(key) {
            let button = gtk::ToggleButton::new();
            button.set_child(Some(&layout_button_content(layout)));
            button.set_tooltip_text(Some(&tr(layout_title(layout))));
            if let Some(first) = &first_button {
                button.set_group(Some(first));
            } else {
                first_button = Some(button.clone());
            }
            button.set_active(layout == self.library_settings(key).layout);
            layout_box.append(&button);
            layout_buttons.borrow_mut().push((layout, button));
        }
        layout_row.add_suffix(&layout_box);
        layout_group.add(&layout_row);
        content.append(&layout_group);

        let fields_group = adw::PreferencesGroup::builder().build();
        let rows = Rc::new(RefCell::new(Vec::<adw::ActionRow>::new()));
        content.append(&fields_group);

        for (layout, button) in layout_buttons.borrow().iter() {
            let shell = Rc::clone(self);
            let fields_group = fields_group.clone();
            let rows = Rc::clone(&rows);
            let layout_buttons = Rc::clone(&layout_buttons);
            let layout = *layout;
            button.connect_toggled(move |button| {
                if !button.is_active() || shell.library_settings(key).layout == layout {
                    return;
                }
                shell.update_library_list_settings(key, |settings| {
                    settings.layout = layout;
                });
                sync_layout_buttons(&layout_buttons, layout);
                populate_library_field_rows(&shell, key, &fields_group, &rows);
                shell.render_current_route_preserving_scroll();
            });
        }

        {
            let shell = Rc::clone(self);
            let fields_group = fields_group.clone();
            let rows = Rc::clone(&rows);
            let layout_buttons = Rc::clone(&layout_buttons);
            reset.connect_clicked(move |_| {
                let default_settings = LibraryListSettings::for_key(key);
                shell.update_library_list_settings(key, |settings| {
                    *settings = default_settings.clone();
                });
                sync_layout_buttons(&layout_buttons, default_settings.layout);
                populate_library_field_rows(&shell, key, &fields_group, &rows);
                shell.render_current_route_preserving_scroll();
            });
        }

        populate_library_field_rows(self, key, &fields_group, &rows);

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_child(Some(&content));
        toolbar.set_content(Some(&scroller));

        let dialog = adw::Dialog::builder()
            .content_width(large_popup_content_width(LIBRARY_CONFIG_DIALOG_WIDTH))
            .content_height(large_popup_content_height(
                self.window.height(),
                LIBRARY_CONFIG_DIALOG_HEIGHT,
            ))
            .child(&toolbar)
            .build();
        dialog.present(Some(&self.window));
    }
    pub(in crate::ui) fn library_settings(&self, key: LibraryListKey) -> LibraryListSettings {
        self.state.settings.borrow().library_list(key)
    }
    pub(in crate::ui) fn album_tracks_for(&self, albums: &[Album]) -> HashMap<AlbumId, Vec<Track>> {
        if let Some(tracks_by_album) = self.complete_snapshot_tracks_for_albums(albums) {
            return tracks_by_album;
        }

        let ids = albums
            .iter()
            .map(|album| album.id.clone())
            .collect::<Vec<_>>();
        self.controller
            .cached_album_tracks(&ids)
            .unwrap_or_else(|error| {
                warn!(%error, "failed to load cached album tracks");
                HashMap::new()
            })
    }
    pub(in crate::ui) fn album_tracks_for_layout(
        &self,
        albums: &[Album],
        settings: &LibraryListSettings,
    ) -> HashMap<AlbumId, Vec<Track>> {
        if settings.layout == LibraryLayout::Detail {
            self.album_tracks_for(albums)
        } else {
            HashMap::new()
        }
    }
    pub(in crate::ui) fn complete_track_snapshot_page(
        &self,
    ) -> Option<rufin_provider::PagedResponse<Track>> {
        let library = self.state.library.borrow();
        if library.cached_track_count > library.tracks.len() {
            return None;
        }
        Some(rufin_provider::PagedResponse::new(
            library.tracks.clone(),
            library.cached_track_count,
        ))
    }
    pub(in crate::ui) fn complete_album_snapshot_page(
        &self,
    ) -> Option<rufin_provider::PagedResponse<Album>> {
        let library = self.state.library.borrow();
        if library.cached_album_count > library.albums.len() {
            return None;
        }
        Some(rufin_provider::PagedResponse::new(
            library.albums.clone(),
            library.cached_album_count,
        ))
    }
    pub(in crate::ui) fn complete_artist_snapshot_page(
        &self,
        album_artist: bool,
    ) -> Option<rufin_provider::PagedResponse<Artist>> {
        let library = self.state.library.borrow();
        let (items, total) = if album_artist {
            (&library.album_artists, library.cached_album_artist_count)
        } else {
            (&library.artists, library.cached_artist_count)
        };
        if total > items.len() {
            return None;
        }
        Some(rufin_provider::PagedResponse::new(items.clone(), total))
    }
    pub(in crate::ui) fn complete_genre_snapshot_page(
        &self,
    ) -> Option<rufin_provider::PagedResponse<Genre>> {
        let library = self.state.library.borrow();
        if library.cached_genre_count > library.genres.len() {
            return None;
        }
        Some(rufin_provider::PagedResponse::new(
            library.genres.clone(),
            library.cached_genre_count,
        ))
    }
    pub(in crate::ui) fn complete_playlist_snapshot_page(
        &self,
    ) -> Option<rufin_provider::PagedResponse<Playlist>> {
        let library = self.state.library.borrow();
        if library.cached_playlist_count > library.playlists.len() {
            return None;
        }
        Some(rufin_provider::PagedResponse::new(
            library.playlists.clone(),
            library.cached_playlist_count,
        ))
    }
    pub(in crate::ui) fn complete_snapshot_tracks_for_albums(
        &self,
        albums: &[Album],
    ) -> Option<HashMap<AlbumId, Vec<Track>>> {
        let album_ids = albums
            .iter()
            .map(|album| album.id.clone())
            .collect::<HashSet<_>>();
        let library = self.state.library.borrow();
        if library.cached_track_count > library.tracks.len() {
            return None;
        }
        let mut tracks_by_album = HashMap::<AlbumId, Vec<Track>>::new();
        for track in &library.tracks {
            if album_ids.contains(&track.album_id) {
                tracks_by_album
                    .entry(track.album_id.clone())
                    .or_default()
                    .push(track.clone());
            }
        }
        Some(tracks_by_album)
    }
}
