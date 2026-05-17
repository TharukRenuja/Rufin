use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use rufin_core::{
    Album, AlbumId, Artist, Genre, LibraryField, LibraryLayout, LibraryListKey,
    LibraryListSettings, Track, available_grid_fields, available_row_fields, available_sort_fields,
    format_duration,
};
use tracing::warn;

use super::{
    GRID_COVER_SIZE, GRID_ROUTE_PAGE_SIZE, PRIMARY_ROUTE_MARGIN_END, PRIMARY_ROUTE_MARGIN_START,
    Route, Shell, THUMB_COVER_SIZE, TRACK_ROUTE_PAGE_SIZE, connect_paged_grid_loader,
    favorite_button_is_active, favorite_icon_button, finish_grid_page, replace_albums_in_model,
    replace_artists_in_model, replace_genres_in_model, set_favorite_button_active, stable_seed,
};
use crate::i18n::tr;

impl Shell {
    pub(super) fn library_albums_view(self: &Rc<Self>) -> gtk::Widget {
        let page = self
            .controller
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
                rufin_provider::PagedResponse::new(albums, self.state.library.borrow().albums.len())
            });
        let albums = Rc::new(RefCell::new(page.items));
        let album_tracks = Rc::new(RefCell::new(self.album_tracks_for(&albums.borrow())));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        populate_album_model(
            &model,
            &albums.borrow(),
            &self.library_settings(LibraryListKey::Albums),
        );

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
            let albums = Rc::clone(&albums);
            let album_tracks = Rc::clone(&album_tracks);
            let cursor = Rc::clone(&cursor);
            let query = Rc::clone(&query);
            search.connect_search_changed(move |entry| {
                let text = entry.text().trim().to_string();
                *query.borrow_mut() = text.clone();
                cursor.offset.set(0);
                cursor.total.set(usize::MAX);
                cursor.loading.set(true);
                match shell
                    .controller
                    .cached_albums_page_matching(&text, 0, GRID_ROUTE_PAGE_SIZE)
                {
                    Ok(page) => {
                        let count = page.items.len();
                        *albums.borrow_mut() = page.items;
                        *album_tracks.borrow_mut() = shell.album_tracks_for(&albums.borrow());
                        populate_album_model(
                            &model,
                            &albums.borrow(),
                            &shell.library_settings(LibraryListKey::Albums),
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
                        albums.borrow_mut().extend(page.items);
                        *album_tracks.borrow_mut() = shell.album_tracks_for(&albums.borrow());
                        populate_album_model(
                            &model,
                            &albums.borrow(),
                            &shell.library_settings(LibraryListKey::Albums),
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

        self.library_page_shell(
            LibraryListKey::Albums,
            albums.borrow().is_empty(),
            "Cached albums will appear here after the background sync finishes.",
            search,
            album_collection_widget(
                self,
                model,
                LibraryListKey::Albums,
                Rc::clone(&album_tracks),
            ),
            Some(load_next),
        )
    }

    pub(super) fn library_tracks_route_view(self: &Rc<Self>) -> gtk::Widget {
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
        self.library_tracks_page(page.items, page.total)
    }

    pub(super) fn library_artist_list_view(self: &Rc<Self>, album_artist: bool) -> gtk::Widget {
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
        let page = self
            .controller
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
            });
        let artists = Rc::new(RefCell::new(page.items));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        populate_artist_model(&model, &artists.borrow(), &self.library_settings(key));

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
            let artists = Rc::clone(&artists);
            let cursor = Rc::clone(&cursor);
            let query = Rc::clone(&query);
            search.connect_search_changed(move |entry| {
                let text = entry.text().trim().to_string();
                *query.borrow_mut() = text.clone();
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
                        let count = page.items.len();
                        *artists.borrow_mut() = page.items;
                        populate_artist_model(
                            &model,
                            &artists.borrow(),
                            &shell.library_settings(key),
                        );
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
                        artists.borrow_mut().extend(page.items);
                        populate_artist_model(
                            &model,
                            &artists.borrow(),
                            &shell.library_settings(key),
                        );
                        finish_grid_page(&cursor, offset, count, page.total);
                    }
                    Err(error) => {
                        warn!(%error, "failed to append cached artists page");
                        cursor.loading.set(false);
                    }
                }
            }) as Rc<dyn Fn()>
        };

        self.library_page_shell(
            key,
            artists.borrow().is_empty(),
            "Cached rows will appear here after the background sync finishes.",
            search,
            artist_collection_widget(self, model, key),
            Some(load_next),
        )
    }

    pub(super) fn library_genre_list_view(self: &Rc<Self>) -> gtk::Widget {
        let page = self
            .controller
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
                rufin_provider::PagedResponse::new(genres, self.state.library.borrow().genres.len())
            });
        let genres = Rc::new(RefCell::new(page.items));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        populate_genre_model(
            &model,
            &genres.borrow(),
            &self.library_settings(LibraryListKey::Genres),
        );

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
            let genres = Rc::clone(&genres);
            let cursor = Rc::clone(&cursor);
            let query = Rc::clone(&query);
            search.connect_search_changed(move |entry| {
                let text = entry.text().trim().to_string();
                *query.borrow_mut() = text.clone();
                cursor.offset.set(0);
                cursor.total.set(usize::MAX);
                cursor.loading.set(true);
                match shell
                    .controller
                    .cached_genres_page_matching(&text, 0, GRID_ROUTE_PAGE_SIZE)
                {
                    Ok(page) => {
                        let count = page.items.len();
                        *genres.borrow_mut() = page.items;
                        populate_genre_model(
                            &model,
                            &genres.borrow(),
                            &shell.library_settings(LibraryListKey::Genres),
                        );
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
                        genres.borrow_mut().extend(page.items);
                        populate_genre_model(
                            &model,
                            &genres.borrow(),
                            &shell.library_settings(LibraryListKey::Genres),
                        );
                        finish_grid_page(&cursor, offset, count, page.total);
                    }
                    Err(error) => {
                        warn!(%error, "failed to append cached genres page");
                        cursor.loading.set(false);
                    }
                }
            }) as Rc<dyn Fn()>
        };

        self.library_page_shell(
            LibraryListKey::Genres,
            genres.borrow().is_empty(),
            "Cached rows will appear here after the background sync finishes.",
            search,
            genre_collection_widget(self, model),
            Some(load_next),
        )
    }

    pub(super) fn library_tracks_panel(
        self: &Rc<Self>,
        tracks: Vec<Track>,
        key: LibraryListKey,
        context: &str,
    ) -> gtk::Widget {
        let source_tracks = Rc::new(tracks);
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        populate_track_model_for_settings(
            &model,
            &source_tracks,
            &self.library_settings(key),
            "",
            false,
        );
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(&tr("Search")));
        search.set_hexpand(true);
        {
            let shell = Rc::clone(self);
            let model = model.clone();
            let source_tracks = Rc::clone(&source_tracks);
            search.connect_search_changed(move |entry| {
                populate_track_model_for_settings(
                    &model,
                    &source_tracks,
                    &shell.library_settings(key),
                    entry.text().as_str(),
                    false,
                );
            });
        }
        let view = track_collection_widget(self, model, key);
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 10);
        wrapper.set_widget_name(context);
        wrapper.append(&self.library_toolbar(key, search));
        wrapper.append(&view);
        wrapper.upcast()
    }

    pub(super) fn library_album_collection_panel(
        self: &Rc<Self>,
        albums: &[Album],
        key: LibraryListKey,
        context: &str,
    ) -> gtk::Widget {
        let source_albums = Rc::new(albums.to_vec());
        let album_tracks = Rc::new(RefCell::new(self.album_tracks_for(&source_albums)));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        populate_album_model(&model, &source_albums, &self.library_settings(key));

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
                populate_album_model(&model, &albums, &shell.library_settings(key));
            });
        }

        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 10);
        wrapper.set_widget_name(context);
        wrapper.append(&self.library_toolbar(key, search));
        wrapper.append(&album_collection_widget(
            self,
            model,
            key,
            Rc::clone(&album_tracks),
        ));
        wrapper.upcast()
    }

    fn library_tracks_page(self: &Rc<Self>, tracks: Vec<Track>, total: usize) -> gtk::Widget {
        let tracks = Rc::new(RefCell::new(tracks));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        populate_track_model_for_settings(
            &model,
            &tracks.borrow(),
            &self.library_settings(LibraryListKey::Tracks),
            "",
            false,
        );
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
                cursor.offset.set(0);
                cursor.total.set(usize::MAX);
                cursor.loading.set(true);
                match shell
                    .controller
                    .cached_tracks_page_matching(&text, 0, TRACK_ROUTE_PAGE_SIZE)
                {
                    Ok(page) => {
                        let count = page.items.len();
                        *tracks.borrow_mut() = page.items;
                        populate_track_model_for_settings(
                            &model,
                            &tracks.borrow(),
                            &shell.library_settings(LibraryListKey::Tracks),
                            "",
                            false,
                        );
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
                        tracks.borrow_mut().extend(page.items);
                        populate_track_model_for_settings(
                            &model,
                            &tracks.borrow(),
                            &shell.library_settings(LibraryListKey::Tracks),
                            "",
                            false,
                        );
                        finish_grid_page(&cursor, offset, count, page.total);
                    }
                    Err(error) => {
                        warn!(%error, "failed to append cached tracks page");
                        cursor.loading.set(false);
                    }
                }
            }) as Rc<dyn Fn()>
        };
        self.library_page_shell(
            LibraryListKey::Tracks,
            tracks.borrow().is_empty(),
            "Cached tracks will appear here after the background sync finishes.",
            search,
            track_collection_widget(self, model, LibraryListKey::Tracks),
            Some(load_next),
        )
    }

    fn library_page_shell(
        self: &Rc<Self>,
        key: LibraryListKey,
        empty: bool,
        empty_body: &str,
        search: gtk::SearchEntry,
        content: gtk::Widget,
        load_next: Option<Rc<dyn Fn()>>,
    ) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 14);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(24);
        wrapper.set_margin_bottom(28);
        wrapper.set_margin_start(PRIMARY_ROUTE_MARGIN_START);
        wrapper.set_margin_end(PRIMARY_ROUTE_MARGIN_END);
        wrapper.set_vexpand(true);
        wrapper.append(&self.library_toolbar(key, search));

        if empty {
            wrapper.append(&self.route_empty_view(empty_body));
        } else {
            let scroller = gtk::ScrolledWindow::new();
            scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
            scroller.set_min_content_width(0);
            scroller.set_vexpand(true);
            scroller.set_child(Some(&content));
            if let Some(load_next) = load_next {
                connect_paged_grid_loader(&scroller, load_next);
            }
            wrapper.append(&scroller);
        }
        wrapper.upcast()
    }

    fn library_toolbar(
        self: &Rc<Self>,
        key: LibraryListKey,
        search: gtk::SearchEntry,
    ) -> gtk::Widget {
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        toolbar.add_css_class("track-toolbar");
        toolbar.append(&search);

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
        layout.set_tooltip_text(Some(&tr("Change layout")));
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

        let configure = gtk::MenuButton::new();
        configure.add_css_class("flat");
        configure.set_icon_name("view-more-symbolic");
        configure.set_tooltip_text(Some(&tr("Customize display")));
        configure.set_popover(Some(&self.library_config_popover(key)));
        toolbar.append(&configure);
        toolbar.upcast()
    }

    fn library_config_popover(self: &Rc<Self>, key: LibraryListKey) -> gtk::Popover {
        let settings = self.library_settings(key);
        let popover = gtk::Popover::new();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
        content.set_margin_top(12);
        content.set_margin_bottom(12);
        content.set_margin_start(12);
        content.set_margin_end(12);
        content.set_width_request(260);
        let heading = gtk::Label::new(Some(&tr(match settings.layout {
            LibraryLayout::Grid => "Grid labels",
            LibraryLayout::Detail => "Detail track columns",
            LibraryLayout::Row => "Columns",
        })));
        heading.add_css_class("muted");
        heading.set_xalign(0.0);
        content.append(&heading);

        let active = active_fields(&settings).to_vec();
        let available = available_fields_for_layout(key, settings.layout);
        let mut fields = active.clone();
        for field in available {
            if !fields.contains(field) {
                fields.push(*field);
            }
        }

        for field in fields {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            row.set_valign(gtk::Align::Center);
            let check = gtk::CheckButton::new();
            check.set_active(active.contains(&field));
            let label = gtk::Label::new(Some(&tr(field.title())));
            label.set_xalign(0.0);
            label.set_hexpand(true);
            let up = gtk::Button::from_icon_name("go-up-symbolic");
            let down = gtk::Button::from_icon_name("go-down-symbolic");
            up.add_css_class("flat");
            down.add_css_class("flat");
            up.set_sensitive(active.contains(&field));
            down.set_sensitive(active.contains(&field));

            {
                let shell = Rc::clone(self);
                check.connect_toggled(move |check| {
                    shell.update_library_list_settings(key, |settings| {
                        let fields = active_fields_mut(settings);
                        if check.is_active() {
                            if !fields.contains(&field) {
                                fields.push(field);
                            }
                        } else {
                            fields.retain(|candidate| *candidate != field);
                        }
                    });
                    shell.render_current_route_preserving_scroll();
                });
            }
            {
                let shell = Rc::clone(self);
                up.connect_clicked(move |_| {
                    shell.update_library_list_settings(key, |settings| {
                        move_field(active_fields_mut(settings), field, -1)
                    });
                    shell.render_current_route_preserving_scroll();
                });
            }
            {
                let shell = Rc::clone(self);
                down.connect_clicked(move |_| {
                    shell.update_library_list_settings(key, |settings| {
                        move_field(active_fields_mut(settings), field, 1)
                    });
                    shell.render_current_route_preserving_scroll();
                });
            }

            row.append(&check);
            row.append(&label);
            row.append(&up);
            row.append(&down);
            content.append(&row);
        }

        popover.set_child(Some(&content));
        popover
    }

    fn library_settings(&self, key: LibraryListKey) -> LibraryListSettings {
        self.state.settings.borrow().library_list(key)
    }

    fn album_tracks_for(&self, albums: &[Album]) -> HashMap<AlbumId, Vec<Track>> {
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
}

