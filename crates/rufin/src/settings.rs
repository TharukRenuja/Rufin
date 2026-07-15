use ::ui::{
    LibraryField, LibraryListKey, LibraryListSettings, LibraryListSettingsEntry,
    Settings as UiSettings,
};
use library::{HomeBlockKind, HomeSectionKind};
use scrobbling::Settings;
use serde::{Deserialize, Serialize};
use sources::LibrarySourceSettings;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
pub(crate) enum LegacyTrackSortKey {
    TrackNumber,
    #[default]
    Title,
    Artist,
    Album,
    Year,
    Duration,
    Favorite,
}

impl LegacyTrackSortKey {
    fn library_field(self) -> LibraryField {
        match self {
            Self::TrackNumber => LibraryField::TrackNumber,
            Self::Title => LibraryField::Title,
            Self::Artist => LibraryField::Artist,
            Self::Album => LibraryField::Album,
            Self::Year => LibraryField::Year,
            Self::Duration => LibraryField::Duration,
            Self::Favorite => LibraryField::Favorite,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct LegacyTrackTableSettings {
    #[serde(default)]
    sort_key: LegacyTrackSortKey,
    #[serde(default)]
    descending: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct StoredSettings {
    #[serde(flatten)]
    pub(crate) ui: UiSettings,
    #[serde(default)]
    pub(crate) sources: LibrarySourceSettings,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) secret_scope_id: String,
    #[serde(default)]
    pub(crate) jellyfin_device_id: String,
    #[serde(default, rename = "home_sections", skip_serializing)]
    pub(crate) legacy_home_sections: Option<Vec<HomeSectionKind>>,
    #[serde(default, rename = "track_table", skip_serializing)]
    pub(crate) legacy_track_table: Option<LegacyTrackTableSettings>,
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            ui: UiSettings::default(),
            sources: LibrarySourceSettings::default(),
            secret_scope_id: String::new(),
            jellyfin_device_id: String::new(),
            legacy_home_sections: None,
            legacy_track_table: None,
        }
    }
}

impl StoredSettings {
    pub(crate) fn migrate_defaults(&mut self) {
        if self.ui.lastfm_api_key.trim().is_empty() && !self.ui.scrobbling.lastfm.api_key.is_empty()
        {
            self.ui.lastfm_api_key = self.ui.scrobbling.lastfm.api_key.clone();
        }
        self.ui.scrobbling.lastfm.api_key.clear();
        self.sources.sanitize();
        self.migrate_home_blocks();
        self.migrate_legacy_track_table();
        self.ui.sanitize();
    }

    pub(crate) fn scrobbling_runtime_settings(&self) -> Settings {
        let mut settings = self.ui.scrobbling.clone();
        settings.lastfm.api_key = self.ui.lastfm_api_key.clone();
        settings
    }

    fn migrate_home_blocks(&mut self) {
        if self.ui.home_blocks.is_empty() {
            let home_sections = self
                .legacy_home_sections
                .take()
                .filter(|sections| !sections.is_empty())
                .unwrap_or_else(default_home_sections);
            self.ui.home_blocks = Vec::with_capacity(home_sections.len() + 2);
            self.ui.home_blocks.push(HomeBlockKind::Showcase);
            for section in home_sections {
                self.ui.home_blocks.push(match section {
                    HomeSectionKind::Explore => HomeBlockKind::Explore,
                    HomeSectionKind::MostPlayed => HomeBlockKind::MostPlayed,
                    HomeSectionKind::NewlyAdded => HomeBlockKind::NewlyAdded,
                    HomeSectionKind::RecentlyPlayed => HomeBlockKind::RecentlyPlayed,
                    HomeSectionKind::RecentlyReleased => HomeBlockKind::RecentlyReleased,
                });
            }
            if !self.ui.home_blocks.contains(&HomeBlockKind::Genres) {
                self.ui.home_blocks.push(HomeBlockKind::Genres);
            }
        } else {
            self.legacy_home_sections.take();
        }
    }

