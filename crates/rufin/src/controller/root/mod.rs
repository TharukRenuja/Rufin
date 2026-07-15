use crate::StoredSettings;
#[cfg(test)]
pub(in crate::controller) use crate::source_setup::local_configured_source as local_source_saved;
use crate::source_setup::{
    ActiveSource, ActiveSourceSlot, OperationOwner, current_active_source,
    map_server_path_to_local, selected_active_source,
};
use async_channel::Sender;
use directories::ProjectDirs;
#[cfg(test)]
use library::ImageRef;
use library::{
    ActiveLibraryQuery, AlbumId, ArtistId, HomeSection, HomeSectionKind, MusicFolderId, Playlist,
    PlaylistId, SmartPlaylistBuiltin, SmartPlaylistDefinition, SmartPlaylistId, SourceFeatureOwner,
    SourceId, Track, TrackId,
};
#[cfg(test)]
use library::{
    Album, Artist, Genre, GenreId, PlaylistDetail, SourceEntityKind, SourceObjectMapping,
};
use library::{
    EntityDelta, LibraryDelta, PlaylistWriteMode, SourceLocalAccess, Store, StoreError,
    StoreResult, StoredSource, SyncCommit,
};
use library::{FavoriteItemId, FolderDetail, FolderId, PlaylistEntry};
#[cfg(test)]
use metadata::{LyricLine, LyricsSource};
use metadata::{Lyrics, LyricsSearchResult};
use playback::PlaybackProjection;
#[cfg(test)]
use secrets::MemorySecretStore;
#[cfg(unix)]
use secrets::SecretServiceStore;
#[cfg(not(unix))]
use secrets::UnavailableSecretStore;
use secrets::{
    CachedSecretStore, ConfigSecretStore, SecretKey, SecretStorageMode, SecretStore,
    SwitchableSecretStore,
};
use sources::StreamRequest;
#[cfg(test)]
use sources::local::LOCAL_SOURCE_ID;
use sources::local::LocalSource;
use sources::{
    GeneratedTrackSeed, LibrarySourceSelection, LibrarySourceSettings, LocalLibraryFolder,
    SourceIdentity, SourceNotice, SourcePlaylistOperation, StreamDescriptor,
};
use std::collections::HashSet;
use std::fs;
use std::hash::Hash;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
#[cfg(test)]
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;
use tracing::{debug, info, warn};
#[cfg(test)]
use ui::runtime::ProductReceivers;

mod auto_dj;
pub(crate) use auto_dj::{cached_auto_dj_operation, native_auto_dj_operation};
mod bounded_runner;
mod cached_reads;
mod controller_bootstrap;
pub(crate) use controller_bootstrap::bootstrap;
mod controller_settings;
mod controller_startup;
mod event_ports;
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
pub(in crate::controller) use source_sync::{
    deactivate_source_sync_state, forget_source_sync_state,
};
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
use event_ports::{
    LibraryEventSender, LyricsEventSender, PlaybackEventSenders, SourceEventSenders,
    product_event_channels,
};
use playback_product::PlaybackProduct;
pub(in crate::controller) use playback_product::{
    activate_playback_source, clear_playback_product_slot, current_playback_entry_from_slot,
    playback_product_if_present_from_slot, send_session_command_to_slot,
};
pub(in crate::controller) use playback_queue::*;
use playback_waveforms::WaveformKey;
use source_presentation::{
    load_runtime_source_presentation, load_source_local_access_presentation,
    load_source_presentation,
};
use source_report_worker::SourceReportWorker;
pub(crate) use sources::{
    LibraryCacheState, LocalAccessStatus, SourceLocalAccessPresentation, SourcePresentationState,
};
pub(in crate::controller) use stream_requests::*;
#[cfg(test)]
pub(in crate::controller) use test_support::*;

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

#[derive(Clone)]
pub(crate) struct SourceCommands {
    pub(in crate::controller) store: StoreHandle,
    pub(in crate::controller) runtime: Arc<Runtime>,
    pub(in crate::controller) active_source: ActiveSourceSlot,
    pub(in crate::controller) secrets: Arc<dyn SecretStore>,
    playback_product: Arc<RwLock<Option<Arc<PlaybackProduct>>>>,
    source_transitions: Arc<SourceTransitions>,
    sync_coordinator: Arc<Mutex<library_sync::SyncCoordinator>>,
    pub(in crate::controller) artwork: ::artwork::Artwork,
    pub(in crate::controller) source_events: SourceEventSenders,
    pub(in crate::controller) library_events: LibraryEventSender,
    pub(in crate::controller) playback_projection: Sender<PlaybackProjection>,
}

