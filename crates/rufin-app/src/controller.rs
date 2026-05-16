use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use rufin_core::{
    Album, AlbumId, AppSettings, Artist, ArtistId, Genre, GenreId, HomeSection, HomeSectionKind,
    ImageRef, Playlist, PlaylistId, QueueEngine, QueueEntry, QueueEntryId, QueueSnapshot,
    RepeatMode, ServerId, ServerIdentity, Track, TrackId,
};
use rufin_playback::{
    FakePlaybackBackend, LazyGStreamerPlaybackBackend, PlaybackBackend, PlaybackCommand,
    PlaybackEvent, PlaybackState, PlaybackTrack, StreamDescriptor,
};
use rufin_provider::{
    FavoriteItemId, ImageKind, ImageRequest, LoginRequest, Lyrics, MusicProvider, PagedRequest,
    PlaybackReport, PlaybackReportKind, PlaylistEntry, SavedProviderSession, SearchResults,
};
use rufin_provider_jellyfin::{JellyfinLyricsSearch, JellyfinProvider};
use rufin_secrets::{MemorySecretStore, SecretServiceStore, SecretStore};
use rufin_store::{
    CachedArtistDetail, CachedGenreDetail, CoverCacheEntry, SavedServer, Store, StoreError,
    image_cache_key,
};
use rufin_test_support::{FakeProvider, FakeScale};
use serde::Deserialize;
use tokio::runtime::Runtime;
use tracing::{debug, info, instrument, warn};

const PAGE_SIZE: usize = 500;
const STARTUP_CACHE_STALE_SECONDS: i64 = 24 * 60 * 60;
const IMAGE_TAG_UNTAGGED: &str = "untagged";
const AUTO_DJ_ITEM_COUNT: usize = 5;
const AUTO_DJ_THRESHOLD: usize = 1;
const AUTO_DJ_LIBRARY_LIMIT: usize = 5_000;
const SEEK_SETTLE_WINDOW: Duration = Duration::from_millis(900);
const SEEK_POSITION_TOLERANCE_MILLIS: u64 = 1_500;

#[derive(Clone, Debug)]
pub struct LibrarySnapshot {
    pub server: Option<ServerIdentity>,
    pub username: Option<String>,
    pub first_run: bool,
    pub sync_status: String,
    pub last_error: Option<String>,
    pub home_sections: Vec<HomeSection>,
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
    pub artists: Vec<Artist>,
    pub album_artists: Vec<Artist>,
    pub genres: Vec<Genre>,
    pub playlists: Vec<Playlist>,
    pub favorites: Vec<Track>,
    pub search: SearchResults,
}

#[derive(Clone, Debug)]
pub struct PlaybackSnapshot {
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LyricsSearchResult {
    pub id: u64,
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
            current: None,
            state: PlaybackState::Stopped,
            position_seconds: 0,
            position_millis: 0,
            duration_seconds: 0,
            volume: 1.0,
            muted: false,
            repeat_mode: RepeatMode::Off,
            shuffle_enabled: false,
            auto_dj_enabled: false,
            buffering_percent: None,
            last_error: None,
        }
    }
}

impl LibrarySnapshot {
    fn first_run() -> Self {
        Self {
            server: None,
            username: None,
            first_run: true,
            sync_status: "Add a Jellyfin server to start.".to_string(),
            last_error: None,
            home_sections: Vec::new(),
            albums: Vec::new(),
            tracks: Vec::new(),
            artists: Vec::new(),
            album_artists: Vec::new(),
            genres: Vec::new(),
            playlists: Vec::new(),
            favorites: Vec::new(),
            search: SearchResults::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum ControllerEvent {
    Snapshot(Box<LibrarySnapshot>),
    FavoriteChanged {
        item_id: FavoriteItemId,
        favorite: bool,
        snapshot: Box<LibrarySnapshot>,
    },
    Queue(Box<Option<QueueSnapshot>>),
    Playback(Box<PlaybackSnapshot>),
    Lyrics(Box<Option<Lyrics>>),
    LyricsSearchResults {
        track_id: TrackId,
        query: String,
        results: Vec<LyricsSearchResult>,
    },
    LyricsSaved {
        path: PathBuf,
        lyrics: Lyrics,
    },
    CoverReady {
        key: String,
        path: PathBuf,
    },
    LoginStatus(String),
    Error(String),
}

#[derive(Clone)]
pub struct AppController {
    store: StoreHandle,
    runtime: Arc<Runtime>,
    secrets: Arc<dyn SecretStore>,
    queue: Arc<Mutex<Option<QueueEngine>>>,
    playback: Arc<Mutex<Box<dyn PlaybackBackend>>>,
    playback_snapshot: Arc<Mutex<PlaybackSnapshot>>,
    pending_seek: Arc<Mutex<Option<PendingSeek>>>,
    auto_dj_enabled: Arc<Mutex<bool>>,
    last_progress_snapshot: Arc<Mutex<Option<(ServerId, u32)>>>,
    last_report_snapshot: Arc<Mutex<Option<(TrackId, u32)>>>,
    events: Sender<ControllerEvent>,
    sync_in_flight: Arc<Mutex<HashSet<ServerId>>>,
    home_refresh_in_flight: Arc<Mutex<HashSet<ServerId>>>,
    cover_in_flight: Arc<Mutex<HashSet<String>>>,
    cover_slots: Arc<(Mutex<usize>, Condvar)>,
}

struct HomeRefreshContext {
    store: StoreHandle,
    runtime: Arc<Runtime>,
    secrets: Arc<dyn SecretStore>,
    events: Sender<ControllerEvent>,
    sync_in_flight: Arc<Mutex<HashSet<ServerId>>>,
    home_refresh_in_flight: Arc<Mutex<HashSet<ServerId>>>,
}

#[derive(Clone, Copy, Debug)]
struct PendingSeek {
    target_millis: u64,
    expires_at: Instant,
}

#[derive(Clone)]
enum StoreHandle {
    Path(PathBuf),
    Memory(Arc<Mutex<Store>>),
}

impl StoreHandle {
    fn open_for_app() -> Result<Self, String> {
        let path = data_dir()
            .map(|dir| dir.join("rufin.sqlite"))
            .unwrap_or_else(|| PathBuf::from("rufin.sqlite"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        Store::open(&path).map_err(|error| error.to_string())?;
        Ok(Self::Path(path))
    }

    fn open_memory() -> Result<Self, String> {
        Store::open_memory()
            .map(|store| Self::Memory(Arc::new(Mutex::new(store))))
            .map_err(|error| error.to_string())
    }

    fn with_store<T>(
        &self,
        operation: impl FnOnce(&Store) -> Result<T, StoreError>,
    ) -> Result<T, String> {
        match self {
            Self::Path(path) => {
                let store = Store::open(path).map_err(|error| error.to_string())?;
                operation(&store).map_err(|error| error.to_string())
            }
            Self::Memory(store) => {
                let store = store
                    .lock()
                    .map_err(|_| "store lock was poisoned".to_string())?;
                operation(&store).map_err(|error| error.to_string())
            }
        }
    }
}

impl AppController {
    pub fn load_settings(&self) -> AppSettings {
        load_settings_from_store(&self.store)
    }

    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), String> {
        self.store.with_store(|store| store.save_settings(settings))
    }

    pub fn cached_album_detail(
        &self,
        album_id: &AlbumId,
    ) -> Result<Option<(Album, Vec<Track>)>, String> {
        let Some(server) = self
            .store
            .with_store(|store| store.active_server())?
            .map(|saved| saved.server)
        else {
            return Ok(None);
        };
        self.store
            .with_store(|store| store.load_album_detail(&server.id, album_id))
    }

    pub fn cached_artist_detail(
        &self,
        artist_id: &ArtistId,
    ) -> Result<Option<CachedArtistDetail>, String> {
        let Some(server) = self
            .store
            .with_store(|store| store.active_server())?
            .map(|saved| saved.server)
        else {
            return Ok(None);
        };
        self.store
            .with_store(|store| store.load_artist_detail(&server.id, artist_id))
    }

    pub fn cached_playlist_detail(
        &self,
        playlist_id: &PlaylistId,
    ) -> Result<Option<rufin_provider::PlaylistDetail>, String> {
        let Some(server) = self
            .store
            .with_store(|store| store.active_server())?
            .map(|saved| saved.server)
        else {
            return Ok(None);
        };
        self.store
            .with_store(|store| store.load_playlist_detail(&server.id, playlist_id))
    }

    pub fn cached_genre_detail(
        &self,
        genre_id: &GenreId,
    ) -> Result<Option<CachedGenreDetail>, String> {
        let Some(server) = self
            .store
            .with_store(|store| store.active_server())?
            .map(|saved| saved.server)
        else {
            return Ok(None);
        };
        self.store
            .with_store(|store| store.load_genre_detail(&server.id, genre_id))
    }

    pub fn cached_albums_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Album>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        self.store
            .with_store(|store| store.load_albums(&saved.server.id, offset, limit))
    }

    pub fn cached_tracks_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Track>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        self.store
            .with_store(|store| store.load_tracks(&saved.server.id, offset, limit))
    }

    pub fn cached_artists_page(
        &self,
        album_artist: bool,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Artist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        self.store
            .with_store(|store| store.load_artists(&saved.server.id, album_artist, offset, limit))
    }

    pub fn cached_genres_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Genre>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        self.store
            .with_store(|store| store.load_genres(&saved.server.id, offset, limit))
    }

    pub fn cached_playlists_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Playlist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        self.store
            .with_store(|store| store.load_playlists(&saved.server.id, offset, limit))
    }

