use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use artwork::{ArtworkBinding, ArtworkBindings};
use gtk::{gio, glib};
use library::{Album, Artist, Library, SearchResults, Track, TrackId};
use localization::{msgid, tr};

use crate::layout::width_allocation_owner;
use crate::localization::{bind_search_placeholder, localized_label};
use crate::runtime::{SelectedLibrary, SelectedSourceHandle};
use crate::shell::Shell;
use crate::shell::cover::presentation::stable_seed;
use crate::shell::cover::{ArtworkTile, LARGE_COVER_SIZE, THUMB_COVER_SIZE};
use crate::shell::route::{MountedRoute, MountedRouteItemNavigation};
use crate::{LibraryField, LibraryLayout, LibraryListKey, LibraryListSettings};

use super::cards;
use super::collections::{
    CollectionTableProjection, LibraryCollectionProjection, LibraryPresentationProjection,
    dynamic_collection_table, library_route_inset,
};
use super::columns::{
    TrackMergedColumnValues, TrackRowPlayingIndicator, row_index_column_with_width, text_column,
    track_column_fit_width, track_column_width, track_merged_column,
    track_row_index_column_with_width,
};
use super::grid_cells::{
    CollectionGridCardCell, ReusableCollectionGridCell, collection_grid_cover_shell,
    collection_grid_with_card_widths,
};
use super::library_fields::{
    ALBUM_COLLECTION_GRID_MAX_CARD_WIDTH, ALBUM_COLLECTION_GRID_MIN_CARD_WIDTH,
    COLLECTION_GRID_MAX_CARD_WIDTH, COLLECTION_GRID_MIN_CARD_WIDTH, album_item_field, apply_desc,
    artist_item_field, column_width, item_at, item_at_from_item, track_field,
};
use super::route::Route;
use super::route_layout::ROUTE_TOP_MARGIN;
use super::route_shell::LibraryToolbarProjection;
use super::table_sizing::route_column_view_initial_width;

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum SearchCategory {
    #[default]
    Tracks,
    Albums,
    Artists,
}

impl SearchCategory {
    const ALL: [Self; 3] = [Self::Tracks, Self::Albums, Self::Artists];

    const fn name(self) -> &'static str {
        match self {
            Self::Tracks => "tracks",
            Self::Albums => "albums",
            Self::Artists => "artists",
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Tracks => msgid("Tracks"),
            Self::Albums => msgid("Albums"),
            Self::Artists => msgid("Artists"),
        }
    }

    const fn icon_name(self) -> &'static str {
        match self {
            Self::Tracks => "rufin-route-tracks-symbolic",
            Self::Albums => "rufin-route-albums-symbolic",
            Self::Artists => "rufin-route-artists-symbolic",
        }
    }

    const fn list_key(self) -> LibraryListKey {
        match self {
            Self::Tracks => LibraryListKey::Tracks,
            Self::Albums => LibraryListKey::Albums,
            Self::Artists => LibraryListKey::Artists,
        }
    }

    fn sort_fields(self) -> &'static [LibraryField] {
        match self {
            Self::Tracks => crate::available_sort_fields(LibraryListKey::Tracks),
            Self::Albums => &[
                LibraryField::Title,
                LibraryField::AlbumArtist,
                LibraryField::Year,
                LibraryField::ReleaseDate,
                LibraryField::DateAdded,
                LibraryField::LastPlayed,
                LibraryField::PlayCount,
                LibraryField::UserRating,
                LibraryField::Favorite,
            ],
            Self::Artists => &[
                LibraryField::Title,
                LibraryField::LastPlayed,
                LibraryField::PlayCount,
                LibraryField::UserRating,
                LibraryField::Favorite,
            ],
        }
    }

    fn result_count(self, results: &SearchResults) -> usize {
        match self {
            Self::Tracks => results.tracks.len(),
            Self::Albums => results.albums.len(),
            Self::Artists => results.artists.len(),
        }
    }
}

#[derive(Clone)]
struct SearchAlbum {
    album: Album,
    artwork: ArtworkBinding,
    navigable: bool,
}

#[derive(Clone)]
struct SearchArtist {
    artist: Artist,
    artwork: ArtworkBinding,
    navigable: bool,
}

#[derive(Clone)]
struct SearchTrack {
    track: Track,
    artwork: ArtworkBinding,
}

struct PreparedSearchResults {
    counts: [usize; 3],
    tracks: Vec<SearchTrack>,
    albums: Vec<SearchAlbum>,
    artists: Vec<SearchArtist>,
}

trait SearchGridItem: Clone + 'static {
    fn title(&self) -> &str;
    fn subtitle(&self) -> &str;
    fn artwork(&self) -> ArtworkBinding;
    fn seed(&self) -> u32;
    fn field(&self, field: LibraryField) -> String;
    fn route(&self) -> Option<Route>;

    fn activate(&self, shell: &Rc<Shell>) {
        if let Some(route) = self.route() {
            shell.navigate(route);
        }
    }

    fn activatable(&self) -> bool {
        self.route().is_some()
    }
}

