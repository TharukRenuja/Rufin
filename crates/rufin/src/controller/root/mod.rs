use super::covers;
pub use super::discovery::{DiscoveredServer, ServerDiscoveryStatus};
pub use super::random::{RandomPlayAction, RandomPlayRequest};
use crate::external_scrobbling::{self, ExternalScrobbleState};
use crate::providers::{
    JellyfinLyricsSearch, LoadedProvider, StreamingProvider,
    jellyfin_stream_descriptor_from_saved_session, login_provider, provider_display_name,
    provider_from_saved,
};
use crate::{cover_art_policy, external_metadata};
#[cfg(any(test, feature = "dev-tools"))]
use ::test_support::{FakeProvider, FakeScale};
use directories::ProjectDirs;
#[cfg(test)]
use domain::ThemePreference;
use domain::{
    Album, AlbumId, AppSettings, Artist, ArtistId, ArtistTrackScope, AutoDjReason,
    ExternalLyricsProvider, FolderPathItem, Genre, GenreId, HomeSection, HomeSectionKind, ImageRef,
    LibraryField, LibraryListSettings, LibrarySourceSelection, LocalLibraryFolder,
    LocalManifestEntry, LocalManifestScan, MusicFolder, MusicFolderId, PlaySourceDescriptor,
    PlaySourceKey, PlaybackSettings, Playlist, PlaylistDetail, PlaylistEntrySortDescriptor,
    PlaylistId, QueueEngine, QueueEntry, QueueEntryId, QueueInsertion, QueueInsertionSource,
    QueueItemInput, QueueReplacement, QueueSnapshot, RepeatMode, SearchKind, SecretStorageMode,
    ServerId, ServerIdentity, SmartPlaylist, SmartPlaylistBuiltin, SmartPlaylistDefinition,
    SmartPlaylistDetail, SmartPlaylistId, SmartPlaylistSortDescriptor, SourceOrder,
    StreamDescriptor, StreamQuality, Track, TrackId, TrackSortDescriptor, TrackSortKey,
    TrackTableSettings,
};
use library::{
    CachedArtistDetail, CachedGenreDetail, CoverCacheEntry, EntityDelta, LibraryDelta,
    LibraryDeltaCollector, LocalLibraryDelta, SavedServer, ServerLocalAccess, Store,
    StoreBackedSourceItem, StoreBackedSourceWindow, StoreError, StoreResult, SyncState,
};
use playback::{
    FakePlaybackBackend, LazyGStreamerPlaybackBackend, PlaybackBackend, PlaybackCommand,
    PlaybackEvent, PlaybackState, PlaybackTrack, PreparedPlaybackItem,
    generate_waveform_peaks_cancellable,
};
#[cfg(any(test, feature = "dev-tools"))]
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
    FavoriteItemId, FolderDetail, Lyrics, MusicProvider, PagedRequest, PlaybackReport,
    PlaybackReportKind, PlaylistEntry, ProviderSession, SavedProviderSession, SearchResults,
    StreamRequest,
};
#[cfg(test)]
use source::{LyricLine, LyricsSource, PlayedFilter};
use source_local::{LOCAL_PROVIDER_ID, LocalProvider, LocalScanProgress, LocalScanStage};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::Hash;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
#[cfg(test)]
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;
use tracing::{debug, info, instrument, warn};

mod auto_dj;
mod auto_dj_commands;
mod cached_library_api;
mod cached_reads;
mod controller_bootstrap;
mod controller_settings;
mod controller_startup;
mod folder_search_commands;
mod library_mutations;
mod local_library_stress;
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
mod server_cache_commands;
mod server_lifecycle_commands;
mod server_local_access_commands;
mod settings_controller;
mod source_image_policy;
mod source_presentation;
mod source_readiness;
mod source_refs;
mod source_selection;
mod sync_command;
mod sync_requests;

#[cfg(test)]
mod cover_playback_tests;
#[cfg(test)]
mod lyrics_local_access_tests;
#[cfg(test)]
mod startup_sync_tests;
#[cfg(test)]
mod test_support;

