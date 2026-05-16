use serde::{Deserialize, Deserializer, Serialize, de};

use crate::domain::HomeSectionKind;
use crate::route::DensityMode;

pub const TRACK_TABLE_LAYOUT_VERSION: u8 = 2;
pub const QUEUE_LYRICS_LAYOUT_VERSION: u8 = 3;
pub const DEFAULT_DISCORD_CLIENT_ID: &str = "1505345384686419979";
const LEGACY_APPLICATION_DISPLAY_BYTES: &[u8] = &[102, 101, 105, 115, 104, 105, 110];

fn default_right_panel_visible() -> bool {
    true
}

fn default_lyrics_panel_visible() -> bool {
    true
}

fn default_discord_client_id() -> String {
    DEFAULT_DISCORD_CLIENT_ID.to_string()
}

fn default_true() -> bool {
    true
}

const DEFAULT_TRACK_TABLE_COLUMNS: [TrackTableColumn; 4] = [
    TrackTableColumn::TrackNumber,
    TrackTableColumn::Title,
    TrackTableColumn::Album,
    TrackTableColumn::Year,
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ThemePreference {
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub enum DiscordDisplayType {
    #[serde(rename = "artist")]
    Artist,
    #[serde(rename = "application")]
    #[default]
    Application,
    #[serde(rename = "song")]
    Song,
}

impl<'de> Deserialize<'de> for DiscordDisplayType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "artist" => Ok(Self::Artist),
            "application" | "app" => Ok(Self::Application),
            "song" => Ok(Self::Song),
            legacy if legacy.as_bytes() == LEGACY_APPLICATION_DISPLAY_BYTES => {
                Ok(Self::Application)
            }
            other => Err(de::Error::unknown_variant(
                other,
                &["artist", "application", "song"],
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum DiscordLinkType {
    #[serde(rename = "last_fm")]
    LastFm,
    #[serde(rename = "musicbrainz")]
    MusicBrainz,
    #[serde(rename = "musicbrainz_last_fm")]
    MusicBrainzLastFm,
    #[serde(rename = "none")]
    #[default]
    None,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum TrackTableColumn {
    TrackNumber,
    Title,
    Artist,
    Album,
    Year,
    Duration,
    Favorite,
}

impl TrackTableColumn {
    pub fn all() -> [Self; 7] {
        [
            Self::TrackNumber,
            Self::Title,
            Self::Artist,
            Self::Album,
            Self::Year,
            Self::Duration,
            Self::Favorite,
        ]
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::TrackNumber => "#",
            Self::Title => "Title",
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Year => "Year",
            Self::Duration => "Duration",
            Self::Favorite => "Favorite",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TrackSortKey {
    TrackNumber,
    Title,
    Artist,
    Album,
    Year,
    Duration,
    Favorite,
}

impl TrackSortKey {
    pub fn all() -> [Self; 7] {
        [
            Self::TrackNumber,
            Self::Title,
            Self::Artist,
            Self::Album,
            Self::Year,
            Self::Duration,
            Self::Favorite,
        ]
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::TrackNumber => "#",
            Self::Title => "Title",
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Year => "Year",
            Self::Duration => "Duration",
            Self::Favorite => "Favorite",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrackTableSettings {
    pub visible_columns: Vec<TrackTableColumn>,
    pub sort_key: TrackSortKey,
    pub descending: bool,
    #[serde(default)]
    pub layout_version: u8,
}

impl Default for TrackTableSettings {
    fn default() -> Self {
        Self {
            visible_columns: DEFAULT_TRACK_TABLE_COLUMNS.to_vec(),
            sort_key: TrackSortKey::TrackNumber,
            descending: false,
            layout_version: TRACK_TABLE_LAYOUT_VERSION,
        }
    }
}

impl TrackTableSettings {
    pub fn migrate_defaults(&mut self) {
        const LEGACY_DEFAULT_COLUMNS: [TrackTableColumn; 7] = [
            TrackTableColumn::TrackNumber,
            TrackTableColumn::Title,
            TrackTableColumn::Artist,
            TrackTableColumn::Album,
            TrackTableColumn::Year,
            TrackTableColumn::Duration,
            TrackTableColumn::Favorite,
        ];
        const COMPOSITE_TITLE_DEFAULT_COLUMNS: [TrackTableColumn; 3] = [
            TrackTableColumn::Title,
            TrackTableColumn::Album,
            TrackTableColumn::Year,
        ];

        if self.layout_version < TRACK_TABLE_LAYOUT_VERSION {
            if self.visible_columns.as_slice() == LEGACY_DEFAULT_COLUMNS
                || self.visible_columns.as_slice() == COMPOSITE_TITLE_DEFAULT_COLUMNS
            {
                self.visible_columns = DEFAULT_TRACK_TABLE_COLUMNS.to_vec();
            }
            self.layout_version = TRACK_TABLE_LAYOUT_VERSION;
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AppSettings {
    pub density_mode: DensityMode,
    pub theme_preference: ThemePreference,
    pub private_mode: bool,
    pub notifications_enabled: bool,
    pub external_lyrics_enabled: bool,
    pub discord_presence_enabled: bool,
    #[serde(default = "default_discord_client_id")]
    pub discord_client_id: String,
    #[serde(default)]
    pub discord_display_type: DiscordDisplayType,
    #[serde(default)]
    pub discord_link_type: DiscordLinkType,
    #[serde(default = "default_true")]
    pub discord_show_paused: bool,
    #[serde(default)]
    pub discord_show_as_listening: bool,
    #[serde(default = "default_true")]
    pub discord_show_state_icon: bool,
    #[serde(default)]
    pub lastfm_api_key: String,
    #[serde(default)]
    pub auto_dj_enabled: bool,
    pub home_sections: Vec<HomeSectionKind>,
    #[serde(default = "default_right_panel_visible")]
    pub right_panel_visible: bool,
    #[serde(default = "default_lyrics_panel_visible")]
    pub lyrics_panel_visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_width: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_height: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_panel_position: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_panel_ratio: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_lyrics_position: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_lyrics_ratio: Option<f64>,
    #[serde(default)]
    pub queue_lyrics_layout_version: u8,
    #[serde(default)]
    pub track_table: TrackTableSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            density_mode: DensityMode::Auto,
            theme_preference: ThemePreference::System,
            private_mode: false,
            notifications_enabled: false,
            external_lyrics_enabled: false,
            discord_presence_enabled: false,
            discord_client_id: default_discord_client_id(),
            discord_display_type: DiscordDisplayType::Application,
            discord_link_type: DiscordLinkType::None,
            discord_show_paused: true,
            discord_show_as_listening: false,
            discord_show_state_icon: true,
            lastfm_api_key: String::new(),
            auto_dj_enabled: false,
            home_sections: vec![
                HomeSectionKind::Explore,
                HomeSectionKind::MostPlayed,
                HomeSectionKind::NewlyAdded,
                HomeSectionKind::RecentlyPlayed,
                HomeSectionKind::RecentlyReleased,
            ],
            right_panel_visible: true,
            lyrics_panel_visible: true,
            window_width: None,
            window_height: None,
            right_panel_position: None,
            right_panel_ratio: None,
            queue_lyrics_position: None,
            queue_lyrics_ratio: None,
            queue_lyrics_layout_version: QUEUE_LYRICS_LAYOUT_VERSION,
            track_table: TrackTableSettings::default(),
        }
    }
}

impl AppSettings {
    pub fn migrate_defaults(&mut self) {
        if self.queue_lyrics_layout_version < QUEUE_LYRICS_LAYOUT_VERSION {
            self.queue_lyrics_position = None;
            self.queue_lyrics_ratio = None;
            self.queue_lyrics_layout_version = QUEUE_LYRICS_LAYOUT_VERSION;
        }
        if self.discord_client_id.trim().is_empty() {
            self.discord_client_id = default_discord_client_id();
            self.discord_presence_enabled = true;
        }
        self.track_table.migrate_defaults();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AppSettings, DEFAULT_DISCORD_CLIENT_ID, DiscordDisplayType, DiscordLinkType,
        LEGACY_APPLICATION_DISPLAY_BYTES, TrackSortKey, TrackTableColumn,
    };

    #[test]
    fn settings_default_to_private_external_features_off() {
        let settings = AppSettings::default();

        assert!(!settings.notifications_enabled);
        assert!(!settings.external_lyrics_enabled);
        assert!(!settings.discord_presence_enabled);
        assert_eq!(settings.discord_client_id, DEFAULT_DISCORD_CLIENT_ID);
        assert_eq!(
            settings.discord_display_type,
            DiscordDisplayType::Application
        );
        assert_eq!(settings.discord_link_type, DiscordLinkType::None);
        assert!(settings.discord_show_paused);
        assert!(!settings.discord_show_as_listening);
        assert!(settings.discord_show_state_icon);
        assert_eq!(settings.lastfm_api_key, "");
        assert!(!settings.auto_dj_enabled);
        assert!(settings.right_panel_visible);
        assert!(settings.lyrics_panel_visible);
        assert_eq!(settings.queue_lyrics_layout_version, 3);
        assert_eq!(settings.home_sections.len(), 5);
        assert_eq!(
            settings.track_table.visible_columns,
            vec![
                TrackTableColumn::TrackNumber,
                TrackTableColumn::Title,
                TrackTableColumn::Album,
                TrackTableColumn::Year,
            ]
        );
        assert_eq!(settings.track_table.sort_key, TrackSortKey::TrackNumber);
    }

    #[test]
    fn settings_serialize_to_json() {
        let settings = AppSettings {
            right_panel_visible: false,
            lyrics_panel_visible: false,
            window_width: Some(1180),
            window_height: Some(760),
            right_panel_position: Some(820),
            right_panel_ratio: Some(0.3),
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
            "density_mode":"Auto",
            "theme_preference":"System",
            "private_mode":false,
            "notifications_enabled":false,
            "external_lyrics_enabled":false,
            "discord_presence_enabled":false,
            "home_sections":["Explore"]
        }"#;

        let restored = serde_json::from_str::<AppSettings>(json).expect("deserialize settings");

        assert_eq!(restored.window_width, None);
        assert_eq!(restored.window_height, None);
        assert!(restored.right_panel_visible);
        assert!(restored.lyrics_panel_visible);
        assert_eq!(restored.right_panel_position, None);
        assert_eq!(restored.right_panel_ratio, None);
        assert_eq!(restored.queue_lyrics_position, None);
        assert_eq!(restored.queue_lyrics_ratio, None);
        assert!(!restored.auto_dj_enabled);
        assert_eq!(restored.discord_client_id, DEFAULT_DISCORD_CLIENT_ID);
        assert_eq!(
            restored.discord_display_type,
            DiscordDisplayType::Application
        );
        assert_eq!(restored.discord_link_type, DiscordLinkType::None);
        assert!(restored.discord_show_paused);
        assert!(!restored.discord_show_as_listening);
        assert!(restored.discord_show_state_icon);
        assert_eq!(restored.lastfm_api_key, "");
        assert_eq!(restored.track_table.sort_key, TrackSortKey::TrackNumber);
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
        let legacy_value =
            std::str::from_utf8(LEGACY_APPLICATION_DISPLAY_BYTES).expect("legacy value");
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
            ]
        );
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
            ]
        );
        assert_eq!(settings.layout_version, super::TRACK_TABLE_LAYOUT_VERSION);
    }
}