impl SearchGridItem for SearchAlbum {
    fn title(&self) -> &str {
        &self.album.title
    }

    fn artwork(&self) -> ArtworkBinding {
        self.artwork.clone()
    }

    fn subtitle(&self) -> &str {
        &self.album.artist
    }

    fn seed(&self) -> u32 {
        stable_seed(self.album.id.as_str())
    }

    fn field(&self, field: LibraryField) -> String {
        album_item_field(&self.album, field)
    }

    fn route(&self) -> Option<Route> {
        self.navigable
            .then(|| Route::AlbumDetail(self.album.id.clone()))
    }
}

impl SearchGridItem for SearchArtist {
    fn title(&self) -> &str {
        &self.artist.name
    }

    fn artwork(&self) -> ArtworkBinding {
        self.artwork.clone()
    }

    fn subtitle(&self) -> &str {
        ""
    }

    fn seed(&self) -> u32 {
        stable_seed(self.artist.id.as_str())
    }

    fn field(&self, field: LibraryField) -> String {
        artist_item_field(&self.artist, field)
    }

    fn route(&self) -> Option<Route> {
        self.navigable
            .then(|| Route::ArtistDetail(self.artist.id.clone()))
    }
}

impl SearchGridItem for SearchTrack {
    fn title(&self) -> &str {
        &self.track.title
    }

    fn subtitle(&self) -> &str {
        &self.track.artist
    }

    fn artwork(&self) -> ArtworkBinding {
        self.artwork.clone()
    }

    fn seed(&self) -> u32 {
        stable_seed(self.track.id.as_str())
    }

    fn field(&self, field: LibraryField) -> String {
        track_field(&self.track, field)
    }

    fn route(&self) -> Option<Route> {
        None
    }

    fn activate(&self, shell: &Rc<Shell>) {
        if let Some(selected) = shell.library.selected.borrow().as_ref() {
            shell
                .products
                .playback
                .queue
                .play_loaded(selected.one_track(self.track.clone(), playback::QueuePlacement::Now));
        }
    }

    fn activatable(&self) -> bool {
        true
    }
}

struct SearchGridCell<T: SearchGridItem> {
    body: CollectionGridCardCell,
    shell: Rc<Shell>,
    cover: ArtworkTile,
    cover_button: gtk::Button,
    current: Rc<RefCell<Option<T>>>,
    cover_size: i32,
}

impl<T: SearchGridItem> SearchGridCell<T> {
    fn new(shell: Rc<Shell>, fields: &[LibraryField], cover_size: i32) -> Self {
        let current = Rc::new(RefCell::new(None::<T>));
        let cover_button = collection_grid_cover_shell();
        let cover = ArtworkTile::new_elastic_square(0);
        cover_button.set_child(Some(&cover.widget()));
        let open_shell = Rc::clone(&shell);
        let open_item = Rc::clone(&current);
        cover_button.connect_clicked(move |_| {
            if let Some(item) = open_item.borrow().as_ref() {
                item.activate(&open_shell);
            }
        });
        let transport = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let cover_frame = cards::square_cover_frame(&cover_button, &transport);
        let body = CollectionGridCardCell::new(&shell, fields, cover_frame.upcast());
        Self {
            body,
            shell,
            cover,
            cover_button,
            current,
            cover_size,
        }
    }
}

impl<T: SearchGridItem> ReusableCollectionGridCell<T> for SearchGridCell<T> {
    fn widget(&self) -> gtk::Widget {
        self.body.widget()
    }

    fn activatable(&self, item: &T) -> bool {
        item.activatable()
    }

    fn bind(&self, _: u32, item: T) {
        self.shell.bind_artwork_tile(
            &self.cover,
            item.artwork(),
            item.seed(),
            self.cover_size,
            LARGE_COVER_SIZE,
        );
        self.body
            .bind(item.title(), |field| (item.field(field), None));
        let activatable = item.activatable();
        self.cover_button.set_can_target(activatable);
        self.cover_button.set_focusable(activatable);
        *self.current.borrow_mut() = Some(item);
    }

    fn clear(&self) {
        self.shell.clear_artwork_tile(&self.cover);
        self.body.clear();
        self.cover_button.set_can_target(false);
        self.cover_button.set_focusable(false);
        self.current.borrow_mut().take();
    }

    fn apply_fields(&self, fields: &[LibraryField]) {
        self.body.replace_fields(&self.shell, fields);
        if let Some(item) = self.current.borrow().as_ref() {
            self.body
                .bind(item.title(), |field| (item.field(field), None));
        }
    }
}

struct SearchRouteProjection {
    root: gtk::Widget,
    shell: Weak<Shell>,
    source_id: library::SourceId,
    source_session_epoch: playback::SourceSessionEpoch,
    source: SelectedSourceHandle,
    library: Arc<Library>,
    search: gtk::SearchEntry,
    toolbar_controls: gtk::Box,
    status: gtk::Stack,
    error: gtk::Label,
    result_pages: [gtk::Stack; 3],
    tracks: gio::ListStore,
    albums: gio::ListStore,
    artists: gio::ListStore,
    track_collection: LibraryCollectionProjection,
    album_collection: LibraryCollectionProjection,
    artist_collection: LibraryCollectionProjection,
    toolbars: [LibraryToolbarProjection; 3],
    item_navigation: MountedRouteItemNavigation,
    generation: Cell<u64>,
    debounce: RefCell<Option<glib::SourceId>>,
}