pub(in crate::controller) use cached_reads::*;
pub(in crate::controller) use controller_startup::*;
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
use source_image_policy::{
    image_ref_allowed, is_local_album_id, is_local_artist_id, is_local_provider_image_ref,
    is_local_track_id, scrub_home_refs, scrub_source_image_ref, source_image_ref_allowed,
};
pub(in crate::controller) use source_image_policy::{
    scrub_selected_album_image_refs, scrub_selected_artist_image_refs,
    scrub_selected_genre_image_refs, scrub_selected_playlist_image_refs,
    scrub_selected_track_image_refs, scrub_smart_refs,
};
use source_presentation::{load_runtime_snapshot, load_snapshot};
#[cfg(test)]
use source_readiness::{
    SourceSyncReadinessInput, SyncRequiredReason, active_source_readiness, source_sync_readiness,
};
use source_readiness::{active_server_needs_sync, active_source_startup_readiness};
pub(in crate::controller) use source_refs::track_album_refs_with_settings;
use source_refs::{
    album_track_refs, home_image_refs, home_local_refs, queue_album_refs, sync_status_text,
    track_album_refs,
};
pub(crate) use source_refs::{grouped_cover_refs_for_items, track_cover_refs_for_items};
pub(in crate::controller) use sync_requests::*;
#[cfg(test)]
pub(in crate::controller) use test_support::*;

