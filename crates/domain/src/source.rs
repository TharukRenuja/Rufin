use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    Album, AlbumId, Artist, ArtistId, ExternalLyricsProvider, Folder, FolderId, Genre, GenreId,
    Playlist, PlaylistId, ServerIdentity, StreamQuality, Track, TrackId,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum PlayedFilter {
    #[default]
    All,
    Unplayed,
    Played,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SourceFeatureOwner {
    Native,
    Store,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum SourceFeatureSupport {
    #[default]
    Unsupported,
    Supported(SourceFeatureOwner),
}

impl SourceFeatureSupport {
    pub const fn native() -> Self {
        Self::Supported(SourceFeatureOwner::Native)
    }

    pub const fn store() -> Self {
        Self::Supported(SourceFeatureOwner::Store)
    }

    pub const fn owner(self) -> Option<SourceFeatureOwner> {
        match self {
            Self::Supported(owner) => Some(owner),
            Self::Unsupported => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourcePlaylistCapabilities {
    pub read_native: bool,
    pub read_store: bool,
    pub create: SourceFeatureSupport,
    pub mutate_native: bool,
    pub mutate_store: bool,
}

/// App-facing feature contract for one configured source.
///
/// Source capabilities describe what Rufin should offer for the active library
/// source and which owner is authoritative for each operation. They may include
/// native adapter behavior and Rufin store-owned behavior in the same source.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceCapabilities {
    pub playlists: SourcePlaylistCapabilities,
    pub smart_playlists: SourceFeatureSupport,
    pub favorites: SourceFeatureSupport,
    pub favorite_mutations: SourceFeatureSupport,
    pub music_folders: SourceFeatureSupport,
    pub folder_browsing: SourceFeatureSupport,
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
    ProviderDefault,
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
pub struct PagedResponse<T> {
    pub items: Vec<T>,
    pub total: usize,
}

impl<T> PagedResponse<T> {
    pub fn new(items: Vec<T>, total: usize) -> Self {
        Self { items, total }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FavoriteItemId {
    Album(AlbumId),
    Track(TrackId),
    Artist(ArtistId),
}

impl FavoriteItemId {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Album(id) => id.as_str(),
            Self::Track(id) => id.as_str(),
            Self::Artist(id) => id.as_str(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlaylistEntry {
    pub entry_id: String,
    pub track: Track,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AlbumDetail {
    pub album: Album,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlaylistDetail {
    pub playlist: Playlist,
    pub tracks: Vec<Track>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<PlaylistEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GenreDetail {
    pub genre: Genre,
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FolderDetail {
    pub folder: Folder,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<FolderId>,
    pub folders: Vec<Folder>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LoginRequest {
    pub base_url: String,
    pub username: String,
    pub password: String,
    pub trust_invalid_cert: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderSession {
    pub server: ServerIdentity,
    pub user_id: String,
    pub username: String,
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SavedProviderSession {
    pub server: ServerIdentity,
    pub user_id: String,
    pub username: String,
    pub trust_invalid_cert: bool,
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ImageKind {
    Primary,
    Backdrop,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImageMetadata {
    pub item_id: String,
    pub kind: ImageKind,
    pub tag: Option<String>,
    pub url: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImageRequest {
    pub item_id: String,
    pub kind: ImageKind,
    pub tag: Option<String>,
    pub size: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImageBytes {
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SearchResults {
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
    pub artists: Vec<Artist>,
    pub playlists: Vec<Playlist>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PlaybackReportKind {
    Started,
    Progress,
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
