use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::glib;
use rufin_core::{
    Album, AlbumId, Artist, ArtistId, LibraryListKey, Route, Track, TrackId, TrackSortKey,
};
use rufin_provider::FavoriteItemId;

use crate::controller::LibrarySnapshot;

use super::{Shell, set_favorite_button_active};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) enum FavoriteControlKey {
    Album(String),
    Track(String),
    Artist(String),
}

pub(super) type FavoriteControls =
    RefCell<HashMap<FavoriteControlKey, Vec<glib::WeakRef<gtk::Button>>>>;

pub(super) fn album_favorite_key(album_id: &AlbumId) -> FavoriteControlKey {
    FavoriteControlKey::Album(album_id.as_str().to_string())
}

pub(super) fn track_favorite_key(track_id: &TrackId) -> FavoriteControlKey {
    FavoriteControlKey::Track(track_id.as_str().to_string())
}

pub(super) fn artist_favorite_key(artist_id: &ArtistId) -> FavoriteControlKey {
    FavoriteControlKey::Artist(artist_id.as_str().to_string())
}

pub(super) fn favorite_control_key(item_id: &FavoriteItemId) -> FavoriteControlKey {
    match item_id {
        FavoriteItemId::Album(album_id) => album_favorite_key(album_id),
        FavoriteItemId::Track(track_id) => track_favorite_key(track_id),
        FavoriteItemId::Artist(artist_id) => artist_favorite_key(artist_id),
    }
}

pub(super) fn register_favorite_control(
    controls: &FavoriteControls,
    key: FavoriteControlKey,
    button: &gtk::Button,
) {
    let weak = glib::WeakRef::new();
    weak.set(Some(button));
    controls.borrow_mut().entry(key).or_default().push(weak);
}

pub(super) fn update_favorite_controls(
    controls: &FavoriteControls,
    key: &FavoriteControlKey,
    favorite: bool,
) {
    if let Some(buttons) = controls.borrow_mut().get_mut(key) {
        buttons.retain(|button| {
            let Some(button) = button.upgrade() else {
                return false;
            };
            set_favorite_button_active(&button, favorite);
            true
        });
    }
}

pub(super) fn clear_favorite_controls(controls: &FavoriteControls) {
    controls.borrow_mut().clear();
}

impl Shell {
    pub(super) fn favorites_view(self: &Rc<Self>) -> gtk::Widget {
        let favorites = self.state.library.borrow().favorites.clone();
        self.library_tracks_route_panel(
            favorites,
            LibraryListKey::Tracks,
            "favorites",
            "Favorite tracks will appear here after you add them.",
        )
    }
}

pub(super) fn merge_favorite_snapshot(
    current: &mut LibrarySnapshot,
    mut snapshot: LibrarySnapshot,
    item_id: &FavoriteItemId,
    favorite: bool,
    preserve_search: bool,
) {
    if preserve_search {
        snapshot.search = current.search.clone();
    }
    apply_favorite_change(&mut snapshot, item_id, favorite);
    *current = snapshot;
}

pub(super) fn favorite_change_needs_route_render(
    route: &Route,
    item_id: &FavoriteItemId,
    track_sort_key: TrackSortKey,
) -> bool {
    if !matches!(item_id, FavoriteItemId::Track(_)) {
        return false;
    }

    if matches!(route, Route::Favorites) {
        return true;
    }

    if matches!(route, Route::ArtistDetail(_)) {
        return true;
    }

    track_sort_key == TrackSortKey::Favorite
        && matches!(
            route,
            Route::Tracks
                | Route::AlbumDetail(_)
                | Route::ArtistTracks(_)
                | Route::GenreDetail(_)
                | Route::PlaylistDetail(_)
                | Route::Search { .. }
        )
}

fn apply_favorite_change(library: &mut LibrarySnapshot, item_id: &FavoriteItemId, favorite: bool) {
    match item_id {
        FavoriteItemId::Album(album_id) => {
            update_albums(&mut library.albums, album_id, favorite);
            for section in &mut library.home_sections {
                update_albums(&mut section.albums, album_id, favorite);
            }
            update_albums(&mut library.search.albums, album_id, favorite);
        }
        FavoriteItemId::Track(track_id) => {
            update_tracks(&mut library.tracks, track_id, favorite);
            update_tracks(&mut library.favorites, track_id, favorite);
            update_tracks(&mut library.search.tracks, track_id, favorite);
            sync_favorite_tracks(library, track_id, favorite);
        }
        FavoriteItemId::Artist(artist_id) => {
            update_artists(&mut library.artists, artist_id, favorite);
            update_artists(&mut library.album_artists, artist_id, favorite);
            update_artists(&mut library.search.artists, artist_id, favorite);
        }
    }
}