fn album_collection_widget(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
    album_tracks: Rc<RefCell<HashMap<AlbumId, Vec<Track>>>>,
) -> gtk::Widget {
    match shell.library_settings(key).layout {
        LibraryLayout::Row => album_table(shell, model, key).upcast(),
        LibraryLayout::Detail if key.supports_layout(LibraryLayout::Detail) => {
            album_detail_list(shell, model, key, album_tracks).upcast()
        }
        LibraryLayout::Grid | LibraryLayout::Detail => album_grid(shell, model, key).upcast(),
    }
}

fn artist_collection_widget(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
) -> gtk::Widget {
    match shell.library_settings(key).layout {
        LibraryLayout::Row => artist_table(shell, model, key).upcast(),
        LibraryLayout::Grid | LibraryLayout::Detail => artist_grid(shell, model, key).upcast(),
    }
}

fn genre_collection_widget(shell: &Rc<Shell>, model: gio::ListStore) -> gtk::Widget {
    match shell.library_settings(LibraryListKey::Genres).layout {
        LibraryLayout::Row => genre_table(shell, model).upcast(),
        LibraryLayout::Grid | LibraryLayout::Detail => genre_grid(shell, model).upcast(),
    }
}

fn track_collection_widget(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
) -> gtk::Widget {
    match shell.library_settings(key).layout {
        LibraryLayout::Grid => track_grid(shell, model, key).upcast(),
        LibraryLayout::Row | LibraryLayout::Detail => {
            track_table(shell, model, key, false).upcast()
        }
    }
}

