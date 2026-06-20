use super::*;
use crate::i18n::msgid;

const EMBEDDED_SCROLL_LATCH_MS: u128 = 280;
const EMBEDDED_SURFACE_SCROLL_FACTOR: f64 = 2.5;

#[derive(Clone, Copy)]
pub(in crate::ui) struct LibraryRouteLoadTiming {
    source: &'static str,
    initial_load_ms: u64,
    complete_load_ms: u64,
    loaded_before_complete: usize,
}

impl Shell {
    pub(in crate::ui) fn library_albums_view(self: &Rc<Self>) -> gtk::Widget {
        let settings = self.library_settings(LibraryListKey::Albums);
        let initial_started = Instant::now();
        let mut source = "snapshot";
        let page = if let Some(page) = self.complete_album_snapshot_page() {
            page
        } else {
            source = "cache";
            self.controller
                .cached_albums_page(0, GRID_ROUTE_PAGE_SIZE)
                .unwrap_or_else(|error| {
                    source = "fallback";
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
                    source::PagedResponse::new(albums, self.state.library.borrow().albums.len())
                })
        };
        let initial_load_ms = initial_started.elapsed().as_millis() as u64;
        let loaded_before_complete = page.items.len();
        let complete_started = Instant::now();
        let page = complete_cached_page(
            page,
            library_layout_loads_complete_page(LibraryListKey::Albums, &settings),
            |limit| self.controller.cached_albums_page(0, limit),
            "albums",
        );
        let timing = LibraryRouteLoadTiming {
            source,
            initial_load_ms,
            complete_load_ms: complete_started.elapsed().as_millis() as u64,
            loaded_before_complete,
        };
        self.library_albums_view_from_page(page, timing)
    }