const PAGE_SIZE: usize = 500;
const SNAPSHOT_GRID_LIMIT: usize = 500;
pub(in crate::controller) const SNAPSHOT_TRACK_LIMIT: usize = 40_000;
const STARTUP_CACHE_STALE_SECONDS: i64 = 24 * 60 * 60;
pub(in crate::controller) const IMAGE_TAG_UNTAGGED: &str = "untagged";
const AUTO_DJ_ITEM_COUNT: usize = 5;
const AUTO_DJ_HISTORY_LIMIT: usize = AUTO_DJ_ITEM_COUNT * 2;
const AUTO_DJ_LIBRARY_LIMIT: usize = 5_000;
const CACHE_DATABASE_FILE_NAME: &str = "rufin-cache.sqlite";
const SETTINGS_FILE_NAME: &str = "settings.json";
const CONFIG_SECRETS_FILE_NAME: &str = "secrets.json";
const STORE_DIR_NAME: &str = "store";
const COVER_CACHE_DIR_NAME: &str = "covers";
const LYRICS_CACHE_DIR_NAME: &str = "lyrics";
const PLAYBACK_CACHE_DIR_NAME: &str = "playback";
const WAVEFORM_CACHE_DIR_NAME: &str = "waveforms";
const TMP_CACHE_DIR_NAME: &str = "tmp";
const LOCAL_SOURCE_SERVER_ID: &str = "local:server:library";
#[derive(Clone, Debug)]
pub struct LibrarySnapshot {
    pub server: Option<ServerIdentity>,
    pub servers: Vec<ServerIdentity>,
    pub selected_source: Option<LibrarySourceSelection>,
    pub local_folders: Vec<LocalLibraryFolder>,
    pub server_local_access: Vec<ServerLocalAccessSnapshot>,
    pub local_access: Option<ServerLocalAccess>,
    pub local_access_status: LocalAccessStatus,
    pub music_folders: Vec<MusicFolder>,
    pub selected_music_folder_id: Option<MusicFolderId>,
    pub username: Option<String>,
    pub first_run: bool,
    pub sync_status: String,
    pub last_error: Option<String>,
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LibrarySyncStatus {
    pub server_id: ServerId,
    pub sync_status: String,
    pub last_error: Option<String>,
    pub counts: LibraryCounts,
    pub home: Option<LibraryHomeUpdate>,
    pub delta: LibraryDelta,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchRequestKey {
    pub request_id: u64,
    pub query: String,
    pub kind: SearchKind,
    pub server_id: Option<ServerId>,
    pub selected_music_folder_id: Option<MusicFolderId>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerLocalAccessSnapshot {
    pub server_id: ServerId,
    pub access: Option<ServerLocalAccess>,
    pub status: LocalAccessStatus,
    pub selected_music_folder_name: Option<String>,
    pub username: Option<String>,
    pub trust_invalid_cert: bool,
    pub sync_status: String,
    pub cached_album_count: usize,
    pub cached_track_count: usize,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalAccessStatus {
    pub sample_server_path: Option<String>,
    pub sample_local_path: Option<String>,
    pub direct_match_count: usize,
    pub prefix_match_count: usize,
    pub metadata_match_count: usize,
    pub unmatched_count: usize,
    pub total_track_count: usize,
}
#[derive(Clone, Debug)]
pub struct PlaybackSnapshot {
    pub current_server_id: Option<ServerId>,
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
            current_server_id: None,
            current: None,
            state: PlaybackState::Stopped,
            position_seconds: 0,
            position_millis: 0,
            duration_seconds: 0,
            volume: 1.0,
            muted: false,
            repeat_mode: RepeatMode::All,
            shuffle_enabled: false,
            auto_dj_enabled: true,
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
            server: None,
            servers: Vec::new(),
            selected_source: None,
            local_folders: Vec::new(),
            server_local_access: Vec::new(),
            local_access: None,
            local_access_status: LocalAccessStatus::default(),
            music_folders: Vec::new(),
            selected_music_folder_id: None,
            username: None,
            first_run: true,
            sync_status: String::new(),
            last_error: None,
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
#[derive(Clone, Debug)]
pub enum ControllerEvent {
    Snapshot(Box<LibrarySnapshot>),
    LibrarySyncStatus(Box<LibrarySyncStatus>),
    LibraryDelta(Box<LibraryDelta>),
    HomeSectionsUpdated {
        snapshot: Box<LibrarySnapshot>,
        include_explore: bool,
    },
    HomeSectionPrefetched {
        server_id: ServerId,
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
    LoginStatus(String),
    Error(String),
}

#[derive(Clone, Debug)]
pub struct LoginRequest {
    pub provider: StreamingProvider,
    pub server_url: String,
    pub username: String,
    pub password: String,
    pub trust_invalid_cert: bool,
    pub local_access_root: Option<PathBuf>,
    pub path_replace_from: Option<String>,
}

#[derive(Clone)]
pub struct AppController {
    pub(in crate::controller) store: StoreHandle,
    pub(in crate::controller) runtime: Arc<Runtime>,
    pub(in crate::controller) secrets: Arc<dyn SecretStore>,
    secret_switch: Arc<SwitchableSecretStore>,
    settings: settings_controller::SettingsController,
    queue: Arc<Mutex<Option<QueueEngine>>>,
    play_activation_generation: Arc<AtomicU64>,
    queue_persist_generation: Arc<AtomicU64>,
    playback_request_generation: Arc<AtomicU64>,
    next_preload: Arc<Mutex<NextPreloadState>>,
    waveform_warm_generation: Arc<AtomicU64>,
    playback: Arc<Mutex<Box<dyn PlaybackBackend>>>,
    playback_snapshot: Arc<Mutex<PlaybackSnapshot>>,
    playback_activity: Arc<Mutex<PlaybackActivityState>>,
    auto_dj_enabled: Arc<Mutex<bool>>,
    last_progress_snapshot: Arc<Mutex<Option<(ServerId, u32)>>>,
    last_report_snapshot: Arc<Mutex<Option<(TrackId, u32)>>>,
    external_scrobble_state: Arc<Mutex<ExternalScrobbleState>>,
    pub(in crate::controller) external_cover_retry_generation: Arc<AtomicU64>,
    pub(in crate::controller) events: Sender<ControllerEvent>,
    sync_in_flight: InFlightGuards<ServerId>,
    home_refresh_in_flight: InFlightGuards<ServerId>,
    explore_prefetch_in_flight: InFlightGuards<ServerId>,
    pub(in crate::controller) cover_in_flight: Arc<Mutex<HashMap<String, u64>>>,
    pub(in crate::controller) external_cover_prefetch_in_flight: Arc<Mutex<HashMap<ServerId, u64>>>,
    pub(in crate::controller) cover_slots: Arc<(Mutex<usize>, Condvar)>,
    #[cfg(test)]
    _test_permit: Option<ControllerTestPermit>,
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

#[cfg(test)]
#[derive(Clone)]
struct ControllerTestPermit {
    _inner: Arc<ControllerTestPermitInner>,
}
#[cfg(test)]
struct ControllerTestPermitInner;
#[cfg(test)]
static CONTROLLER_TEST_GATE: OnceLock<(Mutex<bool>, Condvar)> = OnceLock::new();
#[cfg(test)]
fn controller_test_permit() -> ControllerTestPermit {
    let (lock, cvar) = CONTROLLER_TEST_GATE.get_or_init(|| (Mutex::new(false), Condvar::new()));
    let mut occupied = lock.lock().expect("controller test gate");
    while *occupied {
        occupied = cvar.wait(occupied).expect("controller test gate");
    }
    *occupied = true;
    ControllerTestPermit {
        _inner: Arc::new(ControllerTestPermitInner),
    }
}
#[cfg(test)]
impl Drop for ControllerTestPermitInner {
    fn drop(&mut self) {
        let (lock, cvar) = CONTROLLER_TEST_GATE.get_or_init(|| (Mutex::new(false), Condvar::new()));
        if let Ok(mut occupied) = lock.lock() {
            *occupied = false;
            cvar.notify_one();
        }
    }
}
#[derive(Clone)]
pub(in crate::controller) struct InFlightGuards<K>
where
    K: Eq + Hash,
{
    name: &'static str,
    inner: Arc<Mutex<HashMap<K, CancellationToken>>>,
}
#[derive(Clone, Debug)]
pub(in crate::controller) struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}
pub(in crate::controller) struct InFlightPermit<K>
where
    K: Eq + Hash,
{
    guards: InFlightGuards<K>,
    key: Option<K>,
    token: CancellationToken,
}
impl CancellationToken {
    pub(in crate::controller) fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(in crate::controller) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub(in crate::controller) fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}
impl<K> InFlightGuards<K>
where
    K: Eq + Hash,
{
    fn new(name: &'static str) -> Self {
        Self {
            name,
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn contains_or_blocked(&self, key: &K) -> bool {
        self.inner
            .lock()
            .map(|running| running.contains_key(key))
            .unwrap_or(true)
    }

    fn cancel(&self, key: &K) -> Result<bool, String> {
        self.inner
            .lock()
            .map(|running| {
                let Some(token) = running.get(key) else {
                    return false;
                };
                token.cancel();
                true
            })
            .map_err(|_| self.poisoned_message())
    }

    #[cfg(test)]
    fn cancellation_requested(&self, key: &K) -> bool {
        self.inner
            .lock()
            .map(|running| running.get(key).is_some_and(CancellationToken::cancelled))
            .unwrap_or(true)
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
        if running.contains_key(&key) {
            return Ok(None);
        }
        let token = CancellationToken::new();
        running.insert(key.clone(), token.clone());
        Ok(Some(InFlightPermit {
            guards: self.clone(),
            key: Some(key),
            token,
        }))
    }
}
impl<K> InFlightPermit<K>
where
    K: Eq + Hash,
{
    fn cancellation_token(&self) -> CancellationToken {
        self.token.clone()
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
    secrets: Arc<dyn SecretStore>,
    events: Sender<ControllerEvent>,
    sync_in_flight: InFlightGuards<ServerId>,
    home_refresh_in_flight: InFlightGuards<ServerId>,
}
#[derive(Clone)]
pub(in crate::controller) struct SyncContext {
    store: StoreHandle,
    runtime: Arc<Runtime>,
    secrets: Arc<dyn SecretStore>,
    events: Sender<ControllerEvent>,
    queue: Arc<Mutex<Option<QueueEngine>>>,
    queue_persist_generation: Arc<AtomicU64>,
    playback_snapshot: Arc<Mutex<PlaybackSnapshot>>,
    auto_dj_enabled: Arc<Mutex<bool>>,
    sync_in_flight: InFlightGuards<ServerId>,
    cover_in_flight: Arc<Mutex<HashMap<String, u64>>>,
    external_cover_retry_generation: Arc<AtomicU64>,
    external_cover_prefetch_in_flight: Arc<Mutex<HashMap<ServerId, u64>>>,
    cover_slots: Arc<(Mutex<usize>, Condvar)>,
}
pub(in crate::controller) struct ExplorePrefetchContext {
    store: StoreHandle,
    runtime: Arc<Runtime>,
    secrets: Arc<dyn SecretStore>,
    events: Sender<ControllerEvent>,
    sync_in_flight: InFlightGuards<ServerId>,
    explore_prefetch_in_flight: InFlightGuards<ServerId>,
}
#[derive(Clone, Copy, Debug)]
pub(in crate::controller) enum HomeRefreshTarget {
    Section(HomeSectionKind),
}
#[derive(Clone)]
pub(in crate::controller) enum StoreHandle {
    Path {
        cache_database_path: PathBuf,
        settings_path: PathBuf,
    },
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
        Store::open(&cache_database_path).map_err(|error| error.to_string())?;

        let settings_path = app_settings_path();
        let handle = Self::Path {
            cache_database_path,
            settings_path,
        };
        Ok(handle)
    }

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

    pub(in crate::controller) fn with_store<T>(
        &self,
        operation: impl FnOnce(&Store) -> Result<T, StoreError>,
    ) -> Result<T, String> {
        match self {
            Self::Path {
                cache_database_path,
                ..
            } => {
                let store = Store::open(cache_database_path).map_err(|error| error.to_string())?;
                operation(&store).map_err(|error| error.to_string())
            }
            Self::Memory { store, .. } => {
                let store = store
                    .lock()
                    .map_err(|_| "store lock was poisoned".to_string())?;
                operation(&store).map_err(|error| error.to_string())
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
            Self::Memory { store, .. } => {
                let store = store
                    .lock()
                    .map_err(|_| "store lock was poisoned".to_string())?;
                operation(&store).map_err(|error| error.to_string())
            }
        }
    }

    pub(in crate::controller) fn load_settings(&self) -> Result<AppSettings, String> {
        match self {
            Self::Path { settings_path, .. } => match fs::read_to_string(settings_path) {
                Ok(value) => serde_json::from_str(&value).map_err(|error| error.to_string()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(AppSettings::default()),
                Err(error) => Err(error.to_string()),
            },
            Self::Memory { settings, .. } => settings
                .lock()
                .map(|settings| settings.clone())
                .map_err(|_| "settings lock was poisoned".to_string()),
        }
    }

    pub(in crate::controller) fn save_settings(
        &self,
        settings: &AppSettings,
    ) -> Result<(), String> {
        match self {
            Self::Path { settings_path, .. } => {
                if let Some(parent) = settings_path.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                let value =
                    serde_json::to_string_pretty(settings).map_err(|error| error.to_string())?;
                let temp_path = settings_path.with_extension("json.tmp");
                fs::write(&temp_path, format!("{value}\n")).map_err(|error| error.to_string())?;
                restrict_settings_file(&temp_path).map_err(|error| error.to_string())?;
                fs::rename(&temp_path, settings_path).map_err(|error| error.to_string())?;
                Ok(())
            }
            Self::Memory {
                settings: stored, ..
            } => {
                let mut stored = stored
                    .lock()
                    .map_err(|_| "settings lock was poisoned".to_string())?;
                *stored = settings.clone();
                Ok(())
            }
        }
    }
}
