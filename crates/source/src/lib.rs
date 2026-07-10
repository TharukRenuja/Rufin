use async_trait::async_trait;
use domain::{
    Album, Artist, FolderId, Genre, GenreId, HomeSection, HomeSectionKind, ImageRef, MusicFolder,
    MusicFolderId, Playlist, PlaylistId, Track, TrackId,
};
use thiserror::Error;

pub mod remote_http;

pub use domain::{
    AlbumDetail, FavoriteItemId, FolderDetail, GeneratedTrackSeed, GeneratedTrackStrategy,
    GeneratedTracksRequest, GenreDetail, ImageBytes, LyricLine, Lyrics, LyricsSource, PagedRequest,
    PagedResponse, PlaybackReport, PlaybackReportKind, PlayedFilter, PlaylistDetail, PlaylistEntry,
    RandomTrackRequest, SearchResults, SourceIdentity, StreamDescriptor, StreamRequest,
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
    #[error("source request is invalid: {0}")]
    InvalidRequest(&'static str),
    #[error("source failed: {0}")]
    Other(String),
}

pub type SourceResult<T> = Result<T, SourceError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LyricsSearch {
    ServerOnly,
    ServerThenRemote,
    RemoteThenServer,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrackChange {
    pub fetch_native_ids: Vec<String>,
    pub removed_native_ids: Vec<String>,
}

impl TrackChange {
    pub fn is_empty(&self) -> bool {
        self.fetch_native_ids.is_empty() && self.removed_native_ids.is_empty()
    }

    pub fn merge(&mut self, other: Self) {
        for removed in other.removed_native_ids {
            let removed = removed.trim();
            if removed.is_empty() {
                continue;
            }
            self.fetch_native_ids
                .retain(|native_id| native_id != removed);
            if !self
                .removed_native_ids
                .iter()
                .any(|existing| existing == removed)
            {
                self.removed_native_ids.push(removed.to_string());
            }
        }
        for native_id in other.fetch_native_ids {
            let native_id = native_id.trim();
            if native_id.is_empty() {
                continue;
            }
            self.removed_native_ids
                .retain(|removed| removed != native_id);
            if !self
                .fetch_native_ids
                .iter()
                .any(|existing| existing == native_id)
            {
                self.fetch_native_ids.push(native_id.to_string());
            }
        }
    }
}

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
    async fn artists(&self, request: PagedRequest) -> SourceResult<PagedResponse<Artist>>;
    async fn album_artists(&self, request: PagedRequest) -> SourceResult<PagedResponse<Artist>>;
    async fn genres(&self, request: PagedRequest) -> SourceResult<PagedResponse<Genre>>;
    async fn genre_detail(&self, genre_id: &GenreId) -> SourceResult<GenreDetail>;
    async fn track(&self, track_id: &TrackId) -> SourceResult<Track>;
    async fn search(&self, query: &str) -> SourceResult<SearchResults>;
}

#[async_trait(?Send)]
pub trait MusicFolderProvider {
    async fn music_folders(&self) -> SourceResult<Vec<MusicFolder>>;
    async fn tracks_in_music_folder(
        &self,
        folder_id: &MusicFolderId,
        request: PagedRequest,
    ) -> SourceResult<PagedResponse<Track>>;
}

#[async_trait(?Send)]
pub trait RecentTrackProvider {
    async fn recent_tracks(&self, limit: usize) -> SourceResult<Vec<Track>>;
}

#[async_trait(?Send)]
pub trait RecentAlbumProvider {
    async fn recent_albums(&self, limit: usize) -> SourceResult<Vec<Album>>;
}

#[async_trait(?Send)]
pub trait TrackChangeFeed {
    async fn listen(
        &self,
        on_change: &mut dyn FnMut(TrackChange) -> bool,
        should_stop: &dyn Fn() -> bool,
    ) -> SourceResult<()>;

    async fn changed_tracks(&self, native_ids: &[String]) -> SourceResult<Vec<Track>>;

    fn track_id_from_native(&self, native_id: &str) -> TrackId;
}

#[async_trait(?Send)]
pub trait FolderBrowser {
    async fn folder(
        &self,
        folder_id: Option<&FolderId>,
        music_folder_id: Option<&MusicFolderId>,
    ) -> SourceResult<FolderDetail>;
}

#[async_trait(?Send)]
pub trait PlaylistReader {
    async fn playlists(&self, request: PagedRequest) -> SourceResult<PagedResponse<Playlist>>;
    async fn playlist_detail(&self, playlist_id: &PlaylistId) -> SourceResult<PlaylistDetail>;
}

#[async_trait(?Send)]
pub trait RandomTrackProvider {
    async fn random_tracks(&self, request: RandomTrackRequest) -> SourceResult<Vec<Track>>;
}

#[async_trait(?Send)]
pub trait GeneratedTrackProvider {
    async fn generated_tracks(&self, request: GeneratedTracksRequest) -> SourceResult<Vec<Track>>;
}

#[async_trait(?Send)]
pub trait StreamResolver {
    async fn resolve_stream(&self, request: &StreamRequest) -> SourceResult<StreamDescriptor>;
}

#[async_trait(?Send)]
pub trait ImageProvider {
    async fn image_bytes(&self, image_ref: &ImageRef, size: u32) -> SourceResult<ImageBytes>;
}

#[async_trait(?Send)]
pub trait FavoriteMutator {
    async fn set_favorite(&self, item_id: FavoriteItemId, favorite: bool) -> SourceResult<()>;
}

#[async_trait(?Send)]
pub trait PlaylistCreator {
    async fn create_playlist(&self, name: &str, track_ids: &[TrackId]) -> SourceResult<PlaylistId>;
}

#[async_trait(?Send)]
pub trait PlaylistRenamer {
    async fn rename_playlist(&self, playlist_id: &PlaylistId, name: &str) -> SourceResult<()>;
}

#[async_trait(?Send)]
pub trait PlaylistDeleter {
    async fn delete_playlist(&self, playlist_id: &PlaylistId) -> SourceResult<()>;
}

#[async_trait(?Send)]
pub trait PlaylistTrackAdder {
    async fn add_playlist_tracks(
        &self,
        playlist_id: &PlaylistId,
        track_ids: &[TrackId],
    ) -> SourceResult<()>;
}

#[async_trait(?Send)]
pub trait PlaylistEntryRemover {
    async fn remove_playlist_entries(
        &self,
        playlist_id: &PlaylistId,
        entry_ids: &[String],
    ) -> SourceResult<()>;
}

#[async_trait(?Send)]
pub trait PlaylistEntryMover {
    async fn move_playlist_entry(
        &self,
        playlist_id: &PlaylistId,
        entry_id: &str,
        new_index: usize,
    ) -> SourceResult<()>;
}

#[async_trait(?Send)]
pub trait LyricsProvider {
    async fn lyrics(
        &self,
        track_id: &TrackId,
        search: LyricsSearch,
    ) -> SourceResult<Option<Lyrics>>;
}

#[async_trait(?Send)]
pub trait PlaybackReporter {
    async fn report_playback(&self, report: PlaybackReport) -> SourceResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_fetch_cancels_a_pending_removal() {
        let mut change = TrackChange {
            fetch_native_ids: Vec::new(),
            removed_native_ids: vec!["track-1".to_string()],
        };
        change.merge(TrackChange {
            fetch_native_ids: vec!["track-1".to_string()],
            removed_native_ids: Vec::new(),
        });

        assert_eq!(change.fetch_native_ids, vec!["track-1"]);
        assert!(change.removed_native_ids.is_empty());
    }

    #[test]
    fn later_removal_cancels_a_pending_fetch() {
        let mut change = TrackChange {
            fetch_native_ids: vec!["track-1".to_string()],
            removed_native_ids: Vec::new(),
        };
        change.merge(TrackChange {
            fetch_native_ids: Vec::new(),
            removed_native_ids: vec!["track-1".to_string()],
        });

        assert!(change.fetch_native_ids.is_empty());
        assert_eq!(change.removed_native_ids, vec!["track-1"]);
    }
}