    fn migrate_legacy_track_table(&mut self) {
        let Some(legacy) = self.legacy_track_table.take() else {
            return;
        };
        if self
            .ui
            .library_lists
            .iter()
            .any(|entry| entry.key == LibraryListKey::Tracks)
        {
            return;
        }

        let mut settings = LibraryListSettings::for_key(LibraryListKey::Tracks);
        settings.sort_key = legacy.sort_key.library_field();
        settings.descending = legacy.descending;
        self.ui.library_lists.push(LibraryListSettingsEntry {
            key: LibraryListKey::Tracks,
            settings,
        });
    }
}

fn default_home_sections() -> Vec<HomeSectionKind> {
    vec![
        HomeSectionKind::Explore,
        HomeSectionKind::MostPlayed,
        HomeSectionKind::NewlyAdded,
        HomeSectionKind::RecentlyPlayed,
        HomeSectionKind::RecentlyReleased,
    ]
}

#[cfg(test)]
mod tests {
    use ::ui::{
        LeftSidebarMode, LibraryField, LibraryLayout, LibraryListKey, LibraryListSettings,
        RightSidebarMode,
    };
    use localization::SYSTEM_LANGUAGE_PREFERENCE;
    use metadata::{ExternalLyricsProvider, Settings as MetadataSettings};
    use playback::{DEFAULT_AUTO_DJ_REFILL_THRESHOLD, MIN_AUTO_DJ_REFILL_THRESHOLD};
    use rich_presence::Settings as RichPresenceSettings;
    use scrobbling::AudioscrobblerSettings;
    use secrets::SecretStorageMode;
    use sources::{LibrarySourceSelection, LocalLibraryFolder};

    use super::*;

    #[test]
    fn sparse_legacy_json_keeps_persisted_defaults_and_home_order() {
        let json = r#"{
            "theme_preference":"System",
            "private_mode":false,
            "notifications_enabled":false,
            "external_lyrics_enabled":false,
            "discord_presence_enabled":false,
            "home_sections":["Explore","RecentlyPlayed"]
        }"#;

        let mut settings =
            serde_json::from_str::<StoredSettings>(json).expect("deserialize legacy settings");

        assert_eq!(
            settings.ui.secret_storage_mode,
            SecretStorageMode::ConfigFile
        );
        assert_eq!(
            settings.ui.auto_dj_refill_threshold,
            DEFAULT_AUTO_DJ_REFILL_THRESHOLD
        );
        assert!(settings.ui.lyrics_panel_visible);
        assert!(settings.ui.type_to_search_enabled);
        assert!(settings.ui.control_notifications_enabled);
        assert!(settings.ui.release_notifications_enabled);
        assert_eq!(
            settings.ui.metadata.external_lyrics_providers,
            metadata::default_external_lyrics_providers()
        );

        settings.migrate_defaults();

        assert_eq!(
            settings.ui.home_blocks,
            vec![
                HomeBlockKind::Showcase,
                HomeBlockKind::Explore,
                HomeBlockKind::RecentlyPlayed,
                HomeBlockKind::Genres,
            ]
        );
        assert!(settings.legacy_home_sections.is_none());
        assert!(
            serde_json::to_value(&settings)
                .expect("serialize migrated settings")
                .get("home_sections")
                .is_none()
        );
        assert_eq!(settings.ui.library_lists.len(), LibraryListKey::all().len());
    }

    #[test]
    fn legacy_track_table_sort_migrates_to_the_tracks_list_owner() {
        let json = r#"{
            "theme_preference":"System",
            "private_mode":false,
            "notifications_enabled":false,
            "external_lyrics_enabled":false,
            "discord_presence_enabled":false,
            "track_table": {
                "visible_columns":["Title","Album","Year"],
                "sort_key":"Artist",
                "descending":true,
                "layout_version":4
            }
        }"#;

        let mut settings =
            serde_json::from_str::<StoredSettings>(json).expect("deserialize legacy track table");
        settings.migrate_defaults();

