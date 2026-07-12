use domain::settings::{DEFAULT_AUTO_DJ_REFILL_THRESHOLD, default_library_list_settings};
use domain::{
    ExternalSiteLinkSettings, LayoutSettings, LibraryListKey, LibraryListSettings,
    LibraryListSettingsEntry, LibrarySourceSettings, MAX_AUTO_DJ_REFILL_THRESHOLD,
    MIN_AUTO_DJ_REFILL_THRESHOLD, SecretStorageMode, SidebarSettings, ThemePreference,
    TrackTableSettings, default_language_preference, sanitize_language_preference,
    sanitized_window_size,
};
use library::{HomeBlockKind, HomeSectionKind};
use metadata::Settings as MetadataSettings;
use playback::PlaybackSettings;
use rich_presence::Settings as RichPresenceSettings;
use scrobbling::Settings;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct StoredSettings {
    #[serde(default)]
    pub(crate) layout: LayoutSettings,
    #[serde(default)]
    pub(crate) sidebar: SidebarSettings,
    #[serde(default)]
    pub(crate) sources: LibrarySourceSettings,
    pub(crate) theme_preference: ThemePreference,
    #[serde(default = "default_language_preference")]
    pub(crate) language: String,
    pub(crate) private_mode: bool,
    pub(crate) notifications_enabled: bool,
    #[serde(default = "default_true")]
    pub(crate) control_notifications_enabled: bool,
    #[serde(default = "default_true")]
    pub(crate) release_notifications_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) release_notification_seen_version: Option<String>,
    #[serde(default = "legacy_secret_storage_mode")]
    pub(crate) secret_storage_mode: SecretStorageMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) secret_scope_id: String,
    #[serde(flatten)]
    pub(crate) metadata: MetadataSettings,
    #[serde(default)]
    pub(crate) external_site_links: ExternalSiteLinkSettings,
    #[serde(default)]
    pub(crate) prefer_server_playlist_covers: bool,
    #[serde(default)]
    pub(crate) seekbar_waveform_enabled: bool,
    #[serde(default)]
    pub(crate) tray_enabled: bool,
    #[serde(default)]
    pub(crate) exit_to_tray: bool,
    #[serde(default)]
    pub(crate) start_minimized: bool,
    #[serde(default = "default_true")]
    pub(crate) type_to_search_enabled: bool,
    #[serde(default)]
    pub(crate) jellyfin_device_id: String,
    #[serde(flatten)]
    pub(crate) rich_presence: RichPresenceSettings,
    #[serde(default)]
    pub(crate) lastfm_api_key: String,
    #[serde(default)]
    pub(crate) scrobbling: Settings,
    #[serde(default)]
    pub(crate) auto_dj_enabled: bool,
    #[serde(default = "default_auto_dj_refill_threshold")]
    pub(crate) auto_dj_refill_threshold: u8,
    #[serde(default)]
    pub(crate) playback: PlaybackSettings,
    #[serde(default = "default_home_sections")]
    pub(crate) home_sections: Vec<HomeSectionKind>,
    #[serde(default)]
    pub(crate) home_blocks: Vec<HomeBlockKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) window_width: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) window_height: Option<i32>,
    #[serde(default = "default_lyrics_panel_visible")]
    pub(crate) lyrics_panel_visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) queue_lyrics_height: Option<i32>,
    #[serde(default)]
    pub(crate) track_table: TrackTableSettings,
    #[serde(default)]
    pub(crate) library_lists: Vec<LibraryListSettingsEntry>,
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            layout: LayoutSettings::default(),
            sidebar: SidebarSettings::default(),
            sources: LibrarySourceSettings::default(),
            theme_preference: ThemePreference::System,
            language: default_language_preference(),
            private_mode: false,
            notifications_enabled: false,
            control_notifications_enabled: true,
            release_notifications_enabled: true,
            release_notification_seen_version: None,
            secret_storage_mode: SecretStorageMode::default(),
            secret_scope_id: String::new(),
            metadata: MetadataSettings::default(),
            external_site_links: ExternalSiteLinkSettings::default(),
            prefer_server_playlist_covers: false,
            seekbar_waveform_enabled: true,
            tray_enabled: false,
            exit_to_tray: false,
            start_minimized: false,
            type_to_search_enabled: true,
            jellyfin_device_id: String::new(),
            rich_presence: RichPresenceSettings::default(),
            lastfm_api_key: String::new(),
            scrobbling: Settings::default(),
            auto_dj_enabled: false,
            auto_dj_refill_threshold: DEFAULT_AUTO_DJ_REFILL_THRESHOLD,
            playback: PlaybackSettings::default(),
            home_sections: default_home_sections(),
            home_blocks: default_home_blocks(),
            window_width: None,
            window_height: None,
            lyrics_panel_visible: true,
            queue_lyrics_height: None,
            track_table: TrackTableSettings::default(),
            library_lists: default_library_list_settings(),
        }
    }
}