impl SearchRouteProjection {
    fn new(shell: &Rc<Shell>, selected: &SelectedLibrary) -> Rc<Self> {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 14);
        wrapper.add_css_class("route-content");
        wrapper.add_css_class("search-route");
        wrapper.set_margin_top(ROUTE_TOP_MARGIN);
        wrapper.set_margin_bottom(8);
        wrapper.set_hexpand(true);
        wrapper.set_vexpand(true);

        let search = gtk::SearchEntry::new();
        search.set_hexpand(true);
        search.set_width_request(1);
        bind_search_placeholder(&search, "Search");

        let tracks = gio::ListStore::new::<glib::BoxedAnyObject>();
        let albums = gio::ListStore::new::<glib::BoxedAnyObject>();
        let artists = gio::ListStore::new::<glib::BoxedAnyObject>();
        let track_collection = search_track_collection(shell, &tracks);
        let album_collection = search_grid_collection::<SearchAlbum>(
            shell,
            &albums,
            LibraryListKey::Albums,
            ALBUM_COLLECTION_GRID_MIN_CARD_WIDTH,
            ALBUM_COLLECTION_GRID_MAX_CARD_WIDTH,
        );
        let artist_collection = search_grid_collection::<SearchArtist>(
            shell,
            &artists,
            LibraryListKey::Artists,
            COLLECTION_GRID_MIN_CARD_WIDTH,
            COLLECTION_GRID_MAX_CARD_WIDTH,
        );
        let track_navigation = track_collection.item_navigation();
        let album_navigation = album_collection.item_navigation();
        let artist_navigation = artist_collection.item_navigation();
        let result_pages = [
            search_result_page(shell, track_collection.scrolling_widget()),
            search_result_page(shell, album_collection.scrolling_widget()),
            search_result_page(shell, artist_collection.scrolling_widget()),
        ];

        let results = adw::ViewStack::builder()
            .hexpand(true)
            .vexpand(true)
            .build();
        for (category, page) in SearchCategory::ALL.into_iter().zip(result_pages.iter()) {
            results.add_titled_with_icon(
                page,
                Some(category.name()),
                &tr(category.title()),
                category.icon_name(),
            );
        }
        results.set_visible_child_name(SearchCategory::default().name());
        let switcher = adw::ViewSwitcher::builder()
            .policy(adw::ViewSwitcherPolicy::Wide)
            .stack(&results)
            .build();
        switcher.set_halign(gtk::Align::Start);
        let toolbars = SearchCategory::ALL.map(|category| {
            shell.library_toolbar_projection_without_detail(
                category.list_key(),
                gtk::SearchEntry::new(),
                category.sort_fields(),
            )
        });
        let controls_stack = gtk::Stack::new();
        controls_stack.set_halign(gtk::Align::End);
        for (category, toolbar) in SearchCategory::ALL.into_iter().zip(toolbars.iter()) {
            controls_stack.add_named(&toolbar.detach_controls(), Some(category.name()));
        }
        controls_stack.set_visible_child_name(SearchCategory::default().name());
        let controls_for_page = controls_stack.clone();
        results.connect_visible_child_notify(move |results| {
            if let Some(name) = results.visible_child_name() {
                controls_for_page.set_visible_child_name(&name);
            }
        });
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        controls.append(&controls_stack);
        shell.set_current_library_toolbar_controls(&controls);
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        toolbar.add_css_class("track-toolbar");
        toolbar.set_hexpand(true);
        toolbar.set_width_request(1);
        toolbar.append(&search);
        toolbar.append(&controls);
        let search_header = gtk::Box::new(gtk::Orientation::Vertical, 8);
        search_header.set_hexpand(true);
        search_header.append(&toolbar);
        search_header.append(&switcher);
        wrapper.append(&library_route_inset(search_header.upcast()));
        shell.set_route_search(Some(search.clone()));

        let status = gtk::Stack::new();
        status.set_hexpand(true);
        status.set_vexpand(true);
        status.add_named(
            &library_route_inset(shell.route_empty_view(msgid("Type to search"))),
            Some("initial"),
        );
        status.add_named(&search_loading(), Some("loading"));
        let error = gtk::Label::new(None);
        error.add_css_class("muted");
        error.set_justify(gtk::Justification::Center);
        error.set_wrap(true);
        error.set_max_width_chars(48);
        status.add_named(&centered_widget(error.clone().upcast()), Some("error"));
        status.add_named(&results, Some("results"));
        status.set_visible_child_name("initial");
        wrapper.append(&status);

        let navigation_stack = results.clone();
        let item_navigation =
            Rc::new(
                move |direction| match navigation_stack.visible_child_name().as_deref() {
                    Some("albums") => album_navigation(direction),
                    Some("artists") => artist_navigation(direction),
                    _ => track_navigation(direction),
                },
            ) as MountedRouteItemNavigation;