fn update_albums(albums: &mut [Album], album_id: &AlbumId, favorite: bool) {
    for album in albums {
        if album.id == *album_id {
            album.favorite = favorite;
        }
    }
}

fn update_tracks(tracks: &mut [Track], track_id: &TrackId, favorite: bool) {
    for track in tracks {
        if track.id == *track_id {
            track.favorite = favorite;
        }
    }
}

fn update_artists(artists: &mut [Artist], artist_id: &ArtistId, favorite: bool) {
    for artist in artists {
        if artist.id == *artist_id {
            artist.favorite = favorite;
        }
    }
}

fn sync_favorite_tracks(library: &mut LibrarySnapshot, track_id: &TrackId, favorite: bool) {
    if favorite {
        if library.favorites.iter().any(|track| track.id == *track_id) {
            return;
        }
        if let Some(track) = favorite_track_source(library, track_id) {
            library.favorites.push(track);
            library
                .favorites
                .sort_by_key(|track| track.title.to_lowercase());
        }
    } else {
        library.favorites.retain(|track| track.id != *track_id);
    }
}

fn favorite_track_source(library: &LibrarySnapshot, track_id: &TrackId) -> Option<Track> {
    library
        .tracks
        .iter()
        .chain(library.search.tracks.iter())
        .find(|track| track.id == *track_id)
        .cloned()
        .map(|mut track| {
            track.favorite = true;
            track
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rufin_core::AlbumId;
    use rufin_provider::SearchResults;

    fn library_with_track(track_id: TrackId) -> LibrarySnapshot {
        LibrarySnapshot {
            server: None,
            username: None,
            first_run: false,
            sync_status: String::new(),
            last_error: None,
            cached_album_count: 0,
            cached_track_count: 1,
            home_sections: Vec::new(),
            prefetched_explore: None,
            albums: Vec::new(),
            tracks: vec![Track {
                id: track_id,
                album_id: AlbumId::fake(1),
                title: "Track".to_string(),
                artist: "Artist".to_string(),
                artist_id: None,
                artist_credits: Vec::new(),
                album_artist_credits: Vec::new(),
                album: "Album".to_string(),
                year: 2026,
                release_date: None,
                date_added: None,
                last_played: None,
                play_count: None,
                user_rating: None,
                duration_seconds: 180,
                favorite: false,
                disc_number: 1,
                track_number: 1,
                image_ref: None,
                genres: Vec::new(),
            }],
            artists: Vec::new(),
            album_artists: Vec::new(),
            genres: Vec::new(),
            playlists: Vec::new(),
            favorites: Vec::new(),
            search: SearchResults::default(),
        }
    }

    #[test]
    fn favorite_track_patch_updates_cached_lists() {
        let track_id = TrackId::fake(4);
        let mut library = library_with_track(track_id.clone());

        apply_favorite_change(&mut library, &FavoriteItemId::Track(track_id.clone()), true);

        assert!(library.tracks[0].favorite);
        assert_eq!(library.favorites.len(), 1);
        assert_eq!(library.favorites[0].id, track_id);

        apply_favorite_change(&mut library, &FavoriteItemId::Track(track_id), false);

        assert!(!library.tracks[0].favorite);
        assert!(library.favorites.is_empty());
    }

    #[test]
    fn ordinary_album_favorite_does_not_require_route_render() {
        let route = Route::Home;
        assert!(!favorite_change_needs_route_render(
            &route,
            &FavoriteItemId::Album(AlbumId::fake(1)),
            TrackSortKey::TrackNumber,
        ));
    }

    #[test]
    fn favorite_track_route_rerenders_when_membership_or_sort_order_changes() {
        let track = FavoriteItemId::Track(TrackId::fake(1));
        assert!(favorite_change_needs_route_render(
            &Route::Favorites,
            &track,
            TrackSortKey::TrackNumber,
        ));
        assert!(favorite_change_needs_route_render(
            &Route::Tracks,
            &track,
            TrackSortKey::Favorite,
        ));
        assert!(favorite_change_needs_route_render(
            &Route::ArtistDetail(ArtistId::fake(1)),
            &track,
            TrackSortKey::Title,
        ));
        assert!(!favorite_change_needs_route_render(
            &Route::ArtistTracks(ArtistId::fake(1)),
            &track,
            TrackSortKey::Title,
        ));
        assert!(favorite_change_needs_route_render(
            &Route::ArtistTracks(ArtistId::fake(1)),
            &track,
            TrackSortKey::Favorite,
        ));
        assert!(!favorite_change_needs_route_render(
            &Route::Tracks,
            &track,
            TrackSortKey::Title,
        ));
    }
}
