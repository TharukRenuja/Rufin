use async_trait::async_trait;
use rufin_core::{Album, Artist, Genre, HomeSection, Playlist, ServerIdentity, Track, TrackId};
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
    pub favorite_mutations: bool,
    pub auto_dj: bool,
    pub search: bool,
    pub image_metadata: bool,
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
            favorite_mutations: false,
            auto_dj: false,
            search: true,
            image_metadata: true,
        }
    }
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
pub struct AlbumDetail {
    pub album: Album,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LoginRequest {
    pub base_url: String,
    pub username: String,
    pub password: String,
    pub trust_invalid_cert: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderSession {
    pub server: ServerIdentity,
    pub user_id: String,
    pub username: String,
    pub access_token: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SavedProviderSession {
    pub server: ServerIdentity,
    pub user_id: String,
    pub username: String,
    pub trust_invalid_cert: bool,
    pub access_token: String,
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

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SearchResults {
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
    pub artists: Vec<Artist>,
    pub playlists: Vec<Playlist>,
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
    async fn albums(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Album>>;
    async fn album_detail(&self, album_id: &rufin_core::AlbumId) -> ProviderResult<AlbumDetail>;
    async fn tracks(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Track>>;
    async fn artists(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Artist>>;
    async fn album_artists(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Artist>>;
    async fn genres(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Genre>>;
    async fn playlists(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Playlist>>;
    async fn track(&self, track_id: &TrackId) -> ProviderResult<Track>;
    async fn stream(&self, track_id: &TrackId) -> ProviderResult<StreamDescriptor>;
    async fn search(&self, query: &str) -> ProviderResult<SearchResults>;
    async fn image_metadata(&self, item_id: &str, kind: ImageKind)
    -> ProviderResult<ImageMetadata>;
}

#[cfg(test)]
mod tests {
    use super::ProviderCapabilities;
    use rufin_core::ServerId;

    #[test]
    fn provider_capabilities_default_to_read_only_library() {
        let capabilities = ProviderCapabilities::default();

        assert!(capabilities.albums);
        assert!(capabilities.tracks);
        assert!(capabilities.favorites);
        assert!(!capabilities.lyrics);
        assert!(!capabilities.playback_reporting);
        assert!(!capabilities.playlist_mutations);
        assert!(capabilities.search);
        assert!(capabilities.image_metadata);
    }

    #[test]
    fn saved_provider_session_keeps_token_separate_from_store_models() {
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
        };

        assert_eq!(session.server.id.as_str(), "jellyfin:server:one");
        assert_eq!(session.access_token, "token");
    }
}