        let root = width_allocation_owner(&wrapper, |_| {}).upcast();
        let projection = Rc::new(Self {
            root,
            shell: Rc::downgrade(shell),
            source_id: selected.source_id.clone(),
            source_session_epoch: selected.source_session_epoch,
            source: selected.operations.clone(),
            library: Arc::clone(&selected.library),
            search,
            toolbar_controls: controls,
            status,
            error,
            result_pages,
            tracks,
            albums,
            artists,
            track_collection,
            album_collection,
            artist_collection,
            toolbars,
            item_navigation,
            generation: Cell::new(0),
            debounce: RefCell::new(None),
        });
        projection.connect_search();
        projection
    }

    fn connect_search(self: &Rc<Self>) {
        let projection = Rc::downgrade(self);
        self.search.connect_search_changed(move |entry| {
            let Some(projection) = projection.upgrade() else {
                return;
            };
            if let Some(pending) = projection.debounce.borrow_mut().take() {
                pending.remove();
            }
            let query = entry.text().trim().to_string();
            if query.is_empty() {
                projection.reset();
                return;
            }
            let delayed = Rc::downgrade(&projection);
            let source = glib::timeout_add_local_once(SEARCH_DEBOUNCE, move || {
                let Some(projection) = delayed.upgrade() else {
                    return;
                };
                projection.debounce.borrow_mut().take();
                projection.submit(query);
            });
            projection.debounce.replace(Some(source));
        });
    }

    fn reset(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        self.clear_results();
        self.status.set_visible_child_name("initial");
    }

    fn submit(self: &Rc<Self>, query: String) {
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        self.clear_results();
        self.status.set_visible_child_name("loading");

        if self.shell.upgrade().is_none() {
            return;
        }
        let request = library::SearchRequest::new(query);
        let receiver = self.source.search(request.clone());
        let library = Arc::clone(&self.library);
        let fallback_library = Arc::clone(&library);
        let projection = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let live_result = receiver
                .recv()
                .await
                .unwrap_or_else(|_| Err("the Search request stopped".to_string()));
            let result = match live_result {
                Ok(results) => Ok(results),
                Err(live_error) => {
                    let offline =
                        gio::spawn_blocking(move || fallback_library.search(&request)).await;
                    match offline {
                        Ok(offline) => offline_search_result(live_error, offline),
                        Err(_) => Err(live_error),
                    }
                }
            };
            let result = match result {
                Ok(results) => {
                    gio::spawn_blocking(move || prepare_search_results(&library, results))
                        .await
                        .map_err(|_| "the Search result preparation stopped".to_string())
                }
                Err(error) => Err(error),
            };
            let Some(projection) = projection.upgrade() else {
                return;
            };
            if projection.generation.get() != generation || !projection.is_active() {
                return;
            }
            match result {
                Ok(results) => projection.apply(results),
                Err(error) => projection.show_error(&error),
            }
        });
    }

    fn is_active(&self) -> bool {
        let Some(shell) = self.shell.upgrade() else {
            return false;
        };
        if shell.navigation.routes.borrow().current() != &Route::Search {
            return false;
        }
        shell
            .library
            .selected
            .borrow()
            .as_ref()
            .is_some_and(|selected| {
                selected.source_id == self.source_id
                    && selected.source_session_epoch == self.source_session_epoch
            })
    }

    fn apply(&self, results: PreparedSearchResults) {
        replace_model(&self.tracks, results.tracks);
        replace_model(&self.albums, results.albums);
        replace_model(&self.artists, results.artists);
        for (page, count) in self.result_pages.iter().zip(results.counts) {
            page.set_visible_child_name(if count == 0 { "empty" } else { "content" });
        }
        if let Some(shell) = self.shell.upgrade() {
            self.apply_display_settings(&shell);
        }
        self.status.set_visible_child_name("results");
    }

    fn show_error(&self, message: &str) {
        self.clear_results();
        self.error.set_label(message);
        self.status.set_visible_child_name("error");
    }

    fn clear_results(&self) {
        self.tracks.remove_all();
        self.albums.remove_all();
        self.artists.remove_all();
    }

    fn resume(&self) {
        if let Some(shell) = self.shell.upgrade() {
            self.apply_display_settings(&shell);
            shell.set_current_library_toolbar_controls(&self.toolbar_controls);
            shell.set_route_search(Some(self.search.clone()));
        }
    }

    fn apply_display_settings(&self, shell: &Shell) {
        let current = shell.settings.current.borrow();
        let tracks = current.library_list(LibraryListKey::Tracks);
        self.toolbars[0].apply(LibraryListKey::Tracks, &tracks);
        self.track_collection.apply_settings(&tracks);
        sort_model::<SearchTrack>(&self.tracks, &tracks, sort_search_tracks);
        let albums = current.library_list(LibraryListKey::Albums);
        self.toolbars[1].apply(LibraryListKey::Albums, &albums);
        let mut album_presentation = albums.clone();
        if album_presentation.layout == LibraryLayout::Detail {
            album_presentation.layout = LibraryLayout::Grid;
        }
        self.album_collection.apply_settings(&album_presentation);
        sort_model::<SearchAlbum>(&self.albums, &albums, sort_search_albums);
        let artists = current.library_list(LibraryListKey::Artists);
        self.toolbars[2].apply(LibraryListKey::Artists, &artists);
        self.artist_collection.apply_settings(&artists);
        sort_model::<SearchArtist>(&self.artists, &artists, sort_search_artists);
    }

    fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }

    fn item_navigation(&self) -> MountedRouteItemNavigation {
        Rc::clone(&self.item_navigation)
    }
}

