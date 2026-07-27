use super::{
    LibraryField, LibraryLayout, LibraryListKey, MAX_RESTORED_WINDOW_HEIGHT,
    MAX_RESTORED_WINDOW_WIDTH, Settings, SidebarRouteItem, available_detail_track_fields,
    available_sort_fields, sanitized_window_size,
};

#[test]
fn private_mode_blocks_external_ui_activity() {
    let settings = Settings {
        private_mode: true,
        notifications_enabled: true,
        release_notifications_enabled: true,
        ..Settings::default()
    };

    assert!(settings.allows_notifications());
    assert!(!settings.allows_external_site_links());
    assert!(!settings.allows_external_album_lookup());
}

#[test]
fn playback_modes_are_one_app_wide_settings_value() {
    let settings = Settings {
        auto_dj_enabled: true,
        shuffle_enabled: true,
        repeat_mode: playback::RepeatMode::All,
        ..Settings::default()
    };
    let value = serde_json::to_value(&settings).expect("serialize playback modes");

    assert_eq!(value["auto_dj_enabled"], true);
    assert_eq!(value["shuffle_enabled"], true);
    assert_eq!(value["repeat_mode"], "All");

    let restored = serde_json::from_value::<Settings>(value).expect("restore playback modes");
    assert!(restored.auto_dj_enabled);
    assert!(restored.shuffle_enabled);
    assert_eq!(restored.repeat_mode, playback::RepeatMode::All);
}

