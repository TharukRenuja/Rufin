use super::*;

impl Shell {
    pub(in crate::ui) fn library_albums_view(self: &Rc<Self>) -> gtk::Widget {
        let view_started = Instant::now();
        let settings = self.library_settings(LibraryListKey::Albums);
        let load_started = Instant::now();
        let page = self.complete_album_snapshot_page().unwrap_or_else(|| {
            self.controller
                .cached_albums_page(0, GRID_ROUTE_PAGE_SIZE)
                .unwrap_or_else(|error| {
                    warn!(%error, "failed to load cached albums page");
                    let albums = self
                        .state
                        .library
                        .borrow()
                        .albums
                        .iter()
                        .take(GRID_ROUTE_PAGE_SIZE)
                        .cloned()
                        .collect::<Vec<_>>();
                    rufin_provider::PagedResponse::new(
                        albums,
                        self.state.library.borrow().albums.len(),
                    )
                })
        });
        let initial_load_ms = load_started.elapsed().as_millis() as u64;
        let complete_started = Instant::now();
        let page = complete_cached_page(
            page,
            library_layout_loads_complete_page(LibraryListKey::Albums, &settings),
            |limit| self.controller.cached_albums_page(0, limit),
            "albums",
        );
        let complete_load_ms = complete_started.elapsed().as_millis() as u64;
        let page_total = page.total;
        let complete_page = page.items.len() >= page.total;
        record_library_route_model_contract(
            self,
            "Albums",
            &settings,
            page.items.len(),
            page.total,
            !complete_page,
        );
        let source_albums = Rc::new(page.items.clone());
        let albums = Rc::new(RefCell::new(page.items));
        let album_count = albums.borrow().len();
        let tracks_started = Instant::now();
        let album_tracks = Rc::new(RefCell::new(
            self.album_tracks_for_layout(&albums.borrow(), &settings),
        ));
        let album_tracks_ms = tracks_started.elapsed().as_millis() as u64;
        warm_album_covers_for_settings(self, &albums.borrow(), &settings);
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let model_started = Instant::now();
        populate_album_collection_model(
            &model,
            &albums.borrow(),
            &settings,
            &album_tracks.borrow(),
        );
        let model_ms = model_started.elapsed().as_millis() as u64;

        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(&tr("Search")));
        search.set_hexpand(true);
        let cursor = Rc::new(super::PagedGridCursor {
            offset: std::cell::Cell::new(albums.borrow().len()),
            total: std::cell::Cell::new(page.total),
            loading: std::cell::Cell::new(false),
        });
        let query = Rc::new(RefCell::new(String::new()));

        {
            let shell = Rc::clone(self);
            let model = model.clone();
            let source_albums = Rc::clone(&source_albums);
            let albums = Rc::clone(&albums);
            let album_tracks = Rc::clone(&album_tracks);
            let cursor = Rc::clone(&cursor);
            let query = Rc::clone(&query);
            search.connect_search_changed(move |entry| {
                let text = entry.text().trim().to_string();
                *query.borrow_mut() = text.clone();
                if complete_page {
                    let query = text.to_lowercase();
                    let values = source_albums
                        .iter()
                        .filter(|album| query.is_empty() || album_matches_query(album, &query))
                        .cloned()
                        .collect::<Vec<_>>();
                    let count = values.len();
                    *albums.borrow_mut() = values;
                    *album_tracks.borrow_mut() = shell.album_tracks_for_layout(
                        &albums.borrow(),
                        &shell.library_settings(LibraryListKey::Albums),
                    );
                    warm_album_covers_for_settings(
                        &shell,
                        &albums.borrow(),
                        &shell.library_settings(LibraryListKey::Albums),
                    );
                    populate_album_collection_model(
                        &model,
                        &albums.borrow(),
                        &shell.library_settings(LibraryListKey::Albums),
                        &album_tracks.borrow(),
                    );
                    cursor.offset.set(count);
                    cursor.total.set(count);
                    cursor.loading.set(false);
                    return;
                }

                cursor.offset.set(0);
                cursor.total.set(usize::MAX);
                cursor.loading.set(true);
                match shell
                    .controller
                    .cached_albums_page_matching(&text, 0, GRID_ROUTE_PAGE_SIZE)
                {
                    Ok(page) => {
                        let settings = shell.library_settings(LibraryListKey::Albums);
                        let page = complete_cached_page(
                            page,
                            library_layout_loads_complete_page(LibraryListKey::Albums, &settings),
                            |limit| {
                                shell
                                    .controller
                                    .cached_albums_page_matching(&text, 0, limit)
                            },
                            "albums search",
                        );
                        let count = page.items.len();
                        *albums.borrow_mut() = page.items;
                        *album_tracks.borrow_mut() =
                            shell.album_tracks_for_layout(&albums.borrow(), &settings);
                        warm_album_covers_for_settings(&shell, &albums.borrow(), &settings);
                        populate_album_collection_model(
                            &model,
                            &albums.borrow(),
                            &settings,
                            &album_tracks.borrow(),
                        );
                        finish_grid_page(&cursor, 0, count, page.total);
                    }
                    Err(error) => {
                        warn!(%error, "failed to search cached albums page");
                        cursor.loading.set(false);
                    }
                }
            });
        }

