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

/// owner of supported features.
///
/// `Native` means the source owns this operation and Rufin supports it. `Store`
/// means Rufin owns the feature for that source.
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

/// playlist ownership is decided at two levels.
///
/// creating a playlist is based on how the active source does it: it is either a
/// supported native source feature (playlists for Jellyfin) or app-owned
/// (playlists for local folders).
///
/// after a playlist exists, edits follow that playlist's owner, either by source
/// API or store edits.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourcePlaylistOperationSupport {
    pub native: bool,
    pub store: bool,
}

impl SourcePlaylistOperationSupport {
    pub const fn supported_for_owner(self, owner: SourceFeatureOwner) -> bool {
        match owner {
            SourceFeatureOwner::Native => self.native,
            SourceFeatureOwner::Store => self.store,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SourcePlaylistOperation {
    Rename,
    Delete,
    AddTracks,
    RemoveEntries,
    ReorderEntries,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourcePlaylistCapabilities {
    pub read_native: bool,
    pub read_store: bool,
    /// owner Rufin uses for the single create-playlist action.
    ///
    /// edits to existing playlists use the per-row operation fields below,
    /// because native and store-owned playlists can coexist for one source.
    pub create: SourceFeatureSupport,
    pub rename: SourcePlaylistOperationSupport,
    pub delete: SourcePlaylistOperationSupport,
    pub add_tracks: SourcePlaylistOperationSupport,
    pub remove_entries: SourcePlaylistOperationSupport,
    pub reorder_entries: SourcePlaylistOperationSupport,
}

impl SourcePlaylistCapabilities {
    pub const fn operation_support(
        self,
        operation: SourcePlaylistOperation,
    ) -> SourcePlaylistOperationSupport {
        match operation {
            SourcePlaylistOperation::Rename => self.rename,
            SourcePlaylistOperation::Delete => self.delete,
            SourcePlaylistOperation::AddTracks => self.add_tracks,
            SourcePlaylistOperation::RemoveEntries => self.remove_entries,
            SourcePlaylistOperation::ReorderEntries => self.reorder_entries,
        }
    }

    pub const fn operation_supported_for_owner(
        self,
        operation: SourcePlaylistOperation,
        owner: SourceFeatureOwner,
    ) -> bool {
        self.operation_support(operation).supported_for_owner(owner)
    }
}

/// contract for what configured sources can do.
///
/// a capability answers: should Rufin offer this operation for this source, and
/// who owns its management? the `owner` here can be the source (`Native`) or the
/// app itself (`Store`). this allows local libraries to support
/// favorites/playlists even though files do not carry these states, and a remote
/// source can still gain Rufin-owned features later, as smart playlists already
/// do.
///
/// this is not a list of every field a source may return. add a capability when
/// UI/controller code needs to decide whether an operation, a page, or an edit
/// is available, or when the app must choose between `Native` and `Store` for
/// the operation. plain metadata belongs on cached entities/projections instead.
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

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
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
pub struct SourceSession {
    pub server: ServerIdentity,
    pub user_id: String,
    pub username: String,
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SavedSourceSession {
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