impl StoredSettings {
    pub(crate) fn migrate_defaults(&mut self) {
        self.rich_presence.sanitize();
        self.track_table.migrate_defaults();
        self.playback.sanitize();
        self.metadata.sanitize();
        self.auto_dj_refill_threshold = self
            .auto_dj_refill_threshold
            .clamp(MIN_AUTO_DJ_REFILL_THRESHOLD, MAX_AUTO_DJ_REFILL_THRESHOLD);
        self.scrobbling.sanitize();
        self.lastfm_api_key = self.lastfm_api_key.trim().to_string();
        self.language = sanitize_language_preference(&self.language);
        self.release_notification_seen_version = self
            .release_notification_seen_version
            .as_deref()
            .map(str::trim)
            .filter(|version| !version.is_empty())
            .map(str::to_string);
        if self.lastfm_api_key.is_empty() && !self.scrobbling.lastfm.api_key.is_empty() {
            self.lastfm_api_key = self.scrobbling.lastfm.api_key.clone();
        }
        self.scrobbling.lastfm.api_key.clear();
        self.layout.sanitize();
        self.sidebar.sanitize();
        self.sources.sanitize();
        if !self.tray_enabled {
            self.exit_to_tray = false;
            self.start_minimized = false;
        }
        if let Some((width, height)) = sanitized_window_size(self.window_width, self.window_height)
        {
            self.window_width = Some(width);
            self.window_height = Some(height);
        } else {
            self.window_width = None;
            self.window_height = None;
        }
        self.migrate_home_blocks();
        self.migrate_library_lists();
    }

    pub(crate) fn scrobbling_runtime_settings(&self) -> Settings {
        let mut settings = self.scrobbling.clone();
        settings.lastfm.api_key = self.lastfm_api_key.clone();
        settings
    }

    fn migrate_home_blocks(&mut self) {
        if self.home_sections.is_empty() {
            self.home_sections = default_home_sections();
        }
        if self.home_blocks.is_empty() {
            self.home_blocks = Vec::with_capacity(self.home_sections.len() + 2);
            self.home_blocks.push(HomeBlockKind::Showcase);
            for section in &self.home_sections {
                self.home_blocks.push(match section {
                    HomeSectionKind::Explore => HomeBlockKind::Explore,
                    HomeSectionKind::MostPlayed => HomeBlockKind::MostPlayed,
                    HomeSectionKind::NewlyAdded => HomeBlockKind::NewlyAdded,
                    HomeSectionKind::RecentlyPlayed => HomeBlockKind::RecentlyPlayed,
                    HomeSectionKind::RecentlyReleased => HomeBlockKind::RecentlyReleased,
                });
            }
            if !self.home_blocks.contains(&HomeBlockKind::Genres) {
                self.home_blocks.push(HomeBlockKind::Genres);
            }
        }
        sanitize_home_blocks(&mut self.home_blocks);
        self.home_sections = self
            .home_blocks
            .iter()
            .filter_map(|block| block.section_kind())
            .collect();
    }

    fn migrate_library_lists(&mut self) {
        if self.library_lists.is_empty() {
            self.library_lists = default_library_list_settings();
        }
        for key in LibraryListKey::all() {
            if !self.library_lists.iter().any(|entry| entry.key == key) {
                self.library_lists.push(LibraryListSettingsEntry {
                    key,
                    settings: LibraryListSettings::for_key(key),
                });
            }
        }
        self.library_lists
            .retain(|entry| LibraryListKey::all().contains(&entry.key));
        self.library_lists.sort_by_key(|entry| {
            LibraryListKey::all()
                .iter()
                .position(|key| *key == entry.key)
                .unwrap_or(usize::MAX)
        });
        for entry in &mut self.library_lists {
            entry.settings.sanitize(entry.key);
        }
    }

    pub(crate) fn library_list(&self, key: LibraryListKey) -> LibraryListSettings {
        self.library_lists
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| entry.settings.clone())
            .unwrap_or_else(|| LibraryListSettings::for_key(key))
    }
}

fn legacy_secret_storage_mode() -> SecretStorageMode {
    SecretStorageMode::ConfigFile
}

fn default_true() -> bool {
    true
}

fn default_lyrics_panel_visible() -> bool {
    true
}

