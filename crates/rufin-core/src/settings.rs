use serde::{Deserialize, Serialize};

use crate::domain::HomeSectionKind;
use crate::route::DensityMode;

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
}

impl Default for TrackTableSettings {
    fn default() -> Self {
        Self {
            visible_columns: vec![
                TrackTableColumn::TrackNumber,
                TrackTableColumn::Title,
                TrackTableColumn::Artist,
                TrackTableColumn::Album,
                TrackTableColumn::Year,
                TrackTableColumn::Duration,
                TrackTableColumn::Favorite,
            ],
            sort_key: TrackSortKey::TrackNumber,
            descending: false,
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
            track_table: TrackTableSettings::default(),
        }
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
        assert!(
            settings
                .track_table
                .visible_columns
                .contains(&TrackTableColumn::Title)
        );
        assert_eq!(settings.track_table.sort_key, TrackSortKey::TrackNumber);
    }

    #[test]
    fn settings_serialize_to_json() {
        let settings = AppSettings {
            window_width: Some(1180),
            window_height: Some(760),
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
        assert_eq!(restored.track_table.sort_key, TrackSortKey::TrackNumber);
    }
}