fn offline_search_result(
    live_error: String,
    offline: library::LibraryQueryResult<SearchResults>,
) -> Result<SearchResults, String> {
    offline.map_err(|_| live_error)
}

fn prepare_search_results(library: &Library, results: SearchResults) -> PreparedSearchResults {
    let counts = SearchCategory::ALL.map(|category| category.result_count(&results));
    let bindings = ArtworkBindings::new(library);
    let tracks = results
        .tracks
        .into_iter()
        .map(|track| SearchTrack {
            artwork: bindings.track(&track).into_binding(),
            track,
        })
        .collect();
    let albums = results
        .albums
        .into_iter()
        .map(|album| {
            let bound = bindings.album(&album);
            SearchAlbum {
                artwork: bound.binding().clone(),
                navigable: bound.is_library_item(),
                album,
            }
        })
        .collect();
    let artists = results
        .artists
        .into_iter()
        .map(|artist| {
            let bound = bindings.artist(&artist);
            SearchArtist {
                artwork: bound.binding().clone(),
                navigable: bound.is_library_item(),
                artist,
            }
        })
        .collect();
    PreparedSearchResults {
        counts,
        tracks,
        albums,
        artists,
    }
}

fn sort_model<T: Clone + 'static>(
    model: &gio::ListStore,
    settings: &LibraryListSettings,
    sort: impl Fn(&mut [T], &LibraryListSettings),
) {
    let mut values = (0..model.n_items())
        .filter_map(|position| item_at::<T>(model, position))
        .collect::<Vec<_>>();
    sort(&mut values, settings);
    replace_model(model, values);
}

fn sort_search_tracks(tracks: &mut [SearchTrack], settings: &LibraryListSettings) {
    tracks.sort_by(|left, right| {
        ::library::compare_tracks(
            &left.track,
            &right.track,
            settings.sort_key.track_sort(),
            settings.descending,
        )
    });
}

fn sort_search_albums(albums: &mut [SearchAlbum], settings: &LibraryListSettings) {
    albums.sort_by(|left, right| {
        apply_desc(
            compare_search_album(left, right, settings.sort_key),
            settings.descending,
        )
    });
}

fn compare_search_album(left: &SearchAlbum, right: &SearchAlbum, field: LibraryField) -> Ordering {
    let left = &left.album;
    let right = &right.album;
    let ordering = match field {
        LibraryField::AlbumArtist | LibraryField::Artist => {
            compare_text(&left.artist, &right.artist)
        }
        LibraryField::Year => left.year.cmp(&right.year),
        LibraryField::ReleaseDate => left.release_date.cmp(&right.release_date),
        LibraryField::DateAdded => left.date_added.cmp(&right.date_added),
        LibraryField::LastPlayed => left.last_played.cmp(&right.last_played),
        LibraryField::PlayCount => left.play_count.cmp(&right.play_count),
        LibraryField::UserRating => left.user_rating.cmp(&right.user_rating),
        LibraryField::Favorite => left.favorite.cmp(&right.favorite),
        _ => compare_text(&left.title, &right.title),
    };
    ordering.then_with(|| compare_text(&left.title, &right.title))
}

fn sort_search_artists(artists: &mut [SearchArtist], settings: &LibraryListSettings) {
    artists.sort_by(|left, right| {
        apply_desc(
            compare_search_artist(left, right, settings.sort_key),
            settings.descending,
        )
    });
}

fn compare_search_artist(
    left: &SearchArtist,
    right: &SearchArtist,
    field: LibraryField,
) -> Ordering {
    let left = &left.artist;
    let right = &right.artist;
    let ordering = match field {
        LibraryField::LastPlayed => left.last_played.cmp(&right.last_played),
        LibraryField::PlayCount => left.play_count.cmp(&right.play_count),
        LibraryField::UserRating => left.user_rating.cmp(&right.user_rating),
        LibraryField::Favorite => left.favorite.cmp(&right.favorite),
        _ => compare_text(&left.name, &right.name),
    };
    ordering.then_with(|| compare_text(&left.name, &right.name))
}

fn compare_text(left: &str, right: &str) -> Ordering {
    left.to_lowercase().cmp(&right.to_lowercase())
}

impl Drop for SearchRouteProjection {
    fn drop(&mut self) {
        if let Some(pending) = self.debounce.borrow_mut().take() {
            pending.remove();
        }
    }
}

