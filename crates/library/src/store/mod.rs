use std::{
    collections::{HashMap, HashSet},
    fs,
    hash::Hash,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use crate::{
    Album, AlbumArtwork, AlbumId, Artist, ArtistCredit, ArtistId, FavoriteItemId, Genre, GenreId,
    HomeSection, HomeSectionKind, ImageRef, LocalCueDependency, LocalCueTrackSource,
    LocalFileFacts, LocalManifestCover, LocalManifestCoverKind, LocalManifestEntry, Mood, MoodId,
    MusicFolder, MusicFolderId, PagedResponse, Playlist, PlaylistDetail, PlaylistEntry,
    PlaylistEntryKey, PlaylistId, PlaylistSnapshot, RandomTrackQuery, SearchResults, SmartPlaylist,
    SmartPlaylistBuiltin, SmartPlaylistDefinition, SmartPlaylistDetail, SmartPlaylistId,
    SmartPlaylistMatchMode, SmartPlaylistRule, SmartPlaylistRuleField, SmartPlaylistRuleGroup,
    SmartPlaylistRuleNode, SmartPlaylistRuleOperator, SmartPlaylistSortField, SourceEntityKind,
    SourceFeatureOwner, SourceId, SourceObjectMapping, Track, TrackId, TrackSort,
    normalize_release_types,
};
use rusqlite::{
    Connection, ErrorCode, OptionalExtension, Row, params, params_from_iter, types::Value,
};
use thiserror::Error;

const SCHEMA_VERSION: i64 = 29;
pub const LOCAL_MANIFEST_VERSION: i64 = 4;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("store lock was poisoned")]
    Unavailable,
    #[error("unsupported store schema version: {0}")]
    UnsupportedSchemaVersion(i64),
    #[error("incomplete store schema version: {0}")]
    IncompleteSchemaVersion(i64),
    #[error("invalid source object: {0}")]
    InvalidSourceObject(String),
    #[error("unsupported play context")]
    UnsupportedPlayContext,
    #[error("folder play context is source-owned and cannot be materialized from Store")]
    UnsupportedFolderPlayContext,
    #[error("the selected play-context anchor is no longer available")]
    PlayContextAnchorNotFound,
    #[error("the smart-playlist definition changed before play-context materialization")]
    StaleSmartPlaylistDefinition,
    #[error("invalid playlist owner: {0}")]
    InvalidPlaylistOwner(String),
    #[error("invalid favorite item kind: {0}")]
    InvalidFavoriteItemKind(String),
    #[error("invalid sync batch: {0}")]
    InvalidSyncBatch(String),
    #[error("library changes require a full sync")]
    NeedsFullSync,
    #[error("stale sync generation {generation} for {source_id}; current generation is {current}")]
    StaleSyncGeneration {
        source_id: String,
        generation: i64,
        current: i64,
    },
    #[error("stale cache revision {revision} for {source_id}; current revision is {current}")]
    StaleCacheRevision {
        source_id: String,
        revision: i64,
        current: i64,
    },
}

impl StoreError {
    pub fn is_contention(&self) -> bool {
        matches!(
            self,
            Self::Sqlite(error)
                if matches!(
                    error.sqlite_error_code(),
                    Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
                )
        )
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

fn playlist_owner_from_str(value: &str) -> StoreResult<SourceFeatureOwner> {
    match value {
        "native" => Ok(SourceFeatureOwner::Native),
        "store" => Ok(SourceFeatureOwner::Store),
        other => Err(StoreError::InvalidPlaylistOwner(other.to_string())),
    }
}

fn playlist_owner_to_str(owner: SourceFeatureOwner) -> &'static str {
    match owner {
        SourceFeatureOwner::Native => "native",
        SourceFeatureOwner::Store => "store",
    }
}

fn favorite_item_kind_to_table(kind: &str) -> StoreResult<(&'static str, &'static str)> {
    match kind {
        "album" => Ok(("albums", "album_id")),
        "track" => Ok(("tracks", "track_id")),
        "artist" => Ok(("artists", "artist_id")),
        "album_artist" => Ok(("album_artists", "artist_id")),
        other => Err(StoreError::InvalidFavoriteItemKind(other.to_string())),
    }
}

pub(super) fn effective_album_favorite_sql(alias: &str) -> String {
    effective_item_favorite_sql(alias, "album", "album_id")
}

pub(super) fn effective_track_favorite_sql(alias: &str) -> String {
    effective_item_favorite_sql(alias, "track", "track_id")
}

pub(super) fn effective_artist_favorite_sql(alias: &str, album_artist: bool) -> String {
    let kind = if album_artist {
        "album_artist"
    } else {
        "artist"
    };
    effective_item_favorite_sql(alias, kind, "artist_id")
}

fn effective_item_favorite_sql(alias: &str, kind: &str, id_column: &str) -> String {
    format!(
        "COALESCE((SELECT o.favorite FROM item_favorite_overrides o \
         WHERE o.source_id = {alias}.source_id \
           AND o.item_kind = '{kind}' \
           AND o.item_id = {alias}.{id_column}), {alias}.favorite)"
    )
}

const STORE_OWNED_PLAYLIST_SYNC_GENERATION: i64 = -1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaylistWriteMode {
    NativeSync { generation: i64 },
    StoreOwned,
}

impl PlaylistWriteMode {
    const fn owner(self) -> SourceFeatureOwner {
        match self {
            Self::NativeSync { .. } => SourceFeatureOwner::Native,
            Self::StoreOwned => SourceFeatureOwner::Store,
        }
    }

