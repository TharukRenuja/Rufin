use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
    time::Instant,
};

use ::library::{
    AcceptedTrackReplacement, AlbumDetail, AlbumSummary, ArtistSummary, Library, MusicFolderId,
    PlaylistSummary, SmartPlaylistSummary, TrackList,
};
use adw::prelude::*;
use gtk::{gio, glib};
use tracing::{info, warn};

use crate::localization::bind_search_placeholder;
use crate::shell::Shell;
use crate::shell::route::{LatestMountedRouteRead, MountedRoute, SelectedRouteIdentity};
use crate::{LibraryLayout, LibraryListKey, LibraryListSettings};
use localization::msgid;

use super::album_detail::{
    AlbumCollectionModels, populate_album_collection_model,
    populate_prepared_album_collection_model, sort_album_details,
};
use super::collections::{
    album_collection_projection, artist_collection_projection, library_route_inset,
    playlist_collection_projection, smart_playlist_collection_projection,
    track_collection_projection,
};
use super::library_fields::{
    album_matches_query, artist_matches_query, playlist_matches_query,
    smart_playlist_matches_query, sort_playlists,
};
use super::models::{
    populate_artist_model, populate_playlist_model, populate_smart_playlist_model,
    replace_artists_in_model, replace_playlists_in_model, sort_albums, sort_artists,
};
use super::route::Route;
use super::route_layout::PRIMARY_ROUTE_HORIZONTAL_INSET;
use super::route_shell::{LibraryPageShellOptions, LibraryToolbarProjection};
use super::track_model::{
    PreparedTrackProjection, TrackCollectionModel, TrackProjectionRequest, prepare_track_projection,
};

const SLOW_LIBRARY_ROUTE_SETUP_MS: u64 = 100;
const EMBEDDED_SCROLL_LATCH_MS: u128 = 280;
const EMBEDDED_SURFACE_SCROLL_FACTOR: f64 = 2.5;

type TrackRouteSource =
    Arc<dyn Fn(&LibraryListSettings) -> Result<TrackList, String> + Send + Sync>;
type TrackRouteMembership = Rc<dyn Fn(&::library::Track) -> bool>;

struct RootTrackRouteOptions {
    key: LibraryListKey,
    route: Route,
    context: &'static str,
    empty_body: &'static str,
    reload_on_history_change: bool,
}

#[derive(Clone)]
struct TrackRouteReadRequest {
    identity: SelectedRouteIdentity,
    tracks: TrackProjectionRequest,
}

#[derive(Clone)]
struct CollectionReadRequest {
    identity: SelectedRouteIdentity,
    query: String,
    settings: LibraryListSettings,
}

pub(crate) struct PreparedCollection<T> {
    pub(crate) source: Arc<[T]>,
    pub(crate) visible: Arc<[T]>,
}

pub(crate) struct PreparedAlbums {
    pub(crate) source: Arc<[AlbumSummary]>,
    pub(crate) visible: Arc<[AlbumSummary]>,
    pub(crate) details: Option<Arc<[AlbumDetail]>>,
    pub(crate) visible_details: Option<Arc<[AlbumDetail]>>,
}

pub(crate) struct SearchableTrackOptions {
    pub(crate) on_visible_count_changed: Option<Rc<dyn Fn(usize)>>,
    pub(crate) context_id: String,
    pub(crate) content_inset: i32,
    pub(crate) fixed_layout: Option<LibraryLayout>,
}

#[derive(Clone)]
pub(crate) struct TrackListProjection {
    key: LibraryListKey,
    search: gtk::SearchEntry,
    collection: super::collections::LibraryCollectionProjection,
    model: TrackCollectionModel,
    on_visible_count_changed: Option<Rc<dyn Fn(usize)>>,
    fixed_layout: Option<LibraryLayout>,
}

impl TrackListProjection {
    pub(crate) fn search(&self) -> gtk::SearchEntry {
        self.search.clone()
    }

    pub(crate) fn scrolling_widget(&self) -> gtk::Widget {
        self.collection.scrolling_widget()
    }

    pub(crate) fn item_navigation(&self) -> crate::shell::route::MountedRouteItemNavigation {
        self.collection.item_navigation()
    }

    pub(crate) fn mount_in_scroller(&self, scroller: &gtk::ScrolledWindow) -> gtk::Widget {
        self.collection.mount_in_scroller(scroller, 0, 0)
    }

    pub(crate) fn source_is_empty(&self) -> bool {
        self.model.source_is_empty()
    }

    pub(crate) fn source_play_request(
        &self,
        placement: playback::QueuePlacement,
        context_id: &str,
        shuffled_start: bool,
    ) -> Option<playback::LoadedPlayRequest> {
        self.model
            .source_play_request(placement, context_id, shuffled_start)
    }

    pub(crate) fn projection_request(&self) -> TrackProjectionRequest {
        self.model.projection_request()
    }

    pub(crate) fn connect_search_request(
        &self,
        callback: impl Fn(TrackProjectionRequest) + 'static,
    ) {
        let model = self.model.clone();
        self.search.connect_search_changed(move |_| {
            callback(model.projection_request());
        });
    }

    pub(crate) fn replace_prepared(&self, prepared: PreparedTrackProjection) -> bool {
        let changed = self.model.replace_prepared(prepared);
        if changed {
            self.notify_visible_count();
        }
        changed
    }

    pub(crate) fn apply_track_replacement(
        &self,
        replacements: &[AcceptedTrackReplacement],
        include: impl Fn(&library::Track) -> bool,
    ) -> bool {
        let changed = self.model.apply_track_replacement(replacements, include);
        if changed {
            self.notify_visible_count();
        }
        changed
    }

