use std::{
    collections::{HashMap, HashSet},
    fs,
    hash::Hash,
    path::{Path, PathBuf},
};

use domain::{
    Album, AlbumId, Artist, ArtistCredit, ArtistId, Genre, GenreId, HomeSection, HomeSectionKind,
    ImageRef, LibraryField, LocalCueTrackSource, LocalFileFacts, LocalManifestCover,
    LocalManifestCoverKind, LocalManifestEntry, Lyrics, LyricsSource, Mood, MoodId, MusicFolder,
    MusicFolderId, PagedResponse, Playlist, PlaylistDetail, PlaylistEntry, PlaylistId,
    QueueEntryId, QueueSnapshot, SearchResults, ServerId, ServerIdentity, SmartPlaylist,
    SmartPlaylistBuiltin, SmartPlaylistDefinition, SmartPlaylistDetail, SmartPlaylistId,
    SmartPlaylistMatchMode, SmartPlaylistRule, SmartPlaylistRuleField, SmartPlaylistRuleGroup,
    SmartPlaylistRuleNode, SmartPlaylistRuleOperator, SmartPlaylistSortField, Track, TrackId,
    normalize_release_types,
};
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter, types::Value};
use thiserror::Error;

const SCHEMA_VERSION: i64 = 20;
pub const LOCAL_MANIFEST_VERSION: i64 = 4;
const CACHE_KEY_PART_MAX_LEN: usize = 180;
const CACHE_KEY_HASH_LEN: usize = 16;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported store schema version: {0}")]
    UnsupportedSchemaVersion(i64),
    #[error("incomplete store schema version: {0}")]
    IncompleteSchemaVersion(i64),
    #[error("invalid source object: {0}")]
    InvalidSourceObject(String),
    #[error("unsupported store-backed source window")]
    UnsupportedSourceWindow,
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedServer {
    pub server: ServerIdentity,
    pub user_id: String,
    pub username: String,
    pub trust_invalid_cert: bool,
    pub use_jellyfin_instant_mix: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerLocalAccess {
    pub server_id: ServerId,
    pub root_path: String,
    pub path_replace_from: Option<String>,
    pub path_replace_to: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalAccessStatusFacts {
    pub sample_server_path: Option<String>,
    pub sample_metadata_path: Option<String>,
    pub direct_match_count: usize,
    pub prefix_match_count: usize,
    pub metadata_match_count: usize,
    pub unmatched_count: usize,
    pub total_track_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncState {
    pub server_id: ServerId,
    pub generation: i64,
    pub status: String,
    pub last_started_at: Option<String>,
    pub last_completed_at: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoverCacheEntry {
    pub server_id: ServerId,
    pub item_id: String,
    pub image_tag: String,
    pub size: u32,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceObject {
    pub source_object_id: String,
    pub entity_kind: Option<String>,
    pub entity_id: Option<String>,
    pub source_kind: String,
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
pub struct LocalFileSourceObject {
    pub source_path: String,
    pub root_path: String,
    pub relative_path: String,
    pub sync_generation: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CueTrackSourceObject {
    pub source_object_id: String,
    pub track_id: TrackId,
    pub source_path: String,
    pub parent_source_object_id: String,
    pub cue_path: String,
    pub cue_revision: String,
    pub cue_track_index: i64,
    pub segment_start_ms: i64,
    pub segment_end_ms: i64,
    pub sync_generation: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlbumReleaseTypeLookupCandidate {
    pub album_id: AlbumId,
    pub title: String,
    pub artist: String,
    pub musicbrainz_album_id: Option<String>,
    pub musicbrainz_release_group_id: Option<String>,
    pub lookup_key: String,
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
        self.home_changed |= other.home_changed;
        self.folders_changed |= other.folders_changed;
        self.local_matches_changed |= other.local_matches_changed;
        if self.reset.is_none() {
            self.reset = other.reset;
        }
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

pub struct Store {
    connection: Connection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreBackedSourceItem {
    pub track: Track,
    pub source_index: usize,
    pub source_item_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreBackedSourceWindow {
    pub start_rank: usize,
    pub total_source_items: usize,
    pub items: Vec<StoreBackedSourceItem>,
}

mod identity;
mod library_auxiliary_cache;
mod library_cache_reads;
mod library_cache_writes;
mod library_counts;
mod library_metadata;
mod library_search_helpers;
mod library_track_sort;
mod local_manifest;
mod servers;
mod smart_playlists;
mod source_windows;
mod store_lifecycle_schema;

pub use identity::local_file_source_object_id;
pub use local_manifest::LocalLibraryDelta;
pub use servers::{image_cache_key, lyrics_cache_key};

#[cfg(test)]
mod library_relationship_tests;
#[cfg(test)]
mod schema_cache_tests;
#[cfg(test)]
mod sync_search_cover_tests;
#[cfg(test)]
mod test_support;