fn album_grid(shell: &Rc<Shell>, model: gio::ListStore, key: LibraryListKey) -> gtk::GridView {
    let (columns, card_size) = shell.responsive_card_grid_metrics();
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    let factory = gtk::SignalListItemFactory::new();
    let shell_for_factory = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(boxed) = item
            .item()
            .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        else {
            return;
        };
        let album = boxed.borrow::<Album>();
        item.set_child(Some(&album_card(
            &shell_for_factory,
            &album,
            key,
            card_size,
        )));
    });
    factory.connect_unbind(clear_list_item_child);
    let grid = gtk::GridView::new(Some(selection), Some(factory));
    grid.add_css_class("album-grid");
    grid.set_min_columns(columns as u32);
    grid.set_max_columns(columns as u32);
    grid.set_single_click_activate(true);
    grid.set_hexpand(true);
    grid.set_vexpand(true);
    let shell = Rc::clone(shell);
    grid.connect_activate(move |_, position| {
        if let Some(album) = item_at::<Album>(&model, position) {
            shell.navigate(Route::AlbumDetail(album.id));
        }
    });
    grid
}

fn artist_grid(shell: &Rc<Shell>, model: gio::ListStore, key: LibraryListKey) -> gtk::GridView {
    let (columns, card_size) = shell.responsive_card_grid_metrics();
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    let factory = gtk::SignalListItemFactory::new();
    let shell_for_factory = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(boxed) = item
            .item()
            .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        else {
            return;
        };
        let artist = boxed.borrow::<Artist>();
        item.set_child(Some(&artist_card(
            &shell_for_factory,
            &artist,
            key,
            card_size,
        )));
    });
    factory.connect_unbind(clear_list_item_child);
    let grid = gtk::GridView::new(Some(selection), Some(factory));
    grid.add_css_class("album-grid");
    grid.set_min_columns(columns as u32);
    grid.set_max_columns(columns as u32);
    grid.set_single_click_activate(true);
    grid.set_hexpand(true);
    grid.set_vexpand(true);
    let shell = Rc::clone(shell);
    grid.connect_activate(move |_, position| {
        if let Some(artist) = item_at::<Artist>(&model, position) {
            shell.navigate(Route::ArtistDetail(artist.id));
        }
    });
    grid
}

fn genre_grid(shell: &Rc<Shell>, model: gio::ListStore) -> gtk::GridView {
    let (columns, card_size) = shell.responsive_card_grid_metrics();
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    let factory = gtk::SignalListItemFactory::new();
    let shell_for_factory = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(boxed) = item
            .item()
            .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        else {
            return;
        };
        let genre = boxed.borrow::<Genre>();
        item.set_child(Some(&genre_card(&shell_for_factory, &genre, card_size)));
    });
    factory.connect_unbind(clear_list_item_child);
    let grid = gtk::GridView::new(Some(selection), Some(factory));
    grid.add_css_class("album-grid");
    grid.set_min_columns(columns as u32);
    grid.set_max_columns(columns as u32);
    grid.set_single_click_activate(true);
    grid.set_hexpand(true);
    grid.set_vexpand(true);
    let shell = Rc::clone(shell);
    grid.connect_activate(move |_, position| {
        if let Some(genre) = item_at::<Genre>(&model, position) {
            shell.navigate(Route::GenreDetail(genre.id));
        }
    });
    grid
}