    pub(crate) fn apply_library_list_settings(
        &self,
        key: LibraryListKey,
        settings: &crate::LibraryListSettings,
    ) {
        if key != self.key {
            return;
        }
        let mut settings = settings.clone();
        if let Some(layout) = self.fixed_layout {
            settings.layout = layout;
        }
        let previous = self.model.settings();
        self.model.apply_settings(settings.clone());
        if previous.sort_key != settings.sort_key || previous.descending != settings.descending {
            self.notify_visible_count();
        }
        self.collection.apply_settings(&settings);
    }

    fn notify_visible_count(&self) {
        if let Some(on_visible_count_changed) = self.on_visible_count_changed.as_ref() {
            on_visible_count_changed(self.model.visible_count());
        }
    }
}

impl Shell {
    pub(crate) fn library_albums_route(
        self: &Rc<Self>,
        source_albums: Arc<[AlbumSummary]>,
        prepared_details: Option<Arc<[AlbumDetail]>>,
        loaded: Arc<Library>,
        music_folder_id: Option<MusicFolderId>,
    ) -> MountedRoute {
        let view_started = Instant::now();
        let key = LibraryListKey::Albums;
        let settings = self.settings.current.borrow().library_list(key);
        let applied_settings = Rc::new(RefCell::new(settings.clone()));
        let source_albums = Rc::new(RefCell::new(source_albums));
        let visible = Rc::new(RefCell::new(Arc::clone(&source_albums.borrow())));
        let detail_albums = Rc::new(RefCell::new(prepared_details));
        let models = AlbumCollectionModels::new();
        let model_started = Instant::now();
        let initial_visible = visible.borrow().clone();
        let initial_details = detail_albums.borrow().clone();
        populate_prepared_album_collection_model(
            &models,
            &initial_visible,
            initial_details,
            settings.layout,
        );
        let model_ms = model_started.elapsed().as_millis() as u64;

        let search = gtk::SearchEntry::new();
        bind_search_placeholder(&search, "Search");
        search.set_hexpand(true);
        let query = Rc::new(RefCell::new(String::new()));
        {
            let shell = Rc::clone(self);
            let models = models.clone();
            let source_albums = Rc::clone(&source_albums);
            let visible = Rc::clone(&visible);
            let detail_albums = Rc::clone(&detail_albums);
            let query = Rc::clone(&query);
            search.connect_search_changed(move |entry| {
                let text = entry.text().trim().to_string();
                *query.borrow_mut() = text.clone();
                let filtered = filter_shared(&source_albums.borrow(), &text, album_matches_query);
                *visible.borrow_mut() = filtered;
                let settings = shell.settings.current.borrow().library_list(key);
                let details = album_details_for_route(&detail_albums, &text, settings.layout);
                populate_album_collection_model(&models, &visible.borrow(), details, &settings);
                models.clear_inactive(settings.layout);
                if settings.layout != LibraryLayout::Detail {
                    detail_albums.borrow_mut().take();
                }
            });
        }

        let content_started = Instant::now();
        let content = album_collection_projection(self, models.clone(), key);
        models.clear_inactive(settings.layout);
        let content_ms = content_started.elapsed().as_millis() as u64;
        let shell_started = Instant::now();
        let visible_results = Rc::clone(&visible);
        let page_shell = self.library_page_shell(LibraryPageShellOptions {
            key,
            empty: source_albums.borrow().is_empty(),
            empty_body: msgid("Nothing here yet"),
            search: search.clone(),
            has_visible_results: Rc::new(move || !visible_results.borrow().is_empty()),
            content: content.scrolling_widget(),
        });
        let shell_ms = shell_started.elapsed().as_millis() as u64;
        log_route_setup(
            Route::Albums,
            settings.layout,
            source_albums.borrow().len(),
            model_ms,
            content_ms,
            shell_ms,
            view_started,
        );

        let identity =
            self.mounted_route_read_identity(Route::Albums, &loaded, music_folder_id.clone());
        let apply = {
            let shell = Rc::clone(self);
            let models = models.clone();
            let source_albums = Rc::clone(&source_albums);
            let visible = Rc::clone(&visible);
            let detail_albums = Rc::clone(&detail_albums);
            let content = content.clone();
            let page_shell = page_shell.clone();
            let applied_settings = Rc::clone(&applied_settings);
            Rc::new(
                move |request: CollectionReadRequest, result: Result<PreparedAlbums, String>| {
                    if !shell.mounted_route_read_is_current(&request.identity) {
                        return;
                    }
                    let prepared = match result {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            warn!(%error, "failed to refresh the mounted Albums route");
                            return;
                        }
                    };
                    source_albums.replace(prepared.source);
                    visible.replace(prepared.visible);
                    detail_albums.replace(prepared.details);
                    let prepared_visible = visible.borrow().clone();
                    populate_prepared_album_collection_model(
                        &models,
                        &prepared_visible,
                        prepared.visible_details,
                        request.settings.layout,
                    );
                    content.apply_settings(&request.settings);
                    models.clear_inactive(request.settings.layout);
                    page_shell.apply_library_list_settings(key, &request.settings);
                    page_shell.set_empty(source_albums.borrow().is_empty());
                    applied_settings.replace(request.settings);
                },
            )
        };
        let load = {
            let loaded = Arc::clone(&loaded);
            let music_folder_id = music_folder_id.clone();
            Arc::new(move |request: &CollectionReadRequest| {
                load_albums(
                    &loaded,
                    music_folder_id.as_ref(),
                    &request.query,
                    &request.settings,
                )
            })
        };
        let read = LatestMountedRouteRead::new_with_request(apply, load, "mounted Albums route");
        {
            let read = Rc::downgrade(&read);
            let identity = identity.clone();
            let shell = Rc::clone(self);
            search.connect_search_changed(move |entry| {
                let Some(read) = read.upgrade() else {
                    return;
                };
                let settings = shell.settings.current.borrow().library_list(key);
                read.request_with_if_running(CollectionReadRequest {
                    identity: identity.clone(),
                    query: entry.text().trim().to_string(),
                    settings,
                });
            });
        }
        let resume = {
            let shell = Rc::clone(self);
            let models = models.clone();
            let visible = Rc::clone(&visible);
            let detail_albums = Rc::clone(&detail_albums);
            let query = Rc::clone(&query);
            let content = content.clone();
            let page_shell = page_shell.clone();
            let applied_settings = Rc::clone(&applied_settings);
            let read = Rc::clone(&read);
            let identity = identity.clone();
            Rc::new(move || {
                let settings = shell.settings.current.borrow().library_list(key);
                let previous = applied_settings.borrow().clone();
                if previous.sort_key != settings.sort_key
                    || previous.descending != settings.descending
                    || previous.layout != settings.layout
                {
                    let details =
                        album_details_for_route(&detail_albums, &query.borrow(), settings.layout);
                    populate_album_collection_model(&models, &visible.borrow(), details, &settings);
                }
                content.apply_settings(&settings);
                models.clear_inactive(settings.layout);
                if settings.layout != LibraryLayout::Detail {
                    detail_albums.borrow_mut().take();
                }
                page_shell.apply_library_list_settings(key, &settings);
                *applied_settings.borrow_mut() = settings.clone();
                let request = CollectionReadRequest {
                    identity: identity.clone(),
                    query: query.borrow().clone(),
                    settings,
                };
                if request.settings.layout == LibraryLayout::Detail
                    && detail_albums.borrow().is_none()
                {
                    read.request_with(request);
                } else {
                    read.request_with_if_running(request);
                }
            })
        };
        let update = {
            let read = Rc::clone(&read);
            let identity = identity.clone();
            let shell = Rc::clone(self);
            let query = Rc::clone(&query);
            Rc::new(move |update: &crate::runtime::SelectedLibraryUpdate| {
                if !update.change.albums.is_empty() {
                    read.request_with(CollectionReadRequest {
                        identity: identity.clone(),
                        query: query.borrow().clone(),
                        settings: shell.settings.current.borrow().library_list(key),
                    });
                }
            })
        };
        page_shell
            .mounted_route(resume, content.item_navigation())
            .with_library_update(update)
    }

    pub(crate) fn library_tracks_route(
        self: &Rc<Self>,
        tracks: TrackList,
        loaded: Arc<Library>,
        music_folder_id: Option<MusicFolderId>,
    ) -> MountedRoute {
        let source = {
            let loaded = Arc::clone(&loaded);
            let music_folder_id = music_folder_id.clone();
            Arc::new(move |settings: &LibraryListSettings| {
                load_tracks(&loaded, music_folder_id.as_ref(), settings)
            }) as TrackRouteSource
        };
        let membership_folder_id = music_folder_id.clone();
        let membership = Rc::new(move |track: &::library::Track| {
            membership_folder_id
                .as_ref()
                .is_none_or(|folder_id| track.relations.music_folders.contains(folder_id))
        }) as TrackRouteMembership;
        self.root_track_route(
            RootTrackRouteOptions {
                key: LibraryListKey::Tracks,
                route: Route::Tracks,
                context: "tracks",
                empty_body: msgid("Nothing here yet"),
                reload_on_history_change: false,
            },
            tracks,
            loaded,
            music_folder_id,
            source,
            membership,
        )
    }

    pub(crate) fn favorites_route(
        self: &Rc<Self>,
        favorites: TrackList,
        loaded: Arc<Library>,
        music_folder_id: Option<MusicFolderId>,
    ) -> MountedRoute {
        let key = LibraryListKey::FavoriteTracks;
        let source = {
            let loaded = Arc::clone(&loaded);
            let music_folder_id = music_folder_id.clone();
            Arc::new(move |settings: &LibraryListSettings| {
                load_favorite_tracks(&loaded, music_folder_id.as_ref(), settings)
            }) as TrackRouteSource
        };
        let membership_folder_id = music_folder_id.clone();
        let membership = Rc::new(move |track: &::library::Track| {
            track.favorite
                && membership_folder_id
                    .as_ref()
                    .is_none_or(|folder_id| track.relations.music_folders.contains(folder_id))
        }) as TrackRouteMembership;
        self.root_track_route(
            RootTrackRouteOptions {
                key,
                route: Route::Favorites,
                context: "favorite-tracks",
                empty_body: msgid("No favorites yet"),
                reload_on_history_change: false,
            },
            favorites,
            loaded,
            music_folder_id,
            source,
            membership,
        )
    }

    pub(crate) fn history_route(
        self: &Rc<Self>,
        history: TrackList,
        loaded: Arc<Library>,
        music_folder_id: Option<MusicFolderId>,
    ) -> MountedRoute {
        let source = {
            let loaded = Arc::clone(&loaded);
            let music_folder_id = music_folder_id.clone();
            Arc::new(move |_: &LibraryListSettings| {
                load_history_tracks(&loaded, music_folder_id.as_ref())
            }) as TrackRouteSource
        };
        let membership_folder_id = music_folder_id.clone();
        let membership = Rc::new(move |track: &::library::Track| {
            membership_folder_id
                .as_ref()
                .is_none_or(|folder_id| track.relations.music_folders.contains(folder_id))
        }) as TrackRouteMembership;
        self.root_track_route(
            RootTrackRouteOptions {
                key: LibraryListKey::History,
                route: Route::History,
                context: "history",
                empty_body: msgid("Nothing played yet"),
                reload_on_history_change: true,
            },
            history,
            loaded,
            music_folder_id,
            source,
            membership,
        )
    }

    fn root_track_route(
        self: &Rc<Self>,
        options: RootTrackRouteOptions,
        tracks: TrackList,
        loaded: Arc<Library>,
        music_folder_id: Option<MusicFolderId>,
        source: TrackRouteSource,
        membership: TrackRouteMembership,
    ) -> MountedRoute {
        let context_id = music_folder_id.as_ref().map_or_else(
            || format!("{}:all", options.context),
            |folder_id| format!("{}:{}", options.context, folder_id.as_str()),
        );
        let projection = self.searchable_track_collection(
            tracks,
            options.key,
            SearchableTrackOptions {
                on_visible_count_changed: None,
                context_id,
                content_inset: PRIMARY_ROUTE_HORIZONTAL_INSET,
                fixed_layout: None,
            },
        );
        let identity = self.mounted_route_read_identity(options.route, &loaded, music_folder_id);
        self.track_page_route(
            options.key,
            options.empty_body,
            projection,
            identity,
            source,
            membership,
            options.reload_on_history_change,
        )
    }

    pub(crate) fn library_artist_list_route(
        self: &Rc<Self>,
        album_artist: bool,
        source_artists: Arc<[ArtistSummary]>,
        loaded: Arc<Library>,
        music_folder_id: Option<MusicFolderId>,
    ) -> MountedRoute {
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
        let settings = self.settings.current.borrow().library_list(key);
        let applied_settings = Rc::new(RefCell::new(settings.clone()));
        let source_artists = Rc::new(RefCell::new(source_artists));
        let visible = Rc::new(RefCell::new(Arc::clone(&source_artists.borrow())));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let model_started = Instant::now();
        replace_artists_in_model(&model, visible.borrow().iter().cloned());
        let model_ms = model_started.elapsed().as_millis() as u64;

        let search = gtk::SearchEntry::new();
        bind_search_placeholder(&search, "Search");
        search.set_hexpand(true);
        {
            let shell = Rc::clone(self);
            let model = model.clone();
            let source_artists = Rc::clone(&source_artists);
            let visible = Rc::clone(&visible);
            search.connect_search_changed(move |entry| {
                *visible.borrow_mut() = filter_shared(
                    &source_artists.borrow(),
                    entry.text().as_str(),
                    artist_matches_query,
                );
                populate_artist_model(
                    &model,
                    &visible.borrow(),
                    &shell.settings.current.borrow().library_list(key),
                );
            });
        }

        let content_started = Instant::now();
        let content = artist_collection_projection(self, model.clone(), key);
        let content_ms = content_started.elapsed().as_millis() as u64;
        let shell_started = Instant::now();
        let visible_results = Rc::clone(&visible);
        let page_shell = self.library_page_shell(LibraryPageShellOptions {
            key,
            empty: source_artists.borrow().is_empty(),
            empty_body: msgid("Nothing here yet"),
            search: search.clone(),
            has_visible_results: Rc::new(move || !visible_results.borrow().is_empty()),
            content: content.scrolling_widget(),
        });
        let shell_ms = shell_started.elapsed().as_millis() as u64;
        log_route_setup(
            route.clone(),
            settings.layout,
            source_artists.borrow().len(),
            model_ms,
            content_ms,
            shell_ms,
            view_started,
        );
        let identity = self.mounted_route_read_identity(route, &loaded, music_folder_id.clone());
        let apply = {
            let shell = Rc::clone(self);
            let source_artists = Rc::clone(&source_artists);
            let visible = Rc::clone(&visible);
            let model = model.clone();
            let content = content.clone();
            let page_shell = page_shell.clone();
            let applied_settings = Rc::clone(&applied_settings);
            Rc::new(
                move |request: CollectionReadRequest,
                      result: Result<PreparedCollection<ArtistSummary>, String>| {
                    if !shell.mounted_route_read_is_current(&request.identity) {
                        return;
                    }
                    let prepared = match result {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            warn!(%error, "failed to refresh the mounted Artists route");
                            return;
                        }
                    };
                    source_artists.replace(prepared.source);
                    visible.replace(prepared.visible);
                    replace_artists_in_model(&model, visible.borrow().iter().cloned());
                    content.apply_settings(&request.settings);
                    page_shell.apply_library_list_settings(key, &request.settings);
                    page_shell.set_empty(source_artists.borrow().is_empty());
                    applied_settings.replace(request.settings);
                },
            )
        };
        let load = {
            let loaded = Arc::clone(&loaded);
            let music_folder_id = music_folder_id.clone();
            Arc::new(move |request: &CollectionReadRequest| {
                load_artists(
                    &loaded,
                    music_folder_id.as_ref(),
                    album_artist,
                    &request.query,
                    &request.settings,
                )
            })
        };
        let read = LatestMountedRouteRead::new_with_request(apply, load, "mounted Artists route");
        {
            let read = Rc::downgrade(&read);
            let identity = identity.clone();
            let shell = Rc::clone(self);
            search.connect_search_changed(move |entry| {
                let Some(read) = read.upgrade() else {
                    return;
                };
                read.request_with_if_running(CollectionReadRequest {
                    identity: identity.clone(),
                    query: entry.text().trim().to_string(),
                    settings: shell.settings.current.borrow().library_list(key),
                });
            });
        }
        let resume = {
            let shell = Rc::clone(self);
            let model = model.clone();
            let visible = Rc::clone(&visible);
            let content = content.clone();
            let page_shell = page_shell.clone();
            let applied_settings = Rc::clone(&applied_settings);
            let search = search.clone();
            let read = Rc::clone(&read);
            let identity = identity.clone();
            Rc::new(move || {
                let settings = shell.settings.current.borrow().library_list(key);
                let previous = applied_settings.borrow().clone();
                if previous.sort_key != settings.sort_key
                    || previous.descending != settings.descending
                {
                    populate_artist_model(&model, &visible.borrow(), &settings);
                }
                content.apply_settings(&settings);
                page_shell.apply_library_list_settings(key, &settings);
                *applied_settings.borrow_mut() = settings.clone();
                read.request_with_if_running(CollectionReadRequest {
                    identity: identity.clone(),
                    query: search.text().trim().to_string(),
                    settings,
                });
            })
        };
        let update = {
            let read = Rc::clone(&read);
            let identity = identity.clone();
            let shell = Rc::clone(self);
            let search = search.clone();
            Rc::new(move |update: &crate::runtime::SelectedLibraryUpdate| {
                if !update.change.artists.is_empty() {
                    read.request_with(CollectionReadRequest {
                        identity: identity.clone(),
                        query: search.text().trim().to_string(),
                        settings: shell.settings.current.borrow().library_list(key),
                    });
                }
            })
        };
        page_shell
            .mounted_route(resume, content.item_navigation())
            .with_library_update(update)
    }

    pub(crate) fn library_playlists_route(
        self: &Rc<Self>,
        source_playlists: Arc<[PlaylistSummary]>,
        loaded: Arc<Library>,
    ) -> MountedRoute {
        let key = LibraryListKey::Playlists;
        let settings = self.settings.current.borrow().library_list(key);
        let applied_settings = Rc::new(RefCell::new(settings.clone()));
        let source_playlists = Rc::new(RefCell::new(source_playlists));
        let visible = Rc::new(RefCell::new(Arc::clone(&source_playlists.borrow())));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        replace_playlists_in_model(&model, visible.borrow().iter().cloned());
        let search = gtk::SearchEntry::new();
        bind_search_placeholder(&search, "Search");
        search.set_hexpand(true);
        {
            let shell = Rc::clone(self);
            let model = model.clone();
            let source_playlists = Rc::clone(&source_playlists);
            let visible = Rc::clone(&visible);
            search.connect_search_changed(move |entry| {
                *visible.borrow_mut() = filter_shared(
                    &source_playlists.borrow(),
                    entry.text().as_str(),
                    playlist_matches_query,
                );
                populate_playlist_model(
                    &model,
                    &visible.borrow(),
                    &shell.settings.current.borrow().library_list(key),
                );
            });
        }
        let content = playlist_collection_projection(self, model.clone());
        let visible_results = Rc::clone(&visible);
        let page_shell = self.library_page_shell(LibraryPageShellOptions {
            key,
            empty: source_playlists.borrow().is_empty(),
            empty_body: msgid("Nothing here yet"),
            search: search.clone(),
            has_visible_results: Rc::new(move || !visible_results.borrow().is_empty()),
            content: content.scrolling_widget(),
        });
        let identity = self.mounted_route_read_identity(Route::Playlists, &loaded, None);
        let apply = {
            let shell = Rc::clone(self);
            let source_playlists = Rc::clone(&source_playlists);
            let visible = Rc::clone(&visible);
            let model = model.clone();
            let content = content.clone();
            let page_shell = page_shell.clone();
            let applied_settings = Rc::clone(&applied_settings);
            Rc::new(
                move |request: CollectionReadRequest,
                      result: Result<PreparedCollection<PlaylistSummary>, String>| {
                    if !shell.mounted_route_read_is_current(&request.identity) {
                        return;
                    }
                    let prepared = match result {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            warn!(%error, "failed to refresh the mounted Playlists route");
                            return;
                        }
                    };
                    source_playlists.replace(prepared.source);
                    visible.replace(prepared.visible);
                    replace_playlists_in_model(&model, visible.borrow().iter().cloned());
                    content.apply_settings(&request.settings);
                    page_shell.apply_library_list_settings(key, &request.settings);
                    page_shell.set_empty(source_playlists.borrow().is_empty());
                    applied_settings.replace(request.settings);
                },
            )
        };
        let load = {
            let loaded = Arc::clone(&loaded);
            Arc::new(move |request: &CollectionReadRequest| {
                load_playlists(&loaded, &request.query, &request.settings)
            })
        };
        let read = LatestMountedRouteRead::new_with_request(apply, load, "mounted Playlists route");
        {
            let read = Rc::downgrade(&read);
            let identity = identity.clone();
            let shell = Rc::clone(self);
            search.connect_search_changed(move |entry| {
                let Some(read) = read.upgrade() else {
                    return;
                };
                read.request_with_if_running(CollectionReadRequest {
                    identity: identity.clone(),
                    query: entry.text().trim().to_string(),
                    settings: shell.settings.current.borrow().library_list(key),
                });
            });
        }
        let resume = {
            let shell = Rc::clone(self);
            let model = model.clone();
            let visible = Rc::clone(&visible);
            let content = content.clone();
            let page_shell = page_shell.clone();
            let applied_settings = Rc::clone(&applied_settings);
            let search = search.clone();
            let read = Rc::clone(&read);
            let identity = identity.clone();
            Rc::new(move || {
                let settings = shell.settings.current.borrow().library_list(key);
                let previous = applied_settings.borrow().clone();
                if previous.sort_key != settings.sort_key
                    || previous.descending != settings.descending
                {
                    populate_playlist_model(&model, &visible.borrow(), &settings);
                }
                content.apply_settings(&settings);
                page_shell.apply_library_list_settings(key, &settings);
                *applied_settings.borrow_mut() = settings.clone();
                read.request_with_if_running(CollectionReadRequest {
                    identity: identity.clone(),
                    query: search.text().trim().to_string(),
                    settings,
                });
            })
        };
        let update = {
            let read = Rc::clone(&read);
            let identity = identity.clone();
            let shell = Rc::clone(self);
            let search = search.clone();
            Rc::new(move |update: &crate::runtime::SelectedLibraryUpdate| {
                if !update.change.playlists.is_empty() {
                    read.request_with(CollectionReadRequest {
                        identity: identity.clone(),
                        query: search.text().trim().to_string(),
                        settings: shell.settings.current.borrow().library_list(key),
                    });
                }
            })
        };
        page_shell
            .mounted_route(resume, content.item_navigation())
            .with_library_update(update)
    }

    pub(crate) fn library_smart_playlists_route(
        self: &Rc<Self>,
        initial_playlists: Arc<[SmartPlaylistSummary]>,
        loaded: Arc<Library>,
        music_folder_id: Option<MusicFolderId>,
    ) -> MountedRoute {
        let key = LibraryListKey::SmartPlaylists;
        let settings = self.settings.current.borrow().library_list(key);
        let applied_settings = Rc::new(RefCell::new(settings.clone()));
        let source_playlists = Rc::new(RefCell::new(initial_playlists));
        let visible = Rc::new(RefCell::new(Arc::clone(&source_playlists.borrow())));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        populate_smart_playlist_model(&model, &visible.borrow(), &settings);
        let search = gtk::SearchEntry::new();
        bind_search_placeholder(&search, "Search");
        search.set_hexpand(true);
        {
            let shell = Rc::clone(self);
            let model = model.clone();
            let source_playlists = Rc::clone(&source_playlists);
            let visible = Rc::clone(&visible);
            search.connect_search_changed(move |entry| {
                *visible.borrow_mut() = filter_shared(
                    &source_playlists.borrow(),
                    entry.text().as_str(),
                    smart_playlist_matches_query,
                );
                populate_smart_playlist_model(
                    &model,
                    &visible.borrow(),
                    &shell.settings.current.borrow().library_list(key),
                );
            });
        }
        let content = smart_playlist_collection_projection(self, model.clone());
        let visible_results = Rc::clone(&visible);
        let page_shell = self.library_page_shell(LibraryPageShellOptions {
            key,
            empty: source_playlists.borrow().is_empty(),
            empty_body: msgid("No smart playlists yet"),
            search: search.clone(),
            has_visible_results: Rc::new(move || !visible_results.borrow().is_empty()),
            content: content.scrolling_widget(),
        });

        let apply = {
            let shell = Rc::clone(self);
            let source_playlists = Rc::clone(&source_playlists);
            let visible = Rc::clone(&visible);
            let model = model.clone();
            let search = search.clone();
            let page_shell = page_shell.clone();
            Rc::new(move |result: Result<Arc<[SmartPlaylistSummary]>, String>| {
                let next = match result {
                    Ok(next) => next,
                    Err(error) => {
                        warn!(%error, "failed to read the mounted Smart Playlists route");
                        return;
                    }
                };
                source_playlists.replace(next);
                visible.replace(filter_shared(
                    &source_playlists.borrow(),
                    search.text().as_str(),
                    smart_playlist_matches_query,
                ));
                populate_smart_playlist_model(
                    &model,
                    &visible.borrow(),
                    &shell.settings.current.borrow().library_list(key),
                );
                page_shell.set_empty(source_playlists.borrow().is_empty());
            }) as Rc<dyn Fn(Result<Arc<[SmartPlaylistSummary]>, String>)>
        };
        let load = {
            let loaded = Arc::clone(&loaded);
            let music_folder_id = music_folder_id.clone();
            Arc::new(move || load_smart_playlists(&loaded, music_folder_id.as_ref()))
                as Arc<dyn Fn() -> Result<Arc<[SmartPlaylistSummary]>, String> + Send + Sync>
        };
        let read = LatestMountedRouteRead::new(apply, load, "Smart Playlists");
        let resume = {
            let shell = Rc::clone(self);
            let model = model.clone();
            let visible = Rc::clone(&visible);
            let content = content.clone();
            let page_shell = page_shell.clone();
            let applied_settings = Rc::clone(&applied_settings);
            Rc::new(move || {
                let settings = shell.settings.current.borrow().library_list(key);
                let previous = applied_settings.borrow().clone();
                if previous.sort_key != settings.sort_key
                    || previous.descending != settings.descending
                {
                    populate_smart_playlist_model(&model, &visible.borrow(), &settings);
                }
                content.apply_settings(&settings);
                page_shell.apply_library_list_settings(key, &settings);
                *applied_settings.borrow_mut() = settings;
            })
        };
        let update = {
            let read = Rc::clone(&read);
            Rc::new(move |update: &crate::runtime::SelectedLibraryUpdate| {
                if !update.change.smart_playlists.is_empty() {
                    read.request();
                }
            })
        };
        page_shell
            .mounted_route(resume, content.item_navigation())
            .with_library_update(update)
    }

    pub(crate) fn scrolling_track_projection(
        self: &Rc<Self>,
        tracks: TrackList,
        key: LibraryListKey,
        context: &str,
        context_id: String,
    ) -> (gtk::Widget, TrackListProjection, LibraryToolbarProjection) {
        let projection = self.searchable_track_collection(
            tracks,
            key,
            SearchableTrackOptions {
                on_visible_count_changed: None,
                context_id,
                content_inset: PRIMARY_ROUTE_HORIZONTAL_INSET,
                fixed_layout: None,
            },
        );
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 10);
        wrapper.set_widget_name(context);
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_vexpand(true);
        let toolbar = self.library_toolbar_projection(key, projection.search());
        wrapper.append(&library_route_inset(toolbar.widget()));
        self.set_route_search(Some(projection.search()));
        wrapper.append(&projection.scrolling_widget());
        (wrapper.upcast(), projection, toolbar)
    }

    pub(crate) fn searchable_track_collection(
        self: &Rc<Self>,
        tracks: TrackList,
        key: LibraryListKey,
        options: SearchableTrackOptions,
    ) -> TrackListProjection {
        let selected = self
            .library
            .selected
            .borrow()
            .as_ref()
            .map(|selected| (selected.source_id.clone(), selected.source_session_epoch))
            .expect("a music route requires one selected source");
        let mut settings = self.settings.current.borrow().library_list(key);
        if let Some(layout) = options.fixed_layout {
            settings.layout = layout;
        }
        let model = TrackCollectionModel::new(selected.0, selected.1, tracks, settings.clone());
        if let Some(on_visible_count_changed) = options.on_visible_count_changed.as_ref() {
            on_visible_count_changed(model.visible_count());
        }
        let search = gtk::SearchEntry::new();
        bind_search_placeholder(&search, "Search");
        search.set_hexpand(true);
        {
            let model = model.clone();
            let on_visible_count_changed = options.on_visible_count_changed.clone();
            search.connect_search_changed(move |entry| {
                if model.set_query(entry.text().as_str())
                    && let Some(on_visible_count_changed) = on_visible_count_changed.as_ref()
                {
                    on_visible_count_changed(model.visible_count());
                }
            });
        }
        let collection = track_collection_projection(
            self,
            model.clone(),
            key,
            settings,
            options.context_id,
            options.content_inset,
        );
        TrackListProjection {
            key,
            search,
            collection,
            model,
            on_visible_count_changed: options.on_visible_count_changed,
            fixed_layout: options.fixed_layout,
        }
    }

    fn track_page_route(
        self: &Rc<Self>,
        key: LibraryListKey,
        empty_body: &'static str,
        projection: TrackListProjection,
        identity: SelectedRouteIdentity,
        source: TrackRouteSource,
        membership: TrackRouteMembership,
        reload_on_history_change: bool,
    ) -> MountedRoute {
        let visible_model = projection.model.clone();
        let page_shell = self.library_page_shell(LibraryPageShellOptions {
            key,
            empty: projection.source_is_empty(),
            empty_body,
            search: projection.search(),
            has_visible_results: Rc::new(move || visible_model.visible_count() != 0),
            content: projection.scrolling_widget(),
        });
        let apply = {
            let shell = Rc::clone(self);
            let projection = projection.clone();
            let page_shell = page_shell.clone();
            Rc::new(move |request: TrackRouteReadRequest, result| {
                if !shell.mounted_route_read_is_current(&request.identity) {
                    return;
                }
                let prepared = match result {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        warn!(%error, "failed to read a mounted Track route");
                        return;
                    }
                };
                if projection.replace_prepared(prepared) {
                    page_shell.set_empty(projection.source_is_empty());
                }
            })
        };
        let load = Arc::new(move |request: &TrackRouteReadRequest| {
            source(&request.tracks.settings).and_then(|tracks| {
                prepare_track_projection(tracks, request.tracks.clone())
                    .map_err(|error| error.to_string())
            })
        });
        let read = LatestMountedRouteRead::new_with_request(apply, load, "mounted Track route");
        {
            let read = Rc::downgrade(&read);
            let identity = identity.clone();
            projection.connect_search_request(move |tracks| {
                let Some(read) = read.upgrade() else {
                    return;
                };
                read.request_with_if_running(TrackRouteReadRequest {
                    identity: identity.clone(),
                    tracks,
                });
            });
        }
        let resume = {
            let shell = Rc::clone(self);
            let projection = projection.clone();
            let page_shell = page_shell.clone();
            let read = Rc::clone(&read);
            let identity = identity.clone();
            Rc::new(move || {
                let settings = shell.settings.current.borrow().library_list(key);
                projection.apply_library_list_settings(key, &settings);
                page_shell.apply_library_list_settings(key, &settings);
                read.request_with_if_running(TrackRouteReadRequest {
                    identity: identity.clone(),
                    tracks: projection.projection_request(),
                });
            })
        };
        let update_projection = projection.clone();
        let update_shell = page_shell.clone();
        let read = Rc::clone(&read);
        let identity = identity.clone();
        page_shell
            .mounted_route(resume, projection.item_navigation())
            .with_library_update(Rc::new(move |library_update| {
                if reload_on_history_change && library_update.change.history_changed {
                    read.request_with(TrackRouteReadRequest {
                        identity: identity.clone(),
                        tracks: update_projection.projection_request(),
                    });
                    return;
                }
                if reload_on_history_change && library_update.change.favorite.is_some() {
                    return;
                }
                let replacements = library_update.change.tracks.as_slice();
                if replacements.is_empty() {
                    return;
                }
                if update_projection
                    .apply_track_replacement(replacements, |track| membership(track))
                {
                    update_shell.set_empty(update_projection.source_is_empty());
                    return;
                }
                read.request_with(TrackRouteReadRequest {
                    identity: identity.clone(),
                    tracks: update_projection.projection_request(),
                });
            }))
    }
}

