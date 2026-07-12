pub use super::discovery::{DiscoveredServer, ServerDiscoveryStatus};
pub use super::random::{RandomPlayAction, RandomPlayRequest};
use crate::StoredSettings;
#[cfg(test)]
pub(in crate::controller) use crate::source_setup::local_configured_source as local_source_saved;
use crate::source_setup::{
    ActiveSource, ActiveSourceSlot, OperationOwner, current_active_source,
    map_server_path_to_local, selected_active_source,
};
use directories::ProjectDirs;
use domain::{
    FolderPathItem, LibrarySourceSelection, LocalLibraryFolder, SearchKind, SecretStorageMode,
};
#[cfg(test)]
use library::ImageRef;
use library::{
    Album, AlbumId, Artist, ArtistId, Genre, GenreId, HomeSection, HomeSectionKind, Mood, MoodId,
    MusicFolder, MusicFolderId, PagedResponse, Playlist, PlaylistId, SmartPlaylist,
    SmartPlaylistBuiltin, SmartPlaylistDefinition, SmartPlaylistDetail, SmartPlaylistId,
    SourceFeatureOwner, SourceId, Track, TrackId,
};
use library::{
    CachedArtistDetail, CachedGenreDetail, CachedMoodDetail, EntityDelta, LibraryDelta,
    PlaylistWriteMode, SourceLocalAccess, Store, StoreError, StoreResult, StoredSource, SyncCommit,
};
use library::{FavoriteItemId, FolderDetail, PlaylistEntry, SearchResults};
#[cfg(test)]
use library::{PlaylistDetail, SourceEntityKind, SourceObjectMapping};
#[cfg(test)]
use metadata::{LyricLine, LyricsSource};
use metadata::{Lyrics, LyricsSearchResult};
#[cfg(test)]
use secrets::MemorySecretStore;
#[cfg(unix)]
use secrets::SecretServiceStore;
#[cfg(not(unix))]
use secrets::UnavailableSecretStore;
use secrets::{
    CachedSecretStore, ConfigSecretStore, SecretKey, SecretStore, SwitchableSecretStore,
};
use sources::StreamRequest;
#[cfg(test)]
use sources::local::LOCAL_SOURCE_ID;
use sources::local::LocalSource;
use sources::{GeneratedTrackSeed, SourceIdentity, SourcePlaylistOperation, StreamDescriptor};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::Hash;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
#[cfg(test)]
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;
use tracing::{debug, info, warn};

mod auto_dj;
pub(crate) use auto_dj::{cached_auto_dj_operation, native_auto_dj_operation};
mod bounded_runner;
mod cached_library_api;
mod cached_reads;
mod controller_bootstrap;
mod controller_settings;
mod controller_startup;
mod folder_search_commands;
mod library_mutations;
mod local_source_commands;
mod lyrics_commands;
use lyrics_commands::metadata_runner;
#[cfg(test)]
use lyrics_commands::{load_cached_lyrics, save_cached_lyrics};
mod playback_commands;
mod playback_product;
mod playback_queue;
mod playback_waveforms;
mod playlist_commands;
mod queue_commands;
use queue_commands::library_track_sort;
mod refresh_commands;
mod settings_controller;
mod source_cache_commands;
mod source_lifecycle_commands;
pub(in crate::controller) use source_lifecycle_commands::{
    SourcePersistenceSnapshot, save_source_settings,
};
mod source_local_access_commands;
mod source_presentation;
mod source_report_worker;
mod source_selection;
mod source_sync;
mod stream_requests;

#[cfg(test)]
mod local_radio_tests;
#[cfg(test)]
mod lyrics_local_access_tests;
#[cfg(test)]
mod startup_sync_tests;
#[cfg(test)]
mod test_support;

use bounded_runner::BoundedRunner;
pub(crate) use cached_reads::*;
pub(crate) use controller_startup::*;
use playback_product::PlaybackProduct;
pub use playback_product::{PlaybackNotice, PlaybackProjection};
pub(in crate::controller) use playback_queue::*;
use playback_waveforms::WaveformKey;
use source_presentation::{load_runtime_snapshot, load_snapshot};
use source_report_worker::SourceReportWorker;
pub(in crate::controller) use stream_requests::*;
#[cfg(test)]
pub(in crate::controller) use test_support::*;

const SNAPSHOT_GRID_LIMIT: usize = 500;
pub(in crate::controller) const SNAPSHOT_TRACK_LIMIT: usize = 40_000;
const AUTO_DJ_ITEM_COUNT: usize = 5;
const AUTO_DJ_PROVIDER_CANDIDATE_LIMIT: usize = AUTO_DJ_ITEM_COUNT * 4;
const CACHE_DATABASE_FILE_NAME: &str = "rufin-cache.sqlite";
const SETTINGS_FILE_NAME: &str = "settings.json";
const CONFIG_SECRETS_FILE_NAME: &str = "secrets.json";
const STORE_DIR_NAME: &str = "store";
const ARTWORK_CACHE_DIR_NAME: &str = "covers";
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
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WaveformProjection {
    pub key: Option<String>,
    pub peaks: Option<Arc<Vec<(f64, f64)>>>,
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
    Checking { source_name: String },
    Connected,
    SettingsSaved,
    NoChanges,
    CacheCleared,
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
    QueuePage(playback::QueuePage),
    PlaybackProduct(Box<PlaybackProjection>),
    Waveform(WaveformProjection),
    Lyrics {
        media_key: playback::MediaKey,
        generation: u64,
        lyrics: Box<Option<Lyrics>>,
    },
    LyricsSearchResults {
        media_key: playback::MediaKey,
        generation: u64,
        artist_name: String,
        track_name: String,
        results: Vec<LyricsSearchResult>,
    },
    LyricsSearchFailed {
        media_key: playback::MediaKey,
        generation: u64,
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
        media_key: playback::MediaKey,
        generation: u64,
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
    Artwork(::artwork::ArtworkEvent),
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
    playback_product: Arc<RwLock<Option<Arc<PlaybackProduct>>>>,
    source_transitions: Arc<SourceTransitions>,
    lyrics_request_generation: Arc<AtomicU64>,
    waveform_request_key: Arc<Mutex<Option<WaveformKey>>>,
    waveform_warm_generation: Arc<AtomicU64>,
    sync_coordinator: Arc<Mutex<library_sync::SyncCoordinator>>,
    pub(in crate::controller) artwork: ::artwork::Artwork,
    pub(in crate::controller) events: Sender<ControllerEvent>,
    home_refresh_in_flight: InFlightGuards<SourceId>,
    explore_prefetch_in_flight: InFlightGuards<SourceId>,
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
    Option<PlaybackProjection>,
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
        settings: Arc<Mutex<StoredSettings>>,
    },
    #[cfg(test)]
    Memory {
        store: Arc<Mutex<Store>>,
        settings: Arc<Mutex<StoredSettings>>,
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
            Err(error) if error.kind() == ErrorKind::NotFound => StoredSettings::default(),
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
                settings: Arc::new(Mutex::new(StoredSettings::default())),
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

    pub(crate) fn load_settings(&self) -> StoredSettings {
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
    pub(crate) fn save_settings(&self, settings: &StoredSettings) -> Result<(), String> {
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
        update: impl FnOnce(&mut StoredSettings) -> Result<T, String>,
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