    pub(in crate::ui) fn library_albums_view_from_page(
        self: &Rc<Self>,
        page: source::PagedResponse<Album>,
        timing: LibraryRouteLoadTiming,
    ) -> gtk::Widget {
        let view_started = Instant::now();
        let settings = self.library_settings(LibraryListKey::Albums);
        let page_total = page.total;
        let complete_page = page.items.len() >= page.total;
        let source_albums = Rc::new(page.items.clone());
        let albums = Rc::new(RefCell::new(page.items));
        let album_count = albums.borrow().len();
        let tracks_started = Instant::now();
        let album_tracks = Rc::new(RefCell::new(
            self.album_tracks_for_layout(&albums.borrow(), &settings),
        ));
        let album_tracks_ms = tracks_started.elapsed().as_millis() as u64;
        warm_album_covers_for_settings(self, &albums.borrow(), LibraryListKey::Albums, &settings);
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
                        LibraryListKey::Albums,
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
                let total_started = Instant::now();
                let load_started = Instant::now();
                match shell
                    .controller
                    .cached_albums_page_matching(&text, 0, GRID_ROUTE_PAGE_SIZE)
                {
                    Ok(page) => {
                        let load_ms = load_started.elapsed().as_millis() as u64;
                        let apply_started = Instant::now();
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
                        let total = page.total;
                        *albums.borrow_mut() = page.items;
                        *album_tracks.borrow_mut() =
                            shell.album_tracks_for_layout(&albums.borrow(), &settings);
                        warm_album_covers_for_settings(
                            &shell,
                            &albums.borrow(),
                            LibraryListKey::Albums,
                            &settings,
                        );
                        populate_album_collection_model(
                            &model,
                            &albums.borrow(),
                            &settings,
                            &album_tracks.borrow(),
                        );
                        finish_grid_page(&cursor, 0, count, total);
                        log_route_page_timing(
                            &Route::Albums,
                            "search",
                            0,
                            count,
                            total,
                            load_ms,
                            apply_started.elapsed().as_millis() as u64,
                            total_started.elapsed().as_millis() as u64,
                        );
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
                let total_started = Instant::now();
                let offset = cursor.offset.get();
                let text = query.borrow().clone();
                let load_started = Instant::now();
                match shell.controller.cached_albums_page_matching(
                    &text,
                    offset,
                    GRID_ROUTE_PAGE_SIZE,
                ) {
                    Ok(page) => {
                        let load_ms = load_started.elapsed().as_millis() as u64;
                        let apply_started = Instant::now();
                        let count = page.items.len();
                        let total = page.total;
                        let mut items = page.items;
                        let settings = shell.library_settings(LibraryListKey::Albums);
                        sort_albums(&mut items, &settings);
                        albums.borrow_mut().extend(items.iter().cloned());
                        *album_tracks.borrow_mut() =
                            shell.album_tracks_for_layout(&albums.borrow(), &settings);
                        warm_album_covers_for_settings(
                            &shell,
                            &albums.borrow(),
                            LibraryListKey::Albums,
                            &settings,
                        );
                        append_album_collection_model(
                            &model,
                            items,
                            &settings,
                            &album_tracks.borrow(),
                        );
                        finish_grid_page(&cursor, offset, count, total);
                        log_route_page_timing(
                            &Route::Albums,
                            "append",
                            offset,
                            count,
                            total,
                            load_ms,
                            apply_started.elapsed().as_millis() as u64,
                            total_started.elapsed().as_millis() as u64,
                        );
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
            empty_body: msgid("Cached entries will appear here after sync finishes"),
            search,
            content,
            load_next: if complete_page { None } else { Some(load_next) },
            configure_scroller: Some(configure_scroller),
        });
        let shell_ms = shell_started.elapsed().as_millis() as u64;
        let total_ms = view_started.elapsed().as_millis() as u64;
        info!(
            route = ?Route::Albums,
            layout = ?settings.layout,
            source = timing.source,
            albums = album_count,
            total = page_total,
            loaded_before_complete = timing.loaded_before_complete,
            complete_page,
            initial_load_ms = timing.initial_load_ms,
            complete_load_ms = timing.complete_load_ms,
            album_tracks_ms,
            model_ms,
            content_ms,
            shell_ms,
            total_ms,
            "library route setup timing"
        );
        if total_ms >= SLOW_LIBRARY_ROUTE_SETUP_MS {
            warn!(
                route = ?Route::Albums,
                layout = ?settings.layout,
                albums = album_count,
                total = page_total,
                total_ms,
                "slow library route setup"
            );
        }
        if settings.layout == LibraryLayout::Detail {
            info!(
                albums = album_count,
                total = page_total,
                initial_load_ms = timing.initial_load_ms,
                complete_load_ms = timing.complete_load_ms,
                album_tracks_ms,
                model_ms,
                content_ms,
                shell_ms,
                total_ms,
                "albums detail view timing"
            );
        }
        view
    }
    pub(in crate::ui) fn library_tracks_route_view(self: &Rc<Self>) -> gtk::Widget {
        let settings = self.library_settings(LibraryListKey::Tracks);
        if library_layout_loads_complete_page(LibraryListKey::Tracks, &settings)
            && let Some(page) = self.complete_track_snapshot_page()
        {
            return self.library_tracks_page(page.items, page.total);
        }

        let page = self
            .controller
            .cached_tracks_page(0, self.track_route_cache_limit())
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
                source::PagedResponse::new(tracks, self.state.library.borrow().cached_track_count)
            });
        self.library_tracks_page(page.items, page.total)
    }
    fn track_route_cache_limit(&self) -> usize {
        self.state
            .library
            .borrow()
            .cached_track_count
            .max(TRACK_ROUTE_PAGE_SIZE)
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
        let settings = self.library_settings(key);
        let initial_started = Instant::now();
        let mut source = "snapshot";
        let page = if let Some(page) = self.complete_artist_snapshot_page(album_artist) {
            page
        } else {
            source = "cache";
            self.controller
                .cached_artists_page(album_artist, 0, GRID_ROUTE_PAGE_SIZE)
                .unwrap_or_else(|error| {
                    source = "fallback";
                    warn!(%error, album_artist, "failed to load cached artists page");
                    let library = self.state.library.borrow();
                    let fallback = if album_artist {
                        &library.album_artists
                    } else {
                        &library.artists
                    };
                    source::PagedResponse::new(
                        fallback
                            .iter()
                            .take(GRID_ROUTE_PAGE_SIZE)
                            .cloned()
                            .collect(),
                        fallback.len(),
                    )
                })
        };
        let initial_load_ms = initial_started.elapsed().as_millis() as u64;
        let loaded_before_complete = page.items.len();
        let complete_started = Instant::now();
        let page = complete_cached_page(
            page,
            library_layout_loads_complete_page(key, &settings),
            |limit| self.controller.cached_artists_page(album_artist, 0, limit),
            "artists",
        );
        let timing = LibraryRouteLoadTiming {
            source,
            initial_load_ms,
            complete_load_ms: complete_started.elapsed().as_millis() as u64,
            loaded_before_complete,
        };
        self.library_artist_list_view_from_page(album_artist, page, timing)
    }

