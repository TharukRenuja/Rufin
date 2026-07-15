use crate::LibraryListKey;
use crate::shell::Shell;
use crate::shell::route::{MountedRoute, MountedRouteDeltaApplier};
use ::library::play_context::PlayContextDescriptor;
use ::library::{ActiveLibraryQuery, Album, HomeSectionKind, LibraryDelta, SearchResults};
use adw::prelude::*;
use gtk::{gio, glib};
use localization::msgid;
use std::{cell::RefCell, rc::Rc, sync::Arc};
use tracing::warn;

use super::collection_routes::{MountedRefreshLoader, MountedRouteRefresh};
use super::collections::{album_grid, library_route_inset, set_library_table_content_height};
use super::home_layout::home_section_header;
use super::models::replace_albums_in_model;
use super::play_context::selected_music_folder_id;
use super::route::SearchKind;
use super::route_layout::{
    PRIMARY_ROUTE_MARGIN_END, PRIMARY_ROUTE_MARGIN_START, ROUTE_TOP_MARGIN, detail_route_scroller,
    detail_route_wrapper,
};
use super::route_shell::{LibraryToolbarProjection, non_propagating_width_clip};
use super::routes::{SearchableTrackOptions, TrackListProjection};

#[derive(Clone)]
pub(crate) struct SearchAlbumProjection {
    root: gtk::Box,
    model: gio::ListStore,
    refresh: gtk::Button,
}

impl SearchAlbumProjection {
    fn new(shell: &Rc<Shell>, query: &ActiveLibraryQuery) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
        root.set_hexpand(true);
        let header = home_section_header(HomeSectionKind::Explore.title());
        header.previous.set_visible(false);
        header.next.set_visible(false);
        root.append(&header.root);

        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let grid = album_grid(shell, model.clone(), LibraryListKey::Albums, query.clone());
        let grid = grid.widget();
        grid.set_vexpand(false);
        root.append(&non_propagating_width_clip(grid));

        Rc::new(Self {
            root,
            model,
            refresh: header.refresh,
        })
    }

    fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    fn replace(&self, albums: Vec<Album>) {
        let visible = !albums.is_empty();
        replace_albums_in_model(&self.model, albums);
        self.root.set_visible(visible);
    }
}

pub(crate) struct SearchRouteProjection {
    root: gtk::Widget,
    status: gtk::Stack,
    albums: Rc<SearchAlbumProjection>,
    tracks: TrackListProjection,
    tracks_toolbar: LibraryToolbarProjection,
    tracks_panel: gtk::Box,
    pub(crate) results: RefCell<SearchResults>,
    pub(crate) error: RefCell<Option<String>>,
}

impl SearchRouteProjection {
    pub(crate) fn new(
        shell: &Rc<Shell>,
        library_query: ActiveLibraryQuery,
        query: String,
    ) -> Rc<Self> {
        let wrapper = detail_route_wrapper(0);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.set_margin_top(ROUTE_TOP_MARGIN);
        content.set_margin_bottom(28);
        content.set_hexpand(true);
        content.set_halign(gtk::Align::Fill);
        content.set_width_request(1);
        content.set_vexpand(true);

        let albums = SearchAlbumProjection::new(shell, &library_query);
        content.append(&albums.widget());

        let track_scroller = gtk::ScrolledWindow::new();
        let resize_scroller = track_scroller.clone();
        let resize: Rc<dyn Fn(usize)> = Rc::new(move |row_count| {
            set_library_table_content_height(&resize_scroller, row_count, None);
        });
        let tracks = shell.searchable_track_collection(
            Vec::new(),
            LibraryListKey::Tracks,
            SearchableTrackOptions {
                on_visible_count_changed: Some(resize),
                source_descriptor: Some(PlayContextDescriptor::Search {
                    query: query.clone(),
                    music_folder_id: selected_music_folder_id(shell),
                }),
                favorites_only: false,
                content_inset: PRIMARY_ROUTE_MARGIN_START + PRIMARY_ROUTE_MARGIN_END,
                selection_handle: None,
                fixed_layout: None,
            },
        );
        let tracks_panel = gtk::Box::new(gtk::Orientation::Vertical, 10);
        tracks_panel.set_widget_name("search");
        tracks_panel.set_hexpand(true);
        tracks_panel.set_halign(gtk::Align::Fill);
        tracks_panel.set_width_request(1);
        let tracks_toolbar =
            shell.library_toolbar_projection(LibraryListKey::Tracks, tracks.search());
        tracks_panel.append(&tracks_toolbar.widget());
        shell.set_route_search(Some(tracks.search()));
        track_scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
        track_scroller.set_width_request(1);
        track_scroller.set_min_content_width(0);
        track_scroller.set_max_content_width(1);
        track_scroller.set_propagate_natural_width(false);
        track_scroller.set_hexpand(true);
        track_scroller.set_halign(gtk::Align::Fill);
        let tracks_surface = tracks.mount_in_scroller(&track_scroller);
        tracks_panel.append(&tracks_surface);
        tracks_panel.set_visible(false);
        content.append(&tracks_panel);

        let status = gtk::Stack::new();
        status.set_hexpand(true);
        status.set_vexpand(true);
        status.add_named(&content, Some("content"));
        status.add_named(
            &shell.route_empty_view(msgid("Searching...")),
            Some("loading"),
        );
        status.add_named(
            &shell.route_empty_view(msgid("Search failed.")),
            Some("error"),
        );
        status.add_named(
            &shell.route_empty_view(msgid("No cached results found.")),
            Some("empty"),
        );
        status.set_visible_child_name("loading");
        wrapper.append(&detail_route_scroller(
            shell,
            library_route_inset(status.clone().upcast()),
        ));

        Rc::new(Self {
            root: wrapper.upcast(),
            status,
            albums,
            tracks,
            tracks_toolbar,
            tracks_panel,
            results: RefCell::new(SearchResults::default()),
            error: RefCell::new(None),
        })
    }

