use async_trait::async_trait;
use domain::{
    Album, Artist, FolderId, Genre, GenreId, HomeSection, HomeSectionKind, MusicFolder,
    MusicFolderId, Playlist, PlaylistId, ServerIdentity, Track, TrackId,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use domain::{
    AlbumDetail, FavoriteItemId, FolderDetail, GenreDetail, ImageBytes, ImageKind, ImageMetadata,
    ImageRequest, LoginRequest, LyricLine, Lyrics, LyricsSource, PagedRequest, PagedResponse,
    PlaybackReport, PlaybackReportKind, PlayedFilter, PlaylistDetail, PlaylistEntry,
    ProviderSession, RandomTrackRequest, SavedProviderSession, SearchResults, StreamDescriptor,
    StreamRequest,
};

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
    async fn album_detail(&self, album_id: &domain::AlbumId) -> ProviderResult<AlbumDetail>;
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
