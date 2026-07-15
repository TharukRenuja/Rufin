//! Music source clients, their configuration, and their supported operations.
//!
//! `rufin::source_setup` connects clients to the app; `ui` owns their settings
//! screens.

use std::collections::BTreeSet;

use async_trait::async_trait;
use library::{
    Album, AlbumDetail, AlbumId, Artist, FavoriteItemId, FolderDetail, FolderId, Genre,
    GenreDetail, GenreId, HomeSection, HomeSectionKind, ImageRef, MusicFolder, MusicFolderId,
    PagedResponse, Playlist, PlaylistDetail, PlaylistId, SearchResults, SourceEntityKind,
    SourceObjectMapping, Track, TrackId,
};
use thiserror::Error;

mod config;
mod events;
mod operations;

pub mod jellyfin;
pub mod local;
pub mod remote_http;
pub mod subsonic;

pub use config::{
    CredentialHostInput, CredentialHostPreset, CredentialSettingsInput, CredentialSourceConfig,
    EditableSource, JellyfinSettingsInput, JellyfinSetupInput, LibrarySourceSelection,
    LibrarySourceSettings, LocalFolderHostInput, LocalLibraryFolder, SourceIdentity,
    SourceLocalAccessInput, SourceSettingsInput, SourceSetupInput,
};
pub use events::*;
pub use operations::*;

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
    #[error("saved source configuration is invalid: {0}")]
    InvalidConfig(String),
    #[error("source failed: {0}")]
    Other(String),
}

pub type SourceResult<T> = Result<T, SourceError>;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PageState {
    fetched: usize,
    total: Option<usize>,
}

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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLyrics {
    pub origin: NativeLyricsOrigin,
    pub lines: Vec<NativeLyricLine>,
}

impl PageState {
    pub fn request(&self, limit: usize) -> PagedRequest {
        PagedRequest::new(self.fetched, limit)
    }

    pub fn fetched(&self) -> usize {
        self.fetched
    }

    pub fn total(&self) -> Option<usize> {
        self.total
    }

    pub fn add(&mut self, count: usize, reported_total: Option<usize>) -> Option<bool> {
        if let Some(reported_total) = reported_total {
            if self.total.is_some_and(|total| total != reported_total) {
                return None;
            }
            self.total = Some(reported_total);
        }
        self.fetched = self.fetched.checked_add(count)?;
        match self.total {
            Some(total) if self.fetched > total => None,
            Some(total) if self.fetched == total => Some(true),
            Some(_) if count == 0 => None,
            Some(_) => Some(false),
            None => Some(count == 0),
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
    async fn album_detail(&self, album_id: &AlbumId) -> SourceResult<AlbumDetail>;
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

pub trait SourceObjectKeyProvider {
    fn source_object_key(
        &self,
        entity_kind: SourceEntityKind,
        entity_id: &str,
    ) -> SourceResult<String>;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SourceObjectChanges {
    object_ids: BTreeSet<String>,
}

impl SourceObjectChanges {
    pub fn new(object_ids: impl IntoIterator<Item = String>) -> Self {
        Self {
            object_ids: object_ids
                .into_iter()
                .filter(|object_id| !object_id.is_empty())
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.object_ids.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &String> {
        self.object_ids.iter()
    }

    pub fn merge(&mut self, other: Self) {
        self.object_ids.extend(other.object_ids);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LibraryChange {
    Objects(SourceObjectChanges),
    Full,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibraryObjectObservation {
    pub mappings: Vec<SourceObjectMapping>,
    pub missing_source_objects: BTreeSet<String>,
    pub ignored_source_objects: BTreeSet<String>,
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
    pub artists: Vec<Artist>,
    pub album_artists: Vec<Artist>,
    pub genres: Vec<Genre>,
    pub playlists: Vec<PlaylistDetail>,
    pub home_sections: Vec<HomeSection>,
    pub track_music_folders: Vec<(TrackId, Vec<MusicFolderId>)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LibraryChangeResolution {
    /// Every affected music object was identified
    Exact(Box<LibraryObjectObservation>),
    /// The change needs a complete source read
    Full,
    /// The input does not affect the music library
    Ignored,
}

#[async_trait(?Send)]
pub trait LibraryChangeResolver {
    async fn resolve_changes(
        &self,
        changes: &SourceObjectChanges,
        known: &[SourceObjectMapping],
    ) -> SourceResult<LibraryChangeResolution>;
}

#[async_trait(?Send)]
pub trait LibraryChangeFeed {
    async fn listen(
        &self,
        on_ready: &mut dyn FnMut() -> bool,
        on_change: &mut dyn FnMut(LibraryChange) -> bool,
        should_stop: &dyn Fn() -> bool,
    ) -> SourceResult<()>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LibraryProbeResult {
    Unchanged,
    Changed,
    Unknown,
    Busy,
}

#[async_trait(?Send)]
pub trait LibraryFreshnessProbe {
    async fn probe(&self) -> SourceResult<LibraryProbeResult>;
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
    ) -> SourceResult<Option<NativeLyrics>>;
}

#[async_trait(?Send)]
pub trait PlaybackReporter {
    async fn report_playback(&self, report: PlaybackReport) -> SourceResult<()>;
}