    pub(in crate::ui) fn library_artist_list_view_from_page(
        self: &Rc<Self>,
        album_artist: bool,
        page: source::PagedResponse<Artist>,
        timing: LibraryRouteLoadTiming,
    ) -> gtk::Widget {
        let view_started = Instant::now();
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
        let complete_page = page.items.len() >= page.total;
        let page_total = page.total;
        let source_artists = Rc::new(page.items.clone());
        let artists = Rc::new(RefCell::new(page.items));
        let artist_count = artists.borrow().len();
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let warm_started = Instant::now();
        warm_artist_covers_for_settings(self, &artists.borrow(), key, &settings);
        let warm_ms = warm_started.elapsed().as_millis() as u64;
        let model_started = Instant::now();
        populate_artist_model(&model, &artists.borrow(), &settings);
        let model_ms = model_started.elapsed().as_millis() as u64;

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
            let search_route = route.clone();
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
                        key,
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
                let total_started = Instant::now();
                let load_started = Instant::now();
                match shell.controller.cached_artists_page_matching(
                    album_artist,
                    &text,
                    0,
                    GRID_ROUTE_PAGE_SIZE,
                ) {
                    Ok(page) => {
                        let load_ms = load_started.elapsed().as_millis() as u64;
                        let apply_started = Instant::now();
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
                        let total = page.total;
                        *artists.borrow_mut() = page.items;
                        warm_artist_covers_for_settings(&shell, &artists.borrow(), key, &settings);
                        populate_artist_model(&model, &artists.borrow(), &settings);
                        finish_grid_page(&cursor, 0, count, total);
                        log_route_page_timing(
                            &search_route,
                            "search",
                            0,
                            count,
                            total,
                            load_ms,
                            apply_started.elapsed().as_millis() as u64,
                            total_started.elapsed().as_millis() as u64,
                        );
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
            let load_route = route.clone();
            Rc::new(move || {
                if !shell.can_load_grid_page(&cursor, &load_route) {
                    return;
                }
                let total_started = Instant::now();
                let offset = cursor.offset.get();
                let text = query.borrow().clone();
                let load_started = Instant::now();
                match shell.controller.cached_artists_page_matching(
                    album_artist,
                    &text,
                    offset,
                    GRID_ROUTE_PAGE_SIZE,
                ) {
                    Ok(page) => {
                        let load_ms = load_started.elapsed().as_millis() as u64;
                        let apply_started = Instant::now();
                        let count = page.items.len();
                        let total = page.total;
                        let mut items = page.items;
                        sort_artists(&mut items, &shell.library_settings(key));
                        warm_artist_covers_for_settings(
                            &shell,
                            &items,
                            key,
                            &shell.library_settings(key),
                        );
                        artists.borrow_mut().extend(items.iter().cloned());
                        append_artists_to_model(&model, items);
                        finish_grid_page(&cursor, offset, count, total);
                        log_route_page_timing(
                            &load_route,
                            "append",
                            offset,
                            count,
                            total,
                            load_ms,
                            apply_started.elapsed().as_millis() as u64,
                            total_started.elapsed().as_millis() as u64,
                        );
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
                connect_artist_viewport_cover_warm(&shell, scroller, &model, key, &settings);
            }) as Rc<dyn Fn(&gtk::ScrolledWindow)>
        };

        let content_started = Instant::now();
        let content = artist_collection_widget(self, model, key);
        let content_ms = content_started.elapsed().as_millis() as u64;
        let shell_started = Instant::now();
        let view = self.library_page_shell(LibraryPageShellOptions {
            key,
            empty: artists.borrow().is_empty(),
            empty_body: msgid("Cached entries will appear here after sync finishes"),
            search,
            content,
            load_next: if complete_page { None } else { Some(load_next) },
            configure_scroller: Some(configure_scroller),
        });
        let shell_ms = shell_started.elapsed().as_millis() as u64;
        let total_ms = view_started.elapsed().as_millis() as u64;
        info!(
            route = ?route,
            layout = ?settings.layout,
            source = timing.source,
            artists = artist_count,
            total = page_total,
            loaded_before_complete = timing.loaded_before_complete,
            complete_page,
            initial_load_ms = timing.initial_load_ms,
            complete_load_ms = timing.complete_load_ms,
            warm_ms,
            model_ms,
            content_ms,
            shell_ms,
            total_ms,
            "library route setup timing"
        );
        if total_ms >= SLOW_LIBRARY_ROUTE_SETUP_MS {
            warn!(
                route = ?route,
                layout = ?settings.layout,
                artists = artist_count,
                total = page_total,
                total_ms,
                "slow library route setup"
            );
        }
        view
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
                    source::PagedResponse::new(genres, self.state.library.borrow().genres.len())
                })
        });
        let page = complete_cached_page(
            page,
            library_layout_loads_complete_page(LibraryListKey::Genres, &settings),
            |limit| self.controller.cached_genres_page(0, limit),
            "genres",
        );
        self.library_genre_list_view_from_page(page)
    }

    pub(in crate::ui) fn library_genre_list_view_from_page(
        self: &Rc<Self>,
        page: source::PagedResponse<Genre>,
    ) -> gtk::Widget {
        let settings = self.library_settings(LibraryListKey::Genres);
        let complete_page = page.items.len() >= page.total;
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
                let total_started = Instant::now();
                let load_started = Instant::now();
                match shell
                    .controller
                    .cached_genres_page_matching(&text, 0, GRID_ROUTE_PAGE_SIZE)
                {
                    Ok(page) => {
                        let load_ms = load_started.elapsed().as_millis() as u64;
                        let apply_started = Instant::now();
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
                        let total = page.total;
                        *genres.borrow_mut() = page.items;
                        warm_genre_covers_for_settings(&shell, &genres.borrow(), &settings);
                        populate_genre_model(&model, &genres.borrow(), &settings);
                        finish_grid_page(&cursor, 0, count, total);
                        log_route_page_timing(
                            &Route::Genres,
                            "search",
                            0,
                            count,
                            total,
                            load_ms,
                            apply_started.elapsed().as_millis() as u64,
                            total_started.elapsed().as_millis() as u64,
                        );
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
                let total_started = Instant::now();
                let offset = cursor.offset.get();
                let text = query.borrow().clone();
                let load_started = Instant::now();
                match shell.controller.cached_genres_page_matching(
                    &text,
                    offset,
                    GRID_ROUTE_PAGE_SIZE,
                ) {
                    Ok(page) => {
                        let load_ms = load_started.elapsed().as_millis() as u64;
                        let apply_started = Instant::now();
                        let count = page.items.len();
                        let total = page.total;
                        let mut items = page.items;
                        sort_genres(&mut items, &shell.library_settings(LibraryListKey::Genres));
                        warm_genre_covers_for_settings(
                            &shell,
                            &items,
                            &shell.library_settings(LibraryListKey::Genres),
                        );
                        genres.borrow_mut().extend(items.iter().cloned());
                        append_genres_to_model(&model, items);
                        finish_grid_page(&cursor, offset, count, total);
                        log_route_page_timing(
                            &Route::Genres,
                            "append",
                            offset,
                            count,
                            total,
                            load_ms,
                            apply_started.elapsed().as_millis() as u64,
                            total_started.elapsed().as_millis() as u64,
                        );
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
            empty_body: msgid("Cached entries will appear here after sync finishes"),
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
                    source::PagedResponse::new(
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
        self.library_playlists_view_from_page(page)
    }

    pub(in crate::ui) fn library_playlists_view_from_page(
        self: &Rc<Self>,
        page: source::PagedResponse<Playlist>,
    ) -> gtk::Widget {
        let settings = self.library_settings(LibraryListKey::Playlists);
        let complete_page = page.items.len() >= page.total;
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
                let total_started = Instant::now();
                let load_started = Instant::now();
                match shell.controller.cached_playlists_page_matching(
                    &text,
                    0,
                    GRID_ROUTE_PAGE_SIZE,
                ) {
                    Ok(page) => {
                        let load_ms = load_started.elapsed().as_millis() as u64;
                        let apply_started = Instant::now();
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
                        let total = page.total;
                        *playlists.borrow_mut() = page.items;
                        warm_playlist_covers_for_settings(&shell, &playlists.borrow(), &settings);
                        populate_playlist_model(&model, &playlists.borrow(), &settings);
                        finish_grid_page(&cursor, 0, count, total);
                        log_route_page_timing(
                            &Route::Playlists,
                            "search",
                            0,
                            count,
                            total,
                            load_ms,
                            apply_started.elapsed().as_millis() as u64,
                            total_started.elapsed().as_millis() as u64,
                        );
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
                let total_started = Instant::now();
                let offset = cursor.offset.get();
                let text = query.borrow().clone();
                let load_started = Instant::now();
                match shell.controller.cached_playlists_page_matching(
                    &text,
                    offset,
                    GRID_ROUTE_PAGE_SIZE,
                ) {
                    Ok(page) => {
                        let load_ms = load_started.elapsed().as_millis() as u64;
                        let apply_started = Instant::now();
                        let count = page.items.len();
                        let total = page.total;
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
                        finish_grid_page(&cursor, offset, count, total);
                        log_route_page_timing(
                            &Route::Playlists,
                            "append",
                            offset,
                            count,
                            total,
                            load_ms,
                            apply_started.elapsed().as_millis() as u64,
                            total_started.elapsed().as_millis() as u64,
                        );
                    }
                    Err(error) => {
                        warn!(%error, "failed to append cached playlists page");
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
                connect_playlist_viewport_cover_warm(&shell, scroller, &model, &settings);
            }) as Rc<dyn Fn(&gtk::ScrolledWindow)>
        };

        self.library_page_shell(LibraryPageShellOptions {
            key: LibraryListKey::Playlists,
            empty: playlists.borrow().is_empty(),
            empty_body: msgid("Cached entries will appear here after sync finishes"),
            search,
            content: playlist_collection_widget(self, model),
            load_next: if complete_page { None } else { Some(load_next) },
            configure_scroller: Some(configure_scroller),
        })
    }
    pub(in crate::ui) fn library_smart_playlists_view(self: &Rc<Self>) -> gtk::Widget {
        let settings = self.library_settings(LibraryListKey::SmartPlaylists);
        let items = self.smart_playlists_for_route();
        self.state.smart_playlists.replace(items.clone());
        let source_playlists = Rc::new(items.clone());
        let playlists = Rc::new(RefCell::new(items));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        warm_smart_settings(self, &playlists.borrow(), &settings);
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
                warm_smart_settings(
                    &shell,
                    &playlists.borrow(),
                    &shell.library_settings(LibraryListKey::SmartPlaylists),
                );
                populate_smart_playlist_model(
                    &model,
                    &playlists.borrow(),
                    &shell.library_settings(LibraryListKey::SmartPlaylists),
                );
            });
        }

        let configure_scroller = {
            let shell = Rc::clone(self);
            let model = model.clone();
            let settings = settings.clone();
            Rc::new(move |scroller: &gtk::ScrolledWindow| {
                connect_smart_warm(&shell, scroller, &model, &settings);
            }) as Rc<dyn Fn(&gtk::ScrolledWindow)>
        };

        self.library_page_shell(LibraryPageShellOptions {
            key: LibraryListKey::SmartPlaylists,
            empty: playlists.borrow().is_empty(),
            empty_body: msgid("Smart playlists will appear here after the default set is seeded."),
            search,
            content: smart_playlist_collection_widget(self, model),
            load_next: None,
            configure_scroller: Some(configure_scroller),
        })
    }
    fn smart_playlists_for_route(&self) -> Vec<SmartPlaylist> {
        if self.state.smart_playlists_loaded.get() {
            return self.state.smart_playlists.borrow().clone();
        }
        let page = self
            .controller
            .cached_smart_playlists_page(0, 1_000)
            .unwrap_or_else(|error| {
                warn!(%error, "failed to load cached smart playlists page");
                source::PagedResponse::new(Vec::new(), 0)
            });
        self.state.smart_playlists_loaded.set(true);
        page.items
    }
    pub(in crate::ui) fn library_tracks_panel_with_source(
        self: &Rc<Self>,
        tracks: Vec<Track>,
        key: LibraryListKey,
        context: &str,
        source_descriptor: Option<PlaySourceDescriptor>,
        content_inset: i32,
    ) -> gtk::Widget {
        self.library_tracks_panel_with_source_options(
            tracks,
            key,
            context,
            source_descriptor,
            content_inset,
            None,
        )
    }
    pub(in crate::ui) fn compact_artist_tracks_table(
        self: &Rc<Self>,
        tracks: Vec<Track>,
        context: &str,
        source_descriptor: Option<PlaySourceDescriptor>,
    ) -> gtk::Widget {
        self.library_tracks_panel_with_source_options(
            tracks,
            LibraryListKey::ArtistTracks,
            context,
            source_descriptor,
            0,
            Some(5),
        )
    }
    fn library_tracks_panel_with_source_options(
        self: &Rc<Self>,
        tracks: Vec<Track>,
        key: LibraryListKey,
        context: &str,
        source_descriptor: Option<PlaySourceDescriptor>,
        content_inset: i32,
        max_visible_rows: Option<usize>,
    ) -> gtk::Widget {
        let scroller = gtk::ScrolledWindow::new();
        let resize_scroller = scroller.clone();
        let resize: Rc<dyn Fn(usize)> = Rc::new(move |row_count| {
            set_library_table_content_height(&resize_scroller, row_count, max_visible_rows);
        });
        let width_mode = if max_visible_rows.is_some() {
            ColumnViewWidthMode::EmbeddedScroller
        } else {
            ColumnViewWidthMode::Embedded
        };
        let (_empty, search, view, _model, _settings) = self.searchable_track_collection(
            tracks,
            key,
            Some(resize),
            source_descriptor,
            content_inset,
            width_mode,
        );
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 10);
        wrapper.set_widget_name(context);
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_width_request(1);
        let toolbar = self.library_toolbar(key, search.clone());
        toolbar.set_margin_end(DETAIL_ROUTE_SCROLL_GUTTER);
        wrapper.append(&toolbar);
        self.install_type_to_search(&search);
        if max_visible_rows.is_some() {
            configure_fill_width_clip(&scroller, gtk::PolicyType::Automatic);
            scroller.set_overlay_scrolling(false);
            scroller.set_margin_end(DETAIL_ROUTE_SCROLL_GUTTER);
            install_embedded_track_scroll_latch(&scroller);
        } else {
            scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
            scroller.set_width_request(1);
            scroller.set_min_content_width(0);
            scroller.set_max_content_width(1);
            scroller.set_propagate_natural_width(false);
        }
        scroller.set_width_request(1);
        scroller.set_hexpand(true);
        scroller.set_halign(gtk::Align::Fill);
        view.set_hexpand(true);
        view.set_halign(gtk::Align::Fill);
        scroller.set_child(Some(&non_propagating_width_clip(view)));
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
        let (_empty, search, view, model, settings) = self.searchable_track_collection(
            tracks,
            key,
            None,
            source_descriptor,
            content_margin_start,
            ColumnViewWidthMode::RouteScroller,
        );
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 10);
        wrapper.set_widget_name(context);
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_vexpand(true);
        let toolbar = self.library_toolbar(key, search.clone());
        toolbar.set_margin_start(content_margin_start);
        wrapper.append(&toolbar);
        self.install_type_to_search(&search);

        let scroller = gtk::ScrolledWindow::new();
        mark_route_scroll_owner(&scroller);
        configure_library_route_scroller(self, &scroller);
        connect_track_viewport_cover_warm(self, &scroller, &model, &settings);
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
        let (empty, search, view, model, settings) = self.searchable_track_collection(
            tracks,
            key,
            None,
            source_descriptor,
            0,
            ColumnViewWidthMode::RouteScroller,
        );
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 14);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(ROUTE_TOP_MARGIN);
        wrapper.set_margin_bottom(LIBRARY_ROUTE_BOTTOM_MARGIN);
        wrapper.set_hexpand(true);
        wrapper.set_vexpand(true);
        wrapper.set_widget_name(context);
        wrapper.append(&library_route_inset(
            self.library_toolbar(key, search.clone()),
        ));
        self.install_type_to_search(&search);

        if empty {
            wrapper.append(&library_route_inset(self.route_empty_view(empty_body)));
        } else {
            let scroller = gtk::ScrolledWindow::new();
            mark_route_scroll_owner(&scroller);
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
        content_inset: i32,
        width_mode: ColumnViewWidthMode,
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
        let visible_tracks = tracks_for_settings(source_tracks.as_ref(), &settings, "", false);
        let visible_count = visible_tracks.len();
        if track_route_tracks_key(key, width_mode).is_some() {
            self.state
                .route_track_refs
                .replace(track_image_refs(&visible_tracks));
        }
        warm_track_covers_for_settings(self, &visible_tracks, &settings);
        replace_tracks_in_model(&model, visible_tracks);
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
                let visible_tracks = tracks_for_settings(
                    source_tracks.as_ref(),
                    &settings,
                    entry.text().as_str(),
                    false,
                );
                let visible_count = visible_tracks.len();
                if track_route_tracks_key(key, width_mode).is_some() {
                    shell
                        .state
                        .route_track_refs
                        .replace(track_image_refs(&visible_tracks));
                }
                warm_track_covers_for_settings(&shell, &visible_tracks, &settings);
                replace_tracks_in_model(&model, visible_tracks);
                if let Some(on_visible_count_changed) = on_visible_count_changed.as_ref() {
                    on_visible_count_changed(visible_count);
                }
            });
        }
        let play_context = source_descriptor.map(|descriptor| {
            track_collection_play_context(self, descriptor, key, Rc::clone(&query), false)
        });
        let view = track_collection_widget(
            self,
            model.clone(),
            key,
            play_context,
            content_inset,
            width_mode,
        );
        (empty, search, view, model, settings)
    }
}