fn track_grid(shell: &Rc<Shell>, model: gio::ListStore, key: LibraryListKey) -> gtk::GridView {
    let (columns, card_size) = shell.responsive_card_grid_metrics();
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    let factory = gtk::SignalListItemFactory::new();
    let shell_for_factory = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(boxed) = item
            .item()
            .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        else {
            return;
        };
        let track = boxed.borrow::<Track>();
        item.set_child(Some(&track_card(
            &shell_for_factory,
            &track,
            key,
            card_size,
        )));
    });
    factory.connect_unbind(clear_list_item_child);
    let grid = gtk::GridView::new(Some(selection), Some(factory));
    grid.add_css_class("album-grid");
    grid.set_min_columns(columns as u32);
    grid.set_max_columns(columns as u32);
    grid.set_single_click_activate(true);
    grid.set_hexpand(true);
    grid.set_vexpand(true);
    let controller = shell.controller.clone();
    grid.connect_activate(move |_, position| {
        if let Some(track) = item_at::<Track>(&model, position) {
            controller.play_now(track);
        }
    });
    grid
}

fn album_table(shell: &Rc<Shell>, model: gio::ListStore, key: LibraryListKey) -> gtk::ColumnView {
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    let table = gtk::ColumnView::new(Some(selection));
    table.add_css_class("track-table");
    table.set_hexpand(true);
    table.set_vexpand(true);
    for field in shell.library_settings(key).row_fields {
        table.append_column(&album_column(shell, field));
    }
    let shell = Rc::clone(shell);
    table.connect_activate(move |_, position| {
        if let Some(album) = item_at::<Album>(&model, position) {
            shell.navigate(Route::AlbumDetail(album.id));
        }
    });
    table
}

fn artist_table(shell: &Rc<Shell>, model: gio::ListStore, key: LibraryListKey) -> gtk::ColumnView {
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    let table = gtk::ColumnView::new(Some(selection));
    table.add_css_class("track-table");
    table.set_hexpand(true);
    table.set_vexpand(true);
    for field in shell.library_settings(key).row_fields {
        table.append_column(&artist_column(shell, field));
    }
    let shell = Rc::clone(shell);
    table.connect_activate(move |_, position| {
        if let Some(artist) = item_at::<Artist>(&model, position) {
            shell.navigate(Route::ArtistDetail(artist.id));
        }
    });
    table
}

fn genre_table(shell: &Rc<Shell>, model: gio::ListStore) -> gtk::ColumnView {
    let selection = gtk::SingleSelection::new(Some(model));
    let table = gtk::ColumnView::new(Some(selection));
    table.add_css_class("track-table");
    table.set_hexpand(true);
    table.set_vexpand(true);
    for field in shell.library_settings(LibraryListKey::Genres).row_fields {
        table.append_column(&genre_column(field));
    }
    table
}

fn track_table(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
    detail: bool,
) -> gtk::ColumnView {
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    let table = gtk::ColumnView::new(Some(selection));
    table.add_css_class("track-table");
    table.set_hexpand(true);
    table.set_vexpand(true);
    let fields = if detail {
        shell.library_settings(key).detail_track_fields
    } else {
        shell.library_settings(key).row_fields
    };
    for field in fields {
        table.append_column(&track_column(shell, field));
    }
    let controller = shell.controller.clone();
    table.connect_activate(move |_, position| {
        if let Some(track) = item_at::<Track>(&model, position) {
            controller.play_now(track);
        }
    });
    table
}

fn album_detail_list(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
    album_tracks: Rc<RefCell<HashMap<AlbumId, Vec<Track>>>>,
) -> gtk::ListBox {
    let list = gtk::ListBox::new();
    list.add_css_class("track-table");
    list.set_selection_mode(gtk::SelectionMode::None);
    for position in 0..model.n_items() {
        let Some(album) = item_at::<Album>(&model, position) else {
            continue;
        };
        let tracks = album_tracks
            .borrow()
            .get(&album.id)
            .cloned()
            .unwrap_or_default();
        list.append(&album_detail_row(shell, &album, tracks, key));
    }
    list
}

fn album_detail_row(
    shell: &Rc<Shell>,
    album: &Album,
    tracks: Vec<Track>,
    key: LibraryListKey,
) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    row.add_css_class("album-detail-row");
    row.set_margin_top(12);
    row.set_margin_bottom(16);
    row.set_margin_start(4);
    row.set_margin_end(4);

    let meta = gtk::Box::new(gtk::Orientation::Vertical, 6);
    meta.set_width_request(240);
    meta.append(&shell.cover_tile_for(
        album.image_ref.as_ref(),
        album.color_seed,
        220,
        super::DETAIL_COVER_SIZE,
    ));
    meta.append(&center_label(&album.title, "track-title"));
    meta.append(&center_label(&album.artist, "muted"));
    meta.append(&center_label(&album_fact_text(album), "muted"));
    if !album.genres.is_empty() {
        meta.append(&center_label(&album.genres.join(", "), "muted"));
    }
    row.append(&meta);

    let model = gio::ListStore::new::<glib::BoxedAnyObject>();
    populate_track_model_for_settings(
        &model,
        &tracks,
        &LibraryListSettings {
            row_fields: shell.library_settings(key).detail_track_fields.clone(),
            ..shell.library_settings(key)
        },
        "",
        false,
    );
    let table = track_table(shell, model, key, true);
    table.set_vexpand(false);
    row.append(&table);
    row.upcast()
}

fn album_card(shell: &Rc<Shell>, album: &Album, key: LibraryListKey, size: i32) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.set_width_request(size);
    card.append(&shell.cover_tile_for(
        album.image_ref.as_ref(),
        album.color_seed,
        size,
        GRID_COVER_SIZE,
    ));
    card.append(&center_label(&album.title, "track-title"));
    for field in shell.library_settings(key).grid_fields {
        let value = album_field(album, field);
        if !value.is_empty() {
            card.append(&center_label(&value, "muted"));
        }
    }
    card.upcast()
}

fn artist_card(shell: &Rc<Shell>, artist: &Artist, key: LibraryListKey, size: i32) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.set_width_request(size);
    card.append(&shell.cover_tile_for(
        artist.image_ref.as_ref(),
        stable_seed(artist.id.as_str()),
        size,
        GRID_COVER_SIZE,
    ));
    card.append(&center_label(&artist.name, "track-title"));
    for field in shell.library_settings(key).grid_fields {
        let value = artist_field(artist, field);
        if !value.is_empty() {
            card.append(&center_label(&value, "muted"));
        }
    }
    card.upcast()
}

