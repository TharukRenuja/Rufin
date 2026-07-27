use serde::{Deserialize, Serialize};

mod current;
mod events;
mod lyrics;

pub use current::{LyricsContext, LyricsHandle, LyricsService};
pub use events::{CurrentLyrics, LyricsEvent};
pub use lyrics::{
    LocalLyricsInput, lyrics_from_search_result, save_current_lyrics, save_lyrics_search_result,
    search_lyrics,
};

pub const LYRICS_PROVIDER_SETTINGS_VERSION: u8 = 1;

const fn msgid(message: &'static str) -> &'static str {
    message
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum ExternalLyricsProvider {
    #[serde(rename = "lrclib")]
    Lrclib,
    #[serde(rename = "netease")]
    Netease,
    #[serde(rename = "genius")]
    Genius,
    #[serde(rename = "simpmusic")]
    SimpMusic,
}

impl ExternalLyricsProvider {
    pub const fn all() -> [Self; 4] {
        [Self::Lrclib, Self::Netease, Self::Genius, Self::SimpMusic]
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Lrclib => msgid("LRCLIB"),
            Self::Netease => msgid("NetEase"),
            Self::Genius => msgid("Genius"),
            Self::SimpMusic => msgid("SimpMusic"),
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::Lrclib => "lrclib",
            Self::Netease => "netease",
            Self::Genius => "genius",
            Self::SimpMusic => "simpmusic",
        }
    }

    pub fn from_key(value: &str) -> Option<Self> {
        Self::all()
            .into_iter()
            .find(|provider| provider.key() == value)
    }
}

pub fn default_external_lyrics_providers() -> Vec<ExternalLyricsProvider> {
    vec![
        ExternalLyricsProvider::Lrclib,
        ExternalLyricsProvider::Netease,
    ]
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Settings {
    pub external_lyrics_enabled: bool,
    #[serde(default = "default_external_lyrics_providers")]
    pub external_lyrics_providers: Vec<ExternalLyricsProvider>,
    #[serde(default = "default_true")]
    pub prefer_server_lyrics: bool,
    #[serde(default)]
    pub lyrics_provider_settings_version: u8,
    #[serde(default)]
    pub suppressed_auto_lyrics_track_ids: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            external_lyrics_enabled: true,
            external_lyrics_providers: default_external_lyrics_providers(),
            prefer_server_lyrics: true,
            lyrics_provider_settings_version: LYRICS_PROVIDER_SETTINGS_VERSION,
            suppressed_auto_lyrics_track_ids: Vec::new(),
        }
    }
}

impl Settings {
    pub fn sanitize(&mut self) {
        let mut seen = Vec::new();
        self.external_lyrics_providers.retain(|provider| {
            if seen.contains(provider) {
                false
            } else {
                seen.push(*provider);
                true
            }
        });
        if self.lyrics_provider_settings_version < LYRICS_PROVIDER_SETTINGS_VERSION {
            self.suppressed_auto_lyrics_track_ids.clear();
            self.lyrics_provider_settings_version = LYRICS_PROVIDER_SETTINGS_VERSION;
        }
    }

    pub(crate) const fn external_lyrics_network_allowed(&self, private_mode: bool) -> bool {
        self.external_lyrics_enabled && !private_mode
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum LyricsOrigin {
    Local,
    Native,
    External(ExternalLyricsProvider),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LyricsRole {
    Original,
    Translation,
    Transliteration,
}

impl LyricsRole {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Translation => "translation",
            Self::Transliteration => "transliteration",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LyricsLine {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_millis: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LyricsDocument {
    pub origin: LyricsOrigin,
    pub lines: Vec<LyricsLine>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LyricsSearchResult {
    pub provider: ExternalLyricsProvider,
    pub id: String,
    pub track_name: String,
    pub artist_name: String,
    pub album_name: String,
    pub duration_seconds: u32,
    pub synced_lyrics: Option<String>,
    pub plain_lyrics: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LyricsQuery {
    pub artist_name: String,
    pub track_name: String,
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_settings_preserve_flat_defaults_and_provider_order() {
        let mut settings = serde_json::from_str::<Settings>(
            r#"{"external_lyrics_enabled":false,"external_lyrics_providers":["genius","genius","lrclib"]}"#,
        )
        .expect("settings");
        settings.sanitize();

        assert!(!settings.external_lyrics_enabled);
        assert!(settings.prefer_server_lyrics);
        assert_eq!(
            settings.external_lyrics_providers,
            vec![
                ExternalLyricsProvider::Genius,
                ExternalLyricsProvider::Lrclib
            ]
        );
    }
}
