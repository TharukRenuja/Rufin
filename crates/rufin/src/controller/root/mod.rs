use super::covers;
pub use super::discovery::{DiscoveredServer, ServerDiscoveryStatus};
pub use super::random::{RandomPlayAction, RandomPlayRequest};
use crate::external_scrobbling::{self, ExternalScrobbleState};
#[cfg(test)]
pub(in crate::controller) use crate::sources::local_configured_source as local_source_saved;
use crate::sources::{
    ActiveSource, ActiveSourceSlot, OperationOwner, current_active_source,
    map_server_path_to_local, selected_active_source,
};
use crate::{cover_art_policy, external_metadata};
use directories::ProjectDirs;
#[cfg(test)]
use domain::ThemePreference;
use domain::{
    Album, AlbumId, AppSettings, Artist, ArtistId, ArtistTrackScope, AutoDjReason,
    ExternalLyricsProvider, FolderPathItem, GeneratedTrackSeed, Genre, GenreId, HomeSection,
    HomeSectionKind, ImageRef, LibraryField, LibraryListSettings, LibrarySourceSelection,
    LocalLibraryFolder, Mood, MoodId, MusicFolder, MusicFolderId, PlaySourceDescriptor,
    PlaySourceKey, PlaybackSettings, Playlist, PlaylistDetail, PlaylistEntrySortDescriptor,
    PlaylistId, QueueEngine, QueueEntry, QueueEntryId, QueueInsertion, QueueInsertionSource,
    QueueItemInput, QueueReplacement, QueueSnapshot, RepeatMode, SearchKind, SecretStorageMode,
    SmartPlaylist, SmartPlaylistBuiltin, SmartPlaylistDefinition, SmartPlaylistDetail,
    SmartPlaylistId, SmartPlaylistSortDescriptor, SourceFeatureOwner, SourceId, SourceIdentity,
    SourceOrder, SourcePlaylistOperation, StreamDescriptor, StreamQuality, Track, TrackId,
    TrackSortDescriptor, TrackSortKey, TrackTableSettings,
};
use library::{
    CachedArtistDetail, CachedGenreDetail, CachedMoodDetail, CoverCacheEntry, EntityDelta,
    LibraryDelta, PlaylistWriteMode, SavedSource, SourceLocalAccess, Store, StoreBackedSourceItem,
    StoreBackedSourceWindow, StoreError, StoreResult, SyncCommit,
};
#[cfg(test)]
use library::{SourceEntityKind, SourceObjectMapping};
use playback::{
    LazyGStreamerPlaybackBackend, PlaybackBackend, PlaybackCommand, PlaybackEvent, PlaybackState,
    PlaybackTrack, PreparedPlaybackItem, generate_waveform_peaks_cancellable,
};
#[cfg(test)]
use secrets::MemorySecretStore;
#[cfg(unix)]
use secrets::SecretServiceStore;
#[cfg(not(unix))]
use secrets::UnavailableSecretStore;
use secrets::{
    CachedSecretStore, ConfigSecretStore, SecretKey, SecretStore, SwitchableSecretStore,
};
use serde::{Deserialize, Serialize};
use source::{
    FavoriteItemId, FolderDetail, Lyrics, LyricsSearch, PlaybackReport, PlaybackReportKind,
    PlaylistEntry, SearchResults, StreamRequest,
};
#[cfg(test)]
use source::{LyricLine, LyricsSource, PlayedFilter};
use source_local::{LOCAL_SOURCE_ID, LocalSource};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::Hash;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
#[cfg(test)]
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;
use tracing::{debug, info, warn};

mod auto_dj;
pub(crate) use auto_dj::{cached_auto_dj_operation, native_auto_dj_operation};
mod auto_dj_commands;
mod cached_library_api;
mod cached_reads;
mod controller_bootstrap;
mod controller_settings;
mod controller_startup;
mod folder_search_commands;
mod library_mutations;
mod local_source_commands;
mod lyrics_commands;
pub(in crate::controller) mod play_activation;
mod playback_activity;
mod playback_advance;
mod playback_commands;
mod playback_queue;
mod playback_reporting;
mod playback_runtime;
mod playback_waveforms;
mod playlist_commands;
mod queue_commands;
mod queue_mutation;
mod queue_state;
mod refresh_commands;
mod settings_controller;
mod source_cache_commands;
mod source_image_policy;
mod source_lifecycle_commands;
pub(in crate::controller) use source_lifecycle_commands::{
    SourcePersistenceSnapshot, save_source_settings,
};
mod source_local_access_commands;
mod source_presentation;
mod source_refs;
mod source_selection;
mod source_sync;
mod sync_requests;

