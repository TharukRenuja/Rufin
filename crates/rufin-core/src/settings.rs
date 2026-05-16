use serde::{Deserialize, Serialize};

use crate::domain::HomeSectionKind;
use crate::route::DensityMode;

pub const TRACK_TABLE_LAYOUT_VERSION: u8 = 2;

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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AppSettings {
    pub density_mode: DensityMode,
    pub theme_preference: ThemePreference,
    pub private_mode: bool,
    pub notifications_enabled: bool,
    pub external_lyrics_enabled: bool,
    pub discord_presence_enabled: bool,
    pub home_sections: Vec<HomeSectionKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_width: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_height: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_lyrics_position: Option<i32>,
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
            home_sections: vec![
                HomeSectionKind::Explore,
                HomeSectionKind::MostPlayed,
                HomeSectionKind::NewlyAdded,
                HomeSectionKind::RecentlyPlayed,
                HomeSectionKind::RecentlyReleased,
            ],
            window_width: None,
            window_height: None,
            queue_lyrics_position: None,
            track_table: TrackTableSettings::default(),
        }
    }
}

impl AppSettings {
    pub fn migrate_defaults(&mut self) {
        self.track_table.migrate_defaults();
    }
}

#[cfg(test)]
mod tests {
    use super::{AppSettings, TrackSortKey, TrackTableColumn};

    #[test]
    fn settings_default_to_private_external_features_off() {
        let settings = AppSettings::default();

        assert!(!settings.notifications_enabled);
        assert!(!settings.external_lyrics_enabled);
        assert!(!settings.discord_presence_enabled);
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
            window_width: Some(1180),
            window_height: Some(760),
            queue_lyrics_position: Some(520),
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
        assert_eq!(restored.queue_lyrics_position, None);
        assert_eq!(restored.track_table.sort_key, TrackSortKey::TrackNumber);
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