#[test]
fn split_lyrics_and_album_lookup_settings_keep_the_released_flat_keys() {
    let value = serde_json::to_value(Settings::default()).expect("serialize settings");

    assert_eq!(value["external_metadata_enabled"], true);
    assert!(value.get("external_album_lookup_enabled").is_none());
    assert!(value.get("lyrics").is_none());
    assert!(value.get("metadata").is_none());

    let mut disabled = value.clone();
    disabled["external_metadata_enabled"] = false.into();
    let disabled =
        serde_json::from_value::<Settings>(disabled).expect("deserialize released setting");
    assert!(!disabled.external_album_lookup_enabled);

    let mut missing = value;
    missing
        .as_object_mut()
        .expect("settings object")
        .remove("external_metadata_enabled");
    let missing = serde_json::from_value::<Settings>(missing).expect("deserialize sparse settings");
    assert!(missing.external_album_lookup_enabled);
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
fn saved_table_widths_round_trip_with_safe_bounds() {
    let mut settings = Settings::default();
    let tracks = settings
        .library_lists
        .iter_mut()
        .find(|entry| entry.key == LibraryListKey::Tracks)
        .expect("default Tracks settings");
    tracks.settings.row_column_widths = vec![
        super::LibraryColumnWidth {
            field: LibraryField::Title,
            width: 12,
        },
        super::LibraryColumnWidth {
            field: LibraryField::Title,
            width: 900,
        },
        super::LibraryColumnWidth {
            field: LibraryField::RowIndex,
            width: 80,
        },
    ];
    settings.folder_view.tree_width = Some(10_000);
    settings.folder_view.name_column_width = Some(12);
    settings.folder_view.detail_column_width = Some(10_000);
    settings.sanitize();

    let value = serde_json::to_value(&settings).expect("serialize saved table widths");
    let restored =
        serde_json::from_value::<Settings>(value).expect("deserialize saved table widths");
    let tracks = restored.library_list(LibraryListKey::Tracks);

    assert_eq!(
        tracks.row_column_widths,
        [super::LibraryColumnWidth {
            field: LibraryField::Title,
            width: super::MIN_TABLE_COLUMN_WIDTH,
        }]
    );
    assert_eq!(
        restored.folder_view.tree_width,
        Some(super::MAX_TABLE_COLUMN_WIDTH)
    );
    assert_eq!(
        restored.folder_view.name_column_width,
        Some(super::MIN_TABLE_COLUMN_WIDTH)
    );
    assert_eq!(
        restored.folder_view.detail_column_width,
        Some(super::MAX_TABLE_COLUMN_WIDTH)
    );
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
            LibraryField::RowIndex,
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
                LibraryField::RowIndex,
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
fn history_is_an_enabled_ordered_track_route_by_default() {
    let mut settings = Settings::default();
    let history = settings.library_list(LibraryListKey::History);
    assert_eq!(history.layout, LibraryLayout::Row);
    assert_eq!(history.sort_key, LibraryField::RowIndex);
    assert_eq!(history.row_fields.first(), Some(&LibraryField::RowIndex));
    assert!(
        settings
            .sidebar
            .route_items
            .iter()
            .any(|entry| entry.item == SidebarRouteItem::History && entry.visible)
    );
    let history_position = settings
        .sidebar
        .route_items
        .iter()
        .position(|entry| entry.item == SidebarRouteItem::History)
        .expect("History is present");
    assert_eq!(
        settings.sidebar.route_items[history_position - 1].item,
        SidebarRouteItem::Moods
    );
    assert_eq!(
        settings.sidebar.route_items[history_position + 1].item,
        SidebarRouteItem::Folders
    );

    settings
        .sidebar
        .route_items
        .retain(|entry| entry.item != SidebarRouteItem::History);
    settings.sanitize();
    let history_position = settings
        .sidebar
        .route_items
        .iter()
        .position(|entry| entry.item == SidebarRouteItem::History)
        .expect("sanitize restores History");
    assert_eq!(
        settings
            .sidebar
            .route_items
            .get(history_position)
            .map(|entry| entry.visible),
        Some(true)
    );
    assert_eq!(
        settings.sidebar.route_items[history_position - 1].item,
        SidebarRouteItem::Moods
    );
    assert_eq!(
        settings.sidebar.route_items[history_position + 1].item,
        SidebarRouteItem::Folders
    );
}

#[test]
fn search_is_available_in_the_sidebar_but_hidden_by_default() {
    let settings = Settings::default();
    let search = settings
        .sidebar
        .route_items
        .iter()
        .find(|entry| entry.item == SidebarRouteItem::Search)
        .expect("Search is available");
    assert!(!search.visible);
    assert_eq!(settings.sidebar.route_items[0].item, SidebarRouteItem::Home);
    assert_eq!(
        settings.sidebar.route_items[1].item,
        SidebarRouteItem::Search
    );
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
fn track_row_defaults_start_with_the_index() {
    for key in [
        LibraryListKey::Tracks,
        LibraryListKey::FavoriteTracks,
        LibraryListKey::History,
        LibraryListKey::AlbumDetailTracks,
        LibraryListKey::ArtistTracks,
        LibraryListKey::GenreTracks,
        LibraryListKey::MoodTracks,
        LibraryListKey::PlaylistTracks,
        LibraryListKey::SmartPlaylistTracks,
    ] {
        assert_eq!(
            super::LibraryListSettings::for_key(key).row_fields.first(),
            Some(&LibraryField::RowIndex),
            "{key:?}"
        );
    }

    assert_eq!(
        available_detail_track_fields(),
        &[
            LibraryField::RowIndex,
            LibraryField::TrackNumber,
            LibraryField::Title,
            LibraryField::Duration,
        ]
    );
    assert_eq!(
        super::LibraryListSettings::for_key(LibraryListKey::Albums).detail_track_fields,
        [
            LibraryField::RowIndex,
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
        row_column_widths: Vec::new(),
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
        row_column_widths: Vec::new(),
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
        row_column_widths: Vec::new(),
        layout_version: 4,
    };
    albums.sanitize(LibraryListKey::Albums);
    assert_eq!(
        albums.detail_track_fields,
        [
            LibraryField::RowIndex,
            LibraryField::Title,
            LibraryField::Duration,
        ]
    );

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
        row_column_widths: Vec::new(),
        layout_version: 6,
    };
    favorite_tracks.sanitize(LibraryListKey::FavoriteTracks);
    assert_eq!(
        favorite_tracks.row_fields,
        vec![
            LibraryField::RowIndex,
            LibraryField::TitleMerged,
            LibraryField::Album,
            LibraryField::Year,
            LibraryField::PlayCount,
        ]
    );

    let mut tracks = super::LibraryListSettings {
        layout: LibraryLayout::Row,
        row_fields: vec![
            LibraryField::TitleMerged,
            LibraryField::Album,
            LibraryField::Year,
            LibraryField::Favorite,
        ],
        grid_fields: Vec::new(),
        detail_track_fields: vec![
            LibraryField::TrackNumber,
            LibraryField::Title,
            LibraryField::Duration,
        ],
        sort_key: LibraryField::Title,
        descending: false,
        row_column_widths: Vec::new(),
        layout_version: 7,
    };
    tracks.sanitize(LibraryListKey::Tracks);
    assert_eq!(tracks.row_fields.first(), Some(&LibraryField::RowIndex));
    assert_eq!(
        tracks.detail_track_fields,
        [
            LibraryField::RowIndex,
            LibraryField::Title,
            LibraryField::Duration,
        ]
    );

    let mut custom_tracks = super::LibraryListSettings {
        row_fields: vec![LibraryField::Image, LibraryField::TitleMerged],
        layout_version: 7,
        ..super::LibraryListSettings::for_key(LibraryListKey::Tracks)
    };
    custom_tracks.sanitize(LibraryListKey::Tracks);
    assert_eq!(
        custom_tracks.row_fields,
        [LibraryField::Image, LibraryField::TitleMerged]
    );
}