fn track_route_tracks_key(
    key: LibraryListKey,
    width_mode: ColumnViewWidthMode,
) -> Option<LibraryListKey> {
    if width_mode != ColumnViewWidthMode::RouteScroller {
        return None;
    }
    matches!(key, LibraryListKey::FavoriteTracks).then_some(key)
}

fn install_embedded_track_scroll_latch(scroller: &gtk::ScrolledWindow) {
    let last_parent_scroll = Rc::new(Cell::new(None::<Instant>));
    let connected_parent = Rc::new(Cell::new(false));
    let pointer_y = Rc::new(Cell::new(None::<f64>));

    let motion = gtk::EventControllerMotion::new();
    let motion_y = Rc::clone(&pointer_y);
    motion.connect_motion(move |_, _, y| {
        motion_y.set(Some(y));
    });
    let leave_y = Rc::clone(&pointer_y);
    motion.connect_leave(move |_| {
        leave_y.set(None);
    });
    scroller.add_controller(motion);

    let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
    let scroller_weak = scroller.downgrade();
    wheel.connect_scroll(move |controller, _, dy| {
        if dy == 0.0 {
            return gtk::glib::Propagation::Proceed;
        }

        let Some(scroller) = scroller_weak.upgrade() else {
            return gtk::glib::Propagation::Stop;
        };
        let Some(parent) =
            nearest_parent_scrolled_window(&scroller.clone().upcast::<gtk::Widget>())
        else {
            return gtk::glib::Propagation::Proceed;
        };
        if !connected_parent.get() {
            connected_parent.set(true);
            let last_parent_scroll = Rc::clone(&last_parent_scroll);
            parent.vadjustment().connect_value_changed(move |_| {
                last_parent_scroll.set(Some(Instant::now()));
            });
        }

        let unit = controller.unit();
        let parent_latched = parent_scroll_is_latched(Instant::now(), last_parent_scroll.get());
        if pointer_is_embedded_table_header(pointer_y.get())
            || parent_latched
            || !adjustment_can_scroll(&scroller.vadjustment(), dy, unit)
        {
            scroll_adjustment(&parent.vadjustment(), dy, unit);
            return gtk::glib::Propagation::Stop;
        }
        gtk::glib::Propagation::Proceed
    });
    scroller.add_controller(wheel);
}

