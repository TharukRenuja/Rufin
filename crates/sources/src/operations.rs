use std::fmt;

use library::{AlbumId, ArtistId, GenreId, PlaylistId, TrackId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum PlayedFilter {
    #[default]
    All,
    Unplayed,
    Played,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SourcePlaylistOperation {
    Rename,
    Delete,
    AddTracks,
    RemoveEntries,
    ReorderEntries,
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum GeneratedTrackStrategy {
    #[default]
    SourceDefault,
    SimilarFirst,
    MixOnly,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GeneratedTrackSeed {
    Track(TrackId),
    Album(AlbumId),
    Artist(ArtistId),
    Genre {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<GenreId>,
        name: String,
    },
    Playlist(PlaylistId),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum GeneratedTrackSeedKind {
    Track,
    Album,
    Artist,
    Genre,
    Playlist,
}

impl GeneratedTrackSeed {
    pub const fn kind(&self) -> GeneratedTrackSeedKind {
        match self {
            Self::Track(_) => GeneratedTrackSeedKind::Track,
            Self::Album(_) => GeneratedTrackSeedKind::Album,
            Self::Artist(_) => GeneratedTrackSeedKind::Artist,
            Self::Genre { .. } => GeneratedTrackSeedKind::Genre,
            Self::Playlist(_) => GeneratedTrackSeedKind::Playlist,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GeneratedTracksRequest {
    pub seed: GeneratedTrackSeed,
    pub limit: usize,
    #[serde(default)]
    pub strategy: GeneratedTrackStrategy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PagedRequest {
    pub offset: usize,
    pub limit: usize,
}

impl PagedRequest {
    pub fn new(offset: usize, limit: usize) -> Self {
        Self { offset, limit }
    }
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
            source_start_millis: None,
            source_end_millis: None,
        }
    }

    pub fn with_redacted(uri: impl Into<String>, redacted_uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            redacted_uri: redacted_uri.into(),
            source_start_millis: None,
            source_end_millis: None,
        }
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
