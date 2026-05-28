use super::{
    AppSettings, AudioscrobblerScrobbleSettings, DEFAULT_DISCORD_CLIENT_ID, DEFAULT_WINDOW_HEIGHT,
    DEFAULT_WINDOW_WIDTH, DiscordDisplayType, DiscordLinkType, EQUALIZER_BAND_COUNT,
    LEGACY_APPLICATION_DISPLAY_BYTES, LeftSidebarMode, LibraryField, LibraryLayout, LibraryListKey,
    LocalLibraryFolder, MAX_CROSSFADE_SECONDS, MAX_RESTORED_WINDOW_HEIGHT,
    MAX_RESTORED_WINDOW_WIDTH, MIN_CROSSFADE_SECONDS, PlaybackTransitionMode, ReplayGainMode,
    RightSidebarMode, SYSTEM_LANGUAGE_PREFERENCE, ScrobblingSettings, SidebarRouteItem,
    StreamQuality, TrackSortKey, TrackTableColumn, sanitized_window_size,
};
#[test]
fn settings_default_to_privacy_preserving_remote_features() {
    let settings = AppSettings::default();

    assert!(settings.sources.selected.is_none());
    assert!(settings.sources.local_folders.is_empty());
    assert_eq!(settings.language, SYSTEM_LANGUAGE_PREFERENCE);
    assert!(!settings.notifications_enabled);
    assert!(settings.external_lyrics_enabled);
    assert!(settings.external_metadata_enabled);
    assert!(settings.prefer_server_lyrics);
    assert!(!settings.discord_presence_enabled);
    assert_eq!(settings.discord_client_id, DEFAULT_DISCORD_CLIENT_ID);
    assert_eq!(
        settings.discord_display_type,
        DiscordDisplayType::Application
    );
    assert_eq!(settings.discord_link_type, DiscordLinkType::MusicBrainz);
    assert!(!settings.discord_show_paused);
    assert!(settings.discord_show_as_listening);
    assert!(settings.discord_show_state_icon);
    assert_eq!(settings.lastfm_api_key, "");
    assert!(!settings.scrobbling.lastfm.enabled);
    assert_eq!(settings.scrobbling.lastfm.username, "");
    assert_eq!(settings.scrobbling.lastfm.api_key, "");
    assert_eq!(settings.scrobbling.lastfm.api_secret, "");
    assert_eq!(settings.scrobbling.lastfm.session_key, "");
    assert!(settings.scrobbling.lastfm.now_playing_enabled);
    assert!(!settings.scrobbling.librefm.enabled);
    assert_eq!(settings.scrobbling.librefm.username, "");
    assert_eq!(settings.scrobbling.librefm.api_key, "rufin");
    assert_eq!(settings.scrobbling.librefm.api_secret, "rufin");
    assert_eq!(settings.scrobbling.librefm.session_key, "");
    assert!(settings.scrobbling.librefm.now_playing_enabled);
    assert!(!settings.scrobbling.listenbrainz.enabled);
    assert_eq!(settings.scrobbling.listenbrainz.user_token, "");
    assert!(settings.scrobbling.listenbrainz.now_playing_enabled);
    assert!(settings.auto_dj_enabled);
    assert_eq!(
        settings.playback.transition_mode,
        PlaybackTransitionMode::Gapless
    );
    assert_eq!(settings.playback.crossfade_seconds, 5);
    assert_eq!(settings.playback.replay_gain, ReplayGainMode::Off);
    assert_eq!(settings.playback.stream_quality, StreamQuality::Original);
    assert_eq!(settings.playback.audio_output, None);
    assert!(!settings.playback.equalizer.enabled);
    assert_eq!(
        settings.playback.equalizer.bands.len(),
        EQUALIZER_BAND_COUNT
    );
    assert_eq!(settings.playback.volume, 1.0);
    assert!(!settings.playback.muted);
    assert!(settings.lyrics_panel_visible);
    assert_eq!(
        settings.layout.default_profile.left_sidebar,
        LeftSidebarMode::Full
    );
    assert_eq!(
        settings.layout.default_profile.right_sidebar,
        RightSidebarMode::Comfortable
    );
    assert!(settings.layout.narrow_enabled);
    assert_eq!(settings.layout.narrow_threshold, 1_300);
    assert_eq!(
        settings.layout.narrow_profile.left_sidebar,
        LeftSidebarMode::Compact
    );
    assert_eq!(
        settings.layout.narrow_profile.right_sidebar,
        RightSidebarMode::Default
    );
    assert_eq!(DEFAULT_WINDOW_WIDTH, 1_500);
    assert_eq!(DEFAULT_WINDOW_HEIGHT, 900);
    assert_eq!(settings.window_width, None);
    assert_eq!(settings.window_height, None);
    assert!(settings.sidebar.server_visible);
    assert!(
        settings
            .sidebar
            .route_items
            .iter()
            .any(|entry| entry.item == SidebarRouteItem::AlbumArtists && !entry.visible)
    );
    assert!(
        settings
            .sidebar
            .route_items
            .iter()
            .any(|entry| entry.item == SidebarRouteItem::Folders && entry.visible)
    );
    assert_eq!(settings.queue_lyrics_layout_version, 3);
    assert_eq!(settings.home_sections.len(), 5);
    assert_eq!(settings.home_blocks.len(), 7);
    assert_eq!(
        settings.library_list(LibraryListKey::Albums).layout,
        LibraryLayout::Row
    );
    assert_eq!(
        settings.library_list(LibraryListKey::Tracks).layout,
        LibraryLayout::Row
    );
    assert_eq!(
        settings.library_list(LibraryListKey::Tracks).row_fields,
        vec![
            LibraryField::TitleMerged,
            LibraryField::Album,
            LibraryField::Year,
            LibraryField::Favorite,
        ]
    );
    assert_eq!(
        settings.library_list(LibraryListKey::Tracks).sort_key,
        LibraryField::Title
    );
    assert_eq!(
        settings
            .library_list(LibraryListKey::FavoriteTracks)
            .row_fields,
        settings.library_list(LibraryListKey::Tracks).row_fields
    );
    assert_eq!(
        settings.library_list(LibraryListKey::Albums).row_fields,
        vec![
            LibraryField::TitleMerged,
            LibraryField::AlbumArtist,
            LibraryField::Year,
            LibraryField::Favorite,
        ]
    );
    assert_eq!(
        settings.library_list(LibraryListKey::Artists).row_fields,
        vec![
            LibraryField::Image,
            LibraryField::Title,
            LibraryField::AlbumCount,
            LibraryField::Favorite,
        ]
    );
    assert_eq!(
        settings
            .library_list(LibraryListKey::AlbumArtists)
            .row_fields,
        settings.library_list(LibraryListKey::Artists).row_fields
    );
    assert_eq!(
        settings.library_list(LibraryListKey::Genres).row_fields,
        vec![
            LibraryField::Title,
            LibraryField::AlbumCount,
            LibraryField::SongCount,
        ]
    );
    assert!(
        settings
            .library_list(LibraryListKey::Artists)
            .grid_fields
            .is_empty()
    );
    assert!(
        settings
            .library_list(LibraryListKey::Genres)
            .grid_fields
            .is_empty()
    );
    assert_eq!(
        settings.track_table.visible_columns,
        vec![
            TrackTableColumn::TrackNumber,
            TrackTableColumn::Title,
            TrackTableColumn::Album,
            TrackTableColumn::Year,
            TrackTableColumn::Favorite,
        ]
    );
    assert_eq!(settings.track_table.sort_key, TrackSortKey::Title);
    assert!(settings.suppressed_auto_lyrics_track_ids.is_empty());
}
#[test]
fn playback_settings_sanitize_clamps_crossfade_to_supported_range() {
    let mut settings = super::PlaybackSettings {
        crossfade_seconds: 0,
        ..super::PlaybackSettings::default()
    };

    settings.sanitize();
    assert_eq!(settings.crossfade_seconds, MIN_CROSSFADE_SECONDS);

    settings.crossfade_seconds = MAX_CROSSFADE_SECONDS + 1;
    settings.sanitize();
    assert_eq!(settings.crossfade_seconds, MAX_CROSSFADE_SECONDS);
}
#[test]
fn app_settings_sanitize_local_library_folders() {
    let mut settings = AppSettings {
        sources: super::LibrarySourceSettings {
            selected: None,
            local_folders: vec![
                LocalLibraryFolder {
                    path: " /music ".to_string(),
                },
                LocalLibraryFolder {
                    path: "/music".to_string(),
                },
                LocalLibraryFolder {
                    path: " ".to_string(),
                },
                LocalLibraryFolder {
                    path: "/archive".to_string(),
                },
            ],
        },
        ..AppSettings::default()
    };

    settings.migrate_defaults();

    assert_eq!(
        settings.sources.local_folders,
        vec![
            LocalLibraryFolder {
                path: "/music".to_string()
            },
            LocalLibraryFolder {
                path: "/archive".to_string()
            }
        ]
    );
}
#[test]
fn settings_serialize_to_json() {
    let settings = AppSettings {
        lyrics_panel_visible: false,
        queue_lyrics_position: Some(520),
        queue_lyrics_ratio: Some(0.7),
        ..AppSettings::default()
    };

    let json = serde_json::to_string(&settings).expect("serialize settings");
    let restored = serde_json::from_str::<AppSettings>(&json).expect("deserialize settings");

    assert_eq!(restored, settings);
}
#[test]
fn settings_restore_without_window_geometry() {
    let json = r#"{
            "theme_preference":"System",
            "private_mode":false,
            "notifications_enabled":false,
            "external_lyrics_enabled":false,
            "discord_presence_enabled":false,
            "home_sections":["Explore"]
        }"#;

    let restored = serde_json::from_str::<AppSettings>(json).expect("deserialize settings");

    assert!(restored.lyrics_panel_visible);
    assert_eq!(
        restored.layout.default_profile.right_sidebar,
        RightSidebarMode::Comfortable
    );
    assert_eq!(
        restored.layout.narrow_profile.right_sidebar,
        RightSidebarMode::Default
    );
    assert!(restored.sidebar.server_visible);
    assert_eq!(restored.queue_lyrics_position, None);
    assert_eq!(restored.queue_lyrics_ratio, None);
    assert_eq!(restored.window_width, None);
    assert_eq!(restored.window_height, None);
    assert!(restored.auto_dj_enabled);
    assert_eq!(restored.language, SYSTEM_LANGUAGE_PREFERENCE);
    assert!(!restored.external_lyrics_enabled);
    assert!(restored.external_metadata_enabled);
    assert!(restored.prefer_server_lyrics);
    assert_eq!(
        restored.playback.transition_mode,
        PlaybackTransitionMode::Gapless
    );
    assert_eq!(restored.playback.volume, 1.0);
    assert!(!restored.playback.muted);
    assert_eq!(restored.discord_client_id, DEFAULT_DISCORD_CLIENT_ID);
    assert_eq!(
        restored.discord_display_type,
        DiscordDisplayType::Application
    );
    assert_eq!(restored.discord_link_type, DiscordLinkType::MusicBrainz);
    assert!(!restored.discord_show_paused);
    assert!(restored.discord_show_as_listening);
    assert!(restored.discord_show_state_icon);
    assert_eq!(restored.lastfm_api_key, "");
    assert!(!restored.scrobbling.lastfm.enabled);
    assert_eq!(restored.scrobbling.librefm.api_key, "rufin");
    assert_eq!(restored.scrobbling.librefm.api_secret, "rufin");
    assert!(!restored.scrobbling.listenbrainz.enabled);
    assert_eq!(restored.track_table.sort_key, TrackSortKey::Title);
}
#[test]
fn app_settings_sanitize_language_preference() {
    let mut settings = AppSettings {
        language: " tr_TR.UTF-8 ".to_string(),
        ..AppSettings::default()
    };
    settings.migrate_defaults();
    assert_eq!(settings.language, "tr_TR.UTF-8");

    settings.language = "tr_TR\0".to_string();
    settings.migrate_defaults();
    assert_eq!(settings.language, SYSTEM_LANGUAGE_PREFERENCE);

    settings.language = "default".to_string();
    settings.migrate_defaults();
    assert_eq!(settings.language, SYSTEM_LANGUAGE_PREFERENCE);
}
#[test]
fn window_size_restore_rejects_tiny_and_clamps_huge_geometry() {
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
fn settings_share_lastfm_api_key_when_only_one_value_exists() {
    let mut from_global_key = AppSettings {
        lastfm_api_key: "global-key".to_string(),
        ..AppSettings::default()
    };
    from_global_key.migrate_defaults();
    assert_eq!(from_global_key.scrobbling.lastfm.api_key, "global-key");

    let mut from_scrobbling_key = AppSettings {
        scrobbling: ScrobblingSettings {
            lastfm: AudioscrobblerScrobbleSettings {
                api_key: "scrobble-key".to_string(),
                ..AudioscrobblerScrobbleSettings::default()
            },
            ..ScrobblingSettings::default()
        },
        ..AppSettings::default()
    };
    from_scrobbling_key.migrate_defaults();
    assert_eq!(from_scrobbling_key.lastfm_api_key, "scrobble-key");
}
#[test]
fn settings_migrate_legacy_home_sections_to_home_blocks() {
    let json = r#"{
            "theme_preference":"System",
            "private_mode":false,
            "notifications_enabled":false,
            "external_lyrics_enabled":false,
            "discord_presence_enabled":false,
            "home_sections":["Explore","RecentlyPlayed"]
        }"#;

    let mut settings = serde_json::from_str::<AppSettings>(json).expect("deserialize settings");
    settings.migrate_defaults();

    assert_eq!(
        settings.home_blocks,
        vec![
            crate::domain::HomeBlockKind::Showcase,
            crate::domain::HomeBlockKind::Explore,
            crate::domain::HomeBlockKind::RecentlyPlayed,
            crate::domain::HomeBlockKind::Genres,
        ]
    );
    assert_eq!(
        settings.home_sections,
        vec![
            crate::domain::HomeSectionKind::Explore,
            crate::domain::HomeSectionKind::RecentlyPlayed
        ]
    );
}
#[test]
fn library_layout_unknown_values_fall_back_to_grid() {
    let layout = serde_json::from_str::<LibraryLayout>("\"weird\"").expect("deserialize layout");

    assert_eq!(layout, LibraryLayout::Grid);
}
#[test]
fn default_library_list_settings_include_playlists() {
    let playlists = AppSettings::default().library_list(LibraryListKey::Playlists);

    assert_eq!(playlists.layout, LibraryLayout::Grid);
    assert_eq!(
        playlists.row_fields,
        vec![
            LibraryField::Image,
            LibraryField::Title,
            LibraryField::SongCount,
            LibraryField::Duration
        ]
    );
    assert_eq!(
        playlists.grid_fields,
        vec![LibraryField::SongCount, LibraryField::Duration]
    );
    assert_eq!(playlists.sort_key, LibraryField::Title);
}
#[test]
fn library_list_settings_sanitize_fields_and_layouts() {
    let mut settings = AppSettings {
        library_lists: vec![super::LibraryListSettingsEntry {
            key: LibraryListKey::Genres,
            settings: super::LibraryListSettings {
                layout: LibraryLayout::Detail,
                row_fields: vec![
                    LibraryField::Title,
                    LibraryField::Album,
                    LibraryField::Title,
                ],
                grid_fields: vec![LibraryField::Artist],
                detail_track_fields: Vec::new(),
                sort_key: LibraryField::Album,
                descending: true,
                layout_version: 0,
            },
        }],
        ..AppSettings::default()
    };

    settings.migrate_defaults();
    let genres = settings.library_list(LibraryListKey::Genres);

    assert_eq!(genres.layout, LibraryLayout::Grid);
    assert_eq!(genres.row_fields, vec![LibraryField::Title]);
    assert!(genres.grid_fields.is_empty());
    assert_eq!(genres.sort_key, LibraryField::Title);
}
#[test]
fn library_list_settings_keep_a_usable_row_field() {
    let mut settings = AppSettings {
        library_lists: vec![super::LibraryListSettingsEntry {
            key: LibraryListKey::Tracks,
            settings: super::LibraryListSettings {
                layout: LibraryLayout::Row,
                row_fields: vec![LibraryField::RowIndex, LibraryField::Favorite],
                grid_fields: vec![LibraryField::Artist],
                detail_track_fields: vec![LibraryField::Favorite],
                sort_key: LibraryField::TrackNumber,
                descending: false,
                layout_version: 0,
            },
        }],
        ..AppSettings::default()
    };

    settings.migrate_defaults();
    let tracks = settings.library_list(LibraryListKey::Tracks);

    assert!(tracks.row_fields.contains(&LibraryField::TitleMerged));
    assert!(tracks.detail_track_fields.contains(&LibraryField::Title));
}
#[test]
fn settings_migrate_legacy_queue_lyrics_split_state() {
    let mut settings = AppSettings {
        queue_lyrics_position: Some(160),
        queue_lyrics_ratio: Some(0.3),
        queue_lyrics_layout_version: 2,
        ..AppSettings::default()
    };

    settings.migrate_defaults();

    assert_eq!(settings.queue_lyrics_position, None);
    assert_eq!(settings.queue_lyrics_ratio, None);
    assert_eq!(settings.queue_lyrics_layout_version, 3);
}
#[test]
fn settings_migrate_empty_discord_identity_defaults() {
    let mut settings = AppSettings {
        discord_presence_enabled: false,
        discord_client_id: String::new(),
        ..AppSettings::default()
    };

    settings.migrate_defaults();

    assert_eq!(settings.discord_client_id, DEFAULT_DISCORD_CLIENT_ID);
    assert!(settings.discord_presence_enabled);
}
#[test]
fn settings_restore_previous_application_display_value() {
    let legacy_value = std::str::from_utf8(LEGACY_APPLICATION_DISPLAY_BYTES).expect("legacy value");
    let json = format!("\"{}\"", legacy_value);

    let restored =
        serde_json::from_str::<DiscordDisplayType>(&json).expect("deserialize display type");

    assert_eq!(restored, DiscordDisplayType::Application);
}
#[test]
fn settings_migrate_legacy_track_table_default_columns() {
    let json = r#"{
            "visible_columns":["TrackNumber","Title","Artist","Album","Year","Duration","Favorite"],
            "sort_key":"TrackNumber",
            "descending":false
        }"#;

    let mut settings =
        serde_json::from_str::<super::TrackTableSettings>(json).expect("deserialize settings");
    settings.migrate_defaults();

    assert_eq!(
        settings.visible_columns,
        vec![
            TrackTableColumn::TrackNumber,
            TrackTableColumn::Title,
            TrackTableColumn::Album,
            TrackTableColumn::Year,
            TrackTableColumn::Favorite,
        ]
    );
    assert_eq!(settings.sort_key, TrackSortKey::Title);
    assert_eq!(settings.layout_version, super::TRACK_TABLE_LAYOUT_VERSION);
}
#[test]
fn settings_migrate_previous_composite_title_default_columns() {
    let json = r#"{
            "visible_columns":["Title","Album","Year"],
            "sort_key":"TrackNumber",
            "descending":false,
            "layout_version":1
        }"#;

    let mut settings =
        serde_json::from_str::<super::TrackTableSettings>(json).expect("deserialize settings");
    settings.migrate_defaults();

    assert_eq!(
        settings.visible_columns,
        vec![
            TrackTableColumn::TrackNumber,
            TrackTableColumn::Title,
            TrackTableColumn::Album,
            TrackTableColumn::Year,
            TrackTableColumn::Favorite,
        ]
    );
    assert_eq!(settings.sort_key, TrackSortKey::Title);
    assert_eq!(settings.layout_version, super::TRACK_TABLE_LAYOUT_VERSION);
}