impl Shell {
    pub(crate) fn search_route(self: &Rc<Self>, selected: &SelectedLibrary) -> MountedRoute {
        let projection = SearchRouteProjection::new(self, selected);
        let resume_projection = Rc::clone(&projection);
        MountedRoute::new(
            projection.widget(),
            Rc::new(move || resume_projection.resume()),
        )
        .with_item_navigation(projection.item_navigation())
    }
}

fn search_track_collection(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
) -> LibraryCollectionProjection {
    let settings = shell
        .settings
        .current
        .borrow()
        .library_list(LibraryListKey::Tracks);
    let fields = settings.grid_fields.clone();
    let build_shell = Rc::clone(shell);
    let build_model = model.clone();
    LibraryCollectionProjection::new(
        settings,
        Rc::new(move |layout| match layout {
            LibraryLayout::Grid => search_grid_presentation::<SearchTrack>(
                &build_shell,
                &build_model,
                COLLECTION_GRID_MIN_CARD_WIDTH,
                COLLECTION_GRID_MAX_CARD_WIDTH,
                &fields,
            ),
            LibraryLayout::Row | LibraryLayout::Detail => {
                LibraryPresentationProjection::Row(search_track_table(&build_shell, &build_model))
            }
        }),
    )
}

fn search_track_table(shell: &Rc<Shell>, model: &gio::ListStore) -> CollectionTableProjection {
    let key = LibraryListKey::Tracks;
    let playing = TrackRowPlayingIndicator::new();
    install_search_track_playing_state(shell, model, &playing);
    let fields = shell.settings.current.borrow().library_list(key).row_fields;
    let queue = shell.products.playback.queue.clone();
    let selected = shell.library.selected.borrow().as_ref().cloned();
    let activate = Box::new(move |_, track: SearchTrack| {
        if let Some(selected) = selected.as_ref() {
            queue.play_loaded(selected.one_track(track.track, playback::QueuePlacement::Now));
        }
    });
    let column_shell = Rc::clone(shell);
    let column_playing = playing;
    let table = dynamic_collection_table(
        shell,
        key,
        model.clone(),
        &fields,
        Vec::new(),
        move |field| search_track_column(&column_shell, field, &column_playing),
        move |field| track_column_fit_width(key, field),
        false,
        Some(activate),
        None,
        route_column_view_initial_width(shell),
    );
    table.widget().add_css_class("track-list");
    table
}

fn search_track_column(
    shell: &Rc<Shell>,
    field: LibraryField,
    playing: &TrackRowPlayingIndicator,
) -> gtk::ColumnViewColumn {
    let key = LibraryListKey::Tracks;
    let width = track_column_width(key, field);
    match field {
        LibraryField::RowIndex => track_row_index_column_with_width(width, playing.clone()),
        LibraryField::Image => search_item_image_column::<SearchTrack>(shell, field.title(), width),
        LibraryField::TitleMerged => track_merged_column(
            shell,
            "Title",
            width,
            playing.clone(),
            TrackMergedColumnValues {
                track: |track: &SearchTrack| track.track.clone(),
                artwork: |track: &SearchTrack| track.artwork.clone(),
                title: |track: &SearchTrack| track.track.title.clone(),
                subtitle: |track: &SearchTrack| track.track.artist.clone(),
                seed: |track: &SearchTrack| stable_seed(track.track.id.as_str()),
                subtitle_route: |_: &SearchTrack| None,
                subtitle_link: false,
                context_menu: false,
            },
        ),
        _ => text_column(field.title(), width, move |track: &SearchTrack| {
            track_field(&track.track, field)
        }),
    }
}

fn install_search_track_playing_state(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    indicator: &TrackRowPlayingIndicator,
) {
    let current = Rc::new(RefCell::new(None::<(TrackId, bool)>));
    let changed_current = Rc::clone(&current);
    let changed_indicator = indicator.clone();
    model.connect_items_changed(move |model, _, _, _| {
        let current = changed_current.borrow();
        let position = current
            .as_ref()
            .and_then(|(track_id, _)| search_track_position(model, track_id))
            .unwrap_or(gtk::INVALID_LIST_POSITION);
        changed_indicator.set_position(position);
        changed_indicator.set_paused(current.as_ref().is_some_and(|(_, paused)| *paused));
    });
    let source_id = shell
        .library
        .selected
        .borrow()
        .as_ref()
        .map(|selected| selected.source_id.clone());
    let selection_model = model.clone();
    let selection_current = Rc::clone(&current);
    let selection_indicator = indicator.clone();
    shell.register_current_route_track_selection(Rc::new(move |playing| {
        let playing = playing.filter(|playing| source_id.as_ref() == Some(&playing.source_id));
        let position = playing
            .and_then(|playing| search_track_position(&selection_model, &playing.track_id))
            .unwrap_or(gtk::INVALID_LIST_POSITION);
        selection_indicator.set_position(position);
        selection_indicator.set_paused(playing.is_some_and(|playing| playing.paused));
        selection_current
            .replace(playing.map(|playing| (playing.track_id.clone(), playing.paused)));
        true
    }));
}

