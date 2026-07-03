use super::*;
use crate::i18n::msgid;
use crate::ui::{ADD_ICON, MORE_ICON, sort_order_icon};

pub(in crate::ui) type LibraryRouteLoader = Rc<dyn Fn()>;
pub(in crate::ui) type LibraryRouteScrollerConfigurator = Rc<dyn Fn(&gtk::ScrolledWindow)>;
const LIBRARY_TOOLBAR_END_MARGIN: i32 = 10;
const LIBRARY_TOOLBAR_CONTROL_SPACING: i32 = 12;
const LIBRARY_TOOLBAR_ICON_BUTTON_WIDTH: i32 = 18;
const LIBRARY_TOOLBAR_CLOSE_VISIBLE_SIZE: i32 = 24;
const LIBRARY_TOOLBAR_SORT_MIN_WIDTH: i32 = 112;
const LIBRARY_TOOLBAR_SORT_CHAR_WIDTH: i32 = 8;
const LIBRARY_TOOLBAR_SORT_HORIZONTAL_PADDING: i32 = 44;
const LIBRARY_TOOLBAR_STACK_WIDTH: i32 = 760;
const LIBRARY_TOOLBAR_WINDOW_CONTROLS_RESERVE: i32 =
    WINDOW_CHROME_MARGIN_END + LIBRARY_TOOLBAR_CLOSE_VISIBLE_SIZE + LIBRARY_TOOLBAR_CONTROL_SPACING;

pub(in crate::ui) struct LibraryPageShellOptions {
    pub(in crate::ui) key: LibraryListKey,
    pub(in crate::ui) empty: bool,
    pub(in crate::ui) empty_body: &'static str,
    pub(in crate::ui) search: gtk::SearchEntry,
    pub(in crate::ui) content: gtk::Widget,
    pub(in crate::ui) load_next: Option<LibraryRouteLoader>,
    pub(in crate::ui) configure_scroller: Option<LibraryRouteScrollerConfigurator>,
}

pub(in crate::ui) struct RoutePageTiming<'a> {
    pub(in crate::ui) route: &'a Route,
    pub(in crate::ui) action: &'static str,
    pub(in crate::ui) offset: usize,
    pub(in crate::ui) count: usize,
    pub(in crate::ui) total: usize,
    pub(in crate::ui) load_ms: u64,
    pub(in crate::ui) apply_ms: u64,
    pub(in crate::ui) total_ms: u64,
}