#[cfg(test)]
mod cover_playback_tests;
#[cfg(test)]
mod local_radio_tests;
#[cfg(test)]
mod lyrics_local_access_tests;
#[cfg(test)]
mod startup_sync_tests;
#[cfg(test)]
mod test_support;

pub(crate) use cached_reads::*;
pub(crate) use controller_startup::*;
pub(crate) use play_activation::{
    FULL_LOADED_LIMIT, LoadedCompleteness, MATERIALIZED_WINDOW_BEFORE_ANCHOR,
    MATERIALIZED_WINDOW_LIMIT, NormalizedPlayTarget, PlayActivation, PlayAnchor, PlaySourceItem,
    PlayTarget, normalize_loaded_source_activation,
};
use playback_activity::PlaybackActivityState;
pub(in crate::controller) use playback_queue::*;
pub(in crate::controller) use playback_waveforms::{
    cached_waveform_peaks, request_waveform_for_prepared_item, set_waveform_cache_key,
    waveform_cache_key, waveform_cache_key_for_queue,
};
pub(in crate::controller) use queue_state::{defer_queue_snapshot, sync_queue_snapshot};
pub(in crate::controller) use source_image_policy::is_local_source_image_ref;
use source_image_policy::{scrub_home_refs, scrub_source_image_ref, source_image_ref_allowed};
pub(in crate::controller) use source_image_policy::{
    scrub_selected_album_image_refs, scrub_selected_artist_image_refs,
    scrub_selected_genre_image_refs, scrub_selected_mood_image_refs,
    scrub_selected_playlist_image_refs, scrub_selected_track_image_refs, scrub_smart_refs,
};
use source_presentation::{load_runtime_snapshot, load_snapshot};
pub(in crate::controller) use source_refs::track_album_refs_with_settings;
use source_refs::{
    album_track_refs, album_track_refs_from_store, home_image_refs, home_image_refs_from_store,
    home_local_refs_from_store, queue_track_refs, track_album_refs, track_album_refs_from_store,
};
pub(crate) use source_refs::{grouped_cover_refs_for_items, track_cover_refs_for_items};
pub(in crate::controller) use sync_requests::*;
#[cfg(test)]
pub(in crate::controller) use test_support::*;