pub(crate) fn load_smart_playlists(
    loaded: &Arc<Library>,
    music_folder_id: Option<&MusicFolderId>,
) -> Result<Arc<[SmartPlaylistSummary]>, String> {
    loaded
        .smart_playlists(music_folder_id)
        .map_err(|error| error.to_string())
}

pub(crate) fn load_albums(
    loaded: &Arc<Library>,
    music_folder_id: Option<&MusicFolderId>,
    query: &str,
    settings: &LibraryListSettings,
) -> Result<PreparedAlbums, String> {
    let source = loaded
        .albums(music_folder_id)
        .map_err(|error| error.to_string())?;
    let prepared = prepare_collection(source, query, settings, album_matches_query, sort_albums);
    let (details, visible_details) = if settings.layout == LibraryLayout::Detail {
        let mut details = loaded
            .album_details(music_folder_id)
            .map_err(|error| error.to_string())?;
        sort_album_details(Arc::make_mut(&mut details), settings);
        let visible = filter_shared(&details, query, |detail, query| {
            album_matches_query(&detail.summary, query)
        });
        (Some(details), Some(visible))
    } else {
        (None, None)
    };
    Ok(PreparedAlbums {
        source: prepared.source,
        visible: prepared.visible,
        details,
        visible_details,
    })
}