        let load_next = {
            let shell = Rc::clone(self);
            let model = model.clone();
            let albums = Rc::clone(&albums);
            let album_tracks = Rc::clone(&album_tracks);
            let cursor = Rc::clone(&cursor);
            let query = Rc::clone(&query);
            Rc::new(move || {
                if !shell.can_load_grid_page(&cursor, &Route::Albums) {
                    return;
                }
                let offset = cursor.offset.get();
                let text = query.borrow().clone();
                match shell.controller.cached_albums_page_matching(
                    &text,
                    offset,
                    GRID_ROUTE_PAGE_SIZE,
                ) {
                    Ok(page) => {
                        let count = page.items.len();
                        let mut items = page.items;
                        let settings = shell.library_settings(LibraryListKey::Albums);
                        sort_albums(&mut items, &settings);
                        albums.borrow_mut().extend(items.iter().cloned());
                        *album_tracks.borrow_mut() =
                            shell.album_tracks_for_layout(&albums.borrow(), &settings);
                        warm_album_covers_for_settings(&shell, &albums.borrow(), &settings);
                        append_album_collection_model(
                            &model,
                            items,
                            &settings,
                            &album_tracks.borrow(),
                        );
                        finish_grid_page(&cursor, offset, count, page.total);
                    }
                    Err(error) => {
                        warn!(%error, "failed to append cached albums page");
                        cursor.loading.set(false);
                    }
                }
            }) as Rc<dyn Fn()>
        };

