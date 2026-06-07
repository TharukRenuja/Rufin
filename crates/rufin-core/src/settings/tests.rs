use super::{
    AppSettings, AudioscrobblerScrobbleSettings, DEFAULT_DISCORD_CLIENT_ID, DiscordDisplayType,
    LEGACY_APPLICATION_DISPLAY_BYTES, LibraryField, LibraryLayout, LibraryListKey,
    LocalLibraryFolder, MAX_AUTO_DJ_REFILL_THRESHOLD, MAX_CROSSFADE_SECONDS,
    MAX_RESTORED_WINDOW_HEIGHT, MAX_RESTORED_WINDOW_WIDTH, MIN_AUTO_DJ_REFILL_THRESHOLD,
    MIN_CROSSFADE_SECONDS, RightSidebarMode, SYSTEM_LANGUAGE_PREFERENCE, ScrobblingSettings,
    SidebarRouteItem, TrackSortKey, TrackTableColumn, sanitized_window_size,
};
#[test]
fn settings_default_disabled() {
    let settings = AppSettings::default();

    assert!(settings.sources.selected.is_none());
    assert!(settings.sources.local_folders.is_empty());
    assert_eq!(settings.language, SYSTEM_LANGUAGE_PREFERENCE);
    assert!(!settings.notifications_enabled);
    assert!(settings.type_to_search_enabled);
    assert!(!settings.discord_presence_enabled);
    assert!(!settings.discord_show_paused);
    assert_eq!(settings.lastfm_api_key, "");
    assert!(!settings.scrobbling.lastfm.enabled);
    assert_eq!(settings.scrobbling.lastfm.username, "");
    assert_eq!(settings.scrobbling.lastfm.api_key, "");
    assert_eq!(settings.scrobbling.lastfm.api_secret, "");
    assert_eq!(settings.scrobbling.lastfm.session_key, "");
    assert!(!settings.scrobbling.librefm.enabled);
    assert_eq!(settings.scrobbling.librefm.username, "");
    assert_eq!(settings.scrobbling.librefm.session_key, "");
    assert!(!settings.scrobbling.listenbrainz.enabled);
    assert_eq!(settings.scrobbling.listenbrainz.user_token, "");
}
#[test]
fn settings_clamp_range() {
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
fn settings_clamp_threshold() {
    let mut settings = AppSettings {
        auto_dj_refill_threshold: 0,
        ..AppSettings::default()
    };

    settings.migrate_defaults();
    assert_eq!(
        settings.auto_dj_refill_threshold,
        MIN_AUTO_DJ_REFILL_THRESHOLD
    );

    settings.auto_dj_refill_threshold = MAX_AUTO_DJ_REFILL_THRESHOLD + 1;
    settings.migrate_defaults();
    assert_eq!(
        settings.auto_dj_refill_threshold,
        MAX_AUTO_DJ_REFILL_THRESHOLD
    );
}
#[test]
fn settings_clear_tray() {
    let mut settings = AppSettings {
        exit_to_tray: true,
        start_minimized: true,
        ..AppSettings::default()
    };

    settings.migrate_defaults();
    assert!(!settings.exit_to_tray);
    assert!(!settings.start_minimized);

    settings.tray_enabled = true;
    settings.exit_to_tray = true;
    settings.start_minimized = true;
    settings.migrate_defaults();
    assert!(settings.exit_to_tray);
    assert!(settings.start_minimized);
}
#[test]
fn settings_sanitize_folders() {
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
    assert!(!restored.external_lyrics_enabled);
    assert!(restored.type_to_search_enabled);
}

#[test]
fn settings_migrate_preferences() {
    let mut settings = AppSettings::default();
    settings.sidebar.route_items.retain(|entry| {
        !matches!(
            entry.item,
            SidebarRouteItem::Genres | SidebarRouteItem::SmartPlaylists
        )
    });

    settings.migrate_defaults();

    assert!(
        settings
            .sidebar
            .route_items
            .iter()
            .any(|entry| entry.item == SidebarRouteItem::Genres && entry.visible)
    );
    assert!(
        settings
            .sidebar
            .route_items
            .iter()
            .any(|entry| entry.item == SidebarRouteItem::SmartPlaylists && entry.visible)
    );
}
#[test]
fn app_settings_sanitize_language_preference() {
    let mut settings = AppSettings {
        language: " de_DE.UTF-8 ".to_string(),
        ..AppSettings::default()
    };
    settings.migrate_defaults();
    assert_eq!(settings.language, "de_DE.UTF-8");

    settings.language = "de_DE\0".to_string();
    settings.migrate_defaults();
    assert_eq!(settings.language, SYSTEM_LANGUAGE_PREFERENCE);

    settings.language = "default".to_string();
    settings.migrate_defaults();
    assert_eq!(settings.language, SYSTEM_LANGUAGE_PREFERENCE);
}
#[test]
fn settings_clamp_geometry() {
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
fn settings_share_exists() {
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
fn settings_use_conflict() {
    let mut settings = AppSettings {
        lastfm_api_key: "cover-key".to_string(),
        scrobbling: ScrobblingSettings {
            lastfm: AudioscrobblerScrobbleSettings {
                api_key: "scrobble-key".to_string(),
                ..AudioscrobblerScrobbleSettings::default()
            },
            ..ScrobblingSettings::default()
        },
        ..AppSettings::default()
    };

    settings.migrate_defaults();

    assert_eq!(settings.lastfm_api_key, "cover-key");
    assert_eq!(settings.scrobbling.lastfm.api_key, "cover-key");
}
#[test]
fn settings_migrate_blocks() {
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
fn settings_fall_grid() {
    let layout = serde_json::from_str::<LibraryLayout>("\"weird\"").expect("deserialize layout");

    assert_eq!(layout, LibraryLayout::Grid);
}
#[test]
fn settings_migrate_duration() {
    let mut settings = AppSettings {
        library_lists: vec![super::LibraryListSettingsEntry {
            key: LibraryListKey::Playlists,
            settings: super::LibraryListSettings {
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
            },
        }],
        ..AppSettings::default()
    };

    settings.migrate_defaults();
    let playlists = settings.library_list(LibraryListKey::Playlists);

    assert_eq!(
        playlists.row_fields,
        vec![
            LibraryField::Image,
            LibraryField::Title,
            LibraryField::SongCount
        ]
    );
    assert_eq!(playlists.grid_fields, vec![LibraryField::SongCount]);
}
#[test]
fn settings_migrate_order() {
    let mut settings = AppSettings {
        library_lists: vec![super::LibraryListSettingsEntry {
            key: LibraryListKey::SmartPlaylists,
            settings: super::LibraryListSettings {
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
                layout_version: 3,
            },
        }],
        ..AppSettings::default()
    };

    settings.migrate_defaults();

    assert_eq!(
        settings
            .library_list(LibraryListKey::SmartPlaylists)
            .sort_key,
        LibraryField::RowIndex
    );
}
#[test]
fn settings_sanitize_layouts() {
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
fn settings_keep_field() {
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
fn settings_migrate_state() {
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
fn settings_migrate_defaults() {
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
fn settings_restore_display() {
    let legacy_value = std::str::from_utf8(LEGACY_APPLICATION_DISPLAY_BYTES).expect("legacy value");
    let json = format!("\"{}\"", legacy_value);

    let restored =
        serde_json::from_str::<DiscordDisplayType>(&json).expect("deserialize display type");

    assert_eq!(restored, DiscordDisplayType::Application);
}
#[test]
fn settings_track_defaults() {
    for json in [
        r#"{
            "visible_columns":["TrackNumber","Title","Artist","Album","Year","Duration","Favorite"],
            "sort_key":"TrackNumber",
            "descending":false
        }"#,
        r#"{
            "visible_columns":["Title","Album","Year"],
            "sort_key":"TrackNumber",
            "descending":false,
            "layout_version":1
        }"#,
        r#"{
            "visible_columns":["TrackNumber","Title","Album","Year","Favorite"],
            "sort_key":"Title",
            "descending":false,
            "layout_version":2
        }"#,
    ] {
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
            ]
        );
        assert_eq!(settings.sort_key, TrackSortKey::Title);
        assert_eq!(settings.layout_version, super::TRACK_TABLE_LAYOUT_VERSION);
    }
}