pub(crate) fn load_tracks(
    loaded: &Arc<Library>,
    music_folder_id: Option<&MusicFolderId>,
    settings: &LibraryListSettings,
) -> Result<TrackList, String> {
    loaded
        .track_list(
            music_folder_id,
            settings.sort_key.track_sort(),
            settings.descending,
        )
        .map_err(|error| error.to_string())
}

pub(crate) fn load_favorite_tracks(
    loaded: &Arc<Library>,
    music_folder_id: Option<&MusicFolderId>,
    settings: &LibraryListSettings,
) -> Result<TrackList, String> {
    loaded
        .favorite_track_list(
            music_folder_id,
            settings.sort_key.track_sort(),
            settings.descending,
        )
        .map_err(|error| error.to_string())
}

pub(crate) fn load_history_tracks(
    loaded: &Arc<Library>,
    music_folder_id: Option<&MusicFolderId>,
) -> Result<TrackList, String> {
    loaded
        .history_track_list(music_folder_id)
        .map_err(|error| error.to_string())
}

pub(crate) fn load_artists(
    loaded: &Arc<Library>,
    music_folder_id: Option<&MusicFolderId>,
    album_artists: bool,
    query: &str,
    settings: &LibraryListSettings,
) -> Result<PreparedCollection<ArtistSummary>, String> {
    let source = if album_artists {
        loaded.album_artists(music_folder_id)
    } else {
        loaded.artists(music_folder_id)
    }
    .map_err(|error| error.to_string())?;
    Ok(prepare_collection(
        source,
        query,
        settings,
        artist_matches_query,
        sort_artists,
    ))
}