fn search_track_position(model: &impl IsA<gio::ListModel>, track_id: &TrackId) -> Option<u32> {
    (0..model.n_items()).find(|position| {
        item_at::<SearchTrack>(model, *position).is_some_and(|track| track.track.id == *track_id)
    })
}

fn search_item_table<T: SearchGridItem>(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    key: LibraryListKey,
) -> CollectionTableProjection {
    let fields = shell.settings.current.borrow().library_list(key).row_fields;
    let activate_shell = Rc::clone(shell);
    let activate = Box::new(move |_, item: T| item.activate(&activate_shell));
    let column_shell = Rc::clone(shell);
    dynamic_collection_table(
        shell,
        key,
        model.clone(),
        &fields,
        Vec::new(),
        move |field| search_item_column::<T>(&column_shell, field),
        column_width,
        true,
        Some(activate),
        None,
        route_column_view_initial_width(shell),
    )
}

fn search_item_column<T: SearchGridItem>(
    shell: &Rc<Shell>,
    field: LibraryField,
) -> gtk::ColumnViewColumn {
    let width = column_width(field);
    match field {
        LibraryField::RowIndex => row_index_column_with_width(width),
        LibraryField::Image => search_item_image_column::<T>(shell, field.title(), width),
        LibraryField::TitleMerged => search_item_merged_column::<T>(shell, "Title", width),
        _ => text_column(field.title(), width, move |item: &T| item.field(field)),
    }
}

fn search_item_image_column<T: SearchGridItem>(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let bind_shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(value) = item_at_from_item::<T>(item) else {
            return;
        };
        item.set_child(Some(&bind_shell.cover_tile_for_candidates(
            value.artwork(),
            value.seed(),
            48,
            THUMB_COVER_SIZE,
        )));
    });
    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            item.set_child(None::<&gtk::Widget>);
        }
    });
    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column
}

fn search_item_merged_column<T: SearchGridItem>(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            item.set_child(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 10)));
        }
    });
    let bind_shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(value) = item_at_from_item::<T>(item) else {
            return;
        };
        let Some(row) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Box>().ok())
        else {
            return;
        };
        clear_box(&row);
        row.append(&bind_shell.cover_tile_for_candidates(
            value.artwork(),
            value.seed(),
            48,
            THUMB_COVER_SIZE,
        ));
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let title = gtk::Label::new(Some(value.title()));
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        labels.append(&title);
        if !value.subtitle().is_empty() {
            let subtitle = gtk::Label::new(Some(value.subtitle()));
            subtitle.add_css_class("muted");
            subtitle.set_xalign(0.0);
            subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
            labels.append(&subtitle);
        }
        row.append(&labels);
    });
    factory.connect_unbind(|_, item| {
        let Some(row) = item
            .downcast_ref::<gtk::ListItem>()
            .and_then(gtk::ListItem::child)
            .and_then(|child| child.downcast::<gtk::Box>().ok())
        else {
            return;
        };
        clear_box(&row);
    });
    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column
}

fn clear_box(content: &gtk::Box) {
    while let Some(child) = content.first_child() {
        content.remove(&child);
    }
}

fn search_grid_collection<T: SearchGridItem>(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    key: LibraryListKey,
    minimum_card_width: i32,
    maximum_card_width: i32,
) -> LibraryCollectionProjection {
    let settings = shell.settings.current.borrow().library_list(key);
    let fields = settings.grid_fields.clone();
    let build_shell = Rc::clone(shell);
    let build_model = model.clone();
    LibraryCollectionProjection::new(
        settings,
        Rc::new(move |layout| match layout {
            LibraryLayout::Row => LibraryPresentationProjection::Row(search_item_table::<T>(
                &build_shell,
                &build_model,
                key,
            )),
            LibraryLayout::Grid | LibraryLayout::Detail => search_grid_presentation::<T>(
                &build_shell,
                &build_model,
                minimum_card_width,
                maximum_card_width,
                &fields,
            ),
        }),
    )
}

fn search_grid_presentation<T: SearchGridItem>(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    minimum_card_width: i32,
    maximum_card_width: i32,
    fields: &[LibraryField],
) -> LibraryPresentationProjection {
    let cell_shell = Rc::clone(shell);
    let activate_shell = Rc::clone(shell);
    let grid = collection_grid_with_card_widths(
        model.clone(),
        minimum_card_width,
        maximum_card_width,
        fields,
        move |fields| SearchGridCell::<T>::new(Rc::clone(&cell_shell), fields, maximum_card_width),
        move |_, item: T| item.activate(&activate_shell),
    );
    LibraryPresentationProjection::Grid(grid)
}

fn search_result_page(shell: &Rc<Shell>, content: gtk::Widget) -> gtk::Stack {
    let page = gtk::Stack::new();
    page.set_hexpand(true);
    page.set_vexpand(true);
    page.add_named(
        &library_route_inset(shell.route_empty_view(msgid(r"No results ¯\_(°╭╮°)_/¯"))),
        Some("empty"),
    );
    page.add_named(&content, Some("content"));
    page.set_visible_child_name("empty");
    page
}

fn replace_model<T: Clone + 'static>(model: &gio::ListStore, values: impl IntoIterator<Item = T>) {
    let additions = values
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}