fn genre_card(shell: &Rc<Shell>, genre: &Genre, size: i32) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.set_width_request(size);
    card.append(&shell.cover_tile_for(
        genre.image_ref.as_ref(),
        stable_seed(genre.id.as_str()),
        size,
        GRID_COVER_SIZE,
    ));
    card.append(&center_label(&genre.name, "track-title"));
    for field in shell.library_settings(LibraryListKey::Genres).grid_fields {
        let value = genre_field(genre, field);
        if !value.is_empty() {
            card.append(&center_label(&value, "muted"));
        }
    }
    card.upcast()
}

fn track_card(shell: &Rc<Shell>, track: &Track, key: LibraryListKey, size: i32) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.set_width_request(size);
    card.append(&shell.cover_tile_for(
        track.image_ref.as_ref(),
        stable_seed(track.id.as_str()),
        size,
        GRID_COVER_SIZE,
    ));
    card.append(&center_label(&track.title, "track-title"));
    for field in shell.library_settings(key).grid_fields {
        let value = track_field(track, field);
        if !value.is_empty() {
            card.append(&center_label(&value, "muted"));
        }
    }
    card.upcast()
}

fn album_column(shell: &Rc<Shell>, field: LibraryField) -> gtk::ColumnViewColumn {
    match field {
        LibraryField::RowIndex => row_index_column(),
        LibraryField::Image => image_column::<Album, _, _>(
            shell,
            "Image",
            64,
            |album| album.image_ref.clone(),
            |album| album.color_seed,
        ),
        LibraryField::TitleMerged => merged_column::<Album, _, _, _, _>(
            shell,
            "Title",
            320,
            |album| album.title.clone(),
            |album| album.artist.clone(),
            |album| album.image_ref.clone(),
            |album| album.color_seed,
        ),
        LibraryField::Favorite => album_favorite_column(shell),
        _ => text_column::<Album, _>(field.title(), column_width(field), move |album| {
            album_field(album, field)
        }),
    }
}

fn artist_column(shell: &Rc<Shell>, field: LibraryField) -> gtk::ColumnViewColumn {
    match field {
        LibraryField::RowIndex => row_index_column(),
        LibraryField::Image => image_column::<Artist, _, _>(
            shell,
            "Image",
            64,
            |artist| artist.image_ref.clone(),
            |artist| stable_seed(artist.id.as_str()),
        ),
        LibraryField::TitleMerged | LibraryField::Title => {
            text_column::<Artist, _>("Title", 260, |artist| artist.name.clone())
        }
        LibraryField::Favorite => artist_favorite_column(shell),
        _ => text_column::<Artist, _>(field.title(), column_width(field), move |artist| {
            artist_field(artist, field)
        }),
    }
}

fn genre_column(field: LibraryField) -> gtk::ColumnViewColumn {
    match field {
        LibraryField::RowIndex => row_index_column(),
        _ => text_column::<Genre, _>(field.title(), column_width(field), move |genre| {
            genre_field(genre, field)
        }),
    }
}

fn track_column(shell: &Rc<Shell>, field: LibraryField) -> gtk::ColumnViewColumn {
    match field {
        LibraryField::RowIndex => row_index_column(),
        LibraryField::Image => image_column::<Track, _, _>(
            shell,
            "Image",
            64,
            |track| track.image_ref.clone(),
            |track| stable_seed(track.id.as_str()),
        ),
        LibraryField::TitleMerged => merged_column::<Track, _, _, _, _>(
            shell,
            "Title",
            320,
            |track| track.title.clone(),
            |track| track.artist.clone(),
            |track| track.image_ref.clone(),
            |track| stable_seed(track.id.as_str()),
        ),
        LibraryField::Favorite => track_favorite_column(shell),
        _ => text_column::<Track, _>(field.title(), column_width(field), move |track| {
            track_field(track, field)
        }),
    }
}

fn text_column<T, F>(title: &str, width: i32, value: F) -> gtk::ColumnViewColumn
where
    T: Clone + 'static,
    F: Fn(&T) -> String + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let value = Rc::new(value);
    factory.connect_setup(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let label = gtk::Label::new(None);
            label.set_xalign(0.0);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            item.set_child(Some(&label));
        }
    });
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(label) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        else {
            return;
        };
        let Some(boxed) = item
            .item()
            .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        else {
            return;
        };
        let data = boxed.borrow::<T>();
        label.set_text(&(value)(&data));
    });
    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(true);
    column
}

fn row_index_column() -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let label = gtk::Label::new(None);
            label.add_css_class("muted");
            label.set_xalign(0.0);
            item.set_child(Some(&label));
        }
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(label) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        else {
            return;
        };
        label.set_text(&(item.position() + 1).to_string());
    });
    let column = gtk::ColumnViewColumn::new(Some("#"), Some(factory));
    column.set_fixed_width(54);
    column
}

fn image_column<T, F, S>(
    shell: &Rc<Shell>,
    title: &str,
    width: i32,
    image_ref: F,
    seed: S,
) -> gtk::ColumnViewColumn
where
    T: Clone + 'static,
    F: Fn(&T) -> Option<rufin_core::ImageRef> + 'static,
    S: Fn(&T) -> u32 + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    let image_ref = Rc::new(image_ref);
    let seed = Rc::new(seed);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(boxed) = item
            .item()
            .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        else {
            return;
        };
        let data = boxed.borrow::<T>();
        item.set_child(Some(&shell.cover_tile_for(
            image_ref(&data).as_ref(),
            seed(&data),
            48,
            THUMB_COVER_SIZE,
        )));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column
}