pub(in crate::ui) fn log_route_page_timing(timing: RoutePageTiming<'_>) {
    let RoutePageTiming {
        route,
        action,
        offset,
        count,
        total,
        load_ms,
        apply_ms,
        total_ms,
    } = timing;
    if total_ms >= SLOW_ROUTE_PAGE_LOAD_MS {
        warn!(
            ?route,
            action, offset, count, total, load_ms, apply_ms, total_ms, "slow route page load"
        );
    }
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
        warm_album_covers_for_settings(self, &source_albums, key, &settings);
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
                warm_album_covers_for_settings(&shell, &albums, key, &settings);
                populate_album_collection_model(&model, &albums, &settings, &album_tracks);
            });
        }

        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 10);
        wrapper.set_widget_name(context);
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        let toolbar = self.library_toolbar(key, search.clone());
        wrapper.append(&toolbar);
        self.install_type_to_search(&search);
        let collection = non_propagating_width_clip(album_collection_widget(self, model, key));
        wrapper.append(&collection);
        wrapper.upcast()
    }
    pub(in crate::ui) fn library_tracks_page(
        self: &Rc<Self>,
        tracks: Vec<Track>,
        total: usize,
    ) -> gtk::Widget {
        let settings = self.library_settings(LibraryListKey::Tracks);
        let complete_page = track_route_has_complete_page(tracks.len(), total, &settings);

        let tracks = Rc::new(RefCell::new(tracks));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let visible_tracks = tracks_for_settings(&tracks.borrow(), &settings, "", false);
        self.state
            .route_track_refs
            .replace(track_image_refs(&visible_tracks));
        replace_tracks_in_model(&model, visible_tracks);
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(&tr("Search")));
        search.set_hexpand(true);
        let cursor = Rc::new(super::PagedGridCursor {
            offset: std::cell::Cell::new(tracks.borrow().len()),
            total: std::cell::Cell::new(total),
            loading: std::cell::Cell::new(false),
        });
        let query = Rc::new(RefCell::new(String::new()));
        let play_query = Rc::clone(&query);
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
                let total_started = Instant::now();
                let offset = cursor.offset.get();
                let text = query.borrow().clone();
                let load_started = Instant::now();
                match shell.controller.cached_tracks_page_matching(
                    &text,
                    offset,
                    TRACK_ROUTE_PAGE_SIZE,
                ) {
                    Ok(page) => {
                        let load_ms = load_started.elapsed().as_millis() as u64;
                        let apply_started = Instant::now();
                        let count = page.items.len();
                        let total = page.total;
                        let mut items = page.items;
                        let settings = shell.library_settings(LibraryListKey::Tracks);
                        sort_tracks(&mut items, &settings, false);
                        warm_track_covers_for_settings(&shell, &items, &settings);
                        tracks.borrow_mut().extend(items.iter().cloned());
                        append_tracks_to_model(&model, items);
                        shell.refresh_current_route_now_playing_selections();
                        finish_grid_page(&cursor, offset, count, total);
                        log_route_page_timing(RoutePageTiming {
                            route: &Route::Tracks,
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
                        warn!(%error, "failed to append cached tracks page");
                        cursor.loading.set(false);
                    }
                }
            }) as Rc<dyn Fn()>
        };
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
                    let visible_tracks =
                        tracks_for_settings(&tracks.borrow(), &settings, &text, false);
                    let visible_count = visible_tracks.len();
                    shell
                        .state
                        .route_track_refs
                        .replace(track_image_refs(&visible_tracks));
                    replace_tracks_in_model(&model, visible_tracks);
                    shell.refresh_current_route_now_playing_selections();
                    warm_track_covers_for_settings(&shell, &tracks.borrow(), &settings);
                    cursor.offset.set(visible_count);
                    cursor.total.set(visible_count);
                    cursor.loading.set(false);
                    return;
                }

                cursor.offset.set(0);
                cursor.total.set(usize::MAX);
                cursor.loading.set(true);
                let total_started = Instant::now();
                let load_started = Instant::now();
                match shell
                    .controller
                    .cached_tracks_page_matching(&text, 0, TRACK_ROUTE_PAGE_SIZE)
                {
                    Ok(page) => {
                        let load_ms = load_started.elapsed().as_millis() as u64;
                        let apply_started = Instant::now();
                        let settings = shell.library_settings(LibraryListKey::Tracks);
                        let count = page.items.len();
                        let total = page.total;
                        *tracks.borrow_mut() = page.items;
                        let visible_tracks =
                            tracks_for_settings(&tracks.borrow(), &settings, "", false);
                        shell
                            .state
                            .route_track_refs
                            .replace(track_image_refs(&visible_tracks));
                        replace_tracks_in_model(&model, visible_tracks);
                        shell.refresh_current_route_now_playing_selections();
                        warm_track_covers_for_settings(&shell, &tracks.borrow(), &settings);
                        finish_grid_page(&cursor, 0, count, total);
                        log_route_page_timing(RoutePageTiming {
                            route: &Route::Tracks,
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
                        warn!(%error, "failed to search cached tracks page");
                        cursor.loading.set(false);
                    }
                }
            });
        }
        let track_viewport_warm = {
            let shell = Rc::clone(self);
            let model = model.clone();
            let settings = settings.clone();
            Rc::new(move |scroller: &gtk::ScrolledWindow| {
                connect_track_viewport_cover_warm(&shell, scroller, &model, &settings);
            }) as Rc<dyn Fn(&gtk::ScrolledWindow)>
        };
        let play_context = track_collection_play_context(
            self,
            PlaySourceDescriptor::GlobalTracks {
                selected_music_folder_id: selected_music_folder_id(self),
            },
            LibraryListKey::Tracks,
            play_query,
            false,
        );
        self.library_page_shell(LibraryPageShellOptions {
            key: LibraryListKey::Tracks,
            empty: tracks.borrow().is_empty(),
            empty_body: msgid("Cached entries will appear here after sync finishes"),
            search,
            content: track_collection_widget(
                self,
                model,
                LibraryListKey::Tracks,
                Some(play_context),
                PRIMARY_ROUTE_HORIZONTAL_INSET,
                ColumnViewWidthMode::RouteScroller,
                None,
            ),
            load_next: if complete_page { None } else { Some(load_next) },
            configure_scroller: Some(track_viewport_warm),
        })
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
        wrapper.set_margin_top(ROUTE_TOP_MARGIN);
        wrapper.set_margin_bottom(LIBRARY_ROUTE_BOTTOM_MARGIN);
        wrapper.set_hexpand(true);
        wrapper.set_vexpand(true);
        wrapper.append(&library_route_inset(
            self.library_toolbar(key, search.clone()),
        ));
        self.install_type_to_search(&search);

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
            wrapper.append(&route_scroller_widget(scroller));
        }

        wrapper.upcast()
    }

    pub(in crate::ui) fn install_type_to_search(&self, search: &gtk::SearchEntry) {
        self.state.type_to_search.replace(Some(search.clone()));
    }

    pub(in crate::ui) fn connect_type_to_search(self: &Rc<Self>) {
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        let shell = Rc::clone(self);
        key.connect_key_pressed(move |_, key, _, state| {
            let Some(search) = shell.state.type_to_search.borrow().as_ref().cloned() else {
                return glib::Propagation::Proceed;
            };
            if !shell.state.settings.borrow().type_to_search_enabled
                || shell.login_screen_active()
                || shell.state.fullscreen_player_visible.get()
                || shell.state.preferences_dialog.borrow().is_some()
                || shell.state.add_server_dialog.borrow().is_some()
                || shell.state.lyrics_search_dialog.borrow().is_some()
                || key_should_bypass_type_to_search(state)
                || focus_blocks_type_to_search(GtkWindowExt::focus(&shell.window).as_ref(), &search)
            {
                return glib::Propagation::Proceed;
            }
            let Some(character) = key.to_unicode().filter(|character| !character.is_control())
            else {
                return glib::Propagation::Proceed;
            };
            if character.is_whitespace() && search.text().trim().is_empty() {
                return glib::Propagation::Proceed;
            }
            let mut position = search.position();
            if let Some((start, end)) = search.selection_bounds() {
                search.delete_text(start, end);
                position = start;
            }
            search.insert_text(&character.to_string(), &mut position);
            search.set_position(position);
            search.grab_focus();
            glib::Propagation::Stop
        });
        self.window.add_controller(key);
    }
    pub(in crate::ui) fn library_toolbar(
        self: &Rc<Self>,
        key: LibraryListKey,
        search: gtk::SearchEntry,
    ) -> gtk::Widget {
        let toolbar = gtk::Box::new(
            library_toolbar_orientation_for_width(key, 1),
            LIBRARY_TOOLBAR_CONTROL_SPACING,
        );
        toolbar.add_css_class("track-toolbar");
        toolbar.set_hexpand(true);
        toolbar.set_halign(gtk::Align::Fill);
        toolbar.set_width_request(1);
        search.set_hexpand(true);
        search.set_width_request(1);
        toolbar.append(&search);
        let controls = gtk::Box::new(
            gtk::Orientation::Horizontal,
            LIBRARY_TOOLBAR_CONTROL_SPACING,
        );
        self.set_current_library_toolbar_controls(&controls);
        let command_button = Rc::new(RefCell::new(None::<gtk::Button>));
        let command_compact = Rc::new(Cell::new(false));

        match key {
            LibraryListKey::Playlists => {
                let create = gtk::Button::new();
                set_library_command_button_content(&create, false, ADD_ICON, "New Playlist");
                let shell = Rc::clone(self);
                create.connect_clicked(move |_| shell.new_playlist_dialog());
                controls.append(&create);
                command_button.replace(Some(create));
            }
            LibraryListKey::SmartPlaylists => {
                let create = gtk::Button::new();
                set_library_command_button_content(&create, false, ADD_ICON, "New Playlist");
                let shell = Rc::clone(self);
                create.connect_clicked(move |_| shell.new_smart_playlist_dialog());
                controls.append(&create);
                command_button.replace(Some(create));
            }
            _ => {}
        }

        let settings = self.library_settings(key);
        let sort_titles = available_sort_fields(key)
            .iter()
            .map(|field| tr(field.title()))
            .collect::<Vec<_>>();
        let sort_width = toolbar_sort_width_for_labels(sort_titles.iter().map(String::as_str));
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
        controls.append(&sort_dropdown);

        let direction = gtk::Button::from_icon_name(sort_order_icon(settings.descending));
        configure_library_toolbar_icon_button(&direction, &tr("Change sort order"));
        {
            let shell = Rc::clone(self);
            direction.connect_clicked(move |direction| {
                let mut descending = false;
                shell.update_library_list_settings(key, |settings| {
                    settings.descending = !settings.descending;
                    descending = settings.descending;
                });
                direction.set_icon_name(sort_order_icon(descending));
                shell.render_current_route_preserving_scroll();
            });
        }
        controls.append(&direction);

        let layout = gtk::Button::from_icon_name(layout_icon(settings.layout));
        configure_library_toolbar_icon_button(
            &layout,
            &format!("{}: {}", tr("Layout"), tr(layout_title(settings.layout))),
        );
        {
            let shell = Rc::clone(self);
            layout.connect_clicked(move |_| {
                shell.update_library_list_settings(key, |settings| {
                    settings.layout = next_layout(key, settings.layout);
                });
                shell.render_current_route_preserving_scroll();
            });
        }
        controls.append(&layout);

        let configure = gtk::Button::from_icon_name(MORE_ICON);
        configure_library_toolbar_icon_button(&configure, &tr("Customize display"));
        {
            let shell = Rc::clone(self);
            configure.connect_clicked(move |_| {
                shell.present_library_config_dialog(key);
            });
        }
        controls.append(&configure);
        toolbar.append(&controls);
        apply_library_toolbar_layout(
            key,
            &toolbar,
            &sort_dropdown,
            command_button.borrow().as_ref(),
            &command_compact,
            sort_width,
            1,
        );
        {
            let sort_dropdown = sort_dropdown.clone();
            let command_button = Rc::clone(&command_button);
            let command_compact = Rc::clone(&command_compact);
            toolbar.connect_notify_local(Some("width"), move |toolbar, _| {
                apply_library_toolbar_layout(
                    key,
                    toolbar,
                    &sort_dropdown,
                    command_button.borrow().as_ref(),
                    &command_compact,
                    sort_width,
                    toolbar.width(),
                );
            });
        }
        toolbar.upcast()
    }
    pub(in crate::ui) fn sync_library_toolbar_end_margin(&self) {
        let Some(controls) = self
            .current_library_toolbar_controls
            .borrow()
            .as_ref()
            .and_then(glib::WeakRef::upgrade)
        else {
            return;
        };
        let right_sidebar = self.state.resolved_right_sidebar.get();
        let margin = library_toolbar_end_margin(right_sidebar.is_visible());
        controls.set_margin_end(margin);
    }
    fn set_current_library_toolbar_controls(&self, controls: &gtk::Box) {
        self.current_library_toolbar_controls
            .replace(Some(controls.downgrade()));
        self.sync_library_toolbar_end_margin();
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
        configure_fill_width_clip(&scroller, gtk::PolicyType::Automatic);
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
        present_light_dismiss_dialog(&dialog, &self.window);
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
    ) -> Option<source::PagedResponse<Track>> {
        let library = self.state.library.borrow();
        if library.cached_track_count > library.tracks.len() {
            return None;
        }
        Some(source::PagedResponse::new(
            library.tracks.clone(),
            library.cached_track_count,
        ))
    }
    pub(in crate::ui) fn complete_album_snapshot_page(
        &self,
    ) -> Option<source::PagedResponse<Album>> {
        let library = self.state.library.borrow();
        if library.cached_album_count > library.albums.len() {
            return None;
        }
        Some(source::PagedResponse::new(
            library.albums.clone(),
            library.cached_album_count,
        ))
    }
    pub(in crate::ui) fn complete_artist_snapshot_page(
        &self,
        album_artist: bool,
    ) -> Option<source::PagedResponse<Artist>> {
        let library = self.state.library.borrow();
        let (items, total) = if album_artist {
            (&library.album_artists, library.cached_album_artist_count)
        } else {
            (&library.artists, library.cached_artist_count)
        };
        if total > items.len() {
            return None;
        }
        Some(source::PagedResponse::new(items.clone(), total))
    }
    pub(in crate::ui) fn complete_genre_snapshot_page(
        &self,
    ) -> Option<source::PagedResponse<Genre>> {
        let library = self.state.library.borrow();
        if library.cached_genre_count > library.genres.len() {
            return None;
        }
        Some(source::PagedResponse::new(
            library.genres.clone(),
            library.cached_genre_count,
        ))
    }
    pub(in crate::ui) fn complete_playlist_snapshot_page(
        &self,
    ) -> Option<source::PagedResponse<Playlist>> {
        let library = self.state.library.borrow();
        if library.cached_playlist_count > library.playlists.len() {
            return None;
        }
        Some(source::PagedResponse::new(
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

fn key_should_bypass_type_to_search(state: gtk::gdk::ModifierType) -> bool {
    state.intersects(
        gtk::gdk::ModifierType::ALT_MASK
            | gtk::gdk::ModifierType::CONTROL_MASK
            | gtk::gdk::ModifierType::SUPER_MASK
            | gtk::gdk::ModifierType::HYPER_MASK
            | gtk::gdk::ModifierType::META_MASK,
    )
}

fn focus_blocks_type_to_search(focus: Option<&gtk::Widget>, search: &gtk::SearchEntry) -> bool {
    let Some(focus) = focus else {
        return false;
    };
    focus.is_ancestor(search)
        || focus.is::<gtk::Editable>()
        || focus.is::<gtk::TextView>()
        || focus.ancestor(gtk::Editable::static_type()).is_some()
        || focus.ancestor(gtk::TextView::static_type()).is_some()
}

pub(in crate::ui) fn library_toolbar_stacks_for_width(_width: i32) -> bool {
    false
}
fn library_toolbar_compact_for_width(width: i32) -> bool {
    width < LIBRARY_TOOLBAR_STACK_WIDTH
}
pub(in crate::ui) fn toolbar_key_stack(_key: LibraryListKey, width: i32) -> bool {
    library_toolbar_stacks_for_width(width)
}
pub(in crate::ui) fn library_toolbar_orientation_for_width(
    key: LibraryListKey,
    width: i32,
) -> gtk::Orientation {
    if toolbar_key_stack(key, width) {
        gtk::Orientation::Vertical
    } else {
        gtk::Orientation::Horizontal
    }
}
pub(in crate::ui) fn toolbar_sort_width_for_labels<'a>(
    labels: impl IntoIterator<Item = &'a str>,
) -> i32 {
    labels
        .into_iter()
        .map(toolbar_sort_label_width)
        .max()
        .unwrap_or(LIBRARY_TOOLBAR_SORT_MIN_WIDTH)
}

fn toolbar_sort_label_width(label: &str) -> i32 {
    (label.chars().count() as i32 * LIBRARY_TOOLBAR_SORT_CHAR_WIDTH
        + LIBRARY_TOOLBAR_SORT_HORIZONTAL_PADDING)
        .max(LIBRARY_TOOLBAR_SORT_MIN_WIDTH)
}

pub(in crate::ui) fn library_toolbar_end_margin(right_sidebar_visible: bool) -> i32 {
    if right_sidebar_visible {
        LIBRARY_TOOLBAR_END_MARGIN
    } else {
        LIBRARY_TOOLBAR_WINDOW_CONTROLS_RESERVE
    }
}

fn apply_library_toolbar_layout(
    key: LibraryListKey,
    toolbar: &gtk::Box,
    sort_dropdown: &gtk::DropDown,
    command_button: Option<&gtk::Button>,
    command_compact: &Cell<bool>,
    sort_width: i32,
    width: i32,
) {
    let width = width.max(1);
    toolbar.set_orientation(library_toolbar_orientation_for_width(key, width));
    sort_dropdown.set_hexpand(false);
    sort_dropdown.set_halign(gtk::Align::End);
    sort_dropdown.set_width_request(sort_width);
    if let Some(button) = command_button {
        let compact = library_toolbar_compact_for_width(width);
        if command_compact.replace(compact) != compact {
            set_library_command_button_content(button, compact, ADD_ICON, "New Playlist");
        }
    }
}

fn configure_library_toolbar_icon_button(button: &gtk::Button, tooltip: &str) {
    button.add_css_class("flat");
    button.add_css_class("icon-button");
    button.add_css_class("library-toolbar-icon-button");
    button.set_width_request(LIBRARY_TOOLBAR_ICON_BUTTON_WIDTH);
    button.set_tooltip_text(Some(tooltip));
}

fn set_library_command_button_content(
    button: &gtk::Button,
    compact: bool,
    icon_name: &str,
    label: &str,
) {
    button.add_css_class("flat");
    button.set_tooltip_text(Some(&tr(label)));
    if compact {
        button.remove_css_class("pill-button");
        button.remove_css_class("pill");
        button.add_css_class("icon-button");
        button.add_css_class("circular");
        button.set_child(Some(&gtk::Image::from_icon_name(icon_name)));
        return;
    }

    button.remove_css_class("icon-button");
    button.remove_css_class("circular");
    button.add_css_class("pill-button");
    button.add_css_class("pill");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.append(&gtk::Image::from_icon_name(icon_name));
    content.append(&gtk::Label::new(Some(&tr(label))));
    button.set_child(Some(&content));
}

pub(in crate::ui) fn non_propagating_width_clip(child: gtk::Widget) -> gtk::Widget {
    child.set_hexpand(true);
    child.set_halign(gtk::Align::Fill);

    let clip = gtk::ScrolledWindow::new();
    clip.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
    clip.set_overflow(gtk::Overflow::Hidden);
    clip.set_width_request(1);
    clip.set_min_content_width(0);
    clip.set_max_content_width(1);
    clip.set_propagate_natural_width(false);
    clip.set_propagate_natural_height(true);
    clip.set_hexpand(true);
    clip.set_halign(gtk::Align::Fill);
    clip.set_child(Some(&child));
    clip.upcast()
}
