use super::{
    LibraryField, LibraryLayout, LibraryListKey, MAX_RESTORED_WINDOW_HEIGHT,
    MAX_RESTORED_WINDOW_WIDTH, Settings, available_detail_track_fields, available_sort_fields,
    sanitized_window_size,
};

#[test]
fn private_mode_blocks_outbound_ui_activity() {
    let settings = Settings {
        private_mode: true,
        notifications_enabled: true,
        release_notifications_enabled: true,
        ..Settings::default()
    };

    assert!(settings.allows_notifications());
    assert!(!settings.allows_external_site_links());
    assert!(!settings.allows_release_update_check());
}

#[test]
fn restored_window_size_is_bounded() {
    assert_eq!(sanitized_window_size(None, Some(700)), None);
    assert_eq!(sanitized_window_size(Some(400), Some(700)), None);
    assert_eq!(
        sanitized_window_size(Some(1061), Some(2251)),
        Some((1061, MAX_RESTORED_WINDOW_HEIGHT))
    );
    assert_eq!(
        sanitized_window_size(Some(1800), Some(1200)),
        Some((1800, 1200))
    );
    assert_eq!(
        sanitized_window_size(Some(5000), Some(3000)),
        Some((MAX_RESTORED_WINDOW_WIDTH, MAX_RESTORED_WINDOW_HEIGHT))
    );
}

#[test]
fn unknown_library_layout_falls_back_to_grid() {
    let layout =
        serde_json::from_str::<LibraryLayout>("\"weird\"").expect("deserialize library layout");
    assert_eq!(layout, LibraryLayout::Grid);
}

#[test]
fn default_library_rows_skip_redundant_album_artist() {
    for key in [LibraryListKey::Albums, LibraryListKey::ArtistAlbums] {
        let settings = super::LibraryListSettings::for_key(key);
        assert_eq!(
            settings.row_fields,
            vec![
                LibraryField::TitleMerged,
                LibraryField::PlayCount,
                LibraryField::Year,
                LibraryField::Favorite,
            ],
            "{key:?}"
        );
    }
}

#[test]
fn default_artist_tracks_use_normal_track_rows() {
    let tracks = super::LibraryListSettings::for_key(LibraryListKey::Tracks);
    assert_eq!(
        tracks.row_fields,
        vec![
            LibraryField::TitleMerged,
            LibraryField::Album,
            LibraryField::Year,
            LibraryField::Favorite,
        ]
    );

    for key in [LibraryListKey::FavoriteTracks, LibraryListKey::ArtistTracks] {
        let settings = super::LibraryListSettings::for_key(key);
        assert_eq!(
            settings.row_fields,
            vec![
                LibraryField::TitleMerged,
                LibraryField::Album,
                LibraryField::Year,
                LibraryField::PlayCount,
            ],
            "{key:?}"
        );
    }
}

#[test]
fn default_albums_use_grid() {
    let settings = super::LibraryListSettings::for_key(LibraryListKey::Albums);
    assert_eq!(settings.layout, LibraryLayout::Grid);
}

#[test]
fn playlist_track_sorting_stays_within_playlist_playback_ordering() {
    let settings = super::LibraryListSettings::for_key(LibraryListKey::PlaylistTracks);
    assert_eq!(settings.sort_key, LibraryField::RowIndex);
    assert_eq!(
        available_sort_fields(LibraryListKey::PlaylistTracks),
        &[
            LibraryField::RowIndex,
            LibraryField::Title,
            LibraryField::Artist,
            LibraryField::Album,
        ]
    );
}

#[test]
fn detail_track_defaults_remain_text_columns() {
    assert_eq!(
        available_detail_track_fields(),
        &[
            LibraryField::TrackNumber,
            LibraryField::Title,
            LibraryField::Duration,
        ]
    );
}

#[test]
fn library_list_settings_migrate_persisted_layout_versions() {
    let mut playlists = super::LibraryListSettings {
        layout: LibraryLayout::Grid,
        row_fields: vec![
            LibraryField::Image,
            LibraryField::Title,
            LibraryField::SongCount,
            LibraryField::Duration,
        ],
        grid_fields: vec![LibraryField::SongCount, LibraryField::Duration],
        detail_track_fields: Vec::new(),
        sort_key: LibraryField::Title,
        descending: false,
        layout_version: 2,
    };
    playlists.sanitize(LibraryListKey::Playlists);
    assert_eq!(
        playlists.row_fields,
        vec![
            LibraryField::Image,
            LibraryField::Title,
            LibraryField::SongCount
        ]
    );
    assert_eq!(playlists.grid_fields, vec![LibraryField::SongCount]);

    let mut smart_playlists = super::LibraryListSettings {
        layout: LibraryLayout::Grid,
        row_fields: vec![LibraryField::Image, LibraryField::Title],
        grid_fields: vec![LibraryField::SongCount],
        detail_track_fields: Vec::new(),
        sort_key: LibraryField::Title,
        descending: false,
        layout_version: 3,
    };
    smart_playlists.sanitize(LibraryListKey::SmartPlaylists);
    assert_eq!(smart_playlists.sort_key, LibraryField::RowIndex);

    let mut albums = super::LibraryListSettings {
        layout: LibraryLayout::Detail,
        row_fields: vec![LibraryField::Image, LibraryField::Title],
        grid_fields: vec![LibraryField::AlbumArtist],
        detail_track_fields: vec![
            LibraryField::RowIndex,
            LibraryField::Image,
            LibraryField::Title,
            LibraryField::PlayCount,
        ],
        sort_key: LibraryField::Title,
        descending: false,
        layout_version: 4,
    };
    albums.sanitize(LibraryListKey::Albums);
    assert_eq!(albums.detail_track_fields, available_detail_track_fields());

    let mut favorite_tracks = super::LibraryListSettings {
        layout: LibraryLayout::Row,
        row_fields: vec![
            LibraryField::TitleMerged,
            LibraryField::Album,
            LibraryField::Year,
            LibraryField::Favorite,
        ],
        grid_fields: Vec::new(),
        detail_track_fields: available_detail_track_fields().to_vec(),
        sort_key: LibraryField::Title,
        descending: false,
        layout_version: 6,
    };
    favorite_tracks.sanitize(LibraryListKey::FavoriteTracks);
    assert_eq!(
        favorite_tracks.row_fields,
        vec![
            LibraryField::TitleMerged,
            LibraryField::Album,
            LibraryField::Year,
            LibraryField::PlayCount,
        ]
    );
}