fn merged_column<T, Title, Subtitle, Image, Seed>(
    shell: &Rc<Shell>,
    title: &str,
    width: i32,
    title_value: Title,
    subtitle_value: Subtitle,
    image_ref: Image,
    seed: Seed,
) -> gtk::ColumnViewColumn
where
    T: Clone + 'static,
    Title: Fn(&T) -> String + 'static,
    Subtitle: Fn(&T) -> String + 'static,
    Image: Fn(&T) -> Option<rufin_core::ImageRef> + 'static,
    Seed: Fn(&T) -> u32 + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    let title_value = Rc::new(title_value);
    let subtitle_value = Rc::new(subtitle_value);
    let image_ref = Rc::new(image_ref);
    let seed = Rc::new(seed);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(boxed) = item
            .item()
            .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        else {
            return;
        };
        let data = boxed.borrow::<T>();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.set_valign(gtk::Align::Center);
        row.append(&shell.cover_tile_for(
            image_ref(&data).as_ref(),
            seed(&data),
            48,
            THUMB_COVER_SIZE,
        ));
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let title = gtk::Label::new(Some(&title_value(&data)));
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        labels.append(&title);
        let subtitle = subtitle_value(&data);
        if !subtitle.trim().is_empty() {
            let subtitle = gtk::Label::new(Some(&subtitle));
            subtitle.add_css_class("muted");
            subtitle.set_xalign(0.0);
            subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
            labels.append(&subtitle);
        }
        row.append(&labels);
        item.set_child(Some(&row));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(true);
    column
}

fn album_favorite_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(album) = item_at_from_item::<Album>(item) else {
            return;
        };
        let button = favorite_icon_button("Favorite album");
        set_favorite_button_active(&button, album.favorite);
        let controller = shell.controller.clone();
        button.connect_clicked(move |button| {
            controller.set_album_favorite(album.id.clone(), !favorite_button_is_active(button));
        });
        item.set_child(Some(&button));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(Some(""), Some(factory));
    column.set_fixed_width(64);
    column
}

fn artist_favorite_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(artist) = item_at_from_item::<Artist>(item) else {
            return;
        };
        let button = favorite_icon_button("Favorite artist");
        set_favorite_button_active(&button, artist.favorite);
        let controller = shell.controller.clone();
        button.connect_clicked(move |button| {
            controller.set_artist_favorite(artist.id.clone(), !favorite_button_is_active(button));
        });
        item.set_child(Some(&button));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(Some(""), Some(factory));
    column.set_fixed_width(64);
    column
}

fn track_favorite_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(track) = item_at_from_item::<Track>(item) else {
            return;
        };
        let button = favorite_icon_button("Favorite track");
        set_favorite_button_active(&button, track.favorite);
        let controller = shell.controller.clone();
        button.connect_clicked(move |button| {
            controller.set_track_favorite(track.id.clone(), !favorite_button_is_active(button));
        });
        item.set_child(Some(&button));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(Some(""), Some(factory));
    column.set_fixed_width(64);
    column
}

fn populate_album_model(model: &gio::ListStore, albums: &[Album], settings: &LibraryListSettings) {
    let mut values = albums.to_vec();
    sort_albums(&mut values, settings);
    replace_albums_in_model(model, values);
}

fn populate_artist_model(
    model: &gio::ListStore,
    artists: &[Artist],
    settings: &LibraryListSettings,
) {
    let mut values = artists.to_vec();
    sort_artists(&mut values, settings);
    replace_artists_in_model(model, values);
}

fn populate_genre_model(model: &gio::ListStore, genres: &[Genre], settings: &LibraryListSettings) {
    let mut values = genres.to_vec();
    sort_genres(&mut values, settings);
    replace_genres_in_model(model, values);
}

fn populate_track_model_for_settings(
    model: &gio::ListStore,
    tracks: &[Track],
    settings: &LibraryListSettings,
    query: &str,
    favorite_first: bool,
) {
    let query = query.trim().to_lowercase();
    let mut values = tracks
        .iter()
        .filter(|track| query.is_empty() || track_matches_query(track, &query))
        .cloned()
        .collect::<Vec<_>>();
    sort_tracks(&mut values, settings, favorite_first);
    replace_tracks_in_model(model, values);
}

fn sort_albums(albums: &mut [Album], settings: &LibraryListSettings) {
    albums.sort_by(|left, right| {
        let missing = album_field_missing(left, settings.sort_key)
            .cmp(&album_field_missing(right, settings.sort_key));
        if missing != Ordering::Equal {
            return missing;
        }
        apply_desc(
            compare_album(left, right, settings.sort_key),
            settings.descending,
        )
    });
}

fn sort_artists(artists: &mut [Artist], settings: &LibraryListSettings) {
    artists.sort_by(|left, right| {
        let missing = artist_field_missing(left, settings.sort_key)
            .cmp(&artist_field_missing(right, settings.sort_key));
        if missing != Ordering::Equal {
            return missing;
        }
        apply_desc(
            compare_artist(left, right, settings.sort_key),
            settings.descending,
        )
    });
}

fn sort_genres(genres: &mut [Genre], settings: &LibraryListSettings) {
    genres.sort_by(|left, right| {
        apply_desc(
            compare_genre(left, right, settings.sort_key),
            settings.descending,
        )
    });
}

fn sort_tracks(tracks: &mut [Track], settings: &LibraryListSettings, favorite_first: bool) {
    tracks.sort_by(|left, right| {
        if favorite_first {
            let favorite = right.favorite.cmp(&left.favorite);
            if favorite != Ordering::Equal {
                return favorite;
            }
        }
        let missing = track_field_missing(left, settings.sort_key)
            .cmp(&track_field_missing(right, settings.sort_key));
        if missing != Ordering::Equal {
            return missing;
        }
        apply_desc(
            compare_track(left, right, settings.sort_key),
            settings.descending,
        )
    });
}

fn compare_album(left: &Album, right: &Album, field: LibraryField) -> Ordering {
    match field {
        LibraryField::AlbumArtist => cmp_string(&left.artist, &right.artist),
        LibraryField::Year => left.year.cmp(&right.year),
        LibraryField::ReleaseDate => cmp_option_string(&left.release_date, &right.release_date),
        LibraryField::DateAdded => cmp_option_string(&left.date_added, &right.date_added),
        LibraryField::LastPlayed => cmp_option_string(&left.last_played, &right.last_played),
        LibraryField::PlayCount => cmp_option_u32(left.play_count, right.play_count),
        LibraryField::UserRating => cmp_option_u8(left.user_rating, right.user_rating),
        LibraryField::SongCount => left.track_count.cmp(&right.track_count),
        LibraryField::Duration => left.duration_seconds.cmp(&right.duration_seconds),
        LibraryField::Favorite => left.favorite.cmp(&right.favorite),
        _ => cmp_string(&left.title, &right.title),
    }
    .then_with(|| cmp_string(&left.title, &right.title))
}

