use library::TrackId;
use serde::{Deserialize, Serialize};

mod events;
mod lyrics;
mod releases;

pub use events::LyricsEvent;
pub use lyrics::{
    LocalLyricsInput, LyricsCacheUpdate, LyricsPlan, LyricsRequestKind, LyricsResolution,
    REMOTE_LYRICS_ORIGIN, ResolveLyrics, decode_cached_lyrics, encode_cached_lyrics,
    lyrics_from_search_result, resolve_lyrics, save_lyrics_search_result, search_lyrics,
};
pub use releases::{
    ALBUM_IDENTITY_LOOKUP_LIMIT, AlbumIdentityChange, AlbumIdentityEnrichment,
    AlbumReleaseMetadata, enrich_album_identities, search_album_release_group_ids,
    search_album_release_ids,
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
    pub external_metadata_enabled: bool,
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
            external_metadata_enabled: true,
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

    pub const fn external_lyrics_allowed(&self, private_mode: bool) -> bool {
        self.external_lyrics_enabled && !private_mode
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum LyricsSource {
    Local,
    Server,
    Remote,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LyricLine {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_millis: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Lyrics {
    pub track_id: TrackId,
    pub source: LyricsSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_provider: Option<ExternalLyricsProvider>,
    pub lines: Vec<LyricLine>,
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
        assert!(settings.external_metadata_enabled);
        assert!(settings.prefer_server_lyrics);
        assert_eq!(
            settings.external_lyrics_providers,
            vec![
                ExternalLyricsProvider::Genius,
                ExternalLyricsProvider::Lrclib
            ]
        );
    }

    #[test]
    fn lyrics_cache_json_keeps_existing_variant_and_provider_names() {
        let lyrics = Lyrics {
            track_id: TrackId::new("jellyfin:track:one"),
            source: LyricsSource::Remote,
            external_provider: Some(ExternalLyricsProvider::Netease),
            lines: vec![LyricLine {
                text: "line".to_string(),
                start_millis: Some(1_000),
            }],
        };

        let json = serde_json::to_string(&lyrics).expect("lyrics JSON");
        assert!(json.contains("\"source\":\"Remote\""));
        assert!(json.contains("\"external_provider\":\"netease\""));
        assert_eq!(
            serde_json::from_str::<Lyrics>(&json).expect("lyrics"),
            lyrics
        );
    }
}
