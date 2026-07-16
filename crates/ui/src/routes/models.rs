use std::cmp::Ordering;

use ::library::{Album, Artist, Playlist, SmartPlaylist, Track};
use adw::prelude::*;
use gtk::{gio, glib};

use crate::LibraryListSettings;

use super::library_fields::{
    album_field_missing, apply_desc, artist_field_missing, compare_album, compare_artist,
    sort_playlists, sort_smart_playlists,
};

pub(crate) fn populate_album_model(
    model: &gio::ListStore,
    albums: &[Album],
    settings: &LibraryListSettings,
) {
    let mut values = albums.to_vec();
    sort_albums(&mut values, settings);
    replace_albums_in_model(model, values);
}

pub(crate) fn populate_artist_model(
    model: &gio::ListStore,
    artists: &[Artist],
    settings: &LibraryListSettings,
) {
    let mut values = artists.to_vec();
    sort_artists(&mut values, settings);
    replace_artists_in_model(model, values);
}

pub(crate) fn populate_playlist_model(
    model: &gio::ListStore,
    playlists: &[Playlist],
    settings: &LibraryListSettings,
) {
    let mut values = playlists.to_vec();
    sort_playlists(&mut values, settings);
    replace_playlists_in_model(model, values);
}

pub(crate) fn populate_smart_playlist_model(
    model: &gio::ListStore,
    playlists: &[SmartPlaylist],
    settings: &LibraryListSettings,
) {
    let mut values = playlists.to_vec();
    sort_smart_playlists(&mut values, settings);
    let additions = values
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}

pub(crate) fn sort_albums(albums: &mut [Album], settings: &LibraryListSettings) {
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

pub(crate) fn sort_artists(artists: &mut [Artist], settings: &LibraryListSettings) {
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

pub(crate) fn replace_albums_in_model(
    model: &gio::ListStore,
    albums: impl IntoIterator<Item = Album>,
) {
    let additions = albums
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}

pub(crate) fn replace_artists_in_model(
    model: &gio::ListStore,
    artists: impl IntoIterator<Item = Artist>,
) {
    let additions = artists
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}

pub(crate) fn replace_playlists_in_model(
    model: &gio::ListStore,
    playlists: impl IntoIterator<Item = Playlist>,
) {
    let additions = playlists
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}

pub(crate) fn track_matches_query(track: &Track, query: &str) -> bool {
    track.title.to_lowercase().contains(query)
        || track.artist.to_lowercase().contains(query)
        || track.album.to_lowercase().contains(query)
        || track.year.to_string().contains(query)
}