#[derive(Clone)]
pub(crate) struct LibraryCommands {
    pub(in crate::controller) store: StoreHandle,
    pub(in crate::controller) runtime: Arc<Runtime>,
    pub(in crate::controller) active_source: ActiveSourceSlot,
    pub(in crate::controller) secrets: Arc<dyn SecretStore>,
    pub(in crate::controller) library_events: LibraryEventSender,
    home_refresh_in_flight: InFlightGuards<SourceId>,
    explore_prefetch_in_flight: InFlightGuards<SourceId>,
}

#[derive(Clone)]
pub(crate) struct PlaybackCommands {
    pub(in crate::controller) store: StoreHandle,
    pub(in crate::controller) runtime: Arc<Runtime>,
    pub(in crate::controller) active_source: ActiveSourceSlot,
    settings: settings_controller::SettingsController,
    playback_product: Arc<RwLock<Option<Arc<PlaybackProduct>>>>,
    waveform_request_key: Arc<Mutex<Option<WaveformKey>>>,
    waveform_warm_generation: Arc<AtomicU64>,
    pub(in crate::controller) artwork: ::artwork::Artwork,
    pub(in crate::controller) library_events: LibraryEventSender,
    pub(in crate::controller) playback_events: PlaybackEventSenders,
}

#[derive(Clone)]
pub(crate) struct ArtworkCommands {
    pub(in crate::controller) active_source: ActiveSourceSlot,
    pub(in crate::controller) artwork: ::artwork::Artwork,
}

#[derive(Clone)]
pub(crate) struct LyricsCommands {
    pub(in crate::controller) store: StoreHandle,
    pub(in crate::controller) runtime: Arc<Runtime>,
    pub(in crate::controller) active_source: ActiveSourceSlot,
    playback_product: Arc<RwLock<Option<Arc<PlaybackProduct>>>>,
    lyrics_request_generation: Arc<AtomicU64>,
    pub(in crate::controller) lyrics_events: LyricsEventSender,
}

#[derive(Clone)]
pub(crate) struct UiSettingsStore {
    pub(in crate::controller) store: StoreHandle,
    pub(in crate::controller) active_source: ActiveSourceSlot,
    pub(in crate::controller) secrets: Arc<dyn SecretStore>,
    secret_switch: Arc<SwitchableSecretStore>,
    settings: settings_controller::SettingsController,
    playback_product: Arc<RwLock<Option<Arc<PlaybackProduct>>>>,
    source_transitions: Arc<SourceTransitions>,
    lyrics_request_generation: Arc<AtomicU64>,
    sync_coordinator: Arc<Mutex<library_sync::SyncCoordinator>>,
    pub(in crate::controller) source_presentation: Sender<SourcePresentationState>,
    pub(in crate::controller) library_sync_events: Sender<library_sync::LibrarySyncEvent>,
}

pub(crate) struct ProductOwners {
    pub(in crate::controller) source: SourceCommands,
    pub(in crate::controller) library: LibraryCommands,
    pub(in crate::controller) playback: PlaybackCommands,
    pub(in crate::controller) artwork: ArtworkCommands,
    pub(in crate::controller) lyrics: LyricsCommands,
    pub(in crate::controller) settings: UiSettingsStore,
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

pub(crate) type ProductAssembly = (
    ProductOwners,
    ui::runtime::ProductReceivers,
    SourcePresentationState,
    Option<PlaybackProjection>,
);

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
    secrets: Arc<dyn SecretStore>,
    library_events: LibraryEventSender,
    home_refresh_in_flight: InFlightGuards<SourceId>,
}
pub(in crate::controller) struct ExplorePrefetchContext {
    store: StoreHandle,
    runtime: Arc<Runtime>,
    active_source: ActiveSourceSlot,
    secrets: Arc<dyn SecretStore>,
    library_events: LibraryEventSender,
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
        write_gate: library::StoreWriteGate,
    },
    #[cfg(test)]
    Memory {
        store: Arc<Mutex<Store>>,
        settings: Arc<Mutex<StoredSettings>>,
    },
}
impl StoreHandle {
    fn library_access(&self) -> library::StoreAccess {
        match self {
            Self::Path {
                cache_database_path,
                write_gate,
                ..
            } => library::StoreAccess::from_path(cache_database_path.clone(), write_gate.clone()),
            #[cfg(test)]
            Self::Memory { store, .. } => library::StoreAccess::from_shared(Arc::clone(store)),
        }
    }

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
        let write_gate = library::StoreWriteGate::default();
        let store = Store::open_with_write_gate(&cache_database_path, write_gate.clone())
            .map_err(|error| error.to_string())?;
        store
            .recover_interrupted_syncs()
            .map_err(|error| error.to_string())?;
        store
            .prepare_smart_playlist_defaults()
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
            write_gate,
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
                write_gate,
                ..
            } => {
                let store = Store::open_with_write_gate(cache_database_path, write_gate.clone())
                    .map_err(store_error)?;
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

impl LibraryCommands {
    pub fn library_query(&self, source_id: SourceId) -> ActiveLibraryQuery {
        self.store.library_access().query(source_id)
    }
}