const PAGE_SIZE: usize = 500;
const SNAPSHOT_GRID_LIMIT: usize = 500;
pub(in crate::controller) const SNAPSHOT_TRACK_LIMIT: usize = 40_000;
pub(in crate::controller) const IMAGE_TAG_UNTAGGED: &str = "untagged";
const AUTO_DJ_ITEM_COUNT: usize = 5;
const AUTO_DJ_PROVIDER_CANDIDATE_LIMIT: usize = AUTO_DJ_ITEM_COUNT * 4;
const AUTO_DJ_HISTORY_LIMIT: usize = AUTO_DJ_ITEM_COUNT * 2;
const CACHE_DATABASE_FILE_NAME: &str = "rufin-cache.sqlite";
const SETTINGS_FILE_NAME: &str = "settings.json";
const CONFIG_SECRETS_FILE_NAME: &str = "secrets.json";
const STORE_DIR_NAME: &str = "store";
const COVER_CACHE_DIR_NAME: &str = "covers";
const LYRICS_CACHE_DIR_NAME: &str = "lyrics";
const PLAYBACK_CACHE_DIR_NAME: &str = "playback";
const WAVEFORM_CACHE_DIR_NAME: &str = "waveforms";
const TMP_CACHE_DIR_NAME: &str = "tmp";
pub(crate) const LOCAL_SOURCE_IDENTITY_ID: &str = "local:server:library";
#[derive(Clone, Debug)]
pub struct LibrarySnapshot {
    pub source: Option<SourceIdentity>,
    pub sources: Vec<SourceIdentity>,
    pub selected_source: Option<LibrarySourceSelection>,
    pub local_folders: Vec<LocalLibraryFolder>,
    pub source_local_access: Vec<SourceLocalAccessSnapshot>,
    pub local_access: Option<SourceLocalAccess>,
    pub local_access_status: LocalAccessStatus,
    pub music_folders: Vec<MusicFolder>,
    pub selected_music_folder_id: Option<MusicFolderId>,
    pub first_run: bool,
    pub cache: LibraryCacheState,
    pub cached_album_count: usize,
    pub cached_track_count: usize,
    pub cached_artist_count: usize,
    pub cached_album_artist_count: usize,
    pub cached_genre_count: usize,
    pub cached_playlist_count: usize,
    pub home_sections: Vec<HomeSection>,
    pub prefetched_explore: Option<HomeSection>,
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
    pub artists: Vec<Artist>,
    pub album_artists: Vec<Artist>,
    pub genres: Vec<Genre>,
    pub playlists: Vec<Playlist>,
    pub playlist_entry_keys: HashMap<PlaylistId, Vec<(String, TrackId)>>,
    pub favorites: Vec<Track>,
    pub search: SearchResults,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LibraryCacheState {
    NoCache { revision: i64 },
    Committed { revision: i64 },
}

impl LibraryCacheState {
    pub fn revision(self) -> i64 {
        match self {
            Self::NoCache { revision } | Self::Committed { revision } => revision,
        }
    }

