use async_trait::async_trait;
use rufin_core::{
    Album, AlbumId, Artist, ArtistId, ExternalLyricsProvider, Folder, FolderId, Genre, GenreId,
    HomeSection, HomeSectionKind, MusicFolder, MusicFolderId, Playlist, PlaylistId, ServerIdentity,
    StreamQuality, Track, TrackId,
};
pub use rufin_playback::StreamDescriptor;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderIdentity {
    pub server: ServerIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub albums: bool,
    pub tracks: bool,
    pub artists: bool,
    pub album_artists: bool,
    pub genres: bool,
    pub playlists: bool,
    pub favorites: bool,
    pub lyrics: bool,
    pub playback_reporting: bool,
    pub playlist_mutations: bool,
    pub playlist_delete: bool,
    pub favorite_mutations: bool,
    pub auto_dj: bool,
    pub random_tracks: bool,
    pub random_played_filter: bool,
    pub search: bool,
    pub image_metadata: bool,
    pub music_folders: bool,
    pub folder_browsing: bool,
}

impl Default for ProviderCapabilities {
    fn default() -> Self {
        Self {
            albums: true,
            tracks: true,
            artists: true,
            album_artists: true,
            genres: true,
            playlists: true,
            favorites: true,
            lyrics: false,
            playback_reporting: false,
            playlist_mutations: false,
            playlist_delete: false,
            favorite_mutations: false,
            auto_dj: false,
            random_tracks: false,
            random_played_filter: false,
            search: true,
            image_metadata: true,
            music_folders: false,
            folder_browsing: false,
        }
    }
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

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider authentication failed: {0}")]
    Auth(String),
    #[error("provider TLS validation failed: {0}")]
    Tls(String),
    #[error("provider network failed: {0}")]
    Network(String),
    #[error("provider server failed with status {status}: {message}")]
    Server { status: u16, message: String },
    #[error("provider item was not found")]
    NotFound,
    #[error("provider capability is not supported: {0}")]
    Unsupported(&'static str),
    #[error("provider failed: {0}")]
    Other(String),
}

pub type ProviderResult<T> = Result<T, ProviderError>;

#[async_trait(?Send)]
pub trait MusicProvider {
    fn identity(&self) -> &ProviderIdentity;
    fn capabilities(&self) -> &ProviderCapabilities;