    const fn sync_generation(self) -> i64 {
        match self {
            Self::NativeSync { generation } => generation,
            Self::StoreOwned => STORE_OWNED_PLAYLIST_SYNC_GENERATION,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredSource {
    pub source_id: SourceId,
    pub kind: String,
    pub name: String,
    pub provider_payload: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLocalAccess {
    pub source_id: SourceId,
    pub root_path: String,
    pub path_replace_from: Option<String>,
    pub path_replace_to: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalAccessStatusFacts {
    pub sample_source_path: Option<String>,
    pub sample_metadata_path: Option<String>,
    pub direct_match_count: usize,
    pub prefix_match_count: usize,
    pub metadata_match_count: usize,
    pub unmatched_count: usize,
    pub total_track_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncState {
    pub source_id: SourceId,
    pub generation: i64,
    pub cache_revision: i64,
    pub sync_input_revision: i64,
    pub status: String,
    pub last_started_at: Option<String>,
    pub last_completed_at: Option<String>,
    pub last_all_completed_at: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceObject {
    pub source_object_id: String,
    pub entity_kind: Option<String>,
    pub entity_id: Option<String>,
    pub source_object_kind: String,
    pub source_path: Option<String>,
    pub parent_source_object_id: Option<String>,
    pub cue_path: Option<String>,
    pub cue_revision: Option<String>,
    pub cue_track_index: Option<i64>,
    pub segment_start_ms: Option<i64>,
    pub segment_end_ms: Option<i64>,
    pub sync_generation: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlbumIdentityCandidate {
    pub album_id: AlbumId,
    pub title: String,
    pub artist: String,
    pub musicbrainz_album_id: Option<String>,
    pub musicbrainz_release_group_id: Option<String>,
    pub identity_key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EntityDelta<Id> {
    pub added: Vec<Id>,
    pub deleted: Vec<Id>,
    pub fields: Vec<Id>,
    pub stats: Vec<Id>,
    pub links: Vec<Id>,
    pub cover_refs: Vec<Id>,
}

impl<Id> Default for EntityDelta<Id> {
    fn default() -> Self {
        Self {
            added: Vec::new(),
            deleted: Vec::new(),
            fields: Vec::new(),
            stats: Vec::new(),
            links: Vec::new(),
            cover_refs: Vec::new(),
        }
    }
}

impl<Id> EntityDelta<Id>
where
    Id: Clone + Eq + Hash,
{
    fn merge(&mut self, other: Self) {
        merge_ids(&mut self.added, other.added);
        merge_ids(&mut self.deleted, other.deleted);
        merge_ids(&mut self.fields, other.fields);
        merge_ids(&mut self.stats, other.stats);
        merge_ids(&mut self.links, other.links);
        merge_ids(&mut self.cover_refs, other.cover_refs);
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.deleted.is_empty()
            && self.fields.is_empty()
            && self.stats.is_empty()
            && self.links.is_empty()
            && self.cover_refs.is_empty()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrackDelta {
    pub added: Vec<TrackId>,
    pub deleted: Vec<TrackId>,
    pub fields: Vec<TrackId>,
    pub metadata: Vec<TrackId>,
    pub stats: Vec<TrackId>,
    pub skip_stats: Vec<TrackId>,
    pub favorite: Vec<TrackId>,
    pub cover_refs: Vec<TrackId>,
}

impl TrackDelta {
    fn merge(&mut self, other: Self) {
        merge_ids(&mut self.added, other.added);
        merge_ids(&mut self.deleted, other.deleted);
        merge_ids(&mut self.fields, other.fields);
        merge_ids(&mut self.metadata, other.metadata);
        merge_ids(&mut self.stats, other.stats);
        merge_ids(&mut self.skip_stats, other.skip_stats);
        merge_ids(&mut self.favorite, other.favorite);
        merge_ids(&mut self.cover_refs, other.cover_refs);
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.deleted.is_empty()
            && self.fields.is_empty()
            && self.metadata.is_empty()
            && self.stats.is_empty()
            && self.skip_stats.is_empty()
            && self.favorite.is_empty()
            && self.cover_refs.is_empty()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlaylistDelta {
    pub added: Vec<PlaylistId>,
    pub deleted: Vec<PlaylistId>,
    pub fields: Vec<PlaylistId>,
    pub entries: Vec<PlaylistId>,
    pub cover_refs: Vec<PlaylistId>,
}

impl PlaylistDelta {
    fn merge(&mut self, other: Self) {
        merge_ids(&mut self.added, other.added);
        merge_ids(&mut self.deleted, other.deleted);
        merge_ids(&mut self.fields, other.fields);
        merge_ids(&mut self.entries, other.entries);
        merge_ids(&mut self.cover_refs, other.cover_refs);
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.deleted.is_empty()
            && self.fields.is_empty()
            && self.entries.is_empty()
            && self.cover_refs.is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LibraryReset {
    Source,
    Scope,
    Schema,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibraryDelta {
    pub tracks: TrackDelta,
    pub albums: EntityDelta<AlbumId>,
    pub artists: EntityDelta<ArtistId>,
    pub album_artists: EntityDelta<ArtistId>,
    pub genres: EntityDelta<GenreId>,
    pub playlists: PlaylistDelta,
    pub smart_playlists: EntityDelta<SmartPlaylistId>,
    pub home_changed: bool,
    pub folders_changed: bool,
    pub local_matches_changed: bool,
    pub reset: Option<LibraryReset>,
}

impl LibraryDelta {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
            && self.albums.is_empty()
            && self.artists.is_empty()
            && self.album_artists.is_empty()
            && self.genres.is_empty()
            && self.playlists.is_empty()
            && self.smart_playlists.is_empty()
            && !self.home_changed
            && !self.folders_changed
            && !self.local_matches_changed
            && self.reset.is_none()
    }

    pub fn merge(&mut self, other: Self) {
        self.tracks.merge(other.tracks);
        self.albums.merge(other.albums);
        self.artists.merge(other.artists);
        self.album_artists.merge(other.album_artists);
        self.genres.merge(other.genres);
        self.playlists.merge(other.playlists);
        self.smart_playlists.merge(other.smart_playlists);
        self.home_changed |= other.home_changed;
        self.folders_changed |= other.folders_changed;
        self.local_matches_changed |= other.local_matches_changed;
        if self.reset.is_none() {
            self.reset = other.reset;
        }
    }

    pub fn playlist_changed(playlist_id: PlaylistId) -> Self {
        Self {
            playlists: PlaylistDelta {
                fields: vec![playlist_id.clone()],
                entries: vec![playlist_id],
                ..PlaylistDelta::default()
            },
            ..Self::default()
        }
    }

    pub fn smart_playlist_changed(smart_playlist_id: SmartPlaylistId) -> Self {
        Self {
            smart_playlists: EntityDelta {
                fields: vec![smart_playlist_id],
                ..EntityDelta::default()
            },
            ..Self::default()
        }
    }

    pub fn favorite_changed(item_id: &FavoriteItemId) -> Self {
        let mut delta = Self::default();
        match item_id {
            FavoriteItemId::Album(album_id) => delta.albums.fields.push(album_id.clone()),
            FavoriteItemId::Track(track_id) => delta.tracks.favorite.push(track_id.clone()),
            FavoriteItemId::Artist(artist_id) => {
                delta.artists.fields.push(artist_id.clone());
                delta.album_artists.fields.push(artist_id.clone());
            }
        }
        delta
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibraryDeltaCollector {
    delta: LibraryDelta,
}

impl LibraryDeltaCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn merge(&mut self, delta: LibraryDelta) {
        self.delta.merge(delta);
    }

    pub fn finish(self) -> LibraryDelta {
        self.delta
    }
}

fn merge_ids<Id>(target: &mut Vec<Id>, incoming: Vec<Id>)
where
    Id: Clone + Eq + Hash,
{
    let mut seen = target.iter().cloned().collect::<HashSet<_>>();
    for id in incoming {
        if seen.insert(id.clone()) {
            target.push(id);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedArtistDetail {
    pub artist: Artist,
    pub albums: Vec<Album>,
    pub appears_on: Vec<Album>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedGenreDetail {
    pub genre: Genre,
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedMoodDetail {
    pub mood: Mood,
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Default)]
pub struct StoreWriteGate(Arc<Mutex<()>>);

/// Cloneable access to one library database.
///
/// Disk-backed access reopens the SQLite connection for each operation while
/// sharing the process write gate. Memory-backed access keeps the one in-memory
/// Store behind a lock so application tests observe the same database.
#[derive(Clone)]
pub struct StoreAccess {
    backend: StoreAccessBackend,
}

#[derive(Clone)]
enum StoreAccessBackend {
    Disk {
        path: PathBuf,
        write_gate: StoreWriteGate,
    },
    Shared(Arc<Mutex<Store>>),
}

impl StoreAccess {
    pub fn from_path(path: impl Into<PathBuf>, write_gate: StoreWriteGate) -> Self {
        Self {
            backend: StoreAccessBackend::Disk {
                path: path.into(),
                write_gate,
            },
        }
    }

    pub fn from_shared(store: Arc<Mutex<Store>>) -> Self {
        Self {
            backend: StoreAccessBackend::Shared(store),
        }
    }

    pub fn open_memory() -> StoreResult<Self> {
        Store::open_memory().map(|store| Self::from_shared(Arc::new(Mutex::new(store))))
    }

    pub fn with_store<T>(
        &self,
        operation: impl FnOnce(&Store) -> StoreResult<T>,
    ) -> StoreResult<T> {
        match &self.backend {
            StoreAccessBackend::Disk { path, write_gate } => {
                let store = Store::open_with_write_gate(path, write_gate.clone())?;
                operation(&store)
            }
            StoreAccessBackend::Shared(store) => {
                let store = store.lock().map_err(|_| StoreError::Unavailable)?;
                operation(&store)
            }
        }
    }

    pub fn with_fast_read<T>(
        &self,
        operation: impl FnOnce(&Store) -> StoreResult<T>,
    ) -> StoreResult<T> {
        match &self.backend {
            StoreAccessBackend::Disk { path, .. } => {
                let store = Store::open_fast_read(path)?;
                operation(&store)
            }
            StoreAccessBackend::Shared(store) => {
                let store = store.lock().map_err(|_| StoreError::Unavailable)?;
                operation(&store)
            }
        }
    }
}

pub struct Store {
    connection: Connection,
    write_gate: StoreWriteGate,
}

mod activity;
mod artwork_projection;
mod home_projection;
mod identity;
mod library_auxiliary_cache;
mod library_cache_reads;
mod library_cache_writes;
mod library_counts;
mod library_metadata;
mod library_search_helpers;
mod library_track_sort;
mod local_manifest;
mod play_context;
mod playback_checkpoint;
mod smart_playlists;
mod sources;
mod store_lifecycle_schema;
mod sync;

pub use activity::{ActivityOutcome, LEGACY_ACTIVITY_PERIOD, TrackActivitySummary};
pub use identity::local_file_source_object_id;
pub use local_manifest::{LocalLibraryDelta, LocalManifestDelta};
pub use playback_checkpoint::PlaybackCheckpointRecord;
pub use sync::{
    HomeSectionCommit, LibrarySync, LocalAccessUpdate, MusicFolderSnapshot, SyncCommit,
    SyncCoverage, TrackFolderMembership,
};

#[cfg(test)]
mod library_relationship_tests;
#[cfg(test)]
mod schema_cache_tests;
#[cfg(test)]
mod sync_search_cover_tests;
#[cfg(test)]
mod test_support;