fn compare_artist(left: &Artist, right: &Artist, field: LibraryField) -> Ordering {
    match field {
        LibraryField::AlbumCount => left.album_count.cmp(&right.album_count),
        LibraryField::SongCount => left.track_count.cmp(&right.track_count),
        LibraryField::LastPlayed => cmp_option_string(&left.last_played, &right.last_played),
        LibraryField::PlayCount => cmp_option_u32(left.play_count, right.play_count),
        LibraryField::UserRating => cmp_option_u8(left.user_rating, right.user_rating),
        LibraryField::Favorite => left.favorite.cmp(&right.favorite),
        _ => cmp_string(&left.name, &right.name),
    }
    .then_with(|| cmp_string(&left.name, &right.name))
}

fn compare_genre(left: &Genre, right: &Genre, field: LibraryField) -> Ordering {
    match field {
        LibraryField::AlbumCount => left.album_count.cmp(&right.album_count),
        LibraryField::SongCount => left.track_count.cmp(&right.track_count),
        _ => cmp_string(&left.name, &right.name),
    }
    .then_with(|| cmp_string(&left.name, &right.name))
}

fn compare_track(left: &Track, right: &Track, field: LibraryField) -> Ordering {
    match field {
        LibraryField::TrackNumber => left
            .disc_number
            .cmp(&right.disc_number)
            .then(left.track_number.cmp(&right.track_number)),
        LibraryField::Artist => cmp_string(&left.artist, &right.artist),
        LibraryField::AlbumArtist => cmp_string(
            &joined_credits(&left.album_artist_credits),
            &joined_credits(&right.album_artist_credits),
        ),
        LibraryField::Album => cmp_string(&left.album, &right.album),
        LibraryField::Year => left.year.cmp(&right.year),
        LibraryField::ReleaseDate => cmp_option_string(&left.release_date, &right.release_date),
        LibraryField::DateAdded => cmp_option_string(&left.date_added, &right.date_added),
        LibraryField::LastPlayed => cmp_option_string(&left.last_played, &right.last_played),
        LibraryField::PlayCount => cmp_option_u32(left.play_count, right.play_count),
        LibraryField::UserRating => cmp_option_u8(left.user_rating, right.user_rating),
        LibraryField::Genre => cmp_string(&left.genres.join(", "), &right.genres.join(", ")),
        LibraryField::Duration => left.duration_seconds.cmp(&right.duration_seconds),
        LibraryField::Favorite => left.favorite.cmp(&right.favorite),
        _ => cmp_string(&left.title, &right.title),
    }
    .then_with(|| cmp_string(&left.album, &right.album))
    .then(left.disc_number.cmp(&right.disc_number))
    .then(left.track_number.cmp(&right.track_number))
    .then_with(|| cmp_string(&left.title, &right.title))
}

fn album_field_missing(album: &Album, field: LibraryField) -> bool {
    match field {
        LibraryField::ReleaseDate => album.release_date.is_none(),
        LibraryField::DateAdded => album.date_added.is_none(),
        LibraryField::LastPlayed => album.last_played.is_none(),
        LibraryField::PlayCount => album.play_count.is_none(),
        LibraryField::UserRating => album.user_rating.is_none(),
        _ => false,
    }
}

fn artist_field_missing(artist: &Artist, field: LibraryField) -> bool {
    match field {
        LibraryField::LastPlayed => artist.last_played.is_none(),
        LibraryField::PlayCount => artist.play_count.is_none(),
        LibraryField::UserRating => artist.user_rating.is_none(),
        _ => false,
    }
}

fn track_field_missing(track: &Track, field: LibraryField) -> bool {
    match field {
        LibraryField::ReleaseDate => track.release_date.is_none(),
        LibraryField::DateAdded => track.date_added.is_none(),
        LibraryField::LastPlayed => track.last_played.is_none(),
        LibraryField::PlayCount => track.play_count.is_none(),
        LibraryField::UserRating => track.user_rating.is_none(),
        _ => false,
    }
}

fn album_field(album: &Album, field: LibraryField) -> String {
    match field {
        LibraryField::Title | LibraryField::TitleMerged => album.title.clone(),
        LibraryField::AlbumArtist | LibraryField::Artist => album.artist.clone(),
        LibraryField::Year => nonzero_year(album.year),
        LibraryField::ReleaseDate => album.release_date.clone().unwrap_or_default(),
        LibraryField::DateAdded => album.date_added.clone().unwrap_or_default(),
        LibraryField::LastPlayed => album.last_played.clone().unwrap_or_default(),
        LibraryField::PlayCount => option_count(album.play_count),
        LibraryField::UserRating => option_rating(album.user_rating),
        LibraryField::Genre => album.genres.join(", "),
        LibraryField::SongCount => format!("{} {}", album.track_count, tr("tracks")),
        LibraryField::Duration => format_duration(album.duration_seconds),
        LibraryField::Favorite => favorite_text(album.favorite),
        _ => String::new(),
    }
}

fn artist_field(artist: &Artist, field: LibraryField) -> String {
    match field {
        LibraryField::Title | LibraryField::TitleMerged => artist.name.clone(),
        LibraryField::AlbumCount => format!("{} {}", artist.album_count, tr("albums")),
        LibraryField::SongCount => format!("{} {}", artist.track_count, tr("tracks")),
        LibraryField::LastPlayed => artist.last_played.clone().unwrap_or_default(),
        LibraryField::PlayCount => option_count(artist.play_count),
        LibraryField::UserRating => option_rating(artist.user_rating),
        LibraryField::Favorite => favorite_text(artist.favorite),
        _ => String::new(),
    }
}

fn genre_field(genre: &Genre, field: LibraryField) -> String {
    match field {
        LibraryField::Title | LibraryField::TitleMerged => genre.name.clone(),
        LibraryField::AlbumCount => format!("{} {}", genre.album_count, tr("albums")),
        LibraryField::SongCount => format!("{} {}", genre.track_count, tr("tracks")),
        _ => String::new(),
    }
}

fn track_field(track: &Track, field: LibraryField) -> String {
    match field {
        LibraryField::Title | LibraryField::TitleMerged => track.title.clone(),
        LibraryField::Artist => track.artist.clone(),
        LibraryField::AlbumArtist => joined_credits(&track.album_artist_credits),
        LibraryField::Album => track.album.clone(),
        LibraryField::Year => nonzero_year(track.year),
        LibraryField::ReleaseDate => track.release_date.clone().unwrap_or_default(),
        LibraryField::DateAdded => track.date_added.clone().unwrap_or_default(),
        LibraryField::LastPlayed => track.last_played.clone().unwrap_or_default(),
        LibraryField::PlayCount => option_count(track.play_count),
        LibraryField::UserRating => option_rating(track.user_rating),
        LibraryField::Genre => track.genres.join(", "),
        LibraryField::DiscNumber => track.disc_number.to_string(),
        LibraryField::TrackNumber => format!("{}-{:02}", track.disc_number, track.track_number),
        LibraryField::Duration => format_duration(track.duration_seconds),
        LibraryField::Favorite => favorite_text(track.favorite),
        _ => String::new(),
    }
}