    async fn home_sections(&self) -> ProviderResult<Vec<HomeSection>>;
    async fn home_section(&self, kind: HomeSectionKind) -> ProviderResult<HomeSection> {
        self.home_sections()
            .await?
            .into_iter()
            .find(|section| section.kind == kind)
            .ok_or(ProviderError::NotFound)
    }
    async fn albums(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Album>>;
    async fn album_detail(&self, album_id: &rufin_core::AlbumId) -> ProviderResult<AlbumDetail>;
    async fn tracks(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Track>>;
    async fn music_folders(&self) -> ProviderResult<Vec<MusicFolder>> {
        Err(ProviderError::Unsupported("music folders"))
    }
    async fn tracks_in_music_folder(
        &self,
        folder_id: &MusicFolderId,
        request: PagedRequest,
    ) -> ProviderResult<PagedResponse<Track>> {
        let _unused = folder_id;
        self.tracks(request).await
    }
    async fn folder(
        &self,
        folder_id: Option<&FolderId>,
        music_folder_id: Option<&MusicFolderId>,
    ) -> ProviderResult<FolderDetail> {
        let _unused = (folder_id, music_folder_id);
        Err(ProviderError::Unsupported("folder browsing"))
    }
    async fn artists(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Artist>>;
    async fn album_artists(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Artist>>;
    async fn genres(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Genre>>;
    async fn playlists(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Playlist>>;
    async fn playlist_detail(&self, playlist_id: &PlaylistId) -> ProviderResult<PlaylistDetail>;
    async fn genre_detail(&self, genre_id: &GenreId) -> ProviderResult<GenreDetail>;
    async fn track(&self, track_id: &TrackId) -> ProviderResult<Track>;
    async fn random_tracks(&self, request: RandomTrackRequest) -> ProviderResult<Vec<Track>> {
        let _unused = request;
        Err(ProviderError::Unsupported("random tracks"))
    }
    async fn stream(&self, track_id: &TrackId) -> ProviderResult<StreamDescriptor>;
    async fn stream_with_request(
        &self,
        request: &StreamRequest,
    ) -> ProviderResult<StreamDescriptor> {
        self.stream(&request.track_id).await
    }
    async fn search(&self, query: &str) -> ProviderResult<SearchResults>;
    async fn image_metadata(&self, item_id: &str, kind: ImageKind)
    -> ProviderResult<ImageMetadata>;
    async fn image_bytes(&self, request: ImageRequest) -> ProviderResult<ImageBytes>;
    async fn set_favorite(&self, item_id: FavoriteItemId, favorite: bool) -> ProviderResult<()> {
        let _unused = (item_id, favorite);
        Err(ProviderError::Unsupported("favorite mutations"))
    }
    async fn create_playlist(
        &self,
        name: &str,
        track_ids: &[TrackId],
    ) -> ProviderResult<PlaylistId> {
        let _unused = (name, track_ids);
        Err(ProviderError::Unsupported("playlist mutations"))
    }
    async fn rename_playlist(&self, playlist_id: &PlaylistId, name: &str) -> ProviderResult<()> {
        let _unused = (playlist_id, name);
        Err(ProviderError::Unsupported("playlist mutations"))
    }
    async fn delete_playlist(&self, playlist_id: &PlaylistId) -> ProviderResult<()> {
        let _unused = playlist_id;
        Err(ProviderError::Unsupported("playlist deletion"))
    }
    async fn add_playlist_tracks(
        &self,
        playlist_id: &PlaylistId,
        track_ids: &[TrackId],
    ) -> ProviderResult<()> {
        let _unused = (playlist_id, track_ids);
        Err(ProviderError::Unsupported("playlist mutations"))
    }
    async fn remove_playlist_entries(
        &self,
        playlist_id: &PlaylistId,
        entry_ids: &[String],
    ) -> ProviderResult<()> {
        let _unused = (playlist_id, entry_ids);
        Err(ProviderError::Unsupported("playlist mutations"))
    }
    async fn move_playlist_entry(
        &self,
        playlist_id: &PlaylistId,
        entry_id: &str,
        new_index: usize,
    ) -> ProviderResult<()> {
        let _unused = (playlist_id, entry_id, new_index);
        Err(ProviderError::Unsupported("playlist mutations"))
    }
    async fn lyrics(
        &self,
        track_id: &TrackId,
        allow_remote: bool,
    ) -> ProviderResult<Option<Lyrics>> {
        let _unused = (track_id, allow_remote);
        Err(ProviderError::Unsupported("lyrics"))
    }
    async fn report_playback(&self, report: PlaybackReport) -> ProviderResult<()> {
        let _unused = report;
        Err(ProviderError::Unsupported("playback reporting"))
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderCapabilities;
    use rufin_core::ServerId;

    #[test]
    fn capabilities_read_library() {
        let capabilities = ProviderCapabilities::default();

        assert!(capabilities.albums);
        assert!(capabilities.tracks);
        assert!(capabilities.favorites);
        assert!(!capabilities.lyrics);
        assert!(!capabilities.playback_reporting);
        assert!(!capabilities.playlist_mutations);
        assert!(!capabilities.random_tracks);
        assert!(!capabilities.random_played_filter);
        assert!(capabilities.search);
        assert!(capabilities.image_metadata);
        assert!(!capabilities.folder_browsing);
    }

    #[test]
    fn saved_store_model() {
        let session = super::SavedProviderSession {
            server: rufin_core::ServerIdentity {
                id: ServerId::new("jellyfin:server:one"),
                provider: "jellyfin".to_string(),
                name: "Server".to_string(),
                base_url: "https://music.example".to_string(),
            },
            user_id: "user".to_string(),
            username: "name".to_string(),
            trust_invalid_cert: false,
            access_token: "token".to_string(),
            device_id: Some("rufin-install-one".to_string()),
        };

        assert_eq!(session.server.id.as_str(), "jellyfin:server:one");
        assert_eq!(session.access_token, "token");
        assert_eq!(session.device_id.as_deref(), Some("rufin-install-one"));
    }
}