fn default_auto_dj_refill_threshold() -> u8 {
    DEFAULT_AUTO_DJ_REFILL_THRESHOLD
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

fn default_home_blocks() -> Vec<HomeBlockKind> {
    vec![
        HomeBlockKind::Showcase,
        HomeBlockKind::Explore,
        HomeBlockKind::MostPlayed,
        HomeBlockKind::NewlyAdded,
        HomeBlockKind::RecentlyPlayed,
        HomeBlockKind::RecentlyReleased,
        HomeBlockKind::Genres,
    ]
}

fn sanitize_home_blocks(blocks: &mut Vec<HomeBlockKind>) {
    let mut seen = Vec::new();
    blocks.retain(|block| {
        if seen.contains(block) {
            false
        } else {
            seen.push(*block);
            true
        }
    });
    if blocks.is_empty() {
        *blocks = default_home_blocks();
    }
}

#[cfg(test)]
mod tests {
    use domain::{
        LeftSidebarMode, LibraryField, LibraryLayout, LibraryListSettings, RightSidebarMode,
        SYSTEM_LANGUAGE_PREFERENCE,
    };
    use metadata::ExternalLyricsProvider;
    use scrobbling::AudioscrobblerSettings;

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

        assert_eq!(settings.secret_storage_mode, SecretStorageMode::ConfigFile);
        assert!(settings.lyrics_panel_visible);
        assert!(settings.type_to_search_enabled);
        assert!(settings.control_notifications_enabled);
        assert!(settings.release_notifications_enabled);
        assert_eq!(
            settings.metadata.external_lyrics_providers,
            metadata::default_external_lyrics_providers()
        );

        settings.migrate_defaults();

        assert_eq!(
            settings.home_blocks,
            vec![
                HomeBlockKind::Showcase,
                HomeBlockKind::Explore,
                HomeBlockKind::RecentlyPlayed,
                HomeBlockKind::Genres,
            ]
        );
        assert_eq!(
            settings.home_sections,
            vec![HomeSectionKind::Explore, HomeSectionKind::RecentlyPlayed]
        );
        assert_eq!(settings.library_lists.len(), LibraryListKey::all().len());
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
            serde_json::Value::String("feishin".to_string()),
        );
        object.insert(
            "discord_client_id".to_string(),
            serde_json::Value::String(String::new()),
        );
        object.remove("discord_link_type");

        let mut restored = serde_json::from_value::<StoredSettings>(value)
            .unwrap_or_else(|error| panic!("restore flat rich-presence settings: {error}"));
        restored.migrate_defaults();

        assert!(!restored.rich_presence.enabled);
        assert_eq!(
            restored.rich_presence.client_id,
            rich_presence::DEFAULT_CLIENT_ID
        );
        assert_eq!(
            restored.rich_presence.display_type,
            rich_presence::DisplayType::Application
        );
        assert_eq!(
            restored.rich_presence.link_type,
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
            settings.secret_storage_mode,
            SecretStorageMode::SystemKeyring
        );
        assert_eq!(settings.secret_scope_id, "test-scope");
        assert_eq!(
            settings.layout.default_profile.left_sidebar,
            LeftSidebarMode::Full
        );
        assert_eq!(
            settings.layout.default_profile.right_sidebar,
            RightSidebarMode::Default
        );
        assert_eq!(
            settings.layout.narrow_profile.left_sidebar,
            LeftSidebarMode::Hidden
        );
        assert_eq!(
            settings.layout.narrow_profile.right_sidebar,
            RightSidebarMode::Comfortable
        );
    }

    #[test]
    fn aggregate_migration_preserves_cross_setting_compatibility() {
        let mut settings = StoredSettings {
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
            ..StoredSettings::default()
        };

        settings.migrate_defaults();

        assert_eq!(settings.language, SYSTEM_LANGUAGE_PREFERENCE);
        assert_eq!(settings.release_notification_seen_version, None);
        assert_eq!(
            settings.rich_presence.client_id,
            rich_presence::DEFAULT_CLIENT_ID
        );
        assert!(!settings.rich_presence.enabled);
        assert_eq!(
            settings.metadata.external_lyrics_providers,
            vec![
                ExternalLyricsProvider::Genius,
                ExternalLyricsProvider::Netease
            ]
        );
        assert_eq!(
            settings.metadata.lyrics_provider_settings_version,
            metadata::LYRICS_PROVIDER_SETTINGS_VERSION
        );
        assert!(
            settings
                .metadata
                .suppressed_auto_lyrics_track_ids
                .is_empty()
        );
        assert_eq!(
            settings.auto_dj_refill_threshold,
            MIN_AUTO_DJ_REFILL_THRESHOLD
        );
        assert!(!settings.exit_to_tray);
        assert!(!settings.start_minimized);
        assert_eq!(settings.lastfm_api_key, "scrobble-key");
        assert!(settings.scrobbling.lastfm.api_key.is_empty());
        assert_eq!(
            settings.scrobbling_runtime_settings().lastfm.api_key,
            "scrobble-key"
        );
        assert_eq!(
            settings.library_list(LibraryListKey::Playlists).row_fields,
            vec![
                LibraryField::Image,
                LibraryField::Title,
                LibraryField::SongCount
            ]
        );
    }
}