pub(crate) fn load_playlists(
    loaded: &Arc<Library>,
    query: &str,
    settings: &LibraryListSettings,
) -> Result<PreparedCollection<PlaylistSummary>, String> {
    let source = loaded.playlists().map_err(|error| error.to_string())?;
    Ok(prepare_collection(
        source,
        query,
        settings,
        playlist_matches_query,
        sort_playlists,
    ))
}

fn prepare_collection<T: Clone>(
    mut source: Arc<[T]>,
    query: &str,
    settings: &LibraryListSettings,
    matches: impl Fn(&T, &str) -> bool,
    sort: impl Fn(&mut [T], &LibraryListSettings),
) -> PreparedCollection<T> {
    sort(Arc::make_mut(&mut source), settings);
    let visible = filter_shared(&source, query, matches);
    PreparedCollection { source, visible }
}

fn album_details_for_route(
    cache: &RefCell<Option<Arc<[AlbumDetail]>>>,
    query: &str,
    layout: LibraryLayout,
) -> Option<Arc<[AlbumDetail]>> {
    if layout != LibraryLayout::Detail {
        return None;
    }
    let normalized = query.trim().to_lowercase();
    let cache = cache.borrow();
    let Some(details) = cache.as_ref() else {
        return Some(Arc::from([]));
    };
    if normalized.is_empty() {
        return Some(Arc::clone(details));
    }
    Some(
        details
            .iter()
            .filter(|detail| album_matches_query(&detail.summary, &normalized))
            .cloned()
            .collect::<Vec<_>>()
            .into(),
    )
}