fn search_loading() -> gtk::Widget {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let spinner = gtk::Spinner::new();
    spinner.set_spinning(true);
    content.append(&spinner);
    content.append(&localized_label(msgid("Searching...")));
    centered_widget(content.upcast())
}

fn centered_widget(widget: gtk::Widget) -> gtk::Widget {
    let center = gtk::CenterBox::new();
    center.set_hexpand(true);
    center.set_vexpand(true);
    center.set_center_widget(Some(&widget));
    center.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;
    use library::{AlbumRelations, ArtistCredit, ArtistId, ImageRef, SourceId};

    #[test]
    fn search_defaults_to_tracks_and_preserves_category_result_counts() {
        assert_eq!(SearchCategory::default(), SearchCategory::Tracks);
        assert_eq!(
            SearchCategory::ALL.map(SearchCategory::name),
            ["tracks", "albums", "artists"]
        );
        assert_eq!(
            SearchCategory::ALL.map(SearchCategory::icon_name),
            [
                "rufin-route-tracks-symbolic",
                "rufin-route-albums-symbolic",
                "rufin-route-artists-symbolic"
            ]
        );
        let results = SearchResults {
            tracks: vec![crate::test_support::track(1, "Track")],
            albums: vec![crate::test_support::album(2, "Album")],
            artists: Vec::new(),
        };
        assert_eq!(
            SearchCategory::ALL.map(|category| category.result_count(&results)),
            [1, 1, 0]
        );
    }

    #[test]
    fn loaded_bindings_cover_live_results_without_replacing_their_labels() {
        let live_track = crate::test_support::track(1, "Live track label");
        let mut loaded_track = crate::test_support::track(1, "Library track label");
        loaded_track.image_ref = Some(ImageRef::new("track-image", None));
        let artist_id = ArtistId::fake(3);
        let live_album = crate::test_support::album(2, "Live album label");
        let mut loaded_album = crate::test_support::album(2, "Library album label");
        loaded_album.image_ref = Some(ImageRef::new("album-image", None));
        loaded_album.relations = AlbumRelations {
            album_artists: vec![ArtistCredit {
                id: artist_id.clone(),
                name: "Library artist label".to_string(),
                musicbrainz_artist_id: None,
            }],
            ..AlbumRelations::default()
        };
        let loaded_artist = Artist {
            id: artist_id.clone(),
            name: "Library artist label".to_string(),
            favorite: false,
            last_played: None,
            play_count: None,
            user_rating: None,
            musicbrainz_artist_id: None,
            image_ref: None,
            local_artwork: None,
        };
        let fixture = crate::test_support::source_fixture_with_artists(
            SourceId::fake(1),
            vec![loaded_album],
            vec![loaded_track],
            vec![loaded_artist],
            Vec::new(),
        );
        let results = SearchResults {
            tracks: vec![live_track],
            albums: vec![live_album],
            artists: vec![Artist {
                id: artist_id,
                name: "Live artist label".to_string(),
                favorite: false,
                last_played: None,
                play_count: None,
                user_rating: None,
                musicbrainz_artist_id: None,
                image_ref: None,
                local_artwork: None,
            }],
        };
        let prepared = prepare_search_results(&fixture.library, results);

        assert_eq!(prepared.tracks[0].track.title, "Live track label");
        assert!(!prepared.tracks[0].artwork.is_empty());
        assert_eq!(prepared.albums[0].album.title, "Live album label");
        assert!(!prepared.albums[0].artwork.is_empty());
        assert_eq!(prepared.artists[0].artist.name, "Live artist label");
        assert!(!prepared.artists[0].artwork.is_empty());
    }

    #[test]
    fn failed_live_search_uses_loaded_results_before_showing_an_error() {
        let offline = SearchResults {
            tracks: vec![crate::test_support::track(1, "Offline result")],
            ..SearchResults::default()
        };
        let recovered = offline_search_result("offline".to_string(), Ok(offline)).unwrap();
        assert_eq!(recovered.tracks[0].title, "Offline result");

        let error = offline_search_result(
            "live failure".to_string(),
            Err(library::LibraryQueryError::Unavailable),
        )
        .unwrap_err();
        assert_eq!(error, "live failure");
    }

    #[test]
    fn live_album_sorting_follows_the_active_toolbox_settings() {
        let mut albums = vec![
            SearchAlbum {
                album: crate::test_support::album(1, "Zulu"),
                artwork: ArtworkBinding::new(),
                navigable: false,
            },
            SearchAlbum {
                album: crate::test_support::album(2, "Alpha"),
                artwork: ArtworkBinding::new(),
                navigable: false,
            },
        ];
        let mut settings = LibraryListSettings::for_key(LibraryListKey::Albums);
        settings.sort_key = LibraryField::Title;
        settings.descending = false;
        sort_search_albums(&mut albums, &settings);
        assert_eq!(albums[0].album.title, "Alpha");
        settings.descending = true;
        sort_search_albums(&mut albums, &settings);
        assert_eq!(albums[0].album.title, "Zulu");
    }
}
