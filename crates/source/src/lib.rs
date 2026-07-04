use async_trait::async_trait;
use domain::{
    Album, Artist, FolderId, Genre, GenreId, HomeSection, HomeSectionKind, MusicFolder,
    MusicFolderId, Playlist, PlaylistId, Track, TrackId,
};
use thiserror::Error;

pub mod remote_http;

pub use domain::{
    AlbumDetail, FavoriteItemId, FolderDetail, GeneratedTrackSeed, GeneratedTrackStrategy,
    GeneratedTracksRequest, GenreDetail, ImageBytes, ImageKind, ImageMetadata, ImageRequest,
    LoginRequest, LyricLine, Lyrics, LyricsSource, PagedRequest, PagedResponse, PlaybackReport,
    PlaybackReportKind, PlayedFilter, PlaylistDetail, PlaylistEntry, RandomTrackRequest,
    SavedSourceSession, SearchResults, SourceIdentity, SourceSession, StreamDescriptor,
    StreamRequest,
};

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("source authentication failed: {0}")]
    Auth(String),
    #[error("source TLS validation failed: {0}")]
    Tls(String),
    #[error("source network failed: {0}")]
    Network(String),
    #[error("source server failed with status {status}: {message}")]
    Server { status: u16, message: String },
    #[error("source item was not found")]
    NotFound,
    #[error("source capability is not supported: {0}")]
    Unsupported(&'static str),
    #[error("source failed: {0}")]
    Other(String),
}

pub type SourceResult<T> = Result<T, SourceError>;

#[async_trait(?Send)]
pub trait MusicSource {
    fn identity(&self) -> &SourceIdentity;

    async fn home_sections(&self) -> SourceResult<Vec<HomeSection>>;
    async fn home_section(&self, kind: HomeSectionKind) -> SourceResult<HomeSection> {
        self.home_sections()
            .await?
            .into_iter()
            .find(|section| section.kind == kind)
            .ok_or(SourceError::NotFound)
    }
    async fn albums(&self, request: PagedRequest) -> SourceResult<PagedResponse<Album>>;
    async fn album_detail(&self, album_id: &domain::AlbumId) -> SourceResult<AlbumDetail>;
    async fn tracks(&self, request: PagedRequest) -> SourceResult<PagedResponse<Track>>;
    async fn music_folders(&self) -> SourceResult<Vec<MusicFolder>> {
        Err(SourceError::Unsupported("music folders"))
    }
    async fn tracks_in_music_folder(
        &self,
        folder_id: &MusicFolderId,
        request: PagedRequest,
    ) -> SourceResult<PagedResponse<Track>> {
        let _unused = folder_id;
        self.tracks(request).await
    }
    async fn folder(
        &self,
        folder_id: Option<&FolderId>,
        music_folder_id: Option<&MusicFolderId>,
    ) -> SourceResult<FolderDetail> {
        let _unused = (folder_id, music_folder_id);
        Err(SourceError::Unsupported("folder browsing"))
    }
    async fn artists(&self, request: PagedRequest) -> SourceResult<PagedResponse<Artist>>;
    async fn album_artists(&self, request: PagedRequest) -> SourceResult<PagedResponse<Artist>>;
    async fn genres(&self, request: PagedRequest) -> SourceResult<PagedResponse<Genre>>;
    async fn playlists(&self, request: PagedRequest) -> SourceResult<PagedResponse<Playlist>>;
    async fn playlist_detail(&self, playlist_id: &PlaylistId) -> SourceResult<PlaylistDetail>;
    async fn genre_detail(&self, genre_id: &GenreId) -> SourceResult<GenreDetail>;
    async fn track(&self, track_id: &TrackId) -> SourceResult<Track>;
    async fn random_tracks(&self, request: RandomTrackRequest) -> SourceResult<Vec<Track>> {
        let _unused = request;
        Err(SourceError::Unsupported("random tracks"))
    }
    async fn generated_tracks(&self, request: GeneratedTracksRequest) -> SourceResult<Vec<Track>> {
        let _unused = request;
        Err(SourceError::Unsupported("generated tracks"))
    }
    async fn stream(&self, track_id: &TrackId) -> SourceResult<StreamDescriptor>;
    async fn stream_with_request(&self, request: &StreamRequest) -> SourceResult<StreamDescriptor> {
        self.stream(&request.track_id).await
    }
    async fn search(&self, query: &str) -> SourceResult<SearchResults>;
    async fn image_metadata(&self, item_id: &str, kind: ImageKind) -> SourceResult<ImageMetadata>;
    async fn image_bytes(&self, request: ImageRequest) -> SourceResult<ImageBytes>;
    async fn set_favorite(&self, item_id: FavoriteItemId, favorite: bool) -> SourceResult<()> {
        let _unused = (item_id, favorite);
        Err(SourceError::Unsupported("favorite mutations"))
    }
    async fn create_playlist(&self, name: &str, track_ids: &[TrackId]) -> SourceResult<PlaylistId> {
        let _unused = (name, track_ids);
        Err(SourceError::Unsupported("playlist mutations"))
    }
    async fn rename_playlist(&self, playlist_id: &PlaylistId, name: &str) -> SourceResult<()> {
        let _unused = (playlist_id, name);
        Err(SourceError::Unsupported("playlist mutations"))
    }
    async fn delete_playlist(&self, playlist_id: &PlaylistId) -> SourceResult<()> {
        let _unused = playlist_id;
        Err(SourceError::Unsupported("playlist deletion"))
    }
    async fn add_playlist_tracks(
        &self,
        playlist_id: &PlaylistId,
        track_ids: &[TrackId],
    ) -> SourceResult<()> {
        let _unused = (playlist_id, track_ids);
        Err(SourceError::Unsupported("playlist mutations"))
    }
    async fn remove_playlist_entries(
        &self,
        playlist_id: &PlaylistId,
        entry_ids: &[String],
    ) -> SourceResult<()> {
        let _unused = (playlist_id, entry_ids);
        Err(SourceError::Unsupported("playlist mutations"))
    }
    async fn move_playlist_entry(
        &self,
        playlist_id: &PlaylistId,
        entry_id: &str,
        new_index: usize,
    ) -> SourceResult<()> {
        let _unused = (playlist_id, entry_id, new_index);
        Err(SourceError::Unsupported("playlist mutations"))
    }
    async fn lyrics(&self, track_id: &TrackId, allow_remote: bool) -> SourceResult<Option<Lyrics>> {
        let _unused = (track_id, allow_remote);
        Err(SourceError::Unsupported("lyrics"))
    }
    async fn report_playback(&self, report: PlaybackReport) -> SourceResult<()> {
        let _unused = report;
        Err(SourceError::Unsupported("playback reporting"))
    }
}