    pub fn is_committed(self) -> bool {
        matches!(self, Self::Committed { .. })
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibraryCounts {
    pub albums: usize,
    pub tracks: usize,
    pub artists: usize,
    pub album_artists: usize,
    pub genres: usize,
    pub playlists: usize,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LibraryHomeUpdate {
    pub sections: Vec<HomeSection>,
    pub prefetched_explore: Option<HomeSection>,
}
#[derive(Clone, Debug)]
pub enum LibraryCommitProjection {
    Initial(Box<LibrarySnapshot>),
    Current {
        counts: LibraryCounts,
        home: Option<LibraryHomeUpdate>,
    },
}
#[derive(Clone, Debug)]
pub struct LibraryCommitUpdate {
    pub commit: library_sync::LibraryCommitted,
    pub projection: Option<Result<LibraryCommitProjection, String>>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchRequestKey {
    pub request_id: u64,
    pub query: String,
    pub kind: SearchKind,
    pub source_id: Option<SourceId>,
    pub selected_music_folder_id: Option<MusicFolderId>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLocalAccessSnapshot {
    pub source_id: SourceId,
    pub access: Option<SourceLocalAccess>,
    pub status: LocalAccessStatus,
    pub selected_music_folder_name: Option<String>,
    pub cached_album_count: usize,
    pub cached_track_count: usize,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalAccessStatus {
    pub sample_source_path: Option<String>,
    pub sample_local_path: Option<String>,
    pub direct_match_count: usize,
    pub prefix_match_count: usize,
    pub metadata_match_count: usize,
    pub unmatched_count: usize,
    pub total_track_count: usize,
}
#[derive(Clone, Debug)]
pub struct PlaybackSnapshot {
    pub current_source_id: Option<SourceId>,
    pub current: Option<QueueEntry>,
    pub state: PlaybackState,
    pub position_seconds: u32,
    pub position_millis: u64,
    pub duration_seconds: u32,
    pub volume: f64,
    pub muted: bool,
    pub repeat_mode: RepeatMode,
    pub shuffle_enabled: bool,
    pub auto_dj_enabled: bool,
    pub buffering_percent: Option<u8>,
    pub last_error: Option<String>,
    pub waveform_cache_key: Option<String>,
    pub waveform_peaks: Option<Arc<Vec<(f64, f64)>>>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LyricsSearchResult {
    pub provider: ExternalLyricsProvider,
    pub id: String,
    pub track_name: String,
    pub artist_name: String,
    pub album_name: String,
    pub duration_seconds: u32,
    pub synced_lyrics: Option<String>,
    pub plain_lyrics: Option<String>,
}
impl Default for PlaybackSnapshot {
    fn default() -> Self {
        Self {
            current_source_id: None,
            current: None,
            state: PlaybackState::Stopped,
            position_seconds: 0,
            position_millis: 0,
            duration_seconds: 0,
            volume: 1.0,
            muted: false,
            repeat_mode: RepeatMode::All,
            shuffle_enabled: false,
            auto_dj_enabled: false,
            buffering_percent: None,
            last_error: None,
            waveform_cache_key: None,
            waveform_peaks: None,
        }
    }
}
impl LibrarySnapshot {
    fn first_run() -> Self {
        Self {
            source: None,
            sources: Vec::new(),
            selected_source: None,
            local_folders: Vec::new(),
            source_local_access: Vec::new(),
            local_access: None,
            local_access_status: LocalAccessStatus::default(),
            music_folders: Vec::new(),
            selected_music_folder_id: None,
            first_run: true,
            cache: LibraryCacheState::NoCache { revision: 0 },
            cached_album_count: 0,
            cached_track_count: 0,
            cached_artist_count: 0,
            cached_album_artist_count: 0,
            cached_genre_count: 0,
            cached_playlist_count: 0,
            home_sections: Vec::new(),
            prefetched_explore: None,
            albums: Vec::new(),
            tracks: Vec::new(),
            artists: Vec::new(),
            album_artists: Vec::new(),
            genres: Vec::new(),
            playlists: Vec::new(),
            playlist_entry_keys: HashMap::new(),
            favorites: Vec::new(),
            search: SearchResults::default(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceNotice {
    Checking {
        source_name: String,
    },
    Connected,
    SettingsSaved,
    NoChanges,
    CacheCleared,
    CoverProgress {
        source_id: SourceId,
        processed: usize,
        total: usize,
    },
}

#[derive(Clone, Debug)]
pub enum ControllerEvent {
    Snapshot(Box<LibrarySnapshot>),
    SourceSelectionChanged {
        selected_source: LibrarySourceSelection,
    },
    SourceSyncChanged(library_sync::SourceSyncChanged),
    LibraryCommitted(Box<LibraryCommitUpdate>),
    LibraryDelta(Box<LibraryDelta>),
    HomeSectionsUpdated {
        snapshot: Box<LibrarySnapshot>,
        include_explore: bool,
    },
    HomeSectionPrefetched {
        source_id: SourceId,
        section: HomeSection,
    },
    PlaylistChanged {
        playlist_id: PlaylistId,
        snapshot: Box<LibrarySnapshot>,
    },
    SmartPlaylistChanged {
        smart_playlist_id: SmartPlaylistId,
        snapshot: Box<LibrarySnapshot>,
    },
    FavoriteChanged {
        item_id: FavoriteItemId,
        favorite: bool,
        snapshot: Box<LibrarySnapshot>,
    },
    FavoriteChangeFailed {
        item_id: FavoriteItemId,
        previous_favorite: bool,
        error: String,
    },
    Queue(Box<Option<QueueSnapshot>>),
    Playback(Box<PlaybackSnapshot>),
    Visualizer(Vec<f64>),
    Lyrics {
        track_id: TrackId,
        lyrics: Box<Option<Lyrics>>,
    },
    LyricsSearchResults {
        track_id: TrackId,
        artist_name: String,
        track_name: String,
        results: Vec<LyricsSearchResult>,
    },
    LyricsSearchFailed {
        track_id: TrackId,
        artist_name: String,
        track_name: String,
        error: String,
    },
    SearchLoaded {
        key: SearchRequestKey,
        results: SearchResults,
    },
    SearchFailed {
        key: SearchRequestKey,
        error: String,
    },
    LyricsSaved {
        path: PathBuf,
        lyrics: Lyrics,
    },
    FolderLoaded {
        request_id: u64,
        path: Vec<FolderPathItem>,
        detail: FolderDetail,
    },
    FolderLoadFailed {
        request_id: u64,
        path: Vec<FolderPathItem>,
        error: String,
    },
    CoverReady {
        key: String,
        path: PathBuf,
    },
    CoverUnavailable {
        key: String,
        external_retry_generation: Option<u64>,
    },
    CoverDeferred {
        key: String,
    },
    ServerDiscovery {
        servers: Vec<DiscoveredServer>,
        status: ServerDiscoveryStatus,
        running: bool,
    },
    SourceNotice(SourceNotice),
    SourceTransitionFailed {
        source_id: Option<SourceId>,
        error: String,
    },
    Error(String),
}

#[derive(Clone)]
pub struct AppController {
    pub(in crate::controller) store: StoreHandle,
    pub(in crate::controller) runtime: Arc<Runtime>,
    pub(in crate::controller) active_source: ActiveSourceSlot,
    pub(in crate::controller) secrets: Arc<dyn SecretStore>,
    secret_switch: Arc<SwitchableSecretStore>,
    settings: settings_controller::SettingsController,
    queue: Arc<Mutex<Option<QueueEngine>>>,
    source_transitions: Arc<SourceTransitions>,
    play_activation_generation: Arc<AtomicU64>,
    queue_persist_generation: Arc<AtomicU64>,
    playback_request_generation: Arc<AtomicU64>,
    next_preload: Arc<Mutex<NextPreloadState>>,
    waveform_warm_generation: Arc<AtomicU64>,
    playback: Arc<Mutex<Box<dyn PlaybackBackend>>>,
    playback_snapshot: Arc<Mutex<PlaybackSnapshot>>,
    playback_activity: Arc<Mutex<PlaybackActivityState>>,
    auto_dj_enabled: Arc<Mutex<bool>>,
    last_progress_snapshot: Arc<Mutex<Option<(SourceId, u32)>>>,
    last_report_snapshot: Arc<Mutex<Option<(TrackId, u32)>>>,
    external_scrobble_state: Arc<Mutex<ExternalScrobbleState>>,
    sync_coordinator: Arc<Mutex<library_sync::SyncCoordinator>>,
    pub(in crate::controller) external_cover_retry_generation: Arc<AtomicU64>,
    pub(in crate::controller) events: Sender<ControllerEvent>,
    home_refresh_in_flight: InFlightGuards<SourceId>,
    explore_prefetch_in_flight: InFlightGuards<SourceId>,
    pub(in crate::controller) cover_in_flight: Arc<Mutex<HashMap<String, u64>>>,
    pub(in crate::controller) external_cover_prefetch_in_flight: Arc<Mutex<HashMap<SourceId, u64>>>,
    pub(in crate::controller) cover_slots: Arc<(Mutex<usize>, Condvar)>,
}

/// Ignore outdated source work before it starts. Once a source change starts,
/// finish it before applying the next one
struct SourceTransitions {
    generation: AtomicU64,
    commit: Mutex<()>,
}

impl SourceTransitions {
    fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
            commit: Mutex::new(()),
        }
    }

    fn begin(&self) -> u64 {
        self.generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1)
    }

    fn current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::Acquire) == generation
    }

    fn commit(&self, generation: u64) -> Result<Option<std::sync::MutexGuard<'_, ()>>, String> {
        let guard = self
            .commit
            .lock()
            .map_err(|_| "source transition lock was poisoned".to_string())?;
        Ok(self.current(generation).then_some(guard))
    }
}

pub(crate) type ControllerBootstrap = (
    AppController,
    Receiver<ControllerEvent>,
    LibrarySnapshot,
    Option<QueueSnapshot>,
    PlaybackSnapshot,
);

pub(crate) fn smart_playlist_definition_fingerprint(
    definition: &SmartPlaylistDefinition,
) -> String {
    serde_json::to_string(definition).unwrap_or_else(|_| "unavailable".to_string())
}

#[derive(Clone)]
pub(in crate::controller) struct InFlightGuards<K>
where
    K: Eq + Hash,
{
    name: &'static str,
    inner: Arc<Mutex<HashSet<K>>>,
}
pub(in crate::controller) struct InFlightPermit<K>
where
    K: Eq + Hash,
{
    guards: InFlightGuards<K>,
    key: Option<K>,
}
impl<K> InFlightGuards<K>
where
    K: Eq + Hash,
{
    fn new(name: &'static str) -> Self {
        Self {
            name,
            inner: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    fn poisoned_message(&self) -> String {
        format!("{} guard lock was poisoned.", self.name)
    }
}
impl<K> InFlightGuards<K>
where
    K: Clone + Eq + Hash,
{
    fn acquire(&self, key: K) -> Result<Option<InFlightPermit<K>>, String> {
        let mut running = self.inner.lock().map_err(|_| self.poisoned_message())?;
        if !running.insert(key.clone()) {
            return Ok(None);
        }
        Ok(Some(InFlightPermit {
            guards: self.clone(),
            key: Some(key),
        }))
    }
}
impl<K> Drop for InFlightPermit<K>
where
    K: Eq + Hash,
{
    fn drop(&mut self) {
        let Some(key) = self.key.take() else {
            return;
        };
        match self.guards.inner.lock() {
            Ok(mut running) => {
                running.remove(&key);
            }
            Err(_) => {
                warn!(
                    guard = self.guards.name,
                    "in-flight guard lock was poisoned during release"
                );
            }
        }
    }
}
pub(in crate::controller) struct HomeRefreshContext {
    store: StoreHandle,
    runtime: Arc<Runtime>,
    active_source: ActiveSourceSlot,
    events: Sender<ControllerEvent>,
    home_refresh_in_flight: InFlightGuards<SourceId>,
}
pub(in crate::controller) struct ExplorePrefetchContext {
    store: StoreHandle,
    runtime: Arc<Runtime>,
    active_source: ActiveSourceSlot,
    events: Sender<ControllerEvent>,
    explore_prefetch_in_flight: InFlightGuards<SourceId>,
}
#[derive(Clone, Copy, Debug)]
pub(in crate::controller) enum HomeRefreshTarget {
    Section(HomeSectionKind),
}
/// Keep one current settings value and save one complete change at a time
#[derive(Clone)]
pub(crate) enum StoreHandle {
    Path {
        cache_database_path: PathBuf,
        settings_path: PathBuf,
        settings: Arc<Mutex<AppSettings>>,
    },
    #[cfg(test)]
    Memory {
        store: Arc<Mutex<Store>>,
        settings: Arc<Mutex<AppSettings>>,
    },
}
impl StoreHandle {
    pub(in crate::controller) fn open_for_app() -> Result<Self, String> {
        if let Some(cache_root) = cache_dir() {
            ensure_app_cache_dirs(&cache_root)?;
            if let Err(error) = remove_waveform_tmp(&cache_root) {
                warn!(%error, "failed to remove waveform temp cache");
            }
        }
        let cache_database_path = app_cache_database_path();
        if let Some(parent) = cache_database_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let store = Store::open(&cache_database_path).map_err(|error| error.to_string())?;
        store
            .recover_interrupted_syncs()
            .map_err(|error| error.to_string())?;

        let settings_path = app_settings_path();
        let settings = match fs::read_to_string(&settings_path) {
            Ok(value) => serde_json::from_str(&value).map_err(|error| error.to_string())?,
            Err(error) if error.kind() == ErrorKind::NotFound => AppSettings::default(),
            Err(error) => return Err(error.to_string()),
        };
        let handle = Self::Path {
            cache_database_path,
            settings_path,
            settings: Arc::new(Mutex::new(settings)),
        };
        Ok(handle)
    }

    #[cfg(test)]
    pub(in crate::controller) fn open_memory() -> Result<Self, String> {
        Store::open_memory()
            .map(|store| Self::Memory {
                store: Arc::new(Mutex::new(store)),
                settings: Arc::new(Mutex::new(AppSettings::default())),
            })
            .map_err(|error| error.to_string())
    }

    pub(in crate::controller) fn uses_disk_storage(&self) -> bool {
        matches!(self, Self::Path { .. })
    }

    pub(crate) fn with_store<T>(
        &self,
        operation: impl FnOnce(&Store) -> Result<T, StoreError>,
    ) -> Result<T, String> {
        self.with_store_result(
            operation,
            |error| error.to_string(),
            |error| error.to_string(),
            || "store lock was poisoned".to_string(),
        )
    }

    pub(in crate::controller) fn with_store_session<T>(
        &self,
        operation: impl FnOnce(&Store) -> Result<T, String>,
    ) -> Result<T, String> {
        self.with_store_result(
            operation,
            |error| error.to_string(),
            |error| error,
            || "store lock was poisoned".to_string(),
        )
    }

    pub(in crate::controller) fn with_store_sync<T>(
        &self,
        operation: impl FnOnce(&Store) -> library_sync::SyncResult<T>,
    ) -> library_sync::SyncResult<T> {
        self.with_store_result(
            operation,
            library_sync::SyncError::from,
            |error| error,
            || library_sync::SyncError::Unavailable("Store lock was poisoned"),
        )
    }

    fn with_store_result<T, O, E>(
        &self,
        operation: impl FnOnce(&Store) -> Result<T, O>,
        store_error: impl Fn(StoreError) -> E,
        operation_error: impl Fn(O) -> E,
        _poisoned: impl Fn() -> E,
    ) -> Result<T, E> {
        match self {
            Self::Path {
                cache_database_path,
                ..
            } => {
                let store = Store::open(cache_database_path).map_err(store_error)?;
                operation(&store).map_err(operation_error)
            }
            #[cfg(test)]
            Self::Memory { store, .. } => {
                let store = store.lock().map_err(|_| _poisoned())?;
                operation(&store).map_err(operation_error)
            }
        }
    }

    pub(in crate::controller) fn with_store_fast<T>(
        &self,
        operation: impl FnOnce(&Store) -> Result<T, StoreError>,
    ) -> Result<T, String> {
        match self {
            Self::Path {
                cache_database_path,
                ..
            } => {
                let store = Store::open_fast_read(cache_database_path)
                    .map_err(|error| error.to_string())?;
                operation(&store).map_err(|error| error.to_string())
            }
            #[cfg(test)]
            Self::Memory { store, .. } => {
                let store = store
                    .lock()
                    .map_err(|_| "store lock was poisoned".to_string())?;
                operation(&store).map_err(|error| error.to_string())
            }
        }
    }

    pub(crate) fn load_settings(&self) -> AppSettings {
        match self {
            Self::Path { settings, .. } => settings
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
            #[cfg(test)]
            Self::Memory { settings, .. } => settings
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        }
    }

    #[cfg(test)]
    pub(crate) fn save_settings(&self, settings: &AppSettings) -> Result<(), String> {
        match self {
            Self::Path {
                settings_path,
                settings: stored,
                ..
            } => {
                let mut stored = stored
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(parent) = settings_path.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                let value =
                    serde_json::to_string_pretty(settings).map_err(|error| error.to_string())?;
                let temp_path = settings_path.with_extension("json.tmp");
                fs::write(&temp_path, format!("{value}\n")).map_err(|error| error.to_string())?;
                restrict_settings_file(&temp_path).map_err(|error| error.to_string())?;
                fs::rename(&temp_path, settings_path).map_err(|error| error.to_string())?;
                *stored = settings.clone();
                Ok(())
            }
            #[cfg(test)]
            Self::Memory {
                settings: stored, ..
            } => {
                let mut stored = stored
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *stored = settings.clone();
                Ok(())
            }
        }
    }

    pub(crate) fn update_settings<T>(
        &self,
        update: impl FnOnce(&mut AppSettings) -> Result<T, String>,
    ) -> Result<T, String> {
        match self {
            Self::Path {
                settings_path,
                settings,
                ..
            } => {
                let mut stored = settings
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let mut next = stored.clone();
                let output = update(&mut next)?;
                if let Some(parent) = settings_path.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                let value =
                    serde_json::to_string_pretty(&next).map_err(|error| error.to_string())?;
                let temp_path = settings_path.with_extension("json.tmp");
                fs::write(&temp_path, format!("{value}\n")).map_err(|error| error.to_string())?;
                restrict_settings_file(&temp_path).map_err(|error| error.to_string())?;
                fs::rename(&temp_path, settings_path).map_err(|error| error.to_string())?;
                *stored = next;
                Ok(output)
            }
            #[cfg(test)]
            Self::Memory { settings, .. } => {
                let mut stored = settings
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let mut next = stored.clone();
                let output = update(&mut next)?;
                *stored = next;
                Ok(output)
            }
        }
    }
}