        let content_started = Instant::now();
        let detail_virtual =
            (settings.layout == LibraryLayout::Detail).then(album_detail_virtual_list);
        let content: gtk::Widget = detail_virtual
            .as_ref()
            .map(|list| list.widget.clone().upcast())
            .unwrap_or_else(|| {
                album_collection_widget(self, model.clone(), LibraryListKey::Albums)
            });
        let content_ms = content_started.elapsed().as_millis() as u64;
        let configure_scroller = {
            let shell = Rc::clone(self);
            let model = model.clone();
            let settings = settings.clone();
            let detail_virtual = detail_virtual.clone();
            Rc::new(move |scroller: &gtk::ScrolledWindow| {
                connect_album_viewport_cover_warm(&shell, scroller, &model, &settings);
                if let Some(list) = &detail_virtual {
                    connect_album_detail_virtual_list(
                        &shell,
                        scroller,
                        &model,
                        LibraryListKey::Albums,
                        list,
                    );
                }
            }) as Rc<dyn Fn(&gtk::ScrolledWindow)>
        };
        let shell_started = Instant::now();
        let view = self.library_page_shell(LibraryPageShellOptions {
            key: LibraryListKey::Albums,
            empty: albums.borrow().is_empty(),
            empty_body: "Cached albums will appear here after the background sync finishes.",
            search,
            content,
            load_next: if complete_page { None } else { Some(load_next) },
            configure_scroller: Some(configure_scroller),
        });
        let shell_ms = shell_started.elapsed().as_millis() as u64;
        if settings.layout == LibraryLayout::Detail {
            info!(
                albums = album_count,
                total = page_total,
                initial_load_ms,
                complete_load_ms,
                album_tracks_ms,
                model_ms,
                content_ms,
                shell_ms,
                total_ms = view_started.elapsed().as_millis() as u64,
                "albums detail view timing"
            );
        }
        view
    }
    pub(in crate::ui) fn library_tracks_route_view(self: &Rc<Self>) -> gtk::Widget {
        let started = Instant::now();
        let settings = self.library_settings(LibraryListKey::Tracks);
        if library_layout_loads_complete_page(LibraryListKey::Tracks, &settings)
            && let Some(page) = self.complete_track_snapshot_page()
        {
            if self.state.perf.is_some() {
                println!(
                    "RUFIN_PERF_TRACKS_LOAD source=snapshot tracks={} total={} elapsed_ms={}",
                    page.items.len(),
                    page.total,
                    started.elapsed().as_millis()
                );
            }
            return self.library_tracks_page(page.items, page.total);
        }

        let page = self
            .controller
            .cached_tracks_page(0, TRACK_ROUTE_PAGE_SIZE)
            .unwrap_or_else(|error| {
                warn!(%error, "failed to load cached tracks page");
                let tracks = self
                    .state
                    .library
                    .borrow()
                    .tracks
                    .iter()
                    .take(TRACK_ROUTE_PAGE_SIZE)
                    .cloned()
                    .collect::<Vec<_>>();
                rufin_provider::PagedResponse::new(
                    tracks,
                    self.state.library.borrow().cached_track_count,
                )
            });
        let page = complete_cached_page(
            page,
            library_layout_loads_complete_page(LibraryListKey::Tracks, &settings),
            |limit| self.controller.cached_tracks_page(0, limit),
            "tracks",
        );
        if self.state.perf.is_some() {
            println!(
                "RUFIN_PERF_TRACKS_LOAD source=store tracks={} total={} elapsed_ms={}",
                page.items.len(),
                page.total,
                started.elapsed().as_millis()
            );
        }
        self.library_tracks_page(page.items, page.total)
    }
    pub(in crate::ui) fn library_artist_list_view(
        self: &Rc<Self>,
        album_artist: bool,
    ) -> gtk::Widget {
        let key = if album_artist {
            LibraryListKey::AlbumArtists
        } else {
            LibraryListKey::Artists
        };
        let route = if album_artist {
            Route::AlbumArtists
        } else {
            Route::Artists
        };
        let settings = self.library_settings(key);
        let page = self
            .complete_artist_snapshot_page(album_artist)
            .unwrap_or_else(|| {
                self.controller
                    .cached_artists_page(album_artist, 0, GRID_ROUTE_PAGE_SIZE)
                    .unwrap_or_else(|error| {
                        warn!(%error, album_artist, "failed to load cached artists page");
                        let library = self.state.library.borrow();
                        let fallback = if album_artist {
                            &library.album_artists
                        } else {
                            &library.artists
                        };
                        rufin_provider::PagedResponse::new(
                            fallback
                                .iter()
                                .take(GRID_ROUTE_PAGE_SIZE)
                                .cloned()
                                .collect(),
                            fallback.len(),
                        )
                    })
            });
        let page = complete_cached_page(
            page,
            library_layout_loads_complete_page(key, &settings),
            |limit| self.controller.cached_artists_page(album_artist, 0, limit),
            "artists",
        );
        let complete_page = page.items.len() >= page.total;
        record_library_route_model_contract(
            self,
            if album_artist {
                "AlbumArtists"
            } else {
                "Artists"
            },
            &settings,
            page.items.len(),
            page.total,
            !complete_page,
        );
        let source_artists = Rc::new(page.items.clone());
        let artists = Rc::new(RefCell::new(page.items));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        warm_artist_covers_for_settings(self, &artists.borrow(), &settings);
        populate_artist_model(&model, &artists.borrow(), &settings);

        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(&tr("Search")));
        search.set_hexpand(true);
        let cursor = Rc::new(super::PagedGridCursor {
            offset: std::cell::Cell::new(artists.borrow().len()),
            total: std::cell::Cell::new(page.total),
            loading: std::cell::Cell::new(false),
        });
        let query = Rc::new(RefCell::new(String::new()));

        {
            let shell = Rc::clone(self);
            let model = model.clone();
            let source_artists = Rc::clone(&source_artists);
            let artists = Rc::clone(&artists);
            let cursor = Rc::clone(&cursor);
            let query = Rc::clone(&query);
            search.connect_search_changed(move |entry| {
                let text = entry.text().trim().to_string();
                *query.borrow_mut() = text.clone();
                if complete_page {
                    let query = text.to_lowercase();
                    let values = source_artists
                        .iter()
                        .filter(|artist| query.is_empty() || artist_matches_query(artist, &query))
                        .cloned()
                        .collect::<Vec<_>>();
                    let count = values.len();
                    *artists.borrow_mut() = values;
                    warm_artist_covers_for_settings(
                        &shell,
                        &artists.borrow(),
                        &shell.library_settings(key),
                    );
                    populate_artist_model(&model, &artists.borrow(), &shell.library_settings(key));
                    cursor.offset.set(count);
                    cursor.total.set(count);
                    cursor.loading.set(false);
                    return;
                }

                cursor.offset.set(0);
                cursor.total.set(usize::MAX);
                cursor.loading.set(true);
                match shell.controller.cached_artists_page_matching(
                    album_artist,
                    &text,
                    0,
                    GRID_ROUTE_PAGE_SIZE,
                ) {
                    Ok(page) => {
                        let settings = shell.library_settings(key);
                        let page = complete_cached_page(
                            page,
                            library_layout_loads_complete_page(key, &settings),
                            |limit| {
                                shell.controller.cached_artists_page_matching(
                                    album_artist,
                                    &text,
                                    0,
                                    limit,
                                )
                            },
                            "artists search",
                        );
                        let count = page.items.len();
                        *artists.borrow_mut() = page.items;
                        warm_artist_covers_for_settings(&shell, &artists.borrow(), &settings);
                        populate_artist_model(&model, &artists.borrow(), &settings);
                        finish_grid_page(&cursor, 0, count, page.total);
                    }
                    Err(error) => {
                        warn!(%error, "failed to search cached artists page");
                        cursor.loading.set(false);
                    }
                }
            });
        }

        let load_next = {
            let shell = Rc::clone(self);
            let model = model.clone();
            let artists = Rc::clone(&artists);
            let cursor = Rc::clone(&cursor);
            let query = Rc::clone(&query);
            Rc::new(move || {
                if !shell.can_load_grid_page(&cursor, &route) {
                    return;
                }
                let offset = cursor.offset.get();
                let text = query.borrow().clone();
                match shell.controller.cached_artists_page_matching(
                    album_artist,
                    &text,
                    offset,
                    GRID_ROUTE_PAGE_SIZE,
                ) {
                    Ok(page) => {
                        let count = page.items.len();
                        let mut items = page.items;
                        sort_artists(&mut items, &shell.library_settings(key));
                        warm_artist_covers_for_settings(
                            &shell,
                            &items,
                            &shell.library_settings(key),
                        );
                        artists.borrow_mut().extend(items.iter().cloned());
                        append_artists_to_model(&model, items);
                        finish_grid_page(&cursor, offset, count, page.total);
                    }
                    Err(error) => {
                        warn!(%error, "failed to append cached artists page");
                        cursor.loading.set(false);
                    }
                }
            }) as Rc<dyn Fn()>
        };
        let configure_scroller = {
            let shell = Rc::clone(self);
            let model = model.clone();
            let settings = settings.clone();
            Rc::new(move |scroller: &gtk::ScrolledWindow| {
                connect_artist_viewport_cover_warm(&shell, scroller, &model, &settings);
            }) as Rc<dyn Fn(&gtk::ScrolledWindow)>
        };

        self.library_page_shell(LibraryPageShellOptions {
            key,
            empty: artists.borrow().is_empty(),
            empty_body: "Cached rows will appear here after the background sync finishes.",
            search,
            content: artist_collection_widget(self, model, key),
            load_next: if complete_page { None } else { Some(load_next) },
            configure_scroller: Some(configure_scroller),
        })
    }
    pub(in crate::ui) fn library_genre_list_view(self: &Rc<Self>) -> gtk::Widget {
        let settings = self.library_settings(LibraryListKey::Genres);
        let page = self.complete_genre_snapshot_page().unwrap_or_else(|| {
            self.controller
                .cached_genres_page(0, GRID_ROUTE_PAGE_SIZE)
                .unwrap_or_else(|error| {
                    warn!(%error, "failed to load cached genres page");
                    let genres = self
                        .state
                        .library
                        .borrow()
                        .genres
                        .iter()
                        .take(GRID_ROUTE_PAGE_SIZE)
                        .cloned()
                        .collect::<Vec<_>>();
                    rufin_provider::PagedResponse::new(
                        genres,
                        self.state.library.borrow().genres.len(),
                    )
                })
        });
        let page = complete_cached_page(
            page,
            library_layout_loads_complete_page(LibraryListKey::Genres, &settings),
            |limit| self.controller.cached_genres_page(0, limit),
            "genres",
        );
        let complete_page = page.items.len() >= page.total;
        record_library_route_model_contract(
            self,
            "Genres",
            &settings,
            page.items.len(),
            page.total,
            !complete_page,
        );
        let source_genres = Rc::new(page.items.clone());
        let genres = Rc::new(RefCell::new(page.items));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        warm_genre_covers_for_settings(self, &genres.borrow(), &settings);
        populate_genre_model(&model, &genres.borrow(), &settings);

        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(&tr("Search")));
        search.set_hexpand(true);
        let cursor = Rc::new(super::PagedGridCursor {
            offset: std::cell::Cell::new(genres.borrow().len()),
            total: std::cell::Cell::new(page.total),
            loading: std::cell::Cell::new(false),
        });
        let query = Rc::new(RefCell::new(String::new()));

        {
            let shell = Rc::clone(self);
            let model = model.clone();
            let source_genres = Rc::clone(&source_genres);
            let genres = Rc::clone(&genres);
            let cursor = Rc::clone(&cursor);
            let query = Rc::clone(&query);
            search.connect_search_changed(move |entry| {
                let text = entry.text().trim().to_string();
                *query.borrow_mut() = text.clone();
                if complete_page {
                    let query = text.to_lowercase();
                    let values = source_genres
                        .iter()
                        .filter(|genre| query.is_empty() || genre_matches_query(genre, &query))
                        .cloned()
                        .collect::<Vec<_>>();
                    let count = values.len();
                    *genres.borrow_mut() = values;
                    warm_genre_covers_for_settings(
                        &shell,
                        &genres.borrow(),
                        &shell.library_settings(LibraryListKey::Genres),
                    );
                    populate_genre_model(
                        &model,
                        &genres.borrow(),
                        &shell.library_settings(LibraryListKey::Genres),
                    );
                    cursor.offset.set(count);
                    cursor.total.set(count);
                    cursor.loading.set(false);
                    return;
                }

                cursor.offset.set(0);
                cursor.total.set(usize::MAX);
                cursor.loading.set(true);
                match shell
                    .controller
                    .cached_genres_page_matching(&text, 0, GRID_ROUTE_PAGE_SIZE)
                {
                    Ok(page) => {
                        let settings = shell.library_settings(LibraryListKey::Genres);
                        let page = complete_cached_page(
                            page,
                            library_layout_loads_complete_page(LibraryListKey::Genres, &settings),
                            |limit| {
                                shell
                                    .controller
                                    .cached_genres_page_matching(&text, 0, limit)
                            },
                            "genres search",
                        );
                        let count = page.items.len();
                        *genres.borrow_mut() = page.items;
                        warm_genre_covers_for_settings(&shell, &genres.borrow(), &settings);
                        populate_genre_model(&model, &genres.borrow(), &settings);
                        finish_grid_page(&cursor, 0, count, page.total);
                    }
                    Err(error) => {
                        warn!(%error, "failed to search cached genres page");
                        cursor.loading.set(false);
                    }
                }
            });
        }

        let load_next = {
            let shell = Rc::clone(self);
            let model = model.clone();
            let genres = Rc::clone(&genres);
            let cursor = Rc::clone(&cursor);
            let query = Rc::clone(&query);
            Rc::new(move || {
                if !shell.can_load_grid_page(&cursor, &Route::Genres) {
                    return;
                }
                let offset = cursor.offset.get();
                let text = query.borrow().clone();
                match shell.controller.cached_genres_page_matching(
                    &text,
                    offset,
                    GRID_ROUTE_PAGE_SIZE,
                ) {
                    Ok(page) => {
                        let count = page.items.len();
                        let mut items = page.items;
                        sort_genres(&mut items, &shell.library_settings(LibraryListKey::Genres));
                        warm_genre_covers_for_settings(
                            &shell,
                            &items,
                            &shell.library_settings(LibraryListKey::Genres),
                        );
                        genres.borrow_mut().extend(items.iter().cloned());
                        append_genres_to_model(&model, items);
                        finish_grid_page(&cursor, offset, count, page.total);
                    }
                    Err(error) => {
                        warn!(%error, "failed to append cached genres page");
                        cursor.loading.set(false);
                    }
                }
            }) as Rc<dyn Fn()>
        };
        let configure_scroller = {
            let shell = Rc::clone(self);
            let model = model.clone();
            let settings = settings.clone();
            Rc::new(move |scroller: &gtk::ScrolledWindow| {
                connect_genre_viewport_cover_warm(&shell, scroller, &model, &settings);
            }) as Rc<dyn Fn(&gtk::ScrolledWindow)>
        };

        self.library_page_shell(LibraryPageShellOptions {
            key: LibraryListKey::Genres,
            empty: genres.borrow().is_empty(),
            empty_body: "Cached rows will appear here after the background sync finishes.",
            search,
            content: genre_collection_widget(self, model),
            load_next: if complete_page { None } else { Some(load_next) },
            configure_scroller: Some(configure_scroller),
        })
    }
    pub(in crate::ui) fn library_playlists_view(self: &Rc<Self>) -> gtk::Widget {
        let settings = self.library_settings(LibraryListKey::Playlists);
        let page = self.complete_playlist_snapshot_page().unwrap_or_else(|| {
            self.controller
                .cached_playlists_page(0, GRID_ROUTE_PAGE_SIZE)
                .unwrap_or_else(|error| {
                    warn!(%error, "failed to load cached playlists page");
                    let playlists = self
                        .state
                        .library
                        .borrow()
                        .playlists
                        .iter()
                        .take(GRID_ROUTE_PAGE_SIZE)
                        .cloned()
                        .collect::<Vec<_>>();
                    rufin_provider::PagedResponse::new(
                        playlists,
                        self.state.library.borrow().playlists.len(),
                    )
                })
        });
        let page = complete_cached_page(
            page,
            library_layout_loads_complete_page(LibraryListKey::Playlists, &settings),
            |limit| self.controller.cached_playlists_page(0, limit),
            "playlists",
        );
        let complete_page = page.items.len() >= page.total;
        record_library_route_model_contract(
            self,
            "Playlists",
            &settings,
            page.items.len(),
            page.total,
            !complete_page,
        );
        let source_playlists = Rc::new(page.items.clone());
        let playlists = Rc::new(RefCell::new(page.items));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        warm_playlist_covers_for_settings(self, &playlists.borrow(), &settings);
        populate_playlist_model(&model, &playlists.borrow(), &settings);

        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(&tr("Search")));
        search.set_hexpand(true);
        let cursor = Rc::new(super::PagedGridCursor {
            offset: std::cell::Cell::new(playlists.borrow().len()),
            total: std::cell::Cell::new(page.total),
            loading: std::cell::Cell::new(false),
        });
        let query = Rc::new(RefCell::new(String::new()));

        {
            let shell = Rc::clone(self);
            let model = model.clone();
            let source_playlists = Rc::clone(&source_playlists);
            let playlists = Rc::clone(&playlists);
            let cursor = Rc::clone(&cursor);
            let query = Rc::clone(&query);
            search.connect_search_changed(move |entry| {
                let text = entry.text().trim().to_string();
                *query.borrow_mut() = text.clone();
                if complete_page {
                    let query = text.to_lowercase();
                    let values = source_playlists
                        .iter()
                        .filter(|playlist| {
                            query.is_empty() || playlist_matches_query(playlist, &query)
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    let count = values.len();
                    *playlists.borrow_mut() = values;
                    warm_playlist_covers_for_settings(
                        &shell,
                        &playlists.borrow(),
                        &shell.library_settings(LibraryListKey::Playlists),
                    );
                    populate_playlist_model(
                        &model,
                        &playlists.borrow(),
                        &shell.library_settings(LibraryListKey::Playlists),
                    );
                    cursor.offset.set(count);
                    cursor.total.set(count);
                    cursor.loading.set(false);
                    return;
                }

                cursor.offset.set(0);
                cursor.total.set(usize::MAX);
                cursor.loading.set(true);
                match shell.controller.cached_playlists_page_matching(
                    &text,
                    0,
                    GRID_ROUTE_PAGE_SIZE,
                ) {
                    Ok(page) => {
                        let settings = shell.library_settings(LibraryListKey::Playlists);
                        let page = complete_cached_page(
                            page,
                            library_layout_loads_complete_page(
                                LibraryListKey::Playlists,
                                &settings,
                            ),
                            |limit| {
                                shell
                                    .controller
                                    .cached_playlists_page_matching(&text, 0, limit)
                            },
                            "playlists search",
                        );
                        let count = page.items.len();
                        *playlists.borrow_mut() = page.items;
                        warm_playlist_covers_for_settings(&shell, &playlists.borrow(), &settings);
                        populate_playlist_model(&model, &playlists.borrow(), &settings);
                        finish_grid_page(&cursor, 0, count, page.total);
                    }
                    Err(error) => {
                        warn!(%error, "failed to search cached playlists page");
                        cursor.loading.set(false);
                    }
                }
            });
        }

        let load_next = {
            let shell = Rc::clone(self);
            let model = model.clone();
            let playlists = Rc::clone(&playlists);
            let cursor = Rc::clone(&cursor);
            let query = Rc::clone(&query);
            Rc::new(move || {
                if !shell.can_load_grid_page(&cursor, &Route::Playlists) {
                    return;
                }
                let offset = cursor.offset.get();
                let text = query.borrow().clone();
                match shell.controller.cached_playlists_page_matching(
                    &text,
                    offset,
                    GRID_ROUTE_PAGE_SIZE,
                ) {
                    Ok(page) => {
                        let count = page.items.len();
                        let mut items = page.items;
                        sort_playlists(
                            &mut items,
                            &shell.library_settings(LibraryListKey::Playlists),
                        );
                        warm_playlist_covers_for_settings(
                            &shell,
                            &items,
                            &shell.library_settings(LibraryListKey::Playlists),
                        );
                        playlists.borrow_mut().extend(items.iter().cloned());
                        append_playlists_to_model(&model, items);
                        finish_grid_page(&cursor, offset, count, page.total);
                    }
                    Err(error) => {
                        warn!(%error, "failed to append cached playlists page");
                        cursor.loading.set(false);
                    }
                }
            }) as Rc<dyn Fn()>
        };

        self.library_page_shell(LibraryPageShellOptions {
            key: LibraryListKey::Playlists,
            empty: playlists.borrow().is_empty(),
            empty_body: "Cached playlists will appear here after the background sync finishes.",
            search,
            content: playlist_collection_widget(self, model),
            load_next: if complete_page { None } else { Some(load_next) },
            configure_scroller: Some(Rc::new(|scroller| {
                scroller.set_policy(gtk::PolicyType::External, gtk::PolicyType::Automatic);
                scroller.set_min_content_width(0);
            })),
        })
    }
    pub(in crate::ui) fn library_smart_playlists_view(self: &Rc<Self>) -> gtk::Widget {
        let settings = self.library_settings(LibraryListKey::SmartPlaylists);
        let page = self
            .controller
            .cached_smart_playlists_page(0, 1_000)
            .unwrap_or_else(|error| {
                warn!(%error, "failed to load cached smart playlists page");
                rufin_provider::PagedResponse::new(Vec::new(), 0)
            });
        let page_total = page.total;
        let items = page.items;
        let complete_page = items.len() >= page_total;
        record_library_route_model_contract(
            self,
            "SmartPlaylists",
            &settings,
            items.len(),
            page_total,
            !complete_page,
        );
        self.state.smart_playlists.replace(items.clone());
        let source_playlists = Rc::new(items.clone());
        let playlists = Rc::new(RefCell::new(items));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        populate_smart_playlist_model(&model, &playlists.borrow(), &settings);

        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(&tr("Search")));
        search.set_hexpand(true);
        {
            let shell = Rc::clone(self);
            let model = model.clone();
            let source_playlists = Rc::clone(&source_playlists);
            let playlists = Rc::clone(&playlists);
            search.connect_search_changed(move |entry| {
                let query = entry.text().trim().to_lowercase();
                let values = source_playlists
                    .iter()
                    .filter(|playlist| {
                        query.is_empty() || smart_playlist_matches_query(playlist, &query)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                shell.state.smart_playlists.replace(values.clone());
                *playlists.borrow_mut() = values;
                populate_smart_playlist_model(
                    &model,
                    &playlists.borrow(),
                    &shell.library_settings(LibraryListKey::SmartPlaylists),
                );
            });
        }

        self.library_page_shell(LibraryPageShellOptions {
            key: LibraryListKey::SmartPlaylists,
            empty: playlists.borrow().is_empty(),
            empty_body: "Smart playlists will appear here after the default set is seeded.",
            search,
            content: smart_playlist_collection_widget(self, model),
            load_next: None,
            configure_scroller: Some(Rc::new(|scroller| {
                scroller.set_policy(gtk::PolicyType::External, gtk::PolicyType::Automatic);
                scroller.set_min_content_width(0);
            })),
        })
    }
    pub(in crate::ui) fn library_tracks_panel_with_source(
        self: &Rc<Self>,
        tracks: Vec<Track>,
        key: LibraryListKey,
        context: &str,
        source_descriptor: Option<PlaySourceDescriptor>,
    ) -> gtk::Widget {
        let scroller = gtk::ScrolledWindow::new();
        let resize_scroller = scroller.clone();
        let resize: Rc<dyn Fn(usize)> = Rc::new(move |row_count| {
            set_library_table_content_height(&resize_scroller, row_count);
        });
        let (_empty, search, view, _model, _settings) =
            self.searchable_track_collection(tracks, key, Some(resize), source_descriptor);
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 10);
        wrapper.set_widget_name(context);
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.append(&self.library_toolbar(key, search));
        scroller.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
        scroller.set_min_content_width(0);
        scroller.set_propagate_natural_width(false);
        scroller.set_hexpand(true);
        scroller.set_halign(gtk::Align::Fill);
        view.set_hexpand(true);
        view.set_halign(gtk::Align::Fill);
        scroller.set_child(Some(&view));
        wrapper.append(&scroller);
        wrapper.upcast()
    }
    pub(in crate::ui) fn library_tracks_scrolling_panel(
        self: &Rc<Self>,
        tracks: Vec<Track>,
        key: LibraryListKey,
        context: &str,
        content_margin_start: i32,
        source_descriptor: Option<PlaySourceDescriptor>,
    ) -> gtk::Widget {
        let (_empty, search, view, model, settings) =
            self.searchable_track_collection(tracks, key, None, source_descriptor);
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 10);
        wrapper.set_widget_name(context);
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_vexpand(true);
        let toolbar = self.library_toolbar(key, search);
        toolbar.set_margin_start(content_margin_start);
        wrapper.append(&toolbar);

        let scroller = gtk::ScrolledWindow::new();
        configure_library_route_scroller(self, &scroller);
        connect_track_viewport_cover_warm(self, &scroller, &model, &settings);
        scroller.set_policy(gtk::PolicyType::External, gtk::PolicyType::Automatic);
        view.set_margin_start(content_margin_start);
        scroller.set_child(Some(&view));
        wrapper.append(&scroller);
        wrapper.upcast()
    }
    pub(in crate::ui) fn library_tracks_route_panel(
        self: &Rc<Self>,
        tracks: Vec<Track>,
        key: LibraryListKey,
        context: &str,
        empty_body: &str,
    ) -> gtk::Widget {
        let source_descriptor = match key {
            LibraryListKey::FavoriteTracks => Some(PlaySourceDescriptor::FavoriteTracks {
                selected_music_folder_id: selected_music_folder_id(self),
            }),
            LibraryListKey::Tracks => Some(PlaySourceDescriptor::GlobalTracks {
                selected_music_folder_id: selected_music_folder_id(self),
            }),
            _ => None,
        };
        let (empty, search, view, model, settings) =
            self.searchable_track_collection(tracks, key, None, source_descriptor);
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 14);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(24);
        wrapper.set_margin_bottom(LIBRARY_ROUTE_BOTTOM_MARGIN);
        wrapper.set_hexpand(true);
        wrapper.set_vexpand(true);
        wrapper.set_widget_name(context);
        wrapper.append(&library_route_inset(self.library_toolbar(key, search)));

        if empty {
            wrapper.append(&library_route_inset(self.route_empty_view(empty_body)));
        } else {
            let scroller = gtk::ScrolledWindow::new();
            configure_library_route_scroller(self, &scroller);
            connect_track_viewport_cover_warm(self, &scroller, &model, &settings);
            scroller.set_child(Some(&library_route_inset(view)));
            wrapper.append(&scroller);
        }

        wrapper.upcast()
    }
    pub(in crate::ui) fn searchable_track_collection(
        self: &Rc<Self>,
        tracks: Vec<Track>,
        key: LibraryListKey,
        on_visible_count_changed: Option<Rc<dyn Fn(usize)>>,
        source_descriptor: Option<PlaySourceDescriptor>,
    ) -> (
        bool,
        gtk::SearchEntry,
        gtk::Widget,
        gio::ListStore,
        LibraryListSettings,
    ) {
        let empty = tracks.is_empty();
        let source_tracks = Rc::new(tracks);
        let query = Rc::new(RefCell::new(String::new()));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let settings = self.library_settings(key);
        let visible_count =
            populate_track_model_for_settings(&model, source_tracks.as_ref(), &settings, "", false);
        warm_track_covers_for_settings(self, source_tracks.as_ref(), &settings);
        if let Some(on_visible_count_changed) = on_visible_count_changed.as_ref() {
            on_visible_count_changed(visible_count);
        }
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(&tr("Search")));
        search.set_hexpand(true);
        {
            let shell = Rc::clone(self);
            let model = model.clone();
            let source_tracks = Rc::clone(&source_tracks);
            let on_visible_count_changed = on_visible_count_changed.clone();
            let query = Rc::clone(&query);
            search.connect_search_changed(move |entry| {
                *query.borrow_mut() = entry.text().trim().to_string();
                let settings = shell.library_settings(key);
                let visible_count = populate_track_model_for_settings(
                    &model,
                    source_tracks.as_ref(),
                    &settings,
                    entry.text().as_str(),
                    false,
                );
                warm_track_covers_for_settings(&shell, source_tracks.as_ref(), &settings);
                if let Some(on_visible_count_changed) = on_visible_count_changed.as_ref() {
                    on_visible_count_changed(visible_count);
                }
            });
        }
        let play_context = source_descriptor.map(|descriptor| {
            track_collection_play_context(self, descriptor, key, Rc::clone(&query), false)
        });
        let view = track_collection_widget(self, model.clone(), key, play_context);
        (empty, search, view, model, settings)
    }
}
