use std::fmt;

use library::{GenreId, RadioSeed, TrackId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LyricsSearch {
    ServerOnly,
    ServerThenRemote,
    RemoteThenServer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeLyricsOrigin {
    Server,
    Remote,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLyricLine {
    pub text: String,
    pub start_millis: Option<u64>,
    pub end_millis: Option<u64>,
    pub cue_lines: Vec<NativeLyricCueLine>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLyricCueLine {
    pub text: String,
    pub start_millis: Option<u64>,
    pub end_millis: Option<u64>,
    pub agent_id: Option<String>,
    pub cues: Vec<NativeLyricCue>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLyricCue {
    pub text: String,
    pub start_millis: u64,
    pub end_millis: Option<u64>,
    pub byte_start: usize,
    pub byte_end_exclusive: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeLyricsRole {
    Original,
    Translation,
    Pronunciation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeLyricAgentRole {
    Main,
    Voice,
    Background,
    Group,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLyricAgent {
    pub id: String,
    pub role: NativeLyricAgentRole,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLyricsDocument {
    pub role: NativeLyricsRole,
    pub language: Option<String>,
    pub offset_millis: i64,
    pub lines: Vec<NativeLyricLine>,
    pub agents: Vec<NativeLyricAgent>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLyrics {
    pub origin: NativeLyricsOrigin,
    pub documents: Vec<NativeLyricsDocument>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum PlayedFilter {
    #[default]
    All,
    Unplayed,
    Played,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RandomTrackRequest {
    pub limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_year: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_year: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre_id: Option<GenreId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre_name: Option<String>,
    pub played_filter: PlayedFilter,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GeneratedTracksRequest {
    pub seed: RadioSeed,
    pub limit: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImageBytes {
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PlaybackReportKind {
    Started,
    Progress,
    QualifiedPlay,
    Stopped,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlaybackReport {
    pub kind: PlaybackReportKind,
    pub track_id: TrackId,
    pub started_at_unix_seconds: i64,
    pub position_seconds: u32,
    pub paused: bool,
    pub muted: bool,
    pub volume_percent: u8,
    pub shuffle: bool,
    pub repeat_one: bool,
    pub repeat_all: bool,
    pub failed: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum StreamQuality {
    #[default]
    Original,
    MaxBitrateKbps(u32),
}

impl StreamQuality {
    pub fn max_bitrate_kbps(self) -> Option<u32> {
        match self {
            Self::Original => None,
            Self::MaxBitrateKbps(kbps) => Some(kbps),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StreamRequest {
    pub track_id: TrackId,
    pub quality: StreamQuality,
}

impl StreamRequest {
    pub fn original(track_id: TrackId) -> Self {
        Self {
            track_id,
            quality: StreamQuality::Original,
        }
    }

    pub fn new(track_id: TrackId, quality: StreamQuality) -> Self {
        Self { track_id, quality }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct StreamDescriptor {
    uri: String,
    redacted_uri: String,
    trust_invalid_certificate: bool,
    source_start_millis: Option<u64>,
    source_end_millis: Option<u64>,
}

impl StreamDescriptor {
    pub fn new(uri: impl Into<String>) -> Self {
        let uri = uri.into();
        let redacted_uri = redact_sensitive_uri(&uri);
        Self {
            uri,
            redacted_uri,
            trust_invalid_certificate: false,
            source_start_millis: None,
            source_end_millis: None,
        }
    }

    pub fn with_redacted(uri: impl Into<String>, redacted_uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            redacted_uri: redacted_uri.into(),
            trust_invalid_certificate: false,
            source_start_millis: None,
            source_end_millis: None,
        }
    }

    pub fn with_trust_invalid_certificate(mut self, trust: bool) -> Self {
        self.trust_invalid_certificate = trust;
        self
    }

    pub fn with_source_window(mut self, start_millis: u64, end_millis: u64) -> Self {
        if end_millis > start_millis {
            self.source_start_millis = Some(start_millis);
            self.source_end_millis = Some(end_millis);
        }
        self
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn redacted_uri(&self) -> &str {
        &self.redacted_uri
    }

    pub fn trust_invalid_certificate(&self) -> bool {
        self.trust_invalid_certificate
    }

    pub fn source_start_millis(&self) -> u64 {
        self.source_start_millis.unwrap_or(0)
    }

    pub fn source_end_millis(&self) -> Option<u64> {
        self.source_end_millis
    }
}

impl fmt::Debug for StreamDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StreamDescriptor")
            .field("uri", &self.redacted_uri)
            .finish()
    }
}

fn redact_sensitive_uri(uri: &str) -> String {
    let Some((base, query)) = uri.split_once('?') else {
        return uri.to_string();
    };
    let query = query
        .split('&')
        .map(|pair| {
            let Some((key, value)) = pair.split_once('=') else {
                return pair.to_string();
            };
            let lower = key.to_ascii_lowercase();
            if lower.contains("token") || lower.contains("key") {
                format!("{key}=<redacted>")
            } else {
                format!("{key}={value}")
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}?{query}")
}

#[cfg(test)]
mod tests {
    use super::StreamDescriptor;

    #[test]
    fn stream_descriptor_redacts_sensitive_query_parts() {
        let stream =
            StreamDescriptor::new("https://music.example/stream?api_key=secret&token=hidden&id=1");

        assert_eq!(
            stream.uri(),
            "https://music.example/stream?api_key=secret&token=hidden&id=1"
        );
        assert_eq!(
            stream.redacted_uri(),
            "https://music.example/stream?api_key=<redacted>&token=<redacted>&id=1"
        );
    }
}