        let tracks = settings
            .ui
            .library_lists
            .iter()
            .find(|entry| entry.key == LibraryListKey::Tracks)
            .expect("Tracks settings should be present");
        assert_eq!(tracks.settings.sort_key, LibraryField::Artist);
        assert!(tracks.settings.descending);
        assert!(
            serde_json::to_value(settings)
                .expect("serialize migrated settings")
                .get("track_table")
                .is_none()
        );
    }

    #[test]
    fn rich_presence_owner_preserves_flat_settings_shape() {
        let mut value = serde_json::to_value(StoredSettings::default())
            .unwrap_or_else(|error| panic!("serialize settings: {error}"));
        let object = value
            .as_object_mut()
            .unwrap_or_else(|| panic!("settings should serialize as an object"));
        assert!(!object.contains_key("rich_presence"));
        assert_eq!(object["discord_presence_enabled"], false);
        object.insert(
            "discord_display_type".to_string(),
            serde_json::Value::String("app".to_string()),
        );
        object.insert(
            "discord_client_id".to_string(),
            serde_json::Value::String(String::new()),
        );
        object.remove("discord_link_type");

        let mut restored = serde_json::from_value::<StoredSettings>(value)
            .unwrap_or_else(|error| panic!("restore flat rich-presence settings: {error}"));
        restored.migrate_defaults();

        assert!(!restored.ui.rich_presence.enabled);
        assert_eq!(
            restored.ui.rich_presence.client_id,
            rich_presence::DEFAULT_CLIENT_ID
        );
        assert_eq!(
            restored.ui.rich_presence.display_type,
            rich_presence::DisplayType::Application
        );
        assert_eq!(
            restored.ui.rich_presence.link_type,
            rich_presence::LinkType::MusicBrainz
        );
    }

    #[test]
    fn unknown_layout_modes_do_not_discard_other_stored_fields() {
        let json = r#"{
            "layout": {
                "default_profile": {
                    "left_sidebar": "Future",
                    "right_sidebar": "Future",
                    "last_visible_right_sidebar": "Future"
                },
                "narrow_profile": {
                    "left_sidebar": "Hidden",
                    "right_sidebar": "Comfortable"
                }
            },
            "theme_preference": "System",
            "private_mode": false,
            "notifications_enabled": false,
            "secret_storage_mode": "system-keyring",
            "secret_scope_id": "test-scope",
            "external_lyrics_enabled": true,
            "discord_presence_enabled": false
        }"#;

        let settings =
            serde_json::from_str::<StoredSettings>(json).expect("deserialize stored settings");

        assert_eq!(
            settings.ui.secret_storage_mode,
            SecretStorageMode::SystemKeyring
        );
        assert_eq!(settings.secret_scope_id, "test-scope");
        assert_eq!(
            settings.ui.layout.default_profile.left_sidebar,
            LeftSidebarMode::Full
        );
        assert_eq!(
            settings.ui.layout.default_profile.right_sidebar,
            RightSidebarMode::Visible
        );
        assert_eq!(
            settings.ui.layout.narrow_profile.left_sidebar,
            LeftSidebarMode::Hidden
        );
        assert_eq!(
            settings.ui.layout.narrow_profile.right_sidebar,
            RightSidebarMode::Visible
        );
        assert_eq!(settings.ui.layout.preferred_right_sidebar_width, 300);
    }

    #[test]
    fn layout_migrates_legacy_right_size_to_one_global_preference() {
        let mut value = serde_json::to_value(StoredSettings::default())
            .expect("serialize current settings fixture");
        value["layout"] = serde_json::json!({
            "default_profile": {
                "left_sidebar": "Full",
                "right_sidebar": "Hidden",
                "last_visible_right_sidebar": "Comfortable"
            },
            "narrow_profile": {
                "left_sidebar": "Compact",
                "right_sidebar": "Spacious"
            }
        });

        let settings =
            serde_json::from_value::<StoredSettings>(value).expect("deserialize legacy layout");

        assert_eq!(
            settings.ui.layout.default_profile.right_sidebar,
            RightSidebarMode::Hidden
        );
        assert_eq!(
            settings.ui.layout.narrow_profile.right_sidebar,
            RightSidebarMode::Visible
        );
        assert_eq!(settings.ui.layout.preferred_right_sidebar_width, 400);
        assert_eq!(
            settings.ui.layout.preferred_left_sidebar_width,
            ::ui::DEFAULT_LEFT_SIDEBAR_WIDTH
        );

        let mut value = serde_json::to_value(settings).expect("serialize migrated layout");
        assert_eq!(
            value["layout"]["default_profile"]["right_sidebar"],
            "Hidden"
        );
        assert_eq!(
            value["layout"]["narrow_profile"]["right_sidebar"],
            "Visible"
        );
        assert!(
            value["layout"]["default_profile"]
                .get("last_visible_right_sidebar")
                .is_none()
        );

        value["layout"]["narrow_profile"]["right_sidebar"] =
            serde_json::Value::String("Shown".to_string());
        let previous_name = serde_json::from_value::<StoredSettings>(value)
            .expect("deserialize previous visible-state name");
        assert_eq!(
            previous_name.ui.layout.narrow_profile.right_sidebar,
            RightSidebarMode::Visible
        );
    }

    #[test]
    fn aggregate_migration_preserves_cross_setting_compatibility() {
        let mut settings = StoredSettings {
            ui: UiSettings {
                language: "de_DE\0".to_string(),
                release_notification_seen_version: Some("  ".to_string()),
                rich_presence: RichPresenceSettings {
                    enabled: false,
                    client_id: String::new(),
                    ..RichPresenceSettings::default()
                },
                metadata: MetadataSettings {
                    external_lyrics_providers: vec![
                        ExternalLyricsProvider::Genius,
                        ExternalLyricsProvider::Netease,
                        ExternalLyricsProvider::Genius,
                    ],
                    lyrics_provider_settings_version: 0,
                    suppressed_auto_lyrics_track_ids: vec!["track-one".to_string()],
                    ..MetadataSettings::default()
                },
                auto_dj_refill_threshold: 0,
                tray_enabled: false,
                exit_to_tray: true,
                start_minimized: true,
                lastfm_api_key: String::new(),
                scrobbling: Settings {
                    lastfm: AudioscrobblerSettings {
                        api_key: " scrobble-key ".to_string(),
                        ..AudioscrobblerSettings::default()
                    },
                    ..Settings::default()
                },
                library_lists: vec![LibraryListSettingsEntry {
                    key: LibraryListKey::Playlists,
                    settings: LibraryListSettings {
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
                ..UiSettings::default()
            },
            ..StoredSettings::default()
        };

        settings.migrate_defaults();

        assert_eq!(settings.ui.language, SYSTEM_LANGUAGE_PREFERENCE);
        assert_eq!(settings.ui.release_notification_seen_version, None);
        assert_eq!(
            settings.ui.rich_presence.client_id,
            rich_presence::DEFAULT_CLIENT_ID
        );
        assert!(!settings.ui.rich_presence.enabled);
        assert_eq!(
            settings.ui.metadata.external_lyrics_providers,
            vec![
                ExternalLyricsProvider::Genius,
                ExternalLyricsProvider::Netease
            ]
        );
        assert_eq!(
            settings.ui.metadata.lyrics_provider_settings_version,
            metadata::LYRICS_PROVIDER_SETTINGS_VERSION
        );
        assert!(
            settings
                .ui
                .metadata
                .suppressed_auto_lyrics_track_ids
                .is_empty()
        );
        assert_eq!(
            settings.ui.auto_dj_refill_threshold,
            MIN_AUTO_DJ_REFILL_THRESHOLD
        );
        assert!(!settings.ui.exit_to_tray);
        assert!(!settings.ui.start_minimized);
        assert_eq!(settings.ui.lastfm_api_key, "scrobble-key");
        assert!(settings.ui.scrobbling.lastfm.api_key.is_empty());
        assert_eq!(
            settings.scrobbling_runtime_settings().lastfm.api_key,
            "scrobble-key"
        );
        assert_eq!(
            settings
                .ui
                .library_list(LibraryListKey::Playlists)
                .row_fields,
            vec![
                LibraryField::Image,
                LibraryField::Title,
                LibraryField::SongCount
            ]
        );
    }

    #[test]
    fn current_home_blocks_replace_the_read_only_legacy_input() {
        let sources = LibrarySourceSettings {
            selected: Some(LibrarySourceSelection::Local),
            local_folders: vec![LocalLibraryFolder {
                path: "/music".to_string(),
            }],
        };
        let mut stored = StoredSettings {
            sources: sources.clone(),
            jellyfin_device_id: "device-id".to_string(),
            secret_scope_id: "scope-id".to_string(),
            legacy_home_sections: Some(vec![HomeSectionKind::MostPlayed]),
            ..StoredSettings::default()
        };
        let mut settings = stored.ui.clone();
        settings.private_mode = true;
        settings.home_blocks = vec![HomeBlockKind::Showcase, HomeBlockKind::RecentlyPlayed];

        stored.ui = settings;

        assert_eq!(stored.sources, sources);
        assert_eq!(stored.jellyfin_device_id, "device-id");
        assert_eq!(stored.secret_scope_id, "scope-id");
        assert_eq!(
            stored.legacy_home_sections,
            Some(vec![HomeSectionKind::MostPlayed])
        );
        assert!(stored.ui.private_mode);

        stored.migrate_defaults();

        assert_eq!(stored.sources, sources);
        assert_eq!(stored.jellyfin_device_id, "device-id");
        assert_eq!(stored.secret_scope_id, "scope-id");
        assert!(stored.legacy_home_sections.is_none());
        let serialized = serde_json::to_value(&stored).expect("serialize current settings");
        assert!(serialized.get("home_sections").is_none());
        assert_eq!(
            serialized.get("home_blocks"),
            Some(&serde_json::json!(["Showcase", "RecentlyPlayed"]))
        );
    }
}