fn filter_shared<T: Clone>(
    source: &Arc<[T]>,
    query: &str,
    matches: impl Fn(&T, &str) -> bool,
) -> Arc<[T]> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Arc::clone(source);
    }
    source
        .iter()
        .filter(|item| matches(item, &query))
        .cloned()
        .collect::<Vec<_>>()
        .into()
}

fn log_route_setup(
    route: Route,
    layout: LibraryLayout,
    item_count: usize,
    model_ms: u64,
    content_ms: u64,
    shell_ms: u64,
    started: Instant,
) {
    let total_ms = started.elapsed().as_millis() as u64;
    info!(
        ?route,
        ?layout,
        source = "loaded-library",
        item_count,
        model_ms,
        content_ms,
        shell_ms,
        total_ms,
        "library route setup timing"
    );
    if total_ms >= SLOW_LIBRARY_ROUTE_SETUP_MS {
        warn!(
            ?route,
            ?layout,
            item_count,
            total_ms,
            "slow library route setup"
        );
    }
}

pub(crate) fn install_embedded_track_scroll_latch(
    scroller: &gtk::ScrolledWindow,
    header_height: i32,
) {
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
        if pointer_is_embedded_table_header(pointer_y.get(), header_height)
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

fn pointer_is_embedded_table_header(y: Option<f64>, header_height: i32) -> bool {
    y.is_some_and(|y| y >= 0.0 && y < f64::from(header_height))
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
    use std::time::{Duration, Instant};

    use super::{
        EMBEDDED_SCROLL_LATCH_MS, parent_scroll_is_latched, pointer_is_embedded_table_header,
    };

    const COMPACT_HEADER_HEIGHT: i32 = 30;

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
        assert!(pointer_is_embedded_table_header(
            Some(0.0),
            COMPACT_HEADER_HEIGHT
        ));
        assert!(pointer_is_embedded_table_header(
            Some(f64::from(COMPACT_HEADER_HEIGHT) - 1.0),
            COMPACT_HEADER_HEIGHT
        ));
        assert!(!pointer_is_embedded_table_header(
            Some(f64::from(COMPACT_HEADER_HEIGHT)),
            COMPACT_HEADER_HEIGHT
        ));
        assert!(!pointer_is_embedded_table_header(
            Some(64.0),
            COMPACT_HEADER_HEIGHT
        ));
        assert!(!pointer_is_embedded_table_header(
            None,
            COMPACT_HEADER_HEIGHT
        ));
    }
}