fn track_matches_query(track: &Track, query: &str) -> bool {
    track.title.to_lowercase().contains(query)
        || track.artist.to_lowercase().contains(query)
        || joined_credits(&track.album_artist_credits)
            .to_lowercase()
            .contains(query)
        || track.album.to_lowercase().contains(query)
        || track.genres.join(" ").to_lowercase().contains(query)
        || track.year.to_string().contains(query)
}

fn album_matches_query(album: &Album, query: &str) -> bool {
    album.title.to_lowercase().contains(query)
        || album.artist.to_lowercase().contains(query)
        || album.genres.join(" ").to_lowercase().contains(query)
        || album.year.to_string().contains(query)
}

fn item_at<T: Clone + 'static>(model: &gio::ListStore, position: u32) -> Option<T> {
    model
        .item(position)
        .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        .map(|boxed| boxed.borrow::<T>().clone())
}

fn item_at_from_item<T: Clone + 'static>(item: &gtk::ListItem) -> Option<T> {
    item.item()
        .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        .map(|boxed| boxed.borrow::<T>().clone())
}

fn clear_list_item_child(_: &gtk::SignalListItemFactory, item: &glib::Object) {
    if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
        item.set_child(None::<&gtk::Widget>);
    }
}

fn replace_tracks_in_model(model: &gio::ListStore, tracks: Vec<Track>) {
    let additions = tracks
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}

fn center_label(text: &str, css_class: &str) -> gtk::Widget {
    let label = gtk::Label::new(Some(text));
    if !css_class.is_empty() {
        label.add_css_class(css_class);
    }
    label.set_xalign(0.5);
    label.set_wrap(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.upcast()
}

fn album_fact_text(album: &Album) -> String {
    format!(
        "{} • {} {} • {}",
        nonzero_year(album.year),
        album.track_count,
        tr("tracks"),
        format_duration(album.duration_seconds)
    )
}

fn active_fields(settings: &LibraryListSettings) -> &[LibraryField] {
    match settings.layout {
        LibraryLayout::Grid => &settings.grid_fields,
        LibraryLayout::Detail => &settings.detail_track_fields,
        LibraryLayout::Row => &settings.row_fields,
    }
}

fn active_fields_mut(settings: &mut LibraryListSettings) -> &mut Vec<LibraryField> {
    match settings.layout {
        LibraryLayout::Grid => &mut settings.grid_fields,
        LibraryLayout::Detail => &mut settings.detail_track_fields,
        LibraryLayout::Row => &mut settings.row_fields,
    }
}

fn available_fields_for_layout(
    key: LibraryListKey,
    layout: LibraryLayout,
) -> &'static [LibraryField] {
    match layout {
        LibraryLayout::Grid => available_grid_fields(key),
        LibraryLayout::Detail => available_row_fields(LibraryListKey::Tracks),
        LibraryLayout::Row => available_row_fields(key),
    }
}

fn move_field(fields: &mut [LibraryField], field: LibraryField, delta: isize) {
    let Some(index) = fields.iter().position(|candidate| *candidate == field) else {
        return;
    };
    let new_index = if delta < 0 {
        index.saturating_sub(1)
    } else {
        (index + 1).min(fields.len().saturating_sub(1))
    };
    fields.swap(index, new_index);
}

fn next_layout(key: LibraryListKey, layout: LibraryLayout) -> LibraryLayout {
    if key.supports_layout(LibraryLayout::Detail) {
        match layout {
            LibraryLayout::Grid => LibraryLayout::Detail,
            LibraryLayout::Detail => LibraryLayout::Row,
            LibraryLayout::Row => LibraryLayout::Grid,
        }
    } else {
        match layout {
            LibraryLayout::Grid => LibraryLayout::Row,
            LibraryLayout::Row | LibraryLayout::Detail => LibraryLayout::Grid,
        }
    }
}

fn layout_icon(layout: LibraryLayout) -> &'static str {
    match layout {
        LibraryLayout::Grid => "view-grid-symbolic",
        LibraryLayout::Row => "view-list-symbolic",
        LibraryLayout::Detail => "view-sidebar-symbolic",
    }
}

fn column_width(field: LibraryField) -> i32 {
    match field {
        LibraryField::RowIndex => 54,
        LibraryField::Image | LibraryField::Favorite => 64,
        LibraryField::Title | LibraryField::TitleMerged => 300,
        LibraryField::Album
        | LibraryField::Artist
        | LibraryField::AlbumArtist
        | LibraryField::Genre => 220,
        LibraryField::ReleaseDate | LibraryField::DateAdded | LibraryField::LastPlayed => 130,
        LibraryField::PlayCount
        | LibraryField::UserRating
        | LibraryField::SongCount
        | LibraryField::AlbumCount => 110,
        LibraryField::Year | LibraryField::DiscNumber | LibraryField::TrackNumber => 80,
        LibraryField::Duration => 90,
    }
}

fn apply_desc(ordering: Ordering, descending: bool) -> Ordering {
    if descending {
        ordering.reverse()
    } else {
        ordering
    }
}

fn cmp_string(left: &str, right: &str) -> Ordering {
    left.to_lowercase().cmp(&right.to_lowercase())
}

fn cmp_option_string(left: &Option<String>, right: &Option<String>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => cmp_string(left, right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn cmp_option_u32(left: Option<u32>, right: Option<u32>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn cmp_option_u8(left: Option<u8>, right: Option<u8>) -> Ordering {
    cmp_option_u32(left.map(u32::from), right.map(u32::from))
}

fn option_count(value: Option<u32>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn option_rating(value: Option<u8>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn favorite_text(favorite: bool) -> String {
    if favorite { "♥" } else { "" }.to_string()
}

fn nonzero_year(year: u16) -> String {
    if year == 0 {
        String::new()
    } else {
        year.to_string()
    }
}

fn joined_credits(credits: &[rufin_core::ArtistCredit]) -> String {
    credits
        .iter()
        .map(|credit| credit.name.trim())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}
