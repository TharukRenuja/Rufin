use super::*;

pub(in crate::ui) fn populate_album_model(
    model: &gio::ListStore,
    albums: &[Album],
    settings: &LibraryListSettings,
) {
    let mut values = albums.to_vec();
    sort_albums(&mut values, settings);
    replace_albums_in_model(model, values);
}

pub(in crate::ui) fn populate_artist_model(
    model: &gio::ListStore,
    artists: &[Artist],
    settings: &LibraryListSettings,
) {
    let mut values = artists.to_vec();
    sort_artists(&mut values, settings);
    replace_artists_in_model(model, values);
}

pub(in crate::ui) fn populate_genre_model(
    model: &gio::ListStore,
    genres: &[Genre],
    settings: &LibraryListSettings,
) {
    let mut values = genres.to_vec();
    sort_genres(&mut values, settings);
    replace_genres_in_model(model, values);
}

pub(in crate::ui) fn populate_playlist_model(
    model: &gio::ListStore,
    playlists: &[Playlist],
    settings: &LibraryListSettings,
) {
    let mut values = playlists.to_vec();
    sort_playlists(&mut values, settings);
    replace_playlists_in_model(model, values);
}

pub(in crate::ui) fn populate_smart_playlist_model(
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

pub(in crate::ui) fn tracks_for_settings(
    tracks: &[Track],
    settings: &LibraryListSettings,
    query: &str,
    favorite_first: bool,
) -> Vec<Track> {
    let query = query.trim().to_lowercase();
    let mut values = tracks
        .iter()
        .filter(|track| query.is_empty() || track_matches_query(track, &query))
        .cloned()
        .collect::<Vec<_>>();
    sort_tracks(&mut values, settings, favorite_first);
    values
}

pub(in crate::ui) fn sort_albums(albums: &mut [Album], settings: &LibraryListSettings) {
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

pub(in crate::ui) fn sort_artists(artists: &mut [Artist], settings: &LibraryListSettings) {
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