fn parent_scroll_is_latched(now: Instant, last_parent_scroll: Option<Instant>) -> bool {
    last_parent_scroll
        .and_then(|last| now.checked_duration_since(last))
        .is_some_and(|elapsed| elapsed.as_millis() <= EMBEDDED_SCROLL_LATCH_MS)
}

fn pointer_is_embedded_table_header(y: Option<f64>) -> bool {
    y.is_some_and(|y| y >= 0.0 && y < f64::from(LIBRARY_TABLE_HEADER_HEIGHT))
}

fn adjustment_can_scroll(
    adjustment: &gtk::Adjustment,
    dy: f64,
    unit: gtk::gdk::ScrollUnit,
) -> bool {
    adjusted_scroll_value(adjustment, dy, unit)
        .is_some_and(|value| (value - adjustment.value()).abs() > f64::EPSILON)
}

fn scroll_adjustment(adjustment: &gtk::Adjustment, dy: f64, unit: gtk::gdk::ScrollUnit) {
    if let Some(value) = adjusted_scroll_value(adjustment, dy, unit) {
        adjustment.set_value(value);
    }
}

fn adjusted_scroll_value(
    adjustment: &gtk::Adjustment,
    dy: f64,
    unit: gtk::gdk::ScrollUnit,
) -> Option<f64> {
    let page_size = adjustment.page_size();
    let multiplier = match unit {
        gtk::gdk::ScrollUnit::Surface => EMBEDDED_SURFACE_SCROLL_FACTOR,
        _ => page_size.powf(2.0 / 3.0),
    };
    let max_value = (adjustment.upper() - page_size).max(adjustment.lower());
    let value = (adjustment.value() + dy * multiplier).clamp(adjustment.lower(), max_value);
    (value - adjustment.value())
        .abs()
        .gt(&f64::EPSILON)
        .then_some(value)
}

