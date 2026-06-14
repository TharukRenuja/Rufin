use super::{
    AppSettings, AudioscrobblerScrobbleSettings, DEFAULT_DISCORD_CLIENT_ID, DiscordDisplayType,
    ExternalLyricsProvider, LEGACY_APPLICATION_DISPLAY_BYTES, LibraryField, LibraryLayout,
    LibraryListKey, LocalLibraryFolder, MAX_AUTO_DJ_REFILL_THRESHOLD, MAX_CROSSFADE_SECONDS,
    MAX_RESTORED_WINDOW_HEIGHT, MAX_RESTORED_WINDOW_WIDTH, MIN_AUTO_DJ_REFILL_THRESHOLD,
    MIN_CROSSFADE_SECONDS, RightSidebarMode, SYSTEM_LANGUAGE_PREFERENCE, ScrobblingSettings,
    SecretStorageMode, SidebarRouteItem, TrackSortKey, TrackTableColumn,
    available_detail_track_fields, default_external_lyrics_providers, sanitized_window_size,
};
#[test]
fn settings_missing_secret_storage_mode_uses_legacy_config() {
    let json = r#"{
        "theme_preference": "System",
        "language": "system",
        "private_mode": false,
        "notifications_enabled": false,
        "external_lyrics_enabled": true,
        "discord_presence_enabled": false
    }"#;

    let settings = serde_json::from_str::<AppSettings>(json).expect("deserialize settings");

    assert_eq!(settings.secret_storage_mode, SecretStorageMode::ConfigFile);
    assert!(settings.secret_scope_id.is_empty());
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
        queue_lyrics_height: Some(300),
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
    assert_eq!(restored.queue_lyrics_height, None);
    assert_eq!(restored.window_width, None);
    assert_eq!(restored.window_height, None);
    assert!(!restored.external_lyrics_enabled);
    assert_eq!(
        restored.external_lyrics_providers,
        default_external_lyrics_providers()
    );
    assert!(restored.external_site_links.enabled);
    assert!(restored.external_site_links.lastfm);
    assert!(restored.external_site_links.musicbrainz);
    assert!(restored.external_site_links.server);
    assert!(restored.type_to_search_enabled);
}

#[test]
fn settings_sanitize_lyrics_providers() {
    let mut settings = AppSettings {
        external_lyrics_providers: vec![
            ExternalLyricsProvider::Genius,
            ExternalLyricsProvider::Netease,
            ExternalLyricsProvider::Genius,
            ExternalLyricsProvider::Lrclib,
        ],
        ..AppSettings::default()
    };

    settings.migrate_defaults();

    assert_eq!(
        settings.external_lyrics_providers,
        vec![
            ExternalLyricsProvider::Genius,
            ExternalLyricsProvider::Netease,
            ExternalLyricsProvider::Lrclib
        ]
    );
}

#[test]
fn settings_reset_legacy_lyrics_suppression() {
    let mut settings = AppSettings {
        lyrics_provider_settings_version: 0,
        suppressed_auto_lyrics_track_ids: vec!["track-one".to_string()],
        ..AppSettings::default()
    };

    settings.migrate_defaults();

    assert_eq!(settings.lyrics_provider_settings_version, 1);
    assert!(settings.suppressed_auto_lyrics_track_ids.is_empty());
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
    assert_eq!(tracks.detail_track_fields, available_detail_track_fields());
}
#[test]
fn settings_default_library_rows_skip_redundant_album_artist() {
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
fn settings_default_artist_tracks_use_normal_track_rows() {
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
fn settings_default_albums_use_grid() {
    let settings = super::LibraryListSettings::for_key(LibraryListKey::Albums);

    assert_eq!(settings.layout, LibraryLayout::Grid);
}
#[test]
fn settings_migrate_default_album_and_artist_track_rows() {
    let mut settings = AppSettings {
        library_lists: vec![
            super::LibraryListSettingsEntry {
                key: LibraryListKey::Albums,
                settings: super::LibraryListSettings {
                    layout: LibraryLayout::Row,
                    row_fields: vec![
                        LibraryField::TitleMerged,
                        LibraryField::AlbumArtist,
                        LibraryField::Year,
                        LibraryField::Favorite,
                    ],
                    grid_fields: vec![LibraryField::AlbumArtist],
                    detail_track_fields: available_detail_track_fields().to_vec(),
                    sort_key: LibraryField::Title,
                    descending: false,
                    layout_version: 5,
                },
            },
            super::LibraryListSettingsEntry {
                key: LibraryListKey::FavoriteTracks,
                settings: super::LibraryListSettings {
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
                },
            },
            super::LibraryListSettingsEntry {
                key: LibraryListKey::ArtistTracks,
                settings: super::LibraryListSettings {
                    layout: LibraryLayout::Row,
                    row_fields: vec![
                        LibraryField::RowIndex,
                        LibraryField::TitleMerged,
                        LibraryField::Album,
                        LibraryField::Duration,
                        LibraryField::Favorite,
                    ],
                    grid_fields: Vec::new(),
                    detail_track_fields: available_detail_track_fields().to_vec(),
                    sort_key: LibraryField::TrackNumber,
                    descending: false,
                    layout_version: 5,
                },
            },
        ],
        ..AppSettings::default()
    };

    settings.migrate_defaults();

    assert_eq!(
        settings.library_list(LibraryListKey::Albums).layout,
        LibraryLayout::Grid
    );
    assert_eq!(
        settings.library_list(LibraryListKey::Albums).row_fields,
        vec![
            LibraryField::TitleMerged,
            LibraryField::PlayCount,
            LibraryField::Year,
            LibraryField::Favorite,
        ]
    );
    assert_eq!(
        settings
            .library_list(LibraryListKey::FavoriteTracks)
            .row_fields,
        vec![
            LibraryField::TitleMerged,
            LibraryField::Album,
            LibraryField::Year,
            LibraryField::PlayCount,
        ]
    );
    assert_eq!(
        settings
            .library_list(LibraryListKey::ArtistTracks)
            .row_fields,
        vec![
            LibraryField::TitleMerged,
            LibraryField::Album,
            LibraryField::Year,
            LibraryField::PlayCount,
        ]
    );
}
#[test]
fn settings_migrate_detail_tracks_to_text_columns() {
    let mut settings = AppSettings {
        library_lists: vec![super::LibraryListSettingsEntry {
            key: LibraryListKey::Albums,
            settings: super::LibraryListSettings {
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
            },
        }],
        ..AppSettings::default()
    };

    settings.migrate_defaults();
    let albums = settings.library_list(LibraryListKey::Albums);

    assert_eq!(albums.detail_track_fields, available_detail_track_fields());
}
#[test]
fn settings_migrate_state() {
    let mut json = serde_json::to_value(AppSettings::default()).expect("serialize settings");
    let object = json.as_object_mut().expect("settings object");
    object.insert("queue_lyrics_position".to_string(), 160.into());
    object.insert("queue_lyrics_ratio".to_string(), 0.3.into());
    object.insert("queue_lyrics_layout_version".to_string(), 2.into());

    let mut settings = serde_json::from_value::<AppSettings>(json).expect("deserialize settings");
    settings.migrate_defaults();

    assert_eq!(settings.queue_lyrics_height, None);
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
                TrackTableColumn::Title,
                TrackTableColumn::Album,
                TrackTableColumn::Year,
            ]
        );
        assert_eq!(settings.sort_key, TrackSortKey::Title);
        assert_eq!(settings.layout_version, super::TRACK_TABLE_LAYOUT_VERSION);
    }
}