    pub(crate) fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }

    pub(crate) fn publish(&self) {
        let results = self.results.borrow();
        self.albums.replace(results.albums.clone());
        self.tracks.replace(results.tracks.clone());
        self.tracks_panel.set_visible(!results.tracks.is_empty());
        let has_rendered = !results.albums.is_empty() || !results.tracks.is_empty();
        let has_any = has_rendered || !results.artists.is_empty() || !results.playlists.is_empty();
        let page = if self.error.borrow().is_some() && !has_rendered {
            "error"
        } else if !has_any {
            "empty"
        } else {
            "content"
        };
        self.status.set_visible_child_name(page);
    }

    pub(crate) fn apply_library_list_settings(
        &self,
        key: LibraryListKey,
        settings: &crate::LibraryListSettings,
    ) {
        self.tracks.apply_library_list_settings(key, settings);
        self.tracks_toolbar.apply(key, settings);
    }

    fn begin_refresh(&self) {
        *self.error.borrow_mut() = None;
        let results = self.results.borrow();
        if results.albums.is_empty() && results.tracks.is_empty() {
            self.status.set_visible_child_name("loading");
        }
    }

    fn apply_refresh(&self, result: Result<SearchResults, String>, query: &str, kind: &SearchKind) {
        match result {
            Ok(results) => {
                *self.results.borrow_mut() = results;
                *self.error.borrow_mut() = None;
            }
            Err(error) => {
                warn!(%error, %query, ?kind, "search failed");
                *self.results.borrow_mut() = SearchResults::default();
                *self.error.borrow_mut() = Some(error);
            }
        }
        self.publish();
    }
}

impl Shell {
    pub(crate) fn search_route(self: &Rc<Self>, query: &str, kind: SearchKind) -> MountedRoute {
        let Some(library_query) = self.library.query.borrow().clone() else {
            return MountedRoute::static_widget(
                self.route_empty_view(msgid("Cached entries will appear here after sync finishes")),
            );
        };
        let query = query.to_string();
        let projection = SearchRouteProjection::new(self, library_query.clone(), query.clone());
        let apply_loaded: Rc<dyn Fn(Result<SearchResults, String>)> = {
            let projection = Rc::clone(&projection);
            let query = query.clone();
            let kind = kind.clone();
            Rc::new(move |result| projection.apply_refresh(result, &query, &kind))
        };
        let load_query = library_query;
        let load_search = query;
        let load_kind = kind;
        let load: MountedRefreshLoader<Result<SearchResults, String>> = Arc::new(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                load_query.search(&load_search)
            }))
            .unwrap_or_else(|_| {
                warn!(?load_kind, "library search task panicked");
                Err("Search task failed.".to_string())
            })
        });
        let refresh =
            MountedRouteRefresh::new(Rc::downgrade(&apply_loaded), load, "mounted Search");
        projection.begin_refresh();
        refresh.request();

        let button_projection = Rc::clone(&projection);
        let button_refresh = Rc::clone(&refresh);
        projection.albums.refresh.connect_clicked(move |_| {
            button_projection.begin_refresh();
            button_refresh.request();
        });

        let affected_by = Rc::new(|delta: &LibraryDelta| {
            delta.reset.is_some()
                || !delta.albums.is_empty()
                || !delta.tracks.is_empty()
                || !delta.artists.is_empty()
                || !delta.album_artists.is_empty()
                || !delta.playlists.is_empty()
        });
        let apply_delta = {
            let projection = Rc::clone(&projection);
            let apply_loaded = Rc::clone(&apply_loaded);
            let refresh = Rc::clone(&refresh);
            Rc::new(move |_: &LibraryDelta| {
                let _ = &apply_loaded;
                projection.begin_refresh();
                refresh.request();
            }) as MountedRouteDeltaApplier
        };
        let resume_projection = Rc::clone(&projection);
        let shell = Rc::clone(self);
        let resume = Rc::new(move || {
            let settings = shell
                .settings
                .current
                .borrow()
                .library_list(LibraryListKey::Tracks);
            resume_projection.apply_library_list_settings(LibraryListKey::Tracks, &settings);
        });

        MountedRoute::new(projection.widget(), affected_by, apply_delta, resume)
    }
}