fn nearest_parent_scrolled_window(widget: &gtk::Widget) -> Option<gtk::ScrolledWindow> {
    let mut parent = widget.parent();
    while let Some(widget) = parent {
        if let Ok(scroller) = widget.clone().downcast::<gtk::ScrolledWindow>() {
            return Some(scroller);
        }
        parent = widget.parent();
    }
    None
}

#[cfg(test)]
mod embedded_scroll_tests {
    use super::*;

    #[test]
    fn parent_scroll_latch_expires() {
        let now = Instant::now();

        assert!(parent_scroll_is_latched(now, Some(now)));
        assert!(!parent_scroll_is_latched(
            now,
            Some(now - Duration::from_millis(EMBEDDED_SCROLL_LATCH_MS as u64 + 1))
        ));
        assert!(!parent_scroll_is_latched(now, None));
    }

    #[test]
    fn embedded_table_header_routes_to_parent() {
        assert!(pointer_is_embedded_table_header(Some(0.0)));
        assert!(pointer_is_embedded_table_header(Some(
            f64::from(LIBRARY_TABLE_HEADER_HEIGHT) - 1.0
        )));
        assert!(!pointer_is_embedded_table_header(Some(f64::from(
            LIBRARY_TABLE_HEADER_HEIGHT
        ))));
        assert!(!pointer_is_embedded_table_header(None));
    }
}
