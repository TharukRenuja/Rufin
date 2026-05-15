use async_trait::async_trait;
use rufin_core::{Album, Artist, Genre, HomeSection, Playlist, ServerIdentity, Track, TrackId};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderIdentity {
    pub server: ServerIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
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
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PagedRequest {
    pub offset: usize,
    pub limit: usize,
}

impl PagedRequest {
    pub fn new(offset: usize, limit: usize) -> Self {
        Self { offset, limit }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedResponse<T> {
    pub items: Vec<T>,
    pub total: usize,
}

impl<T> PagedResponse<T> {
    pub fn new(items: Vec<T>, total: usize) -> Self {
        Self { items, total }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlbumDetail {
    pub album: Album,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Error)]
pub enum ProviderError {
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
}

#[cfg(test)]
mod tests {
    use super::ProviderCapabilities;

    #[test]
    fn provider_capabilities_default_to_read_only_library() {
        let capabilities = ProviderCapabilities::default();

        assert!(capabilities.albums);
        assert!(capabilities.tracks);
        assert!(capabilities.favorites);
        assert!(!capabilities.lyrics);
        assert!(!capabilities.playback_reporting);
        assert!(!capabilities.playlist_mutations);
    }
}
