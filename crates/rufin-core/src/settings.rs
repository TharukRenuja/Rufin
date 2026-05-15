use serde::{Deserialize, Serialize};

use crate::domain::HomeSectionKind;
use crate::route::DensityMode;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ThemePreference {
    System,
    Light,
    Dark,
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AppSettings;

    #[test]
    fn settings_default_to_private_external_features_off() {
        let settings = AppSettings::default();

        assert!(!settings.notifications_enabled);
        assert!(!settings.external_lyrics_enabled);
        assert!(!settings.discord_presence_enabled);
        assert_eq!(settings.home_sections.len(), 5);
    }

    #[test]
    fn settings_serialize_to_json() {
        let settings = AppSettings::default();

        let json = serde_json::to_string(&settings).expect("serialize settings");
        let restored = serde_json::from_str::<AppSettings>(&json).expect("deserialize settings");

        assert_eq!(restored, settings);
    }
}