    #[cfg(test)]
    pub fn cover_key(&self, image_ref: &ImageRef, size: u32) -> Option<String> {
        let server = self
            .store
            .with_store(|store| store.active_server())
            .ok()
            .flatten()?
            .server;
        Some(image_cache_key(
            &server.id,
            &image_ref.item_id,
            image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED),
            size,
        ))
    }

    #[cfg(test)]
    pub fn cached_cover_path(&self, image_ref: &ImageRef, size: u32) -> Option<PathBuf> {
        let saved = self
            .store
            .with_store(|store| store.active_server())
            .ok()
            .flatten()?;
        let tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
        let entry = self
            .store
            .with_store(|store| {
                store.load_cover_cache_entry(&saved.server.id, &image_ref.item_id, tag, size)
            })
            .ok()
            .flatten()?;
        let path = PathBuf::from(entry.path);
        if path.exists() {
            return Some(path);
        }
        let _invalidated = self.store.with_store(|store| {
            store.delete_cover_cache_entry(&saved.server.id, &image_ref.item_id, tag, size)
        });
        None
    }

    pub fn cached_cover_path_for_key(&self, key: &str) -> Option<PathBuf> {
        cached_cover_path_for_key(key)
    }

    #[cfg(test)]
    pub fn request_cover(&self, image_ref: ImageRef, size: u32) {
        let Some(saved) = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None)
        else {
            return;
        };
        if saved.server.provider == "fake" {
            return;
        }
        if let Some(path) = self.cached_cover_path(&image_ref, size) {
            if let Some(key) = self.cover_key(&image_ref, size) {
                let _sent = self.events.send(ControllerEvent::CoverReady { key, path });
            }
            return;
        }
        let tag = image_ref
            .tag
            .clone()
            .unwrap_or_else(|| IMAGE_TAG_UNTAGGED.to_string());
        let key = image_cache_key(&saved.server.id, &image_ref.item_id, &tag, size);
        match self.cover_in_flight.lock() {
            Ok(mut in_flight) => {
                if !in_flight.insert(key.clone()) {
                    return;
                }
            }
            Err(_) => return,
        }

        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        let cover_in_flight = Arc::clone(&self.cover_in_flight);
        let cover_slots = Arc::clone(&self.cover_slots);
        thread::spawn(move || {
            if !acquire_cover_slot(&cover_slots) {
                if let Ok(mut in_flight) = cover_in_flight.lock() {
                    in_flight.remove(&key);
                }
                return;
            }
            let result = fetch_and_cache_cover(&store, &runtime, &secrets, &saved, image_ref, size);
            release_cover_slot(&cover_slots);
            if let Ok(mut in_flight) = cover_in_flight.lock() {
                in_flight.remove(&key);
            }
            match result {
                Ok(path) => {
                    let _sent = events.send(ControllerEvent::CoverReady { key, path });
                }
                Err(error) => {
                    warn!(%error, "failed to fetch cover");
                }
            }
        });
    }

    pub fn request_cover_for_key(&self, key: String, image_ref: ImageRef, size: u32) {
        match self.cover_in_flight.lock() {
            Ok(mut in_flight) => {
                if !in_flight.insert(key.clone()) {
                    return;
                }
            }
            Err(_) => return,
        }

        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        let cover_in_flight = Arc::clone(&self.cover_in_flight);
        let cover_slots = Arc::clone(&self.cover_slots);
        thread::spawn(move || {
            let result = (|| -> Result<Option<PathBuf>, String> {
                if let Some(path) = cached_cover_path_for_key(&key) {
                    return Ok(Some(path));
                }

                let Some(saved) = store.with_store(|store| store.active_server())? else {
                    return Ok(None);
                };
                if saved.server.provider == "fake" {
                    return Ok(None);
                }

                let tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
                let expected_key = image_cache_key(&saved.server.id, &image_ref.item_id, tag, size);
                if expected_key != key {
                    return Ok(None);
                }

                if let Some(path) = cached_cover_path_for_saved(&store, &saved, &image_ref, size)? {
                    return Ok(Some(path));
                }

                if !acquire_cover_slot(&cover_slots) {
                    return Ok(None);
                }
                let result =
                    fetch_and_cache_cover(&store, &runtime, &secrets, &saved, image_ref, size)
                        .map(Some);
                release_cover_slot(&cover_slots);
                result
            })();

            if let Ok(mut in_flight) = cover_in_flight.lock() {
                in_flight.remove(&key);
            }
            match result {
                Ok(Some(path)) => {
                    let _sent = events.send(ControllerEvent::CoverReady { key, path });
                }
                Ok(None) => {}
                Err(error) => {
                    if is_provider_not_found_error(&error) {
                        debug!(%error, "cached cover source item is no longer available");
                    } else {
                        warn!(%error, "failed to prepare cover");
                    }
                }
            }
        });
    }

    pub fn bootstrap(
        fake_scale: Option<FakeScale>,
    ) -> (
        Self,
        Receiver<ControllerEvent>,
        LibrarySnapshot,
        Option<QueueSnapshot>,
        PlaybackSnapshot,
    ) {
        let (events, receiver) = channel();
        let runtime = Runtime::new()
            .map(Arc::new)
            .unwrap_or_else(|error| panic!("failed to create Tokio runtime: {error}"));

        if let Some(scale) = fake_scale {
            let store = StoreHandle::open_memory()
                .unwrap_or_else(|error| panic!("failed to open fake memory store: {error}"));
            seed_fake_cache(&store, scale)
                .unwrap_or_else(|error| panic!("failed to seed fake cache: {error}"));
            let snapshot = load_snapshot(&store).unwrap_or_else(|error| {
                warn!(%error, "failed to load fake snapshot");
                LibrarySnapshot::first_run()
            });
            let settings = load_settings_from_store(&store);
            let queue = restore_queue(&store, snapshot.server.as_ref());
            let queue_snapshot = queue.as_ref().map(QueueEngine::snapshot);
            let playback_snapshot =
                playback_snapshot_from_queue(queue.as_ref(), settings.auto_dj_enabled);
            let controller = Self {
                store,
                runtime,
                secrets: Arc::new(MemorySecretStore::new()),
                queue: Arc::new(Mutex::new(queue)),
                playback: Arc::new(Mutex::new(Box::new(FakePlaybackBackend::new()))),
                playback_snapshot: Arc::new(Mutex::new(playback_snapshot.clone())),
                pending_seek: Arc::new(Mutex::new(None)),
                auto_dj_enabled: Arc::new(Mutex::new(settings.auto_dj_enabled)),
                last_progress_snapshot: Arc::new(Mutex::new(None)),
                last_report_snapshot: Arc::new(Mutex::new(None)),
                events,
                sync_in_flight: Arc::new(Mutex::new(HashSet::new())),
                home_refresh_in_flight: Arc::new(Mutex::new(HashSet::new())),
                cover_in_flight: Arc::new(Mutex::new(HashSet::new())),
                cover_slots: Arc::new((Mutex::new(0), Condvar::new())),
            };
            return (
                controller,
                receiver,
                snapshot,
                queue_snapshot,
                playback_snapshot,
            );
        }

        let store = StoreHandle::open_for_app().unwrap_or_else(|error| {
            warn!(%error, "failed to open app store, falling back to memory");
            StoreHandle::open_memory().unwrap_or_else(|memory_error| {
                panic!("failed to open memory store: {memory_error}")
            })
        });
        let snapshot = load_snapshot(&store).unwrap_or_else(|error| {
            warn!(%error, "failed to load app snapshot");
            LibrarySnapshot::first_run()
        });
        let settings = load_settings_from_store(&store);
        let queue = restore_queue(&store, snapshot.server.as_ref());
        let queue_snapshot = queue.as_ref().map(QueueEngine::snapshot);
        let playback_snapshot =
            playback_snapshot_from_queue(queue.as_ref(), settings.auto_dj_enabled);
        let controller = Self {
            store,
            runtime,
            secrets: Arc::new(SecretServiceStore::new()),
            queue: Arc::new(Mutex::new(queue)),
            playback: Arc::new(Mutex::new(playback_backend(false))),
            playback_snapshot: Arc::new(Mutex::new(playback_snapshot.clone())),
            pending_seek: Arc::new(Mutex::new(None)),
            auto_dj_enabled: Arc::new(Mutex::new(settings.auto_dj_enabled)),
            last_progress_snapshot: Arc::new(Mutex::new(None)),
            last_report_snapshot: Arc::new(Mutex::new(None)),
            events,
            sync_in_flight: Arc::new(Mutex::new(HashSet::new())),
            home_refresh_in_flight: Arc::new(Mutex::new(HashSet::new())),
            cover_in_flight: Arc::new(Mutex::new(HashSet::new())),
            cover_slots: Arc::new((Mutex::new(0), Condvar::new())),
        };
        (
            controller,
            receiver,
            snapshot,
            queue_snapshot,
            playback_snapshot,
        )
    }

    #[cfg(test)]
    fn bootstrap_memory_for_test() -> (
        Self,
        Receiver<ControllerEvent>,
        LibrarySnapshot,
        Option<QueueSnapshot>,
        PlaybackSnapshot,
    ) {
        let (events, receiver) = channel();
        let runtime = Runtime::new()
            .map(Arc::new)
            .unwrap_or_else(|error| panic!("failed to create Tokio runtime: {error}"));
        let store = StoreHandle::open_memory()
            .unwrap_or_else(|error| panic!("failed to open memory store: {error}"));
        let snapshot = load_snapshot(&store).unwrap_or_else(|error| {
            panic!("failed to load memory snapshot: {error}");
        });
        let settings = load_settings_from_store(&store);
        let controller = Self {
            store,
            runtime,
            secrets: Arc::new(MemorySecretStore::new()),
            queue: Arc::new(Mutex::new(None)),
            playback: Arc::new(Mutex::new(Box::new(FakePlaybackBackend::new()))),
            playback_snapshot: Arc::new(Mutex::new(PlaybackSnapshot {
                auto_dj_enabled: settings.auto_dj_enabled,
                ..PlaybackSnapshot::default()
            })),
            pending_seek: Arc::new(Mutex::new(None)),
            auto_dj_enabled: Arc::new(Mutex::new(settings.auto_dj_enabled)),
            last_progress_snapshot: Arc::new(Mutex::new(None)),
            last_report_snapshot: Arc::new(Mutex::new(None)),
            events,
            sync_in_flight: Arc::new(Mutex::new(HashSet::new())),
            home_refresh_in_flight: Arc::new(Mutex::new(HashSet::new())),
            cover_in_flight: Arc::new(Mutex::new(HashSet::new())),
            cover_slots: Arc::new((Mutex::new(0), Condvar::new())),
        };
        (
            controller,
            receiver,
            snapshot,
            None,
            PlaybackSnapshot {
                auto_dj_enabled: settings.auto_dj_enabled,
                ..PlaybackSnapshot::default()
            },
        )
    }

    pub fn clear_active_server_cache_for_app() -> Result<(), String> {
        let store = StoreHandle::open_for_app()?;
        let Some(saved) = store.with_store(|store| store.active_server())? else {
            return Err("No active server is saved.".to_string());
        };
        store.with_store(|store| {
            store.clear_library_cache(&saved.server.id)?;
            Ok(())
        })?;
        clear_disk_cover_cache(&saved.server.id)?;
        Ok(())
    }

    pub fn forget_active_server_for_app() -> Result<(), String> {
        let store = StoreHandle::open_for_app()?;
        let Some(saved) = store.with_store(|store| store.active_server())? else {
            return Err("No active server is saved.".to_string());
        };
        SecretServiceStore::new()
            .delete_token(&saved.server.id)
            .map_err(|error| error.to_string())?;
        store.with_store(|store| {
            store.forget_server(&saved.server.id)?;
            Ok(())
        })?;
        clear_disk_cover_cache(&saved.server.id)?;
        Ok(())
    }

    pub fn start_background_sync_for_active(&self) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_sync(saved);
        }
    }

    pub fn resync_active_server(&self) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_sync(saved);
        } else {
            let _sent = self.events.send(ControllerEvent::Error(
                "No active Jellyfin server is saved.".to_string(),
            ));
        }
    }

    pub fn refresh_home_sections_for_active(&self) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_home_refresh_for_saved(saved, None);
        }
    }

    pub fn refresh_home_section_for_active(&self, kind: HomeSectionKind) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_home_refresh_for_saved(saved, Some(kind));
        }
    }

    fn start_home_refresh_for_saved(
        &self,
        saved: SavedServer,
        section_kind: Option<HomeSectionKind>,
    ) {
        start_home_refresh_thread(
            HomeRefreshContext {
                store: self.store.clone(),
                runtime: Arc::clone(&self.runtime),
                secrets: Arc::clone(&self.secrets),
                events: self.events.clone(),
                sync_in_flight: Arc::clone(&self.sync_in_flight),
                home_refresh_in_flight: Arc::clone(&self.home_refresh_in_flight),
            },
            saved,
            section_kind,
        );
    }

    pub fn startup_sync_delay_ms(&self) -> Option<u64> {
        let saved = self
            .store
            .with_store(|store| store.active_server())
            .ok()
            .flatten()?;
        let albums = self
            .store
            .with_store(|store| {
                store
                    .load_albums(&saved.server.id, 0, 1)
                    .map(|page| page.total)
            })
            .unwrap_or(0);
        let tracks = self
            .store
            .with_store(|store| {
                store
                    .load_tracks(&saved.server.id, 0, 1)
                    .map(|page| page.total)
            })
            .unwrap_or(0);
        if albums == 0 && tracks == 0 {
            return Some(500);
        }
        let sync_state = self
            .store
            .with_store(|store| store.sync_state(&saved.server.id))
            .ok();
        if sync_state
            .as_ref()
            .is_some_and(|state| state.status == "error")
        {
            return Some(8_000);
        }
        let age = self
            .store
            .with_store(|store| store.sync_completed_age_seconds(&saved.server.id))
            .ok()
            .flatten();
        match age {
            Some(seconds) if seconds < STARTUP_CACHE_STALE_SECONDS => None,
            _ => Some(8_000),
        }
    }

    pub fn play_tracks_now(&self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            let _sent = self.events.send(ControllerEvent::Error(
                "No tracks are available to play.".to_string(),
            ));
            return;
        }

        let result = self.with_queue_mut(|queue| {
            queue.clear();
            let mut tracks = tracks.into_iter();
            if let Some(first) = tracks.next() {
                queue.play_now(&first);
            }
            for track in tracks {
                queue.append(&track);
            }
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.auto_dj_top_up_or_emit_error();
        self.persist_and_emit_queue();
        self.start_current_track();
    }

    pub fn play_now(&self, track: Track) {
        self.play_tracks_now(vec![track]);
    }

    pub fn play_album_now(&self, album_id: AlbumId) {
        match self.cached_album_detail(&album_id) {
            Ok(Some((_, tracks))) => self.play_tracks_now(tracks),
            Ok(None) => {
                let _sent = self.events.send(ControllerEvent::Error(
                    "The selected cached album was not found.".to_string(),
                ));
            }
            Err(error) => {
                let _sent = self.events.send(ControllerEvent::Error(error));
            }
        }
    }

    pub fn play_next(&self, track: Track) {
        let result = self.with_queue_mut(|queue| {
            queue.play_next(&track);
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.persist_and_emit_queue();
    }

    pub fn remove_from_queue(&self, entry_id: QueueEntryId) {
        let mut removed_current = false;
        let mut has_current_after_remove = false;
        let result = self.with_queue_mut(|queue| {
            let current_id = queue.current().map(|entry| entry.id.clone());
            let removed = queue.remove(&entry_id).is_some();
            removed_current = removed && current_id.as_ref() == Some(&entry_id);
            has_current_after_remove = queue.current().is_some();
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        if removed_current && !has_current_after_remove {
            let _result = self.send_playback_command(PlaybackCommand::Stop);
        }
        self.persist_and_emit_queue();
        if removed_current && has_current_after_remove {
            self.start_current_track();
        }
    }

    pub fn activate_queue_entry(&self, entry_id: QueueEntryId) {
        let result = self.with_queue_mut(|queue| {
            if queue.activate(&entry_id) {
                Ok(())
            } else {
                Err("The selected queue entry was not found.".to_string())
            }
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.auto_dj_top_up_or_emit_error();
        self.persist_and_emit_queue();
        self.start_current_track();
    }

    pub fn move_queue_entry_after_current(&self, entry_id: QueueEntryId) {
        let result = self.with_queue_mut(|queue| {
            if queue.move_after_current(&entry_id) {
                Ok(())
            } else {
                Err("The selected queue entry was not found.".to_string())
            }
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.persist_and_emit_queue();
    }

    pub fn clear_queue(&self) {
        let result = self.with_queue_mut(|queue| {
            queue.clear();
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        let _result = self.send_playback_command(PlaybackCommand::Stop);
        self.persist_and_emit_queue();
    }

    pub fn toggle_shuffle(&self) {
        let result = self.with_queue_mut(|queue| {
            let enabled = !queue.shuffle().enabled;
            let seed = if enabled {
                shuffle_seed()
            } else {
                queue.shuffle().seed
            };
            queue.set_shuffle(enabled, seed);
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.persist_and_emit_queue();
    }

    pub fn cycle_repeat(&self) {
        let result = self.with_queue_mut(|queue| {
            let next = match queue.repeat_mode() {
                RepeatMode::Off => RepeatMode::All,
                RepeatMode::All => RepeatMode::One,
                RepeatMode::One => RepeatMode::Off,
            };
            queue.set_repeat_mode(next);
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.persist_and_emit_queue();
    }

    pub fn toggle_auto_dj(&self) {
        let enabled = self
            .auto_dj_enabled
            .lock()
            .map(|mut current| {
                *current = !*current;
                *current
            })
            .unwrap_or(false);

        let mut settings = self.load_settings();
        settings.auto_dj_enabled = enabled;
        if let Err(error) = self.save_settings(&settings) {
            let _sent = self.events.send(ControllerEvent::Error(error));
        }

        self.update_playback_snapshot(|snapshot| {
            snapshot.auto_dj_enabled = enabled;
        });
        if enabled && self.auto_dj_top_up_or_emit_error() {
            self.persist_and_emit_queue();
        } else {
            self.emit_playback_snapshot();
        }
    }

    pub fn play_pause(&self) {
        let state = self
            .playback_snapshot
            .lock()
            .map(|snapshot| snapshot.state)
            .unwrap_or(PlaybackState::Stopped);
        match state {
            PlaybackState::Playing | PlaybackState::Buffering => {
                if let Err(error) = self.send_playback_command(PlaybackCommand::Pause) {
                    let _sent = self.events.send(ControllerEvent::Error(error));
                } else {
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.state = PlaybackState::Paused;
                        snapshot.buffering_percent = None;
                    });
                    self.persist_current_queue_snapshot();
                    self.emit_playback_snapshot();
                    self.report_playback(PlaybackReportKind::Progress, false);
                }
            }
            PlaybackState::Paused => {
                if let Err(error) = self.send_playback_command(PlaybackCommand::Resume) {
                    let _sent = self.events.send(ControllerEvent::Error(error));
                } else {
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.state = PlaybackState::Playing;
                        snapshot.buffering_percent = None;
                    });
                    self.emit_playback_snapshot();
                    self.report_playback(PlaybackReportKind::Progress, false);
                }
            }
            PlaybackState::Stopped => self.start_current_track(),
        }
    }

    pub fn stop(&self) {
        self.report_playback(PlaybackReportKind::Stopped, false);
        let _result = self.with_queue_mut(|queue| {
            queue.set_progress_seconds(0);
            Ok(())
        });
        if let Err(error) = self.send_playback_command(PlaybackCommand::Stop) {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.update_playback_snapshot(|snapshot| {
            snapshot.state = PlaybackState::Stopped;
            snapshot.position_seconds = 0;
            snapshot.position_millis = 0;
            snapshot.buffering_percent = None;
        });
        self.persist_and_emit_queue();
    }

    pub fn next_track(&self) {
        self.auto_dj_top_up_or_emit_error();
        let mut moved = false;
        let mut had_current = false;
        let result = self.with_queue_mut(|queue| {
            had_current = queue.current().is_some();
            moved = queue.next_track().is_some();
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        if !moved {
            if had_current {
                self.seek(0);
            } else {
                self.stop();
            }
            return;
        }
        self.persist_and_emit_queue();
        self.start_current_track();
    }

    pub fn previous_track(&self) {
        let should_restart_current = self
            .playback_snapshot
            .lock()
            .map(|snapshot| snapshot.position_seconds > 10)
            .unwrap_or(false);
        if should_restart_current {
            self.seek(0);
            return;
        }

        let mut moved = false;
        let result = self.with_queue_mut(|queue| {
            moved = queue.previous_track().is_some();
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        if !moved {
            self.seek(0);
            return;
        }
        self.auto_dj_top_up_or_emit_error();
        self.persist_and_emit_queue();
        self.start_current_track();
    }

    pub fn seek(&self, seconds: u32) {
        self.seek_millis(u64::from(seconds) * 1_000);
    }

    pub fn seek_millis(&self, millis: u64) {
        let seconds = (millis / 1_000).min(u64::from(u32::MAX)) as u32;
        let _result = self.with_queue_mut(|queue| {
            queue.set_progress_seconds(seconds);
            Ok(())
        });
        if let Err(error) = self.send_playback_command(PlaybackCommand::SeekMillis(millis)) {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.remember_pending_seek(millis);
        let queue_snapshot = self.queue_snapshot();
        if let Some(snapshot) = &queue_snapshot {
            self.persist_queue_snapshot(snapshot);
        }
        self.sync_playback_snapshot_from_queue();
        self.update_playback_snapshot(|snapshot| {
            snapshot.position_seconds = seconds;
            snapshot.position_millis = millis;
        });
        self.emit_playback_snapshot();
    }

    pub fn set_volume(&self, volume: f64) {
        let volume = volume.clamp(0.0, 1.0);
        if let Err(error) = self.send_playback_command(PlaybackCommand::SetVolume(volume)) {
            let _sent = self.events.send(ControllerEvent::Error(error));
        } else {
            self.update_playback_snapshot(|snapshot| {
                snapshot.volume = volume;
            });
            self.emit_playback_snapshot();
        }
    }

    pub fn toggle_mute(&self) {
        let muted = self
            .playback_snapshot
            .lock()
            .map(|snapshot| !snapshot.muted)
            .unwrap_or(true);
        if let Err(error) = self.send_playback_command(PlaybackCommand::SetMuted(muted)) {
            let _sent = self.events.send(ControllerEvent::Error(error));
        } else {
            self.update_playback_snapshot(|snapshot| {
                snapshot.muted = muted;
            });
            self.emit_playback_snapshot();
        }
    }

    pub fn poll_playback_events(&self) {
        let events = self
            .playback
            .lock()
            .map(|mut playback| playback.drain_events())
            .unwrap_or_default();
        if events.is_empty() {
            return;
        }

        for event in events {
            match event {
                PlaybackEvent::StateChanged(state) => {
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.state = state;
                        snapshot.buffering_percent = None;
                    });
                }
                PlaybackEvent::PositionChanged { seconds, millis } => {
                    if self.should_ignore_position_after_seek(millis) {
                        continue;
                    }
                    let _result = self.with_queue_mut(|queue| {
                        queue.set_progress_seconds(seconds);
                        Ok(())
                    });
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.position_seconds = seconds;
                        snapshot.position_millis = millis;
                    });
                    self.persist_progress_if_needed(seconds);
                    self.report_playback_progress_if_needed(seconds);
                }
                PlaybackEvent::DurationChanged(seconds) => {
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.duration_seconds = seconds;
                    });
                }
                PlaybackEvent::Buffering(percent) => {
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.state = PlaybackState::Buffering;
                        snapshot.buffering_percent = Some(percent);
                    });
                }
                PlaybackEvent::EndOfStream => self.advance_after_end_of_stream(),
                PlaybackEvent::VolumeChanged { volume, muted } => {
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.volume = volume;
                        snapshot.muted = muted;
                    });
                }
                PlaybackEvent::Error(error) => {
                    self.report_playback(PlaybackReportKind::Stopped, true);
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.last_error = Some(error.clone());
                        snapshot.state = PlaybackState::Stopped;
                    });
                    let _sent = self.events.send(ControllerEvent::Error(error));
                }
            }
        }
        self.emit_playback_snapshot();
    }

    pub fn clear_active_server_cache(&self) {
        let store = self.store.clone();
        let events = self.events.clone();
        let sync_in_flight = Arc::clone(&self.sync_in_flight);
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_server())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active Jellyfin server is saved.".to_string(),
                ));
                return;
            };
            if sync_is_running(&sync_in_flight, &saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(
                    "Wait for the current library sync to finish before clearing cache."
                        .to_string(),
                ));
                return;
            }
            let result = store.with_store(|store| {
                store.clear_library_cache(&saved.server.id)?;
                Ok(())
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Err(error) = clear_disk_cover_cache(&saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            let _sent = events.send(ControllerEvent::LoginStatus(
                "Cached library cleared.".to_string(),
            ));
            match load_snapshot(&store) {
                Ok(snapshot) => {
                    let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }
        });
    }

    pub fn forget_active_server(&self) {
        let store = self.store.clone();
        let events = self.events.clone();
        let secrets = Arc::clone(&self.secrets);
        let queue = Arc::clone(&self.queue);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        let sync_in_flight = Arc::clone(&self.sync_in_flight);
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_server())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Snapshot(Box::new(
                    LibrarySnapshot::first_run(),
                )));
                return;
            };
            if sync_is_running(&sync_in_flight, &saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(
                    "Wait for the current library sync to finish before forgetting the server."
                        .to_string(),
                ));
                return;
            }
            if let Err(error) = secrets.delete_token(&saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(error.to_string()));
                return;
            }
            let result = store.with_store(|store| {
                store.forget_server(&saved.server.id)?;
                Ok(())
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Err(error) = clear_disk_cover_cache(&saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Ok(mut queue) = queue.lock() {
                *queue = None;
            }
            if let Ok(mut playback) = playback.lock() {
                let _result = playback.send(PlaybackCommand::Stop);
            }
            if let Ok(mut snapshot) = playback_snapshot.lock() {
                *snapshot = PlaybackSnapshot {
                    auto_dj_enabled: auto_dj_enabled
                        .lock()
                        .map(|enabled| *enabled)
                        .unwrap_or_default(),
                    ..PlaybackSnapshot::default()
                };
            }
            let _sent = events.send(ControllerEvent::Queue(Box::new(None)));
            let _sent = events.send(ControllerEvent::Playback(Box::new(PlaybackSnapshot {
                auto_dj_enabled: auto_dj_enabled
                    .lock()
                    .map(|enabled| *enabled)
                    .unwrap_or_default(),
                ..PlaybackSnapshot::default()
            })));
            let _sent = events.send(ControllerEvent::Snapshot(Box::new(
                LibrarySnapshot::first_run(),
            )));
        });
    }

    #[instrument(skip(self, password), fields(server_url = %server_url, username = %username, trust_invalid_cert = trust_invalid_cert))]
    pub fn login(
        &self,
        server_url: String,
        username: String,
        password: String,
        trust_invalid_cert: bool,
    ) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        let queue = Arc::clone(&self.queue);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        let sync_in_flight = Arc::clone(&self.sync_in_flight);
        thread::spawn(move || {
            let _sent = events.send(ControllerEvent::LoginStatus(
                "Checking Jellyfin server...".to_string(),
            ));
            let result = runtime.block_on(JellyfinProvider::login(LoginRequest {
                base_url: server_url,
                username,
                password,
                trust_invalid_cert,
            }));

            let session = match result {
                Ok(session) => session,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error.to_string()));
                    return;
                }
            };

            let saved = SavedServer {
                server: session.server.clone(),
                user_id: session.user_id.clone(),
                username: session.username.clone(),
                trust_invalid_cert,
            };
            if let Err(error) = store.with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&saved.server.id)?;
                Ok(())
            }) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Err(error) = secrets.save_token(&saved.server.id, &session.access_token) {
                let _sent = events.send(ControllerEvent::Error(error.to_string()));
                return;
            }
            let queue_snapshot = QueueEngine::new(saved.server.id.clone()).snapshot();
            if let Ok(mut queue) = queue.lock() {
                *queue = Some(QueueEngine::restore(queue_snapshot.clone()));
            }
            let auto_dj_enabled = auto_dj_enabled
                .lock()
                .map(|enabled| *enabled)
                .unwrap_or_default();
            let player = playback_snapshot_from_queue(
                Some(&QueueEngine::restore(queue_snapshot.clone())),
                auto_dj_enabled,
            );
            if let Ok(mut snapshot) = playback_snapshot.lock() {
                *snapshot = player.clone();
            }
            let _sent = events.send(ControllerEvent::Queue(Box::new(Some(queue_snapshot))));
            let _sent = events.send(ControllerEvent::Playback(Box::new(player)));

            let _sent = events.send(ControllerEvent::LoginStatus(
                "Connected. Loading cached library...".to_string(),
            ));
            match load_snapshot(&store) {
                Ok(snapshot) => {
                    let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }

            start_sync_thread(store, runtime, secrets, events, sync_in_flight, saved);
        });
    }

    pub fn search(&self, query: String) {
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            let mut snapshot = match load_snapshot(&store) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            if let Some(server) = &snapshot.server {
                match store.with_store(|store| store.search_library(&server.id, &query, 50)) {
                    Ok(results) => snapshot.search = results,
                    Err(error) => {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                }
            }
            let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
        });
    }

    pub fn set_album_favorite(&self, album_id: AlbumId, favorite: bool) {
        self.set_favorite(FavoriteItemId::Album(album_id), favorite);
    }

    pub fn set_track_favorite(&self, track_id: TrackId, favorite: bool) {
        self.set_favorite(FavoriteItemId::Track(track_id), favorite);
    }

    pub fn set_artist_favorite(&self, artist_id: ArtistId, favorite: bool) {
        self.set_favorite(FavoriteItemId::Artist(artist_id), favorite);
    }

    pub fn toggle_current_favorite(&self) {
        let Some(entry) = self
            .playback_snapshot
            .lock()
            .ok()
            .and_then(|snapshot| snapshot.current.clone())
        else {
            let _sent = self
                .events
                .send(ControllerEvent::Error("No track is playing.".to_string()));
            return;
        };
        self.set_favorite(FavoriteItemId::Track(entry.track_id), !entry.favorite);
    }

    fn set_favorite(&self, item_id: FavoriteItemId, favorite: bool) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        let queue = Arc::clone(&self.queue);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_server())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active Jellyfin server is saved.".to_string(),
                ));
                return;
            };

            if saved.server.provider != "fake" {
                let result =
                    provider_for_saved(&store, &runtime, &secrets, &saved).and_then(|provider| {
                        runtime
                            .block_on(provider.set_favorite(item_id.clone(), favorite))
                            .map_err(|error| error.to_string())
                    });
                if let Err(error) = result {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            }

            let result = store.with_store(|store| {
                match &item_id {
                    FavoriteItemId::Album(album_id) => {
                        store.set_album_favorite(&saved.server.id, album_id, favorite)?;
                    }
                    FavoriteItemId::Track(track_id) => {
                        store.set_track_favorite(&saved.server.id, track_id, favorite)?;
                    }
                    FavoriteItemId::Artist(artist_id) => {
                        store.set_artist_favorite(&saved.server.id, artist_id, favorite)?;
                    }
                }
                Ok(())
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }

            if let FavoriteItemId::Track(track_id) = &item_id {
                if let Ok(mut queue) = queue.lock()
                    && let Some(queue) = queue.as_mut()
                {
                    queue.set_track_favorite(track_id, favorite);
                    let snapshot = queue.snapshot();
                    let _saved = store.with_store(|store| store.save_queue_snapshot(&snapshot));
                    let _sent = events.send(ControllerEvent::Queue(Box::new(Some(snapshot))));
                }
                if let Ok(mut snapshot) = playback_snapshot.lock()
                    && let Some(current) = snapshot.current.as_mut()
                    && current.track_id == *track_id
                {
                    current.favorite = favorite;
                    let _sent = events.send(ControllerEvent::Playback(Box::new(snapshot.clone())));
                }
            }

            match load_snapshot(&store) {
                Ok(snapshot) => {
                    let _sent = events.send(ControllerEvent::FavoriteChanged {
                        item_id,
                        favorite,
                        snapshot: Box::new(snapshot),
                    });
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }
        });
    }

    pub fn create_playlist(&self, name: String, tracks: Vec<Track>) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_server())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active Jellyfin server is saved.".to_string(),
                ));
                return;
            };
            let track_ids = tracks
                .iter()
                .map(|track| track.id.clone())
                .collect::<Vec<_>>();
            let playlist_id = if saved.server.provider == "fake" {
                PlaylistId::new(format!(
                    "fake:playlist:{}",
                    unique_millis().unwrap_or(tracks.len() as u128)
                ))
            } else {
                match provider_for_saved(&store, &runtime, &secrets, &saved).and_then(|provider| {
                    runtime
                        .block_on(provider.create_playlist(&name, &track_ids))
                        .map_err(|error| error.to_string())
                }) {
                    Ok(playlist_id) => playlist_id,
                    Err(error) => {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                }
            };
            let playlist = Playlist {
                id: playlist_id.clone(),
                name: name.trim().to_string(),
                track_count: tracks.len() as u32,
                duration_seconds: tracks.iter().map(|track| track.duration_seconds).sum(),
                image_ref: tracks.iter().find_map(|track| track.image_ref.clone()),
            };
            let entries = playlist_entries_for_tracks(&playlist_id, &tracks);
            let result = store.with_store(|store| {
                store.upsert_playlists(&saved.server.id, &[playlist], 0)?;
                store.upsert_tracks(&saved.server.id, &tracks, 0)?;
                store.upsert_playlist_entries(&saved.server.id, &playlist_id, &entries, 0)?;
                Ok(())
            });
            emit_snapshot_result(&store, &events, result);
        });
    }

    pub fn rename_playlist(&self, playlist_id: PlaylistId, name: String) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_server())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active Jellyfin server is saved.".to_string(),
                ));
                return;
            };
            if saved.server.provider != "fake" {
                let result =
                    provider_for_saved(&store, &runtime, &secrets, &saved).and_then(|provider| {
                        runtime
                            .block_on(provider.rename_playlist(&playlist_id, &name))
                            .map_err(|error| error.to_string())
                    });
                if let Err(error) = result {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            }
            let result = store.with_store(|store| {
                store.rename_playlist(&saved.server.id, &playlist_id, name.trim())?;
                Ok(())
            });
            emit_snapshot_result(&store, &events, result);
        });
    }

    pub fn add_tracks_to_playlist(&self, playlist_id: PlaylistId, tracks: Vec<Track>) {
        self.mutate_playlist_entries(playlist_id, move |mut detail| {
            let mut entries = detail.entries;
            entries.extend(playlist_entries_for_tracks(&detail.playlist.id, &tracks));
            detail.tracks.extend(tracks);
            detail.entries = entries;
            detail
        });
    }

    pub fn remove_playlist_entry(&self, playlist_id: PlaylistId, entry_id: String) {
        self.mutate_playlist_entries(playlist_id, move |mut detail| {
            detail.entries.retain(|entry| entry.entry_id != entry_id);
            detail.tracks = detail
                .entries
                .iter()
                .map(|entry| entry.track.clone())
                .collect();
            detail
        });
    }

    pub fn move_playlist_entry(&self, playlist_id: PlaylistId, entry_id: String, new_index: usize) {
        self.mutate_playlist_entries(playlist_id, move |mut detail| {
            if let Some(old_index) = detail
                .entries
                .iter()
                .position(|entry| entry.entry_id == entry_id)
            {
                let entry = detail.entries.remove(old_index);
                detail
                    .entries
                    .insert(new_index.min(detail.entries.len()), entry);
                detail.tracks = detail
                    .entries
                    .iter()
                    .map(|entry| entry.track.clone())
                    .collect();
            }
            detail
        });
    }

    fn mutate_playlist_entries(
        &self,
        playlist_id: PlaylistId,
        mutate: impl FnOnce(rufin_provider::PlaylistDetail) -> rufin_provider::PlaylistDetail
        + Send
        + 'static,
    ) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_server())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active Jellyfin server is saved.".to_string(),
                ));
                return;
            };
            let before = match store
                .with_store(|store| store.load_playlist_detail(&saved.server.id, &playlist_id))
            {
                Ok(Some(detail)) => detail,
                Ok(None) => {
                    let _sent = events.send(ControllerEvent::Error(
                        "The selected cached playlist was not found.".to_string(),
                    ));
                    return;
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            let mut after = mutate(before.clone());
            if saved.server.provider != "fake" {
                match sync_playlist_mutation(&store, &runtime, &secrets, &saved, &before, &after) {
                    Ok(Some(fresh)) => after = fresh,
                    Ok(None) => {}
                    Err(error) => {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                }
            }
            let playlist = Playlist {
                track_count: after.entries.len() as u32,
                duration_seconds: after
                    .entries
                    .iter()
                    .map(|entry| entry.track.duration_seconds)
                    .sum(),
                image_ref: after
                    .entries
                    .iter()
                    .find_map(|entry| entry.track.image_ref.clone())
                    .or(after.playlist.image_ref.clone()),
                ..after.playlist.clone()
            };
            let result = store.with_store(|store| {
                store.upsert_playlists(&saved.server.id, &[playlist], 0)?;
                store.upsert_tracks(&saved.server.id, &after.tracks, 0)?;
                store.upsert_playlist_entries(
                    &saved.server.id,
                    &after.playlist.id,
                    &after.entries,
                    0,
                )?;
                Ok(())
            });
            emit_snapshot_result(&store, &events, result);
        });
    }

    pub fn request_lyrics_for_current(&self) {
        self.request_lyrics_for_current_with_cache(true);
    }

    pub fn refresh_lyrics_for_current(&self) {
        self.request_lyrics_for_current_with_cache(false);
    }

    fn request_lyrics_for_current_with_cache(&self, use_cache: bool) {
        let Some((server_id, entry, _position)) = self.current_queue_entry() else {
            debug!("lyrics request skipped because the queue has no current track");
            let _sent = self.events.send(ControllerEvent::Lyrics(Box::new(None)));
            return;
        };
        let settings = self
            .store
            .with_store(|store| store.load_settings())
            .unwrap_or_else(|_| AppSettings::default());
        let search = lyrics_search_for_settings(&settings);
        let cached = use_cache.then(|| {
            self.store
                .with_store(|store| store.load_lyrics(&server_id, &entry.track_id))
                .unwrap_or(None)
        });
        if let Some(cached) = cached
            .flatten()
            .filter(|lyrics| cached_lyrics_allowed(lyrics, search))
        {
            debug!(track_id = %entry.track_id, "loaded lyrics from cache");
            let _sent = self
                .events
                .send(ControllerEvent::Lyrics(Box::new(Some(cached))));
            return;
        }
        let allow_remote = matches!(
            search,
            JellyfinLyricsSearch::ServerThenRemote | JellyfinLyricsSearch::RemoteThenServer
        );
        debug!(track_id = %entry.track_id, allow_remote, ?search, "requesting lyrics from provider");
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_server())
                .unwrap_or(None)
                .filter(|saved| saved.server.id == server_id)
            else {
                let _sent = events.send(ControllerEvent::Lyrics(Box::new(None)));
                return;
            };
            if saved.server.provider == "fake" {
                let _sent = events.send(ControllerEvent::Lyrics(Box::new(None)));
                return;
            }
            let result =
                provider_for_saved(&store, &runtime, &secrets, &saved).and_then(|provider| {
                    runtime
                        .block_on(provider.lyrics_with_search(&entry.track_id, search))
                        .map_err(|error| error.to_string())
                });
            match result {
                Ok(Some(lyrics)) => {
                    debug!(track_id = %entry.track_id, source = ?lyrics.source, "loaded lyrics from provider");
                    let _saved = store.with_store(|store| store.save_lyrics(&server_id, &lyrics));
                    let _sent = events.send(ControllerEvent::Lyrics(Box::new(Some(lyrics))));
                }
                Ok(None) => {
                    debug!(track_id = %entry.track_id, allow_remote, "provider returned no lyrics");
                    let _sent = events.send(ControllerEvent::Lyrics(Box::new(None)));
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    let _sent = events.send(ControllerEvent::Lyrics(Box::new(None)));
                }
            }
        });
    }

    pub fn search_lyrics_for_current(&self, query: String) {
        let query = query.trim().to_string();
        if query.is_empty() {
            return;
        }
        let Some((_server_id, entry, _position)) = self.current_queue_entry() else {
            let _sent = self
                .events
                .send(ControllerEvent::Error("No track is playing.".to_string()));
            return;
        };
        let track_id = entry.track_id.clone();
        let events = self.events.clone();
        thread::spawn(move || match lrclib_search(&query) {
            Ok(results) => {
                let _sent = events.send(ControllerEvent::LyricsSearchResults {
                    track_id,
                    query,
                    results,
                });
            }
            Err(error) => {
                let _sent = events.send(ControllerEvent::Error(error));
                let _sent = events.send(ControllerEvent::LyricsSearchResults {
                    track_id,
                    query,
                    results: Vec::new(),
                });
            }
        });
    }

    pub fn save_lyrics_search_result(&self, track_id: TrackId, result: LyricsSearchResult) {
        let Some((server_id, entry, _position)) = self.current_queue_entry() else {
            let _sent = self
                .events
                .send(ControllerEvent::Error("No track is playing.".to_string()));
            return;
        };
        if entry.track_id != track_id {
            let _sent = self.events.send(ControllerEvent::Error(
                "The playing track changed before lyrics were saved.".to_string(),
            ));
            return;
        }

        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(
            move || match save_lrclib_result(&server_id, &entry, &result) {
                Ok((path, lyrics)) => {
                    let _saved = store.with_store(|store| store.save_lyrics(&server_id, &lyrics));
                    let _sent = events.send(ControllerEvent::LyricsSaved { path, lyrics });
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            },
        );
    }

    fn with_queue_mut<T>(
        &self,
        operation: impl FnOnce(&mut QueueEngine) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| "queue lock was poisoned".to_string())?;
        let Some(queue) = queue.as_mut() else {
            return Err("No active queue is available.".to_string());
        };
        operation(queue)
    }

    fn auto_dj_top_up_or_emit_error(&self) -> bool {
        match self.auto_dj_top_up() {
            Ok(topped_up) => topped_up,
            Err(error) => {
                let _sent = self.events.send(ControllerEvent::Error(error));
                false
            }
        }
    }

    fn auto_dj_top_up(&self) -> Result<bool, String> {
        if !self
            .auto_dj_enabled
            .lock()
            .map(|enabled| *enabled)
            .unwrap_or_default()
        {
            return Ok(false);
        }

        let Some((server_id, current, queued_track_ids, remaining)) = self.auto_dj_queue_state()
        else {
            return Ok(false);
        };
        if remaining >= AUTO_DJ_THRESHOLD {
            return Ok(false);
        }

        let tracks = self
            .store
            .with_store(|store| store.load_tracks(&server_id, 0, AUTO_DJ_LIBRARY_LIMIT))
            .map(|page| page.items)?;
        let candidates = auto_dj_candidates(&tracks, &current, &queued_track_ids, shuffle_seed());
        if candidates.is_empty() {
            return Ok(false);
        }

        self.with_queue_mut(|queue| {
            for track in &candidates {
                queue.append(track);
            }
            Ok(())
        })?;
        Ok(true)
    }

    fn auto_dj_queue_state(&self) -> Option<(ServerId, QueueEntry, HashSet<TrackId>, usize)> {
        self.queue.lock().ok().and_then(|queue| {
            let queue = queue.as_ref()?;
            let snapshot = queue.snapshot();
            let current = queue.current()?.clone();
            let queued = queue
                .entries()
                .iter()
                .map(|entry| entry.track_id.clone())
                .collect::<HashSet<_>>();
            Some((
                snapshot.server_id,
                current,
                queued,
                queue.remaining_after_current(),
            ))
        })
    }

    fn persist_and_emit_queue(&self) {
        let queue_snapshot = self.queue_snapshot();
        if let Some(snapshot) = &queue_snapshot {
            self.persist_queue_snapshot(snapshot);
        }
        self.sync_playback_snapshot_from_queue();
        let _sent = self
            .events
            .send(ControllerEvent::Queue(Box::new(queue_snapshot)));
        self.emit_playback_snapshot();
    }

    fn persist_current_queue_snapshot(&self) {
        if let Some(snapshot) = self.queue_snapshot() {
            self.persist_queue_snapshot(&snapshot);
        }
    }

    fn persist_queue_snapshot(&self, snapshot: &QueueSnapshot) {
        if let Err(error) = self
            .store
            .with_store(|store| store.save_queue_snapshot(snapshot))
        {
            let _sent = self.events.send(ControllerEvent::Error(error));
        }
    }

    fn queue_snapshot(&self) -> Option<QueueSnapshot> {
        self.queue
            .lock()
            .ok()
            .and_then(|queue| queue.as_ref().map(QueueEngine::snapshot))
    }

    fn update_playback_snapshot(&self, operation: impl FnOnce(&mut PlaybackSnapshot)) {
        if let Ok(mut snapshot) = self.playback_snapshot.lock() {
            operation(&mut snapshot);
        }
    }

    fn remember_pending_seek(&self, target_millis: u64) {
        if let Ok(mut pending_seek) = self.pending_seek.lock() {
            *pending_seek = Some(PendingSeek {
                target_millis,
                expires_at: Instant::now() + SEEK_SETTLE_WINDOW,
            });
        }
    }

    fn should_ignore_position_after_seek(&self, millis: u64) -> bool {
        let now = Instant::now();
        let Ok(mut pending_seek) = self.pending_seek.lock() else {
            return false;
        };
        let Some(pending) = *pending_seek else {
            return false;
        };

        if now >= pending.expires_at {
            *pending_seek = None;
            return false;
        }

        seek_position_is_stale(pending, millis, now)
    }

    fn sync_playback_snapshot_from_queue(&self) {
        let queue = self.queue.lock().ok();
        let queue = queue.as_ref().and_then(|queue| queue.as_ref());
        self.update_playback_snapshot(|snapshot| {
            snapshot.current = queue.and_then(|queue| queue.current().cloned());
            snapshot.position_seconds = queue.map(QueueEngine::progress_seconds).unwrap_or(0);
            snapshot.position_millis = u64::from(snapshot.position_seconds) * 1_000;
            snapshot.duration_seconds = snapshot
                .current
                .as_ref()
                .map(|entry| entry.duration_seconds)
                .unwrap_or(0);
            snapshot.repeat_mode = queue
                .map(QueueEngine::repeat_mode)
                .unwrap_or(RepeatMode::Off);
            snapshot.shuffle_enabled = queue
                .map(|queue| queue.shuffle().enabled)
                .unwrap_or_default();
            snapshot.auto_dj_enabled = self
                .auto_dj_enabled
                .lock()
                .map(|enabled| *enabled)
                .unwrap_or_default();
            if snapshot.current.is_none() {
                snapshot.state = PlaybackState::Stopped;
                snapshot.last_error = None;
                snapshot.buffering_percent = None;
            }
        });
    }

    fn emit_playback_snapshot(&self) {
        let snapshot = self
            .playback_snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_default();
        let _sent = self
            .events
            .send(ControllerEvent::Playback(Box::new(snapshot)));
    }

    fn send_playback_command(&self, command: PlaybackCommand) -> Result<(), String> {
        self.playback
            .lock()
            .map_err(|_| "playback lock was poisoned".to_string())?
            .send(command)
            .map_err(|error| error.to_string())
    }

    fn start_current_track(&self) {
        let Some((server_id, entry, position_seconds)) = self.current_queue_entry() else {
            let _sent = self
                .events
                .send(ControllerEvent::Error("Queue is empty.".to_string()));
            return;
        };
        self.update_playback_snapshot(|snapshot| {
            snapshot.current = Some(entry.clone());
            snapshot.state = PlaybackState::Buffering;
            snapshot.position_seconds = position_seconds;
            snapshot.position_millis = u64::from(position_seconds) * 1_000;
            snapshot.duration_seconds = entry.duration_seconds;
            snapshot.last_error = None;
        });
        self.emit_playback_snapshot();
        self.report_playback(PlaybackReportKind::Started, false);

        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let events = self.events.clone();
        thread::spawn(move || {
            let stream =
                match resolve_stream(&store, &runtime, &secrets, &server_id, &entry.track_id) {
                    Ok(stream) => stream,
                    Err(error) => {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                };
            let command = PlaybackCommand::Play {
                track: playback_track_from_entry(&entry),
                stream,
                start_position_seconds: position_seconds,
            };
            if let Err(error) = playback
                .lock()
                .map_err(|_| "playback lock was poisoned".to_string())
                .and_then(|mut playback| playback.send(command).map_err(|error| error.to_string()))
            {
                if let Ok(mut snapshot) = playback_snapshot.lock() {
                    snapshot.state = PlaybackState::Stopped;
                    snapshot.last_error = Some(error.clone());
                }
                let _sent = events.send(ControllerEvent::Error(error));
            }
        });
    }

    fn current_queue_entry(&self) -> Option<(ServerId, QueueEntry, u32)> {
        self.queue.lock().ok().and_then(|queue| {
            let queue = queue.as_ref()?;
            let snapshot = queue.snapshot();
            let entry = queue.current()?.clone();
            Some((snapshot.server_id, entry, snapshot.progress_seconds))
        })
    }

    fn persist_progress_if_needed(&self, seconds: u32) {
        let Some(snapshot) = self.queue_snapshot() else {
            return;
        };
        let bucket = seconds / 10;
        let should_save = self
            .last_progress_snapshot
            .lock()
            .map(|mut last| {
                let changed = last.as_ref() != Some(&(snapshot.server_id.clone(), bucket));
                if changed {
                    *last = Some((snapshot.server_id.clone(), bucket));
                }
                changed
            })
            .unwrap_or(false);
        if should_save {
            let _result = self
                .store
                .with_store(|store| store.save_queue_snapshot(&snapshot));
        }
    }

    fn report_playback_progress_if_needed(&self, seconds: u32) {
        let Some(current) = self
            .playback_snapshot
            .lock()
            .ok()
            .and_then(|snapshot| snapshot.current.clone())
        else {
            return;
        };
        let bucket = seconds / 10;
        let should_report = self
            .last_report_snapshot
            .lock()
            .map(|mut last| {
                let changed = last.as_ref() != Some(&(current.track_id.clone(), bucket));
                if changed {
                    *last = Some((current.track_id.clone(), bucket));
                }
                changed
            })
            .unwrap_or(false);
        if should_report {
            self.report_playback(PlaybackReportKind::Progress, false);
        }
    }

    fn report_playback(&self, kind: PlaybackReportKind, failed: bool) {
        let snapshot = self
            .playback_snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_default();
        let Some(current) = snapshot.current.clone() else {
            return;
        };
        let Some((server_id, _, _)) = self.current_queue_entry() else {
            return;
        };
        let settings = self.load_settings();
        if settings.private_mode {
            return;
        }
        let report = PlaybackReport {
            kind,
            track_id: current.track_id,
            position_seconds: snapshot.position_seconds,
            paused: snapshot.state == PlaybackState::Paused,
            muted: snapshot.muted,
            volume_percent: (snapshot.volume.clamp(0.0, 1.0) * 100.0).round() as u8,
            shuffle: snapshot.shuffle_enabled,
            repeat_one: snapshot.repeat_mode == RepeatMode::One,
            repeat_all: snapshot.repeat_mode == RepeatMode::All,
            failed,
        };
        report_playback_async(
            self.store.clone(),
            Arc::clone(&self.runtime),
            Arc::clone(&self.secrets),
            self.events.clone(),
            server_id,
            report,
        );
    }

    fn advance_after_end_of_stream(&self) {
        self.report_playback(PlaybackReportKind::Stopped, false);
        self.auto_dj_top_up_or_emit_error();
        let mut has_next = false;
        let result = self.with_queue_mut(|queue| {
            has_next = queue.advance_after_end_of_stream().is_some();
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        if has_next {
            self.persist_and_emit_queue();
            self.start_current_track();
        } else {
            self.stop();
        }
    }

    fn start_sync(&self, saved: SavedServer) {
        start_sync_thread(
            self.store.clone(),
            Arc::clone(&self.runtime),
            Arc::clone(&self.secrets),
            self.events.clone(),
            Arc::clone(&self.sync_in_flight),
            saved,
        );
    }
}

fn start_sync_thread(
    store: StoreHandle,
    runtime: Arc<Runtime>,
    secrets: Arc<dyn SecretStore>,
    events: Sender<ControllerEvent>,
    sync_in_flight: Arc<Mutex<HashSet<ServerId>>>,
    saved: SavedServer,
) {
    let server_id = saved.server.id.clone();
    match sync_in_flight.lock() {
        Ok(mut running) => {
            if !running.insert(server_id.clone()) {
                let _sent = events.send(ControllerEvent::LoginStatus(
                    "Sync already running.".to_string(),
                ));
                return;
            }
        }
        Err(_) => {
            let _sent = events.send(ControllerEvent::Error(
                "Sync guard lock was poisoned.".to_string(),
            ));
            return;
        }
    }

    thread::spawn(move || {
        let _sent = events.send(ControllerEvent::LoginStatus(
            "Syncing Jellyfin library...".to_string(),
        ));
        let sync_result = run_sync_job(&store, &runtime, &secrets, &saved);
        if let Ok(mut running) = sync_in_flight.lock() {
            running.remove(&server_id);
        }
        match sync_result {
            Ok(()) => {
                let _sent = events.send(ControllerEvent::LoginStatus(
                    "Library sync complete.".to_string(),
                ));
                match load_snapshot(&store) {
                    Ok(snapshot) => {
                        let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
                    }
                    Err(error) => {
                        let _sent = events.send(ControllerEvent::Error(error));
                    }
                }
            }
            Err(error) => {
                let _failed = store.with_store(|store| {
                    store.fail_sync(&saved.server.id, &error)?;
                    Ok(())
                });
                let _sent = events.send(ControllerEvent::Error(error));
            }
        }
    });
}

fn start_home_refresh_thread(
    context: HomeRefreshContext,
    saved: SavedServer,
    section_kind: Option<HomeSectionKind>,
) {
    if saved.server.provider == "fake" {
        return;
    }

    let server_id = saved.server.id.clone();
    if sync_is_running(&context.sync_in_flight, &server_id) {
        return;
    }
    match context.home_refresh_in_flight.lock() {
        Ok(mut running) => {
            if !running.insert(server_id.clone()) {
                return;
            }
        }
        Err(_) => {
            let _sent = context.events.send(ControllerEvent::Error(
                "Home refresh guard lock was poisoned.".to_string(),
            ));
            return;
        }
    }

    thread::spawn(move || {
        let result = match section_kind {
            Some(kind) => refresh_home_section_for_saved(
                &context.store,
                &context.runtime,
                &context.secrets,
                &saved,
                kind,
            ),
            None => refresh_home_sections_for_saved(
                &context.store,
                &context.runtime,
                &context.secrets,
                &saved,
            ),
        }
        .and_then(|()| load_snapshot(&context.store).map(Box::new));
        if let Ok(mut running) = context.home_refresh_in_flight.lock() {
            running.remove(&server_id);
        }
        match result {
            Ok(snapshot) => {
                let _sent = context.events.send(ControllerEvent::Snapshot(snapshot));
            }
            Err(error) => {
                warn!(%error, "failed to refresh home sections");
            }
        }
    });
}

fn run_sync_job(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
) -> Result<(), String> {
    let token = secrets
        .load_token(&saved.server.id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "No saved token found for the active server.".to_string())?;
    let session = SavedProviderSession {
        server: saved.server.clone(),
        user_id: saved.user_id.clone(),
        username: saved.username.clone(),
        trust_invalid_cert: saved.trust_invalid_cert,
        access_token: token,
    };
    let provider =
        JellyfinProvider::from_saved_session(session).map_err(|error| error.to_string())?;
    runtime.block_on(sync_provider(store, &saved.server.id, &provider))
}

fn refresh_home_sections_for_saved(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
) -> Result<(), String> {
    let provider = provider_for_saved(store, runtime, secrets, saved)?;
    runtime.block_on(refresh_home_sections(store, &saved.server.id, &provider))
}

fn refresh_home_section_for_saved(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
    kind: HomeSectionKind,
) -> Result<(), String> {
    let provider = provider_for_saved(store, runtime, secrets, saved)?;
    runtime.block_on(refresh_home_section(
        store,
        &saved.server.id,
        &provider,
        kind,
    ))
}

#[instrument(skip(store, provider), fields(server_id = %server_id.as_str()))]
async fn sync_provider(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &impl MusicProvider,
) -> Result<(), String> {
    let generation = store.with_store(|store| store.begin_sync(server_id))?;
    info!(generation, "started Jellyfin cache sync");
    sync_album_pages(store, server_id, provider, generation).await?;
    sync_track_pages(store, server_id, provider, generation).await?;
    sync_artist_pages(store, server_id, provider, generation, false).await?;
    sync_artist_pages(store, server_id, provider, generation, true).await?;
    sync_genre_pages(store, server_id, provider, generation).await?;
    sync_playlist_pages(store, server_id, provider, generation).await?;
    sync_home_sections(store, server_id, provider, generation).await?;
    store.with_store(|store| store.refresh_library_counts(server_id))?;
    store.with_store(|store| store.complete_sync(server_id, generation))?;
    info!(generation, "completed Jellyfin cache sync");
    Ok(())
}

async fn sync_album_pages(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &impl MusicProvider,
    generation: i64,
) -> Result<(), String> {
    let mut offset = 0;
    loop {
        let page = provider
            .albums(PagedRequest::new(offset, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        store.with_store(|store| store.upsert_albums(server_id, &page.items, generation))?;
        let item_count = page.items.len();
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(());
        }
    }
}

async fn sync_track_pages(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &impl MusicProvider,
    generation: i64,
) -> Result<(), String> {
    let mut offset = 0;
    loop {
        let page = provider
            .tracks(PagedRequest::new(offset, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        store.with_store(|store| store.upsert_tracks(server_id, &page.items, generation))?;
        let item_count = page.items.len();
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(());
        }
    }
}

async fn sync_artist_pages(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &impl MusicProvider,
    generation: i64,
    album_artist: bool,
) -> Result<(), String> {
    let mut offset = 0;
    loop {
        let page = if album_artist {
            provider
                .album_artists(PagedRequest::new(offset, PAGE_SIZE))
                .await
        } else {
            provider.artists(PagedRequest::new(offset, PAGE_SIZE)).await
        }
        .map_err(|error| error.to_string())?;
        store.with_store(|store| {
            store.upsert_artists(server_id, &page.items, album_artist, generation)
        })?;
        let item_count = page.items.len();
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(());
        }
    }
}

async fn sync_genre_pages(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &impl MusicProvider,
    generation: i64,
) -> Result<(), String> {
    let mut offset = 0;
    loop {
        let page = provider
            .genres(PagedRequest::new(offset, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        store.with_store(|store| store.upsert_genres(server_id, &page.items, generation))?;
        let item_count = page.items.len();
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(());
        }
    }
}

async fn sync_playlist_pages(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &impl MusicProvider,
    generation: i64,
) -> Result<(), String> {
    let mut offset = 0;
    loop {
        let page = provider
            .playlists(PagedRequest::new(offset, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        store.with_store(|store| store.upsert_playlists(server_id, &page.items, generation))?;
        for playlist in &page.items {
            let detail = provider
                .playlist_detail(&playlist.id)
                .await
                .map_err(|error| error.to_string())?;
            store.with_store(|store| {
                store.upsert_tracks(server_id, &detail.tracks, generation)?;
                store.upsert_playlist_entries(
                    server_id,
                    &detail.playlist.id,
                    &detail.entries,
                    generation,
                )?;
                Ok(())
            })?;
        }
        let item_count = page.items.len();
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(());
        }
    }
}

async fn sync_home_sections(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &impl MusicProvider,
    generation: i64,
) -> Result<(), String> {
    let sections = provider
        .home_sections()
        .await
        .map_err(|error| error.to_string())?;
    cache_home_sections(store, server_id, &sections, generation)
}

async fn refresh_home_sections(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &impl MusicProvider,
) -> Result<(), String> {
    let generation =
        store.with_store(|store| store.sync_state(server_id).map(|state| state.generation))?;
    let sections = provider
        .home_sections()
        .await
        .map_err(|error| error.to_string())?;
    cache_home_sections(store, server_id, &sections, generation)
}

async fn refresh_home_section(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &impl MusicProvider,
    kind: HomeSectionKind,
) -> Result<(), String> {
    let generation =
        store.with_store(|store| store.sync_state(server_id).map(|state| state.generation))?;
    let section = provider
        .home_section(kind)
        .await
        .map_err(|error| error.to_string())?;
    cache_home_section(store, server_id, &section, generation)
}

fn cache_home_sections(
    store: &StoreHandle,
    server_id: &ServerId,
    sections: &[HomeSection],
    generation: i64,
) -> Result<(), String> {
    for section in sections {
        cache_home_section_items(store, server_id, section, generation)?;
    }
    store.with_store(|store| store.upsert_home_sections(server_id, sections, generation))?;
    Ok(())
}

fn cache_home_section(
    store: &StoreHandle,
    server_id: &ServerId,
    section: &HomeSection,
    generation: i64,
) -> Result<(), String> {
    cache_home_section_items(store, server_id, section, generation)?;
    store.with_store(|store| store.upsert_home_section(server_id, section, generation))?;
    Ok(())
}

fn cache_home_section_items(
    store: &StoreHandle,
    server_id: &ServerId,
    section: &HomeSection,
    generation: i64,
) -> Result<(), String> {
    if !section.albums.is_empty() {
        store.with_store(|store| store.upsert_albums(server_id, &section.albums, generation))?;
    }
    if !section.tracks.is_empty() {
        store.with_store(|store| store.upsert_tracks(server_id, &section.tracks, generation))?;
    }
    Ok(())
}

fn sync_page_finished(item_count: usize, total: usize, offset: usize) -> bool {
    item_count == 0 || (total > 0 && offset >= total) || (total == 0 && item_count < PAGE_SIZE)
}

fn load_snapshot(store: &StoreHandle) -> Result<LibrarySnapshot, String> {
    let Some(saved) = store.with_store(|store| store.active_server())? else {
        return Ok(LibrarySnapshot::first_run());
    };
    let sync_state = store
        .with_store(|store| store.sync_state(&saved.server.id))
        .ok();
    let home_sections = store.with_store(|store| store.load_home_sections(&saved.server.id))?;
    let albums = store.with_store(|store| {
        store
            .load_albums(&saved.server.id, 0, 500)
            .map(|page| page.items)
    })?;
    let tracks = store.with_store(|store| {
        store
            .load_tracks(&saved.server.id, 0, 1_000)
            .map(|page| page.items)
    })?;
    let artists = store.with_store(|store| {
        store
            .load_artists(&saved.server.id, false, 0, 500)
            .map(|page| page.items)
    })?;
    let album_artists = store.with_store(|store| {
        store
            .load_artists(&saved.server.id, true, 0, 500)
            .map(|page| page.items)
    })?;
    let genres = store.with_store(|store| {
        store
            .load_genres(&saved.server.id, 0, 500)
            .map(|page| page.items)
    })?;
    let playlists = store.with_store(|store| {
        store
            .load_playlists(&saved.server.id, 0, 500)
            .map(|page| page.items)
    })?;
    let favorites = store.with_store(|store| store.load_favorite_tracks(&saved.server.id))?;
    let status = sync_state
        .as_ref()
        .map(|state| match state.status.as_str() {
            "running" => "Syncing library...".to_string(),
            "error" => "Sync needs attention.".to_string(),
            _ => "Cached library ready.".to_string(),
        })
        .unwrap_or_else(|| "Cached library ready.".to_string());
    let last_error = sync_state.and_then(|state| state.last_error);

    Ok(LibrarySnapshot {
        server: Some(saved.server),
        username: Some(saved.username),
        first_run: false,
        sync_status: status,
        last_error,
        home_sections,
        albums,
        tracks,
        artists,
        album_artists,
        genres,
        playlists,
        favorites,
        search: SearchResults::default(),
    })
}

fn seed_fake_cache(store: &StoreHandle, scale: FakeScale) -> Result<(), String> {
    let provider = FakeProvider::new(scale);
    let server = provider.identity().server.clone();
    let saved = SavedServer {
        server: server.clone(),
        user_id: "fake-user".to_string(),
        username: "fake".to_string(),
        trust_invalid_cert: false,
    };
    store.with_store(|store| {
        store.save_server(&saved)?;
        store.set_active_server(&server.id)?;
        Ok(())
    })?;
    let generation = store.with_store(|store| store.begin_sync(&server.id))?;

    let runtime = Runtime::new().map_err(|error| error.to_string())?;
    let album_limit = match scale {
        FakeScale::Small => provider.album_count(),
        FakeScale::Large => 1_000,
    };
    let track_limit = match scale {
        FakeScale::Small => provider.track_count(),
        FakeScale::Large => 2_000,
    };
    runtime.block_on(async {
        let albums = provider
            .albums(PagedRequest::new(0, album_limit))
            .await
            .map_err(|error| error.to_string())?;
        let tracks = provider
            .tracks(PagedRequest::new(0, track_limit))
            .await
            .map_err(|error| error.to_string())?;
        let artists = provider
            .artists(PagedRequest::new(0, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        let album_artists = provider
            .album_artists(PagedRequest::new(0, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        let genres = provider
            .genres(PagedRequest::new(0, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        let playlists = provider
            .playlists(PagedRequest::new(0, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        let home_sections = provider
            .home_sections()
            .await
            .map_err(|error| error.to_string())?;

        store.with_store(|store| {
            store.upsert_albums(&server.id, &albums.items, generation)?;
            store.upsert_tracks(&server.id, &tracks.items, generation)?;
            store.upsert_artists(&server.id, &artists.items, false, generation)?;
            store.upsert_artists(&server.id, &album_artists.items, true, generation)?;
            store.refresh_library_counts(&server.id)?;
            store.upsert_genres(&server.id, &genres.items, generation)?;
            store.upsert_playlists(&server.id, &playlists.items, generation)?;
            store.upsert_home_sections(&server.id, &home_sections, generation)?;
            store.complete_sync(&server.id, generation)?;
            Ok(())
        })
    })?;
    Ok(())
}

fn restore_queue(store: &StoreHandle, server: Option<&ServerIdentity>) -> Option<QueueEngine> {
    let server = server?;
    match store.with_store(|store| store.load_queue_snapshot(&server.id)) {
        Ok(Some(snapshot)) => Some(QueueEngine::restore(snapshot)),
        Ok(None) => Some(QueueEngine::new(server.id.clone())),
        Err(error) => {
            warn!(%error, "failed to restore queue snapshot");
            Some(QueueEngine::new(server.id.clone()))
        }
    }
}

fn load_settings_from_store(store: &StoreHandle) -> AppSettings {
    let mut settings = store
        .with_store(|store| store.load_settings())
        .unwrap_or_default();
    settings.migrate_defaults();
    settings
}

fn playback_snapshot_from_queue(
    queue: Option<&QueueEngine>,
    auto_dj_enabled: bool,
) -> PlaybackSnapshot {
    queue
        .map(|queue| PlaybackSnapshot {
            current: queue.current().cloned(),
            state: PlaybackState::Stopped,
            position_seconds: queue.progress_seconds(),
            position_millis: u64::from(queue.progress_seconds()) * 1_000,
            duration_seconds: queue
                .current()
                .map(|entry| entry.duration_seconds)
                .unwrap_or_default(),
            volume: 1.0,
            muted: false,
            repeat_mode: queue.repeat_mode(),
            shuffle_enabled: queue.shuffle().enabled,
            auto_dj_enabled,
            buffering_percent: None,
            last_error: None,
        })
        .unwrap_or_else(|| PlaybackSnapshot {
            auto_dj_enabled,
            ..PlaybackSnapshot::default()
        })
}

fn seek_position_is_stale(pending: PendingSeek, millis: u64, now: Instant) -> bool {
    now < pending.expires_at && !seek_position_matches_target(pending.target_millis, millis)
}

fn seek_position_matches_target(target_millis: u64, millis: u64) -> bool {
    let lower = target_millis.saturating_sub(SEEK_POSITION_TOLERANCE_MILLIS);
    let upper = target_millis.saturating_add(SEEK_POSITION_TOLERANCE_MILLIS);
    (lower..=upper).contains(&millis)
}

fn shuffle_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(1)
}

fn auto_dj_candidates(
    tracks: &[Track],
    current: &QueueEntry,
    queued_track_ids: &HashSet<TrackId>,
    seed: u64,
) -> Vec<Track> {
    let current_genres = tracks
        .iter()
        .find(|track| track.id == current.track_id)
        .map(|track| {
            track
                .genres
                .iter()
                .map(|genre| genre.to_lowercase())
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();

    let mut candidates = tracks
        .iter()
        .filter(|track| !queued_track_ids.contains(&track.id))
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by_key(|track| {
        (
            std::cmp::Reverse(auto_dj_score(track, current, &current_genres)),
            auto_dj_shuffle_key(seed, track.id.as_str()),
        )
    });
    candidates.truncate(AUTO_DJ_ITEM_COUNT);
    candidates
}

fn auto_dj_score(track: &Track, current: &QueueEntry, current_genres: &HashSet<String>) -> u8 {
    let mut score = 0;
    if !current_genres.is_empty()
        && track
            .genres
            .iter()
            .any(|genre| current_genres.contains(&genre.to_lowercase()))
    {
        score += 80;
    }
    if current
        .artist_id
        .as_ref()
        .is_some_and(|artist_id| track.artist_id.as_ref() == Some(artist_id))
    {
        score += 60;
    } else if !current.artist.trim().is_empty()
        && track.artist.eq_ignore_ascii_case(current.artist.as_str())
    {
        score += 50;
    }
    if current
        .album_id
        .as_ref()
        .is_some_and(|album_id| track.album_id == *album_id)
    {
        score += 25;
    }
    score
}

fn auto_dj_shuffle_key(seed: u64, value: &str) -> u64 {
    value
        .bytes()
        .fold(seed ^ 0xa24b_aed4_963e_e407, |hash, byte| {
            hash.rotate_left(7) ^ u64::from(byte)
        })
}

fn playback_backend(fake: bool) -> Box<dyn PlaybackBackend> {
    if fake {
        return Box::new(FakePlaybackBackend::new());
    }
    Box::new(LazyGStreamerPlaybackBackend::new())
}

fn playback_track_from_entry(entry: &QueueEntry) -> PlaybackTrack {
    PlaybackTrack {
        id: entry.track_id.clone(),
        title: entry.title.clone(),
        artist: entry.artist.clone(),
        album: entry.album.clone(),
        duration_seconds: entry.duration_seconds,
    }
}

fn resolve_stream(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    server_id: &ServerId,
    track_id: &TrackId,
) -> Result<StreamDescriptor, String> {
    let saved = store
        .with_store(|store| store.active_server())?
        .filter(|saved| saved.server.id == *server_id)
        .ok_or_else(|| "No matching active server is saved.".to_string())?;
    if saved.server.provider == "fake" {
        return Ok(StreamDescriptor::new(format!(
            "fake://local/stream/{}",
            track_id.as_str()
        )));
    }

    let token = secrets
        .load_token(&saved.server.id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "No saved token found for the active server.".to_string())?;
    let session = SavedProviderSession {
        server: saved.server.clone(),
        user_id: saved.user_id.clone(),
        username: saved.username.clone(),
        trust_invalid_cert: saved.trust_invalid_cert,
        access_token: token,
    };
    let provider =
        JellyfinProvider::from_saved_session(session).map_err(|error| error.to_string())?;
    runtime
        .block_on(provider.stream(track_id))
        .map_err(|error| error.to_string())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrcLibLyricsDto {
    id: u64,
    track_name: String,
    artist_name: String,
    #[serde(default)]
    album_name: Option<String>,
    #[serde(default)]
    duration: Option<u32>,
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

impl From<LrcLibLyricsDto> for LyricsSearchResult {
    fn from(value: LrcLibLyricsDto) -> Self {
        Self {
            id: value.id,
            track_name: value.track_name,
            artist_name: value.artist_name,
            album_name: value.album_name.unwrap_or_default(),
            duration_seconds: value.duration.unwrap_or_default(),
            synced_lyrics: value.synced_lyrics,
            plain_lyrics: value.plain_lyrics,
        }
    }
}

fn lrclib_search(query: &str) -> Result<Vec<LyricsSearchResult>, String> {
    let mut url =
        reqwest::Url::parse("https://lrclib.net/api/search").map_err(|error| error.to_string())?;
    url.query_pairs_mut().append_pair("q", query);
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("Rufin/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| error.to_string())?;
    let results = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("Lyric search failed: {error}"))?
        .json::<Vec<LrcLibLyricsDto>>()
        .map_err(|error| format!("Lyric search returned invalid data: {error}"))?;
    Ok(results.into_iter().map(LyricsSearchResult::from).collect())
}

fn save_lrclib_result(
    server_id: &ServerId,
    entry: &QueueEntry,
    result: &LyricsSearchResult,
) -> Result<(PathBuf, Lyrics), String> {
    let content = lyrics_result_content(result)
        .ok_or_else(|| "Selected lyric result has no lyrics to save.".to_string())?;
    let path = lyrics_save_path(&entry.title)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temp_path = path.with_extension("lrc.tmp");
    fs::write(&temp_path, content).map_err(|error| error.to_string())?;
    fs::rename(&temp_path, &path).map_err(|error| error.to_string())?;
    let lyrics = lyrics_from_text(entry.track_id.clone(), result);
    debug!(server_id = %server_id, path = %path.display(), "saved lyric file");
    Ok((path, lyrics))
}

fn lyrics_result_content(result: &LyricsSearchResult) -> Option<&str> {
    result
        .synced_lyrics
        .as_deref()
        .filter(|lyrics| !lyrics.trim().is_empty())
        .or_else(|| {
            result
                .plain_lyrics
                .as_deref()
                .filter(|lyrics| !lyrics.trim().is_empty())
        })
}

fn lyrics_save_path(track_title: &str) -> Result<PathBuf, String> {
    let user_dirs = directories::UserDirs::new()
        .ok_or_else(|| "Could not find the user home directory.".to_string())?;
    // Local-library support can replace this with the audio file stem; Music is the fallback.
    let base = user_dirs
        .audio_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| user_dirs.home_dir().join("Music"));
    Ok(base.join(format!("{}.lrc", lyrics_file_stem(track_title))))
}

fn lyrics_file_stem(track_title: &str) -> String {
    let stem = track_title
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            other if other.is_control() => '_',
            other => other,
        })
        .collect::<String>()
        .trim()
        .trim_end_matches('.')
        .to_string();
    if stem.is_empty() {
        "lyrics".to_string()
    } else {
        stem
    }
}

fn lyrics_from_text(track_id: TrackId, result: &LyricsSearchResult) -> Lyrics {
    let content = lyrics_result_content(result).unwrap_or_default();
    Lyrics {
        track_id,
        source: rufin_provider::LyricsSource::Remote,
        lines: content
            .lines()
            .filter_map(lyric_line_from_text)
            .collect::<Vec<_>>(),
    }
}

fn lyric_line_from_text(line: &str) -> Option<rufin_provider::LyricLine> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some((start_millis, text)) = parse_lrc_timestamp(trimmed) {
        return Some(rufin_provider::LyricLine {
            text: text.to_string(),
            start_millis: Some(start_millis),
        });
    }
    if trimmed.starts_with('[') && trimmed.contains(']') {
        return None;
    }
    Some(rufin_provider::LyricLine {
        text: trimmed.to_string(),
        start_millis: None,
    })
}

fn parse_lrc_timestamp(line: &str) -> Option<(u64, &str)> {
    let timestamp_end = line.find(']')?;
    let timestamp = line.get(1..timestamp_end)?;
    let (minutes, seconds) = timestamp.split_once(':')?;
    let minutes = minutes.parse::<u64>().ok()?;
    let (seconds, fraction) = seconds
        .split_once('.')
        .map(|(seconds, fraction)| (seconds, Some(fraction)))
        .unwrap_or((seconds, None));
    let seconds = seconds.parse::<u64>().ok()?;
    let fraction_millis = match fraction {
        Some(fraction) => fraction_to_millis(fraction)?,
        None => 0,
    };
    Some((
        (minutes * 60 + seconds) * 1_000 + fraction_millis,
        line.get(timestamp_end + 1..)?.trim(),
    ))
}

fn fraction_to_millis(fraction: &str) -> Option<u64> {
    let mut millis = 0_u64;
    for (index, character) in fraction.chars().take(3).enumerate() {
        let digit = character.to_digit(10)? as u64;
        millis += digit
            * match index {
                0 => 100,
                1 => 10,
                _ => 1,
            };
    }
    Some(millis)
}

fn lyrics_search_for_settings(settings: &AppSettings) -> JellyfinLyricsSearch {
    if settings.private_mode || !settings.external_lyrics_enabled {
        JellyfinLyricsSearch::ServerOnly
    } else if settings.prefer_server_lyrics {
        JellyfinLyricsSearch::ServerThenRemote
    } else {
        JellyfinLyricsSearch::RemoteThenServer
    }
}

fn cached_lyrics_allowed(lyrics: &Lyrics, search: JellyfinLyricsSearch) -> bool {
    match lyrics.source {
        rufin_provider::LyricsSource::Server => true,
        rufin_provider::LyricsSource::Remote => !matches!(search, JellyfinLyricsSearch::ServerOnly),
    }
}

fn provider_for_saved(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
) -> Result<JellyfinProvider, String> {
    let _unused = (store, runtime);
    let token = secrets
        .load_token(&saved.server.id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "No saved token found for the active server.".to_string())?;
    let session = SavedProviderSession {
        server: saved.server.clone(),
        user_id: saved.user_id.clone(),
        username: saved.username.clone(),
        trust_invalid_cert: saved.trust_invalid_cert,
        access_token: token,
    };
    JellyfinProvider::from_saved_session(session).map_err(|error| error.to_string())
}

fn sync_playlist_mutation(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
    before: &rufin_provider::PlaylistDetail,
    after: &rufin_provider::PlaylistDetail,
) -> Result<Option<rufin_provider::PlaylistDetail>, String> {
    let provider = provider_for_saved(store, runtime, secrets, saved)?;
    let before_ids = before
        .entries
        .iter()
        .map(|entry| entry.entry_id.as_str())
        .collect::<HashSet<_>>();
    let after_ids = after
        .entries
        .iter()
        .map(|entry| entry.entry_id.as_str())
        .collect::<HashSet<_>>();

    let removed = before
        .entries
        .iter()
        .filter(|entry| !after_ids.contains(entry.entry_id.as_str()))
        .map(|entry| entry.entry_id.clone())
        .collect::<Vec<_>>();
    if !removed.is_empty() {
        runtime
            .block_on(provider.remove_playlist_entries(&before.playlist.id, &removed))
            .map_err(|error| error.to_string())?;
    }

    let added = after
        .entries
        .iter()
        .filter(|entry| !before_ids.contains(entry.entry_id.as_str()))
        .map(|entry| entry.track.id.clone())
        .collect::<Vec<_>>();
    if !added.is_empty() {
        runtime
            .block_on(provider.add_playlist_tracks(&before.playlist.id, &added))
            .map_err(|error| error.to_string())?;
    }

    for (new_index, entry) in after.entries.iter().enumerate() {
        let Some(old_index) = before
            .entries
            .iter()
            .position(|candidate| candidate.entry_id == entry.entry_id)
        else {
            continue;
        };
        if old_index != new_index && before_ids.contains(entry.entry_id.as_str()) {
            runtime
                .block_on(provider.move_playlist_entry(
                    &before.playlist.id,
                    &entry.entry_id,
                    new_index,
                ))
                .map_err(|error| error.to_string())?;
        }
    }

    runtime
        .block_on(provider.playlist_detail(&before.playlist.id))
        .map(Some)
        .map_err(|error| error.to_string())
}

fn report_playback_async(
    store: StoreHandle,
    runtime: Arc<Runtime>,
    secrets: Arc<dyn SecretStore>,
    _events: Sender<ControllerEvent>,
    server_id: ServerId,
    report: PlaybackReport,
) {
    thread::spawn(move || {
        let Some(saved) = store
            .with_store(|store| store.active_server())
            .unwrap_or(None)
            .filter(|saved| saved.server.id == server_id)
        else {
            return;
        };
        if saved.server.provider == "fake" {
            return;
        }
        let result = provider_for_saved(&store, &runtime, &secrets, &saved).and_then(|provider| {
            runtime
                .block_on(provider.report_playback(report))
                .map_err(|error| error.to_string())
        });
        if let Err(error) = result {
            warn!(%error, "failed to report playback to provider");
        }
    });
}

fn playlist_entries_for_tracks(playlist_id: &PlaylistId, tracks: &[Track]) -> Vec<PlaylistEntry> {
    let prefix = unique_millis().unwrap_or(0);
    tracks
        .iter()
        .enumerate()
        .map(|(index, track)| PlaylistEntry {
            entry_id: format!("{}:{prefix}:{index}", playlist_id.as_str()),
            track: track.clone(),
        })
        .collect()
}

fn unique_millis() -> Option<u128> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis())
}

fn emit_snapshot_result(
    store: &StoreHandle,
    events: &Sender<ControllerEvent>,
    result: Result<(), String>,
) {
    if let Err(error) = result {
        let _sent = events.send(ControllerEvent::Error(error));
        return;
    }
    match load_snapshot(store) {
        Ok(snapshot) => {
            let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
        }
        Err(error) => {
            let _sent = events.send(ControllerEvent::Error(error));
        }
    }
}

fn cached_cover_path_for_saved(
    store: &StoreHandle,
    saved: &SavedServer,
    image_ref: &ImageRef,
    size: u32,
) -> Result<Option<PathBuf>, String> {
    let tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
    let Some(entry) = store.with_store(|store| {
        store.load_cover_cache_entry(&saved.server.id, &image_ref.item_id, tag, size)
    })?
    else {
        return Ok(None);
    };
    let path = PathBuf::from(entry.path);
    if path.exists() {
        return Ok(Some(path));
    }
    store.with_store(|store| {
        store.delete_cover_cache_entry(&saved.server.id, &image_ref.item_id, tag, size)
    })?;
    Ok(None)
}

fn cached_cover_path_for_key(key: &str) -> Option<PathBuf> {
    let path = cache_dir()?.join("covers").join(key);
    path.exists().then_some(path)
}

fn fetch_and_cache_cover(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
    image_ref: ImageRef,
    size: u32,
) -> Result<PathBuf, String> {
    let token = secrets
        .load_token(&saved.server.id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "No saved token found for the active server.".to_string())?;
    let session = SavedProviderSession {
        server: saved.server.clone(),
        user_id: saved.user_id.clone(),
        username: saved.username.clone(),
        trust_invalid_cert: saved.trust_invalid_cert,
        access_token: token,
    };
    let provider =
        JellyfinProvider::from_saved_session(session).map_err(|error| error.to_string())?;
    let image = runtime
        .block_on(provider.image_bytes(ImageRequest {
            item_id: image_ref.item_id.clone(),
            kind: ImageKind::Primary,
            tag: image_ref.tag.clone(),
            size,
        }))
        .map_err(|error| error.to_string())?;
    if image.bytes.is_empty() {
        return Err("cover response was empty".to_string());
    }

    let tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
    let key = image_cache_key(&saved.server.id, &image_ref.item_id, tag, size);
    let path = cache_dir()
        .map(|dir| dir.join("covers").join(&key))
        .ok_or_else(|| "cache directory is unavailable".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temp_path = path.with_extension("tmp");
    fs::write(&temp_path, image.bytes).map_err(|error| error.to_string())?;
    fs::rename(&temp_path, &path).map_err(|error| error.to_string())?;

    store.with_store(|store| {
        store.save_cover_cache_entry(&CoverCacheEntry {
            server_id: saved.server.id.clone(),
            item_id: image_ref.item_id,
            image_tag: tag.to_string(),
            size,
            path: path.to_string_lossy().to_string(),
        })
    })?;

    Ok(path)
}

fn is_provider_not_found_error(error: &str) -> bool {
    error == "provider item was not found"
}

fn data_dir() -> Option<PathBuf> {
    ProjectDirs::from("io.github", "screwys", "Rufin").map(|dirs| dirs.data_dir().to_path_buf())
}

fn cache_dir() -> Option<PathBuf> {
    ProjectDirs::from("io.github", "screwys", "Rufin").map(|dirs| dirs.cache_dir().to_path_buf())
}

fn clear_disk_cover_cache(server_id: &ServerId) -> Result<(), String> {
    let Some(path) =
        cache_dir().map(|dir| dir.join("covers").join(encode_key_part(server_id.as_str())))
    else {
        return Ok(());
    };
    remove_dir_if_exists(&path)
}

fn remove_dir_if_exists(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn encode_key_part(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => character,
            _ => '_',
        })
        .collect()
}

fn sync_is_running(sync_in_flight: &Arc<Mutex<HashSet<ServerId>>>, server_id: &ServerId) -> bool {
    sync_in_flight
        .lock()
        .map(|running| running.contains(server_id))
        .unwrap_or(true)
}

fn acquire_cover_slot(slots: &Arc<(Mutex<usize>, Condvar)>) -> bool {
    let (lock, ready) = &**slots;
    let Ok(mut active) = lock.lock() else {
        return false;
    };
    while *active >= 2 {
        let Ok(waiting) = ready.wait(active) else {
            return false;
        };
        active = waiting;
    }
    *active += 1;
    true
}

fn release_cover_slot(slots: &Arc<(Mutex<usize>, Condvar)>) {
    let (lock, ready) = &**slots;
    if let Ok(mut active) = lock.lock() {
        *active = active.saturating_sub(1);
        ready.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::mpsc::{Receiver, channel};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    use super::{
        AppController, ControllerEvent, LibrarySnapshot, PendingSeek, StoreHandle,
        auto_dj_candidates, load_settings_from_store, load_snapshot, playback_snapshot_from_queue,
        refresh_home_section, refresh_home_sections, restore_queue, seek_position_is_stale,
        sync_page_finished,
    };
    use rufin_core::{
        AlbumId, AppSettings, ArtistId, HomeSection, HomeSectionKind, ImageRef, PlaylistId,
        QueueEngine, RepeatMode, ServerId, ServerIdentity, Track, TrackId,
    };
    use rufin_playback::{
        PlaybackBackend, PlaybackCommand, PlaybackError, PlaybackEvent, PlaybackState,
    };
    use rufin_provider::{
        FavoriteItemId, LyricLine, Lyrics, LyricsSource, MusicProvider, PagedRequest,
    };
    use rufin_provider_jellyfin::JellyfinLyricsSearch;
    use rufin_secrets::MemorySecretStore;
    use rufin_store::{CoverCacheEntry, SavedServer};
    use rufin_test_support::{FakeProvider, FakeScale};
    use tokio::runtime::Runtime;

    struct StalePositionAfterSeekBackend {
        stale_millis: u64,
        events: Vec<PlaybackEvent>,
    }

    impl StalePositionAfterSeekBackend {
        fn new(stale_millis: u64) -> Self {
            Self {
                stale_millis,
                events: Vec::new(),
            }
        }
    }

    impl PlaybackBackend for StalePositionAfterSeekBackend {
        fn send(&mut self, command: PlaybackCommand) -> Result<(), PlaybackError> {
            if let PlaybackCommand::SeekMillis(millis) = command {
                self.events.push(position_event_for_test(millis));
                self.events.push(position_event_for_test(self.stale_millis));
            }
            Ok(())
        }

        fn drain_events(&mut self) -> Vec<PlaybackEvent> {
            std::mem::take(&mut self.events)
        }
    }

    fn position_event_for_test(millis: u64) -> PlaybackEvent {
        PlaybackEvent::PositionChanged {
            seconds: (millis / 1_000).min(u64::from(u32::MAX)) as u32,
            millis,
        }
    }

    #[test]
    fn no_server_bootstrap_enters_first_run_state() {
        let (_controller, _events, snapshot, queue, player) =
            AppController::bootstrap_memory_for_test();

        assert!(snapshot.first_run);
        assert!(snapshot.server.is_none());
        assert!(queue.is_none());
        assert_eq!(player.state, PlaybackState::Stopped);
    }

    #[test]
    fn fake_bootstrap_routes_data_through_store_cache() {
        let (_controller, _events, snapshot, queue, player) =
            AppController::bootstrap(Some(FakeScale::Small));

        assert!(!snapshot.first_run);
        assert!(queue.expect("queue").entries.is_empty());
        assert_eq!(player.state, PlaybackState::Stopped);
        assert_eq!(
            snapshot.albums.len(),
            500.min(FakeScale::Small.album_count())
        );
        assert_eq!(
            snapshot.tracks.len(),
            1_000.min(FakeScale::Small.track_count())
        );
    }

    #[test]
    fn sync_pages_continue_when_total_is_unknown() {
        assert!(!sync_page_finished(500, 0, 500));
        assert!(sync_page_finished(120, 0, 620));
        assert!(!sync_page_finished(120, 1_000, 620));
        assert!(sync_page_finished(500, 1_000, 1_000));
    }

    #[test]
    fn large_fake_bootstrap_seeds_visible_cache_window() {
        let (_controller, _events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Large));

        assert!(!snapshot.first_run);
        assert_eq!(snapshot.albums.len(), 500);
        assert_eq!(snapshot.tracks.len(), 1_000);
    }

    #[test]
    fn home_refresh_replaces_cached_sections_without_full_sync() {
        let runtime = Runtime::new().expect("runtime");
        let store = StoreHandle::open_memory().expect("memory store");
        let provider = FakeProvider::new(FakeScale::Small);
        let saved = SavedServer {
            server: provider.identity().server.clone(),
            user_id: "fake-user".to_string(),
            username: "fake".to_string(),
            trust_invalid_cert: false,
        };
        let stale_album = runtime
            .block_on(provider.albums(PagedRequest::new(8, 1)))
            .expect("stale album page")
            .items
            .into_iter()
            .next()
            .expect("stale album");
        let stale_track = runtime
            .block_on(provider.tracks(PagedRequest::new(8, 1)))
            .expect("stale track page")
            .items
            .into_iter()
            .next()
            .expect("stale track");

        store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&saved.server.id)?;
                store.upsert_albums(&saved.server.id, std::slice::from_ref(&stale_album), 0)?;
                store.upsert_tracks(&saved.server.id, std::slice::from_ref(&stale_track), 0)?;
                store.upsert_home_sections(
                    &saved.server.id,
                    &[
                        HomeSection {
                            kind: HomeSectionKind::Explore,
                            albums: vec![stale_album.clone()],
                            tracks: Vec::new(),
                        },
                        HomeSection {
                            kind: HomeSectionKind::MostPlayed,
                            albums: Vec::new(),
                            tracks: vec![stale_track.clone()],
                        },
                    ],
                    0,
                )?;
                Ok(())
            })
            .expect("seed stale home sections");

        let before = store
            .with_store(|store| store.load_home_sections(&saved.server.id))
            .expect("load stale home sections");
        assert_eq!(before[0].albums[0].id, AlbumId::fake(9));
        assert_eq!(before[1].tracks[0].id, TrackId::fake(9));

        runtime
            .block_on(refresh_home_sections(&store, &saved.server.id, &provider))
            .expect("refresh home sections");

        let after = store
            .with_store(|store| store.load_home_sections(&saved.server.id))
            .expect("load refreshed home sections");
        let sync_state = store
            .with_store(|store| store.sync_state(&saved.server.id))
            .expect("sync state");

        assert_eq!(after[0].kind, HomeSectionKind::Explore);
        assert_eq!(after[0].albums[0].id, AlbumId::fake(1));
        assert_eq!(after[1].kind, HomeSectionKind::MostPlayed);
        assert_eq!(after[1].tracks[0].id, TrackId::fake(1));
        assert_eq!(sync_state.generation, 0);
        assert_eq!(sync_state.status, "idle");
    }

    #[test]
    fn home_section_refresh_replaces_only_selected_section() {
        let runtime = Runtime::new().expect("runtime");
        let store = StoreHandle::open_memory().expect("memory store");
        let provider = FakeProvider::new(FakeScale::Small);
        let saved = SavedServer {
            server: provider.identity().server.clone(),
            user_id: "fake-user".to_string(),
            username: "fake".to_string(),
            trust_invalid_cert: false,
        };
        let stale_album = runtime
            .block_on(provider.albums(PagedRequest::new(8, 1)))
            .expect("stale album page")
            .items
            .into_iter()
            .next()
            .expect("stale album");
        let stale_track = runtime
            .block_on(provider.tracks(PagedRequest::new(8, 1)))
            .expect("stale track page")
            .items
            .into_iter()
            .next()
            .expect("stale track");

        store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&saved.server.id)?;
                store.upsert_albums(&saved.server.id, std::slice::from_ref(&stale_album), 0)?;
                store.upsert_tracks(&saved.server.id, std::slice::from_ref(&stale_track), 0)?;
                store.upsert_home_sections(
                    &saved.server.id,
                    &[
                        HomeSection {
                            kind: HomeSectionKind::Explore,
                            albums: vec![stale_album],
                            tracks: Vec::new(),
                        },
                        HomeSection {
                            kind: HomeSectionKind::MostPlayed,
                            albums: Vec::new(),
                            tracks: vec![stale_track.clone()],
                        },
                    ],
                    0,
                )?;
                Ok(())
            })
            .expect("seed stale home sections");

        runtime
            .block_on(refresh_home_section(
                &store,
                &saved.server.id,
                &provider,
                HomeSectionKind::Explore,
            ))
            .expect("refresh Explore");

        let after = store
            .with_store(|store| store.load_home_sections(&saved.server.id))
            .expect("load refreshed home sections");

        assert_eq!(after[0].kind, HomeSectionKind::Explore);
        assert_eq!(after[0].albums[0].id, AlbumId::fake(1));
        assert_eq!(after[1].kind, HomeSectionKind::MostPlayed);
        assert_eq!(after[1].tracks, vec![stale_track]);
    }

    #[test]
    fn clear_cache_emits_empty_active_server_snapshot() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let server = snapshot.server.expect("server");

        controller.clear_active_server_cache();
        let snapshot = wait_for_snapshot(&events);

        assert!(!snapshot.first_run);
        assert_eq!(snapshot.server.expect("server").id, server.id);
        assert!(snapshot.albums.is_empty());
        assert!(snapshot.tracks.is_empty());
        assert!(snapshot.search.albums.is_empty());
    }

    #[test]
    fn startup_sync_policy_uses_empty_fresh_and_error_cache_states() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let server_id = snapshot.server.expect("server").id;

        assert_eq!(controller.startup_sync_delay_ms(), None);

        controller
            .store
            .with_store(|store| store.fail_sync(&server_id, "previous sync failed"))
            .expect("mark sync failed");
        assert_eq!(controller.startup_sync_delay_ms(), Some(8_000));

        controller.clear_active_server_cache();
        let _snapshot = wait_for_snapshot(&events);
        assert_eq!(controller.startup_sync_delay_ms(), Some(500));
    }

    #[test]
    fn cached_cover_request_emits_cover_ready_without_fetching() {
        let (controller, events, _snapshot, _queue, _player) =
            AppController::bootstrap_memory_for_test();
        let server_id = ServerId::new("jellyfin:server:test");
        let saved = SavedServer {
            server: ServerIdentity {
                id: server_id.clone(),
                provider: "jellyfin".to_string(),
                name: "Test".to_string(),
                base_url: "https://music.example".to_string(),
            },
            user_id: "user".to_string(),
            username: "demo".to_string(),
            trust_invalid_cert: false,
        };
        let path = std::env::temp_dir().join(format!(
            "rufin-cover-ready-{}-{}.jpg",
            std::process::id(),
            "cached"
        ));
        fs::write(&path, [1_u8, 2, 3]).expect("write cover");
        let image_ref = ImageRef::new("jellyfin:album:one", Some("tag-one".to_string()));

        controller
            .store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&server_id)?;
                store.save_cover_cache_entry(&CoverCacheEntry {
                    server_id: server_id.clone(),
                    item_id: image_ref.item_id.clone(),
                    image_tag: "tag-one".to_string(),
                    size: 256,
                    path: path.to_string_lossy().to_string(),
                })
            })
            .expect("seed cover cache");
        let key = controller.cover_key(&image_ref, 256).expect("cover key");

        controller.request_cover(image_ref, 256);

        assert_eq!(wait_for_cover_ready(&events, &key), path);
        let _cleanup = fs::remove_file(path);
    }

    #[test]
    fn missing_cached_cover_file_invalidates_cover_index() {
        let (controller, _events, _snapshot, _queue, _player) =
            AppController::bootstrap_memory_for_test();
        let server_id = ServerId::new("jellyfin:server:test");
        let saved = SavedServer {
            server: ServerIdentity {
                id: server_id.clone(),
                provider: "jellyfin".to_string(),
                name: "Test".to_string(),
                base_url: "https://music.example".to_string(),
            },
            user_id: "user".to_string(),
            username: "demo".to_string(),
            trust_invalid_cert: false,
        };
        let path = std::env::temp_dir().join(format!(
            "rufin-missing-cover-{}-{}.jpg",
            std::process::id(),
            "cached"
        ));
        let _cleanup = fs::remove_file(&path);
        let image_ref = ImageRef::new("jellyfin:album:one", Some("tag-one".to_string()));

        controller
            .store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&server_id)?;
                store.save_cover_cache_entry(&CoverCacheEntry {
                    server_id: server_id.clone(),
                    item_id: image_ref.item_id.clone(),
                    image_tag: "tag-one".to_string(),
                    size: 256,
                    path: path.to_string_lossy().to_string(),
                })
            })
            .expect("seed cover cache");

        assert_eq!(controller.cached_cover_path(&image_ref, 256), None);
        assert_eq!(
            controller
                .store
                .with_store(|store| store.load_cover_cache_entry(
                    &server_id,
                    &image_ref.item_id,
                    "tag-one",
                    256
                ))
                .expect("load cover cache"),
            None
        );
    }

    #[test]
    fn forget_server_emits_first_run_and_deletes_token() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let server_id = snapshot.server.expect("server").id;
        controller
            .secrets
            .save_token(&server_id, "token")
            .expect("save token");

        controller.forget_active_server();
        let snapshot = wait_for_snapshot(&events);

        assert!(snapshot.first_run);
        assert_eq!(
            controller
                .secrets
                .load_token(&server_id)
                .expect("load token"),
            None
        );
    }

    #[test]
    fn duplicate_resync_requests_do_not_start_another_sync() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let server_id = snapshot.server.expect("server").id;
        controller
            .sync_in_flight
            .lock()
            .expect("sync guard")
            .insert(server_id);

        controller.resync_active_server();

        assert_eq!(wait_for_status(&events), "Sync already running.");
    }

    #[test]
    fn play_now_starts_fake_playback_and_persists_queue() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let track = snapshot.tracks[0].clone();

        controller.play_now(track.clone());
        let queue = wait_for_queue(&events).expect("queue");
        assert_eq!(queue.entries.len(), 1);
        assert_eq!(queue.entries[0].track_id, track.id);

        let playback = wait_for_playback_state(&controller, &events, PlaybackState::Playing);
        assert_eq!(
            playback.current.expect("current").track_id,
            queue.entries[0].track_id
        );
        assert_eq!(
            controller
                .store
                .with_store(|store| store.load_queue_snapshot(&queue.server_id))
                .expect("store")
                .expect("snapshot")
                .entries
                .len(),
            1
        );
    }

    #[test]
    fn activate_queue_entry_starts_selected_track() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let first = snapshot.tracks[0].clone();
        let second = snapshot.tracks[1].clone();

        controller.play_tracks_now(vec![first, second.clone()]);
        let queue = wait_for_queue(&events).expect("queue");
        let second_entry = queue
            .entries
            .iter()
            .find(|entry| entry.track_id == second.id)
            .expect("second entry")
            .id
            .clone();
        let _initial_playback =
            wait_for_playback_state(&controller, &events, PlaybackState::Playing);

        controller.activate_queue_entry(second_entry);

        let queue = wait_for_queue(&events).expect("activated queue");
        assert_eq!(queue.current_index, Some(1));
        let playback = wait_for_playback_state(&controller, &events, PlaybackState::Playing);
        assert_eq!(playback.current.expect("current").track_id, second.id);
    }

    #[test]
    fn seek_millis_emits_exact_playback_position() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        controller.play_now(snapshot.tracks[0].clone());
        let _playback = wait_for_playback_state(&controller, &events, PlaybackState::Playing);

        controller.seek_millis(12_345);

        let playback = wait_for_playback_position(&events, 12_345);
        assert_eq!(playback.position_seconds, 12);
    }

    #[test]
    fn poll_playback_events_ignores_stale_positions_after_seek() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        controller.play_now(snapshot.tracks[0].clone());
        let _playback = wait_for_playback_state(&controller, &events, PlaybackState::Playing);
        {
            let mut playback = controller.playback.lock().expect("playback");
            *playback = Box::new(StalePositionAfterSeekBackend::new(125_000));
        }

        controller.seek_millis(42_000);
        assert_eq!(
            controller
                .playback_snapshot
                .lock()
                .expect("playback snapshot")
                .position_millis,
            42_000
        );

        controller.poll_playback_events();

        assert_eq!(
            controller
                .playback_snapshot
                .lock()
                .expect("playback snapshot")
                .position_millis,
            42_000
        );
    }

    #[test]
    fn pending_seek_rejects_far_positions_only_during_settle_window() {
        let now = std::time::Instant::now();
        let pending = PendingSeek {
            target_millis: 42_000,
            expires_at: now + super::SEEK_SETTLE_WINDOW,
        };

        assert!(seek_position_is_stale(pending, 125_000, now));
        assert!(!seek_position_is_stale(pending, 43_000, now));
        assert!(!seek_position_is_stale(
            pending,
            125_000,
            pending.expires_at
        ));
    }

    #[test]
    fn next_previous_and_clear_keep_queue_and_player_synchronized() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let first = snapshot.tracks[0].clone();
        let second = snapshot.tracks[1].clone();

        controller.play_tracks_now(vec![first.clone(), second.clone()]);
        let _queue = wait_for_queue(&events).expect("queue");
        controller.next_track();
        let queue = wait_for_queue(&events).expect("next queue");
        assert_eq!(
            queue.entries[queue.current_index.expect("current")].track_id,
            second.id
        );

        controller.previous_track();
        let queue = wait_for_queue(&events).expect("previous queue");
        assert_eq!(
            queue.entries[queue.current_index.expect("current")].track_id,
            first.id
        );

        controller.clear_queue();
        let queue = wait_for_queue(&events).expect("clear queue");
        assert!(queue.entries.is_empty());
    }

    #[test]
    fn manual_next_at_queue_end_restarts_current_track() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let first = snapshot.tracks[0].clone();
        let second = snapshot.tracks[1].clone();

        controller.play_tracks_now(vec![first, second.clone()]);
        let _queue = wait_for_queue(&events).expect("queue");
        controller.next_track();
        let _queue = wait_for_queue(&events).expect("next queue");
        controller.seek_millis(12_000);
        let _playback = wait_for_playback_position(&events, 12_000);

        controller.next_track();

        let playback = wait_for_playback_position(&events, 0);
        assert_eq!(playback.current.expect("current").track_id, second.id);
        assert_ne!(playback.state, PlaybackState::Stopped);
    }

    #[test]
    fn manual_previous_after_ten_seconds_restarts_current_track() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let first = snapshot.tracks[0].clone();
        let second = snapshot.tracks[1].clone();

        controller.play_tracks_now(vec![first, second.clone()]);
        let _queue = wait_for_queue(&events).expect("queue");
        controller.next_track();
        let _queue = wait_for_queue(&events).expect("next queue");
        controller.seek_millis(11_000);
        let _playback = wait_for_playback_position(&events, 11_000);

        controller.previous_track();

        let playback = wait_for_playback_position(&events, 0);
        assert_eq!(playback.current.expect("current").track_id, second.id);
    }

    #[test]
    fn cycle_repeat_uses_off_all_one_order() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));

        controller.play_now(snapshot.tracks[0].clone());
        let _queue = wait_for_queue(&events).expect("queue");

        controller.cycle_repeat();
        let queue = wait_for_queue(&events).expect("repeat all");
        assert_eq!(queue.repeat_mode, RepeatMode::All);

        controller.cycle_repeat();
        let queue = wait_for_queue(&events).expect("repeat one");
        assert_eq!(queue.repeat_mode, RepeatMode::One);

        controller.cycle_repeat();
        let queue = wait_for_queue(&events).expect("repeat off");
        assert_eq!(queue.repeat_mode, RepeatMode::Off);
    }

    #[test]
    fn toggle_auto_dj_persists_and_emits_playback_state() {
        let (controller, events, _snapshot, _queue, player) =
            AppController::bootstrap(Some(FakeScale::Small));

        assert!(!player.auto_dj_enabled);

        controller.toggle_auto_dj();

        let playback = wait_for_playback_auto_dj(&events, true);
        assert!(playback.auto_dj_enabled);
        assert!(controller.load_settings().auto_dj_enabled);
    }

    #[test]
    fn auto_dj_tops_up_low_queue_from_cached_library() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let first = snapshot.tracks[0].clone();

        controller.toggle_auto_dj();
        let _playback = wait_for_playback_auto_dj(&events, true);
        controller.play_now(first.clone());

        let queue = wait_for_queue(&events).expect("queue");
        assert_eq!(queue.entries.len(), 1 + super::AUTO_DJ_ITEM_COUNT);
        assert_eq!(queue.entries[0].track_id, first.id);
        assert_eq!(
            queue
                .entries
                .iter()
                .map(|entry| entry.track_id.clone())
                .collect::<HashSet<_>>()
                .len(),
            queue.entries.len()
        );
    }

    #[test]
    fn auto_dj_extends_queue_before_manual_next_at_end() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let first = snapshot.tracks[0].clone();
        let second = snapshot.tracks[1].clone();

        controller.play_tracks_now(vec![first, second.clone()]);
        let _queue = wait_for_queue(&events).expect("queue");
        controller.toggle_auto_dj();
        let _playback = wait_for_playback_auto_dj(&events, true);

        controller.next_track();
        let queue = wait_for_queue(&events).expect("second queue");
        assert_eq!(
            queue.entries[queue.current_index.expect("current")].track_id,
            second.id
        );

        controller.next_track();
        let queue = wait_for_queue(&events).expect("auto dj queue");

        assert_eq!(queue.entries.len(), 2 + super::AUTO_DJ_ITEM_COUNT);
        assert_ne!(
            queue.entries[queue.current_index.expect("current")].track_id,
            second.id
        );
    }

    #[test]
    fn auto_dj_candidates_prefer_related_tracks() {
        let current = library_track(
            1,
            Some(ArtistId::fake(1)),
            AlbumId::fake(1),
            "Artist",
            &["Rock"],
        );
        let related = library_track(
            2,
            Some(ArtistId::fake(1)),
            AlbumId::fake(1),
            "Artist",
            &["Rock"],
        );
        let genre_only = library_track(
            3,
            Some(ArtistId::fake(2)),
            AlbumId::fake(2),
            "Other",
            &["Rock"],
        );
        let unrelated = library_track(
            4,
            Some(ArtistId::fake(3)),
            AlbumId::fake(3),
            "Other",
            &["Jazz"],
        );
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.play_now(&current);
        let current_entry = queue.current().expect("current").clone();
        let queued = HashSet::from([current.id.clone()]);

        let candidates = auto_dj_candidates(
            &[
                unrelated.clone(),
                current.clone(),
                genre_only,
                related.clone(),
            ],
            &current_entry,
            &queued,
            7,
        );

        assert_eq!(candidates[0].id, related.id);
        assert!(candidates.iter().all(|track| track.id != current.id));
    }

    #[test]
    fn end_of_stream_repeat_one_restarts_current_track() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let first = snapshot.tracks[0].clone();
        let second = snapshot.tracks[1].clone();

        controller.play_tracks_now(vec![first.clone(), second]);
        let _queue = wait_for_queue(&events).expect("queue");
        controller.cycle_repeat();
        let _queue = wait_for_queue(&events).expect("repeat all");
        controller.cycle_repeat();
        let _queue = wait_for_queue(&events).expect("repeat one");

        controller.advance_after_end_of_stream();
        let queue = wait_for_queue(&events).expect("repeated queue");

        assert_eq!(
            queue.entries[queue.current_index.expect("current")].track_id,
            first.id
        );
    }

    #[test]
    fn end_of_stream_advances_queue() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let first = snapshot.tracks[0].clone();
        let second = snapshot.tracks[1].clone();

        controller.play_tracks_now(vec![first, second.clone()]);
        let _queue = wait_for_queue(&events).expect("queue");
        controller.advance_after_end_of_stream();
        let queue = wait_for_queue(&events).expect("next queue");

        assert_eq!(
            queue.entries[queue.current_index.expect("current")].track_id,
            second.id
        );
    }

    #[test]
    fn favorite_toggles_update_fake_cache_and_current_player_snapshot() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let track = snapshot
            .tracks
            .iter()
            .find(|track| !track.favorite)
            .expect("non-favorite track")
            .clone();

        controller.play_now(track.clone());
        let _playback = wait_for_playback_state(&controller, &events, PlaybackState::Playing);
        controller.toggle_current_favorite();

        let playback = wait_for_playback_current_favorite(&controller, &events, true);
        assert_eq!(playback.current.expect("current").track_id, track.id);
        let (item_id, favorite, snapshot) = wait_for_favorite_changed(&events);
        assert_eq!(item_id, FavoriteItemId::Track(track.id.clone()));
        assert!(favorite);
        assert!(
            snapshot
                .tracks
                .iter()
                .find(|candidate| candidate.id == track.id)
                .expect("cached track")
                .favorite
        );
        assert!(
            snapshot
                .favorites
                .iter()
                .any(|candidate| candidate.id == track.id)
        );
    }

    #[test]
    fn explicit_favorite_updates_can_unfavorite_persistent_controls() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let album = snapshot
            .albums
            .iter()
            .find(|album| !album.favorite)
            .expect("non-favorite album")
            .clone();

        controller.set_album_favorite(album.id.clone(), true);
        let (item_id, favorite, snapshot) = wait_for_favorite_changed(&events);
        assert_eq!(item_id, FavoriteItemId::Album(album.id.clone()));
        assert!(favorite);
        assert!(
            snapshot
                .albums
                .iter()
                .find(|candidate| candidate.id == album.id)
                .expect("cached album")
                .favorite
        );

        controller.set_album_favorite(album.id.clone(), false);
        let (item_id, favorite, snapshot) = wait_for_favorite_changed(&events);
        assert_eq!(item_id, FavoriteItemId::Album(album.id.clone()));
        assert!(!favorite);
        assert!(
            !snapshot
                .albums
                .iter()
                .find(|candidate| candidate.id == album.id)
                .expect("cached album")
                .favorite
        );
    }

    #[test]
    fn fake_playlist_mutations_create_move_and_remove_entries() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let first = snapshot.tracks[0].clone();
        let second = snapshot.tracks[1].clone();

        controller.create_playlist(
            "Controller Playlist".to_string(),
            vec![first.clone(), second.clone()],
        );
        let snapshot = wait_for_snapshot(&events);
        let playlist = snapshot
            .playlists
            .iter()
            .find(|playlist| playlist.name == "Controller Playlist")
            .expect("created playlist")
            .clone();
        assert_playlist_order(
            &controller,
            &playlist.id,
            &[first.id.as_str(), second.id.as_str()],
        );

        let detail = controller
            .cached_playlist_detail(&playlist.id)
            .expect("playlist detail")
            .expect("playlist detail");
        controller.move_playlist_entry(playlist.id.clone(), detail.entries[1].entry_id.clone(), 0);
        let _snapshot = wait_for_snapshot(&events);
        assert_playlist_order(
            &controller,
            &playlist.id,
            &[second.id.as_str(), first.id.as_str()],
        );

        let detail = controller
            .cached_playlist_detail(&playlist.id)
            .expect("playlist detail")
            .expect("playlist detail");
        controller.remove_playlist_entry(playlist.id.clone(), detail.entries[0].entry_id.clone());
        let _snapshot = wait_for_snapshot(&events);
        assert_playlist_order(&controller, &playlist.id, &[first.id.as_str()]);
    }

    #[test]
    fn fake_lyrics_request_emits_empty_lyrics_event() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        controller.play_now(snapshot.tracks[0].clone());
        let _playback = wait_for_playback_state(&controller, &events, PlaybackState::Playing);

        controller.request_lyrics_for_current();

        assert!(wait_for_lyrics(&events).is_none());
    }

    #[test]
    fn restored_queue_request_lyrics_emits_cached_current_lyrics() {
        let store = StoreHandle::open_memory().expect("memory store");
        let saved = SavedServer {
            server: ServerIdentity {
                id: ServerId::new("jellyfin:server:lyrics"),
                provider: "jellyfin".to_string(),
                name: "Lyrics Server".to_string(),
                base_url: "https://music.example".to_string(),
            },
            user_id: "user".to_string(),
            username: "demo".to_string(),
            trust_invalid_cert: false,
        };
        let track = restored_track();
        let mut queue = QueueEngine::new(saved.server.id.clone());
        queue.play_now(&track);
        queue.set_progress_seconds(12);
        let lyrics = Lyrics {
            track_id: track.id.clone(),
            source: LyricsSource::Server,
            lines: vec![LyricLine {
                text: "first line".to_string(),
                start_millis: Some(1_000),
            }],
        };
        store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&saved.server.id)?;
                store.save_queue_snapshot(&queue.snapshot())?;
                store.save_lyrics(&saved.server.id, &lyrics)?;
                Ok(())
            })
            .expect("seed restored state");

        let (controller, events) = controller_from_store_for_test(store);

        controller.request_lyrics_for_current();

        assert_eq!(wait_for_lyrics(&events), Some(lyrics));
    }

    #[test]
    fn lyrics_search_respects_private_mode_and_preference() {
        let mut settings = AppSettings {
            external_lyrics_enabled: true,
            ..AppSettings::default()
        };
        assert_eq!(
            super::lyrics_search_for_settings(&settings),
            JellyfinLyricsSearch::ServerThenRemote
        );

        settings.prefer_server_lyrics = false;
        assert_eq!(
            super::lyrics_search_for_settings(&settings),
            JellyfinLyricsSearch::RemoteThenServer
        );

        settings.private_mode = true;
        assert_eq!(
            super::lyrics_search_for_settings(&settings),
            JellyfinLyricsSearch::ServerOnly
        );

        settings.private_mode = false;
        settings.external_lyrics_enabled = false;
        assert_eq!(
            super::lyrics_search_for_settings(&settings),
            JellyfinLyricsSearch::ServerOnly
        );
    }

    #[test]
    fn lyrics_save_path_uses_music_dir_and_track_title() {
        let path = super::lyrics_save_path("Song Title").expect("lyrics save path");
        let path = path.to_string_lossy();

        assert!(path.contains("Music") || path.contains("music"));
        assert!(path.ends_with("Song Title.lrc"));
    }

    #[test]
    fn lrclib_result_text_becomes_timed_lyrics() {
        let result = super::LyricsSearchResult {
            id: 7,
            track_name: "Song".to_string(),
            artist_name: "Artist".to_string(),
            album_name: "Album".to_string(),
            duration_seconds: 180,
            synced_lyrics: Some(
                "[00:12.34]first line\n[ar:Artist]\n[00:13.005]second line".to_string(),
            ),
            plain_lyrics: None,
        };

        let lyrics = super::lyrics_from_text(TrackId::new("track-one"), &result);

        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[0].text, "first line");
        assert_eq!(lyrics.lines[0].start_millis, Some(12_340));
        assert_eq!(lyrics.lines[1].text, "second line");
        assert_eq!(lyrics.lines[1].start_millis, Some(13_005));
    }

    #[test]
    fn controller_events_are_sendable() {
        fn assert_send<T: Send>() {}
        assert_send::<ControllerEvent>();
    }

    #[test]
    fn provider_not_found_cover_errors_are_classified() {
        assert!(super::is_provider_not_found_error(
            "provider item was not found"
        ));
        assert!(!super::is_provider_not_found_error(
            "provider network failed: offline"
        ));
    }

    fn controller_from_store_for_test(
        store: StoreHandle,
    ) -> (AppController, Receiver<ControllerEvent>) {
        let (events, receiver) = channel();
        let runtime = Runtime::new()
            .map(Arc::new)
            .unwrap_or_else(|error| panic!("failed to create Tokio runtime: {error}"));
        let snapshot = load_snapshot(&store).expect("load snapshot");
        let settings = load_settings_from_store(&store);
        let queue = restore_queue(&store, snapshot.server.as_ref());
        let playback_snapshot =
            playback_snapshot_from_queue(queue.as_ref(), settings.auto_dj_enabled);
        let controller = AppController {
            store,
            runtime,
            secrets: Arc::new(MemorySecretStore::new()),
            queue: Arc::new(Mutex::new(queue)),
            playback: Arc::new(Mutex::new(Box::new(
                rufin_playback::FakePlaybackBackend::new(),
            ))),
            playback_snapshot: Arc::new(Mutex::new(playback_snapshot)),
            pending_seek: Arc::new(Mutex::new(None)),
            auto_dj_enabled: Arc::new(Mutex::new(settings.auto_dj_enabled)),
            last_progress_snapshot: Arc::new(Mutex::new(None)),
            last_report_snapshot: Arc::new(Mutex::new(None)),
            events,
            sync_in_flight: Arc::new(Mutex::new(HashSet::new())),
            home_refresh_in_flight: Arc::new(Mutex::new(HashSet::new())),
            cover_in_flight: Arc::new(Mutex::new(HashSet::new())),
            cover_slots: Arc::new((Mutex::new(0), Condvar::new())),
        };
        (controller, receiver)
    }

    fn restored_track() -> Track {
        Track {
            id: TrackId::new("jellyfin:track:lyrics"),
            album_id: AlbumId::fake(1),
            title: "Restored Track".to_string(),
            artist: "Artist".to_string(),
            artist_id: Some(ArtistId::fake(1)),
            album: "Album".to_string(),
            year: 2026,
            duration_seconds: 180,
            favorite: false,
            disc_number: 1,
            track_number: 1,
            image_ref: None,
            genres: Vec::new(),
        }
    }

    fn library_track(
        number: u32,
        artist_id: Option<ArtistId>,
        album_id: AlbumId,
        artist: &str,
        genres: &[&str],
    ) -> Track {
        Track {
            id: TrackId::fake(number),
            album_id,
            title: format!("Track {number}"),
            artist: artist.to_string(),
            artist_id,
            album: "Album".to_string(),
            year: 2026,
            duration_seconds: 180,
            favorite: false,
            disc_number: 1,
            track_number: number as u16,
            image_ref: None,
            genres: genres.iter().map(|genre| genre.to_string()).collect(),
        }
    }

    fn wait_for_snapshot(events: &Receiver<ControllerEvent>) -> LibrarySnapshot {
        loop {
            match events
                .recv_timeout(Duration::from_secs(5))
                .expect("controller event")
            {
                ControllerEvent::Snapshot(snapshot) => return *snapshot,
                ControllerEvent::Queue(_)
                | ControllerEvent::FavoriteChanged { .. }
                | ControllerEvent::Playback(_)
                | ControllerEvent::Lyrics(_)
                | ControllerEvent::LyricsSearchResults { .. }
                | ControllerEvent::LyricsSaved { .. }
                | ControllerEvent::CoverReady { .. } => {}
                ControllerEvent::LoginStatus(_) => {}
                ControllerEvent::Error(error) => panic!("controller error: {error}"),
            }
        }
    }

    fn wait_for_favorite_changed(
        events: &Receiver<ControllerEvent>,
    ) -> (FavoriteItemId, bool, LibrarySnapshot) {
        loop {
            match events
                .recv_timeout(Duration::from_secs(5))
                .expect("controller event")
            {
                ControllerEvent::FavoriteChanged {
                    item_id,
                    favorite,
                    snapshot,
                } => return (item_id, favorite, *snapshot),
                ControllerEvent::Snapshot(_)
                | ControllerEvent::Queue(_)
                | ControllerEvent::Playback(_)
                | ControllerEvent::Lyrics(_)
                | ControllerEvent::LyricsSearchResults { .. }
                | ControllerEvent::LyricsSaved { .. }
                | ControllerEvent::CoverReady { .. } => {}
                ControllerEvent::LoginStatus(_) => {}
                ControllerEvent::Error(error) => panic!("controller error: {error}"),
            }
        }
    }

    fn wait_for_status(events: &Receiver<ControllerEvent>) -> String {
        loop {
            match events
                .recv_timeout(Duration::from_secs(5))
                .expect("controller event")
            {
                ControllerEvent::LoginStatus(status) => return status,
                ControllerEvent::Snapshot(_)
                | ControllerEvent::FavoriteChanged { .. }
                | ControllerEvent::Queue(_)
                | ControllerEvent::Playback(_)
                | ControllerEvent::Lyrics(_)
                | ControllerEvent::LyricsSearchResults { .. }
                | ControllerEvent::LyricsSaved { .. }
                | ControllerEvent::CoverReady { .. } => {}
                ControllerEvent::Error(error) => panic!("controller error: {error}"),
            }
        }
    }

    fn wait_for_queue(events: &Receiver<ControllerEvent>) -> Option<rufin_core::QueueSnapshot> {
        loop {
            match events
                .recv_timeout(Duration::from_secs(5))
                .expect("controller event")
            {
                ControllerEvent::Queue(queue) => return *queue,
                ControllerEvent::Snapshot(_)
                | ControllerEvent::FavoriteChanged { .. }
                | ControllerEvent::Playback(_)
                | ControllerEvent::LoginStatus(_)
                | ControllerEvent::Lyrics(_)
                | ControllerEvent::LyricsSearchResults { .. }
                | ControllerEvent::LyricsSaved { .. }
                | ControllerEvent::CoverReady { .. } => {}
                ControllerEvent::Error(error) => panic!("controller error: {error}"),
            }
        }
    }

    fn wait_for_cover_ready(events: &Receiver<ControllerEvent>, expected_key: &str) -> PathBuf {
        loop {
            match events
                .recv_timeout(Duration::from_secs(5))
                .expect("controller event")
            {
                ControllerEvent::CoverReady { key, path } if key == expected_key => return path,
                ControllerEvent::Snapshot(_)
                | ControllerEvent::FavoriteChanged { .. }
                | ControllerEvent::Queue(_)
                | ControllerEvent::Playback(_)
                | ControllerEvent::LoginStatus(_)
                | ControllerEvent::Lyrics(_)
                | ControllerEvent::LyricsSearchResults { .. }
                | ControllerEvent::LyricsSaved { .. }
                | ControllerEvent::CoverReady { .. } => {}
                ControllerEvent::Error(error) => panic!("controller error: {error}"),
            }
        }
    }

    fn wait_for_lyrics(events: &Receiver<ControllerEvent>) -> Option<rufin_provider::Lyrics> {
        loop {
            match events
                .recv_timeout(Duration::from_secs(5))
                .expect("controller event")
            {
                ControllerEvent::Lyrics(lyrics) => return *lyrics,
                ControllerEvent::Snapshot(_)
                | ControllerEvent::FavoriteChanged { .. }
                | ControllerEvent::Queue(_)
                | ControllerEvent::Playback(_)
                | ControllerEvent::LoginStatus(_)
                | ControllerEvent::LyricsSearchResults { .. }
                | ControllerEvent::LyricsSaved { .. }
                | ControllerEvent::CoverReady { .. } => {}
                ControllerEvent::Error(error) => panic!("controller error: {error}"),
            }
        }
    }

    fn wait_for_playback_state(
        controller: &AppController,
        events: &Receiver<ControllerEvent>,
        state: PlaybackState,
    ) -> super::PlaybackSnapshot {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for playback state"
            );
            controller.poll_playback_events();
            match events.recv_timeout(Duration::from_millis(50)) {
                Ok(event) => match event {
                    ControllerEvent::Playback(playback) if playback.state == state => {
                        return *playback;
                    }
                    ControllerEvent::Playback(_)
                    | ControllerEvent::Queue(_)
                    | ControllerEvent::Lyrics(_)
                    | ControllerEvent::LyricsSearchResults { .. }
                    | ControllerEvent::LyricsSaved { .. }
                    | ControllerEvent::CoverReady { .. } => {}
                    ControllerEvent::Snapshot(_)
                    | ControllerEvent::FavoriteChanged { .. }
                    | ControllerEvent::LoginStatus(_) => {}
                    ControllerEvent::Error(error) => panic!("controller error: {error}"),
                },
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("controller event channel closed")
                }
            }
        }
    }

    fn wait_for_playback_position(
        events: &Receiver<ControllerEvent>,
        position_millis: u64,
    ) -> super::PlaybackSnapshot {
        loop {
            match events
                .recv_timeout(Duration::from_secs(5))
                .expect("controller event")
            {
                ControllerEvent::Playback(playback)
                    if playback.position_millis == position_millis =>
                {
                    return *playback;
                }
                ControllerEvent::Playback(_)
                | ControllerEvent::Queue(_)
                | ControllerEvent::Lyrics(_)
                | ControllerEvent::LyricsSearchResults { .. }
                | ControllerEvent::LyricsSaved { .. }
                | ControllerEvent::CoverReady { .. } => {}
                ControllerEvent::Snapshot(_)
                | ControllerEvent::FavoriteChanged { .. }
                | ControllerEvent::LoginStatus(_) => {}
                ControllerEvent::Error(error) => panic!("controller error: {error}"),
            }
        }
    }

    fn wait_for_playback_auto_dj(
        events: &Receiver<ControllerEvent>,
        enabled: bool,
    ) -> super::PlaybackSnapshot {
        loop {
            match events
                .recv_timeout(Duration::from_secs(5))
                .expect("controller event")
            {
                ControllerEvent::Playback(playback) if playback.auto_dj_enabled == enabled => {
                    return *playback;
                }
                ControllerEvent::Playback(_)
                | ControllerEvent::Queue(_)
                | ControllerEvent::Lyrics(_)
                | ControllerEvent::LyricsSearchResults { .. }
                | ControllerEvent::LyricsSaved { .. }
                | ControllerEvent::CoverReady { .. } => {}
                ControllerEvent::Snapshot(_)
                | ControllerEvent::FavoriteChanged { .. }
                | ControllerEvent::LoginStatus(_) => {}
                ControllerEvent::Error(error) => panic!("controller error: {error}"),
            }
        }
    }

    fn wait_for_playback_current_favorite(
        controller: &AppController,
        events: &Receiver<ControllerEvent>,
        favorite: bool,
    ) -> super::PlaybackSnapshot {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for playback favorite"
            );
            controller.poll_playback_events();
            match events.recv_timeout(Duration::from_millis(50)) {
                Ok(event) => match event {
                    ControllerEvent::Playback(playback)
                        if playback
                            .current
                            .as_ref()
                            .is_some_and(|entry| entry.favorite == favorite) =>
                    {
                        return *playback;
                    }
                    ControllerEvent::Playback(_)
                    | ControllerEvent::Queue(_)
                    | ControllerEvent::Lyrics(_)
                    | ControllerEvent::LyricsSearchResults { .. }
                    | ControllerEvent::LyricsSaved { .. }
                    | ControllerEvent::CoverReady { .. } => {}
                    ControllerEvent::Snapshot(_)
                    | ControllerEvent::FavoriteChanged { .. }
                    | ControllerEvent::LoginStatus(_) => {}
                    ControllerEvent::Error(error) => panic!("controller error: {error}"),
                },
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("controller event channel closed")
                }
            }
        }
    }

    fn assert_playlist_order(controller: &AppController, playlist_id: &PlaylistId, ids: &[&str]) {
        let detail = controller
            .cached_playlist_detail(playlist_id)
            .expect("playlist detail")
            .expect("playlist detail");
        assert_eq!(
            detail
                .entries
                .iter()
                .map(|entry| entry.track.id.as_str())
                .collect::<Vec<_>>(),
            ids
        );
    }
}
