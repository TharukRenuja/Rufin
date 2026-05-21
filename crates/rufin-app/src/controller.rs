use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use rufin_core::{
    Album, AlbumId, AppSettings, Artist, ArtistId, FolderPathItem, Genre, GenreId, HomeSection,
    HomeSectionKind, LibrarySourceSelection, LocalLibraryFolder, MusicFolder, MusicFolderId,
    PlaybackSettings, Playlist, PlaylistId, QueueEngine, QueueEntry, QueueEntryId, QueueSnapshot,
    RepeatMode, ServerId, ServerIdentity, Track, TrackId,
};
use rufin_playback::{
    FakePlaybackBackend, LazyGStreamerPlaybackBackend, PlaybackBackend, PlaybackCommand,
    PlaybackEvent, PlaybackState, PlaybackTrack, PreparedPlaybackItem, StreamDescriptor,
};
use rufin_provider::{
    FavoriteItemId, FolderDetail, Lyrics, MusicProvider, PagedRequest, PlaybackReport,
    PlaybackReportKind, PlaylistEntry, SavedProviderSession, SearchResults, StreamRequest,
};
use rufin_provider_local::{LOCAL_PROVIDER_ID, LocalProvider};
#[cfg(unix)]
use rufin_secrets::SecretServiceStore;
use rufin_secrets::{MemorySecretStore, SecretStore};
use rufin_store::{
    CachedArtistDetail, CachedGenreDetail, SavedServer, ServerLocalAccess, Store, StoreError,
    SyncState,
};
use rufin_test_support::{FakeProvider, FakeScale};
use serde::Deserialize;
use tokio::runtime::Runtime;
use tracing::{debug, info, instrument, warn};

use crate::external_metadata;
use crate::external_scrobbling::{self, ExternalScrobbleState};
use crate::providers::{
    JellyfinLyricsSearch, LoadedProvider, StreamingProvider, login_provider, provider_display_name,
    provider_from_saved,
};

mod covers;
mod discovery;
mod random;

pub use discovery::DiscoveredServer;
pub use random::{RandomPlayAction, RandomPlayRequest};

const PAGE_SIZE: usize = 500;
const SNAPSHOT_GRID_LIMIT: usize = 500;
const SNAPSHOT_TRACK_LIMIT: usize = 25_000;
const STARTUP_CACHE_STALE_SECONDS: i64 = 24 * 60 * 60;
const IMAGE_TAG_UNTAGGED: &str = "untagged";
const AUTO_DJ_ITEM_COUNT: usize = 5;
const AUTO_DJ_THRESHOLD: usize = 1;
const AUTO_DJ_LIBRARY_LIMIT: usize = 5_000;
const SEEK_SETTLE_WINDOW: Duration = Duration::from_millis(900);
const SEEK_POSITION_TOLERANCE_MILLIS: u64 = 1_500;
const DATABASE_FILE_NAME: &str = "rufin.sqlite";
const SETTINGS_FILE_NAME: &str = "settings.json";
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
    pub favorites: Vec<Track>,
    pub search: SearchResults,
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
            repeat_mode: RepeatMode::All,
            shuffle_enabled: false,
            auto_dj_enabled: true,
            buffering_percent: None,
            last_error: None,
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
            favorites: Vec::new(),
            search: SearchResults::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum ControllerEvent {
    Snapshot(Box<LibrarySnapshot>),
    HomeSectionsUpdated {
        snapshot: Box<LibrarySnapshot>,
        include_explore: bool,
    },
    HomeSectionPrefetched {
        server_id: ServerId,
        section: HomeSection,
    },
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
        artist_name: String,
        track_name: String,
        results: Vec<LyricsSearchResult>,
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
    ServerDiscovery {
        servers: Vec<DiscoveredServer>,
        status: String,
        running: bool,
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
    external_scrobble_state: Arc<Mutex<ExternalScrobbleState>>,
    events: Sender<ControllerEvent>,
    sync_in_flight: Arc<Mutex<HashSet<ServerId>>>,
    home_refresh_in_flight: Arc<Mutex<HashSet<ServerId>>>,
    playlist_refresh_in_flight: Arc<Mutex<HashSet<ServerId>>>,
    explore_prefetch_in_flight: Arc<Mutex<HashSet<ServerId>>>,
    cover_in_flight: Arc<Mutex<HashSet<String>>>,
    external_cover_prefetch_in_flight: Arc<Mutex<HashSet<ServerId>>>,
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

struct PlaylistRefreshContext {
    store: StoreHandle,
    runtime: Arc<Runtime>,
    secrets: Arc<dyn SecretStore>,
    events: Sender<ControllerEvent>,
    sync_in_flight: Arc<Mutex<HashSet<ServerId>>>,
    playlist_refresh_in_flight: Arc<Mutex<HashSet<ServerId>>>,
}

#[derive(Clone)]
struct SyncContext {
    store: StoreHandle,
    runtime: Arc<Runtime>,
    secrets: Arc<dyn SecretStore>,
    events: Sender<ControllerEvent>,
    sync_in_flight: Arc<Mutex<HashSet<ServerId>>>,
    cover_in_flight: Arc<Mutex<HashSet<String>>>,
    external_cover_prefetch_in_flight: Arc<Mutex<HashSet<ServerId>>>,
    cover_slots: Arc<(Mutex<usize>, Condvar)>,
}

struct ExplorePrefetchContext {
    store: StoreHandle,
    runtime: Arc<Runtime>,
    secrets: Arc<dyn SecretStore>,
    events: Sender<ControllerEvent>,
    sync_in_flight: Arc<Mutex<HashSet<ServerId>>>,
    explore_prefetch_in_flight: Arc<Mutex<HashSet<ServerId>>>,
}

#[derive(Clone, Copy, Debug)]
enum HomeRefreshTarget {
    WithoutExplore,
    Section(HomeSectionKind),
}

#[derive(Clone, Copy, Debug)]
struct PendingSeek {
    target_millis: u64,
    expires_at: Instant,
}

#[derive(Clone)]
enum StoreHandle {
    Path {
        database_path: PathBuf,
        settings_path: PathBuf,
    },
    Memory {
        store: Arc<Mutex<Store>>,
        settings: Arc<Mutex<AppSettings>>,
    },
}

impl StoreHandle {
    fn open_for_app() -> Result<Self, String> {
        let database_path = data_dir()
            .map(|dir| dir.join(DATABASE_FILE_NAME))
            .unwrap_or_else(|| PathBuf::from(DATABASE_FILE_NAME));
        if let Some(parent) = database_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        Store::open(&database_path).map_err(|error| error.to_string())?;

        let settings_path = config_dir()
            .map(|dir| dir.join(SETTINGS_FILE_NAME))
            .unwrap_or_else(|| PathBuf::from(SETTINGS_FILE_NAME));
        let handle = Self::Path {
            database_path,
            settings_path,
        };
        Ok(handle)
    }

    fn open_memory() -> Result<Self, String> {
        Store::open_memory()
            .map(|store| Self::Memory {
                store: Arc::new(Mutex::new(store)),
                settings: Arc::new(Mutex::new(AppSettings::default())),
            })
            .map_err(|error| error.to_string())
    }

    fn with_store<T>(
        &self,
        operation: impl FnOnce(&Store) -> Result<T, StoreError>,
    ) -> Result<T, String> {
        match self {
            Self::Path { database_path, .. } => {
                let store = Store::open(database_path).map_err(|error| error.to_string())?;
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

    fn load_settings(&self) -> Result<AppSettings, String> {
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

    fn save_settings(&self, settings: &AppSettings) -> Result<(), String> {
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

    fn database_exists(&self) -> bool {
        match self {
            Self::Path { database_path, .. } => database_path.exists(),
            Self::Memory { .. } => true,
        }
    }
}

impl AppController {
    pub fn load_settings(&self) -> AppSettings {
        load_settings_from_store(&self.store)
    }

    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), String> {
        self.store.save_settings(settings)
    }

    pub fn reload_snapshot(&self) {
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || match load_snapshot(&store) {
            Ok(snapshot) => {
                let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
            }
            Err(error) => {
                let _sent = events.send(ControllerEvent::Error(error));
            }
        });
    }

    pub fn cached_album_detail(
        &self,
        album_id: &AlbumId,
    ) -> Result<Option<(Album, Vec<Track>)>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(None);
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_album_detail(&saved.server.id, album_id))
            .map(|detail| {
                detail.map(|(mut album, mut tracks)| {
                    external_metadata::normalize_album_detail(&mut album, &mut tracks, &settings);
                    (album, tracks)
                })
            })
    }

    pub fn cached_album_tracks(
        &self,
        album_ids: &[AlbumId],
    ) -> Result<std::collections::HashMap<AlbumId, Vec<Track>>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(std::collections::HashMap::new());
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_tracks_for_albums(&saved.server.id, album_ids))
            .map(|mut tracks_by_album| {
                for tracks in tracks_by_album.values_mut() {
                    external_metadata::normalize_tracks(tracks, &settings);
                }
                tracks_by_album
            })
    }

    pub fn cached_artist_detail(
        &self,
        artist_id: &ArtistId,
    ) -> Result<Option<CachedArtistDetail>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(None);
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_artist_detail(&saved.server.id, artist_id))
            .map(|detail| {
                detail.map(|mut detail| {
                    external_metadata::normalize_artist(&mut detail.artist, &settings);
                    external_metadata::normalize_albums(&mut detail.albums, &settings);
                    external_metadata::normalize_albums(&mut detail.appears_on, &settings);
                    external_metadata::normalize_tracks(&mut detail.tracks, &settings);
                    detail
                })
            })
    }

    pub fn cached_playlist_detail(
        &self,
        playlist_id: &PlaylistId,
    ) -> Result<Option<rufin_provider::PlaylistDetail>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(None);
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_playlist_detail(&saved.server.id, playlist_id))
            .map(|detail| {
                detail.map(|mut detail| {
                    external_metadata::normalize_tracks(&mut detail.tracks, &settings);
                    for entry in &mut detail.entries {
                        external_metadata::normalize_track(&mut entry.track, &settings);
                    }
                    detail
                })
            })
    }

    pub fn cached_genre_detail(
        &self,
        genre_id: &GenreId,
    ) -> Result<Option<CachedGenreDetail>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(None);
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_genre_detail(&saved.server.id, genre_id))
            .map(|detail| {
                detail.map(|mut detail| {
                    external_metadata::normalize_albums(&mut detail.albums, &settings);
                    external_metadata::normalize_tracks(&mut detail.tracks, &settings);
                    detail
                })
            })
    }

    pub fn cached_albums_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Album>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_albums(&saved.server.id, offset, limit))
            .map(|mut page| {
                external_metadata::normalize_albums(&mut page.items, &settings);
                page
            })
    }

    pub fn cached_albums_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Album>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_albums_matching(&saved.server.id, query, offset, limit))
            .map(|mut page| {
                external_metadata::normalize_albums(&mut page.items, &settings);
                page
            })
    }

    pub fn cached_tracks_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Track>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_tracks(&saved.server.id, offset, limit))
            .map(|mut page| {
                external_metadata::normalize_tracks(&mut page.items, &settings);
                page
            })
    }

    pub fn cached_tracks_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Track>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_tracks_matching(&saved.server.id, query, offset, limit))
            .map(|mut page| {
                external_metadata::normalize_tracks(&mut page.items, &settings);
                page
            })
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
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| store.load_artists(&saved.server.id, album_artist, offset, limit))
            .map(|mut page| {
                external_metadata::normalize_artists(&mut page.items, &settings);
                page
            })
    }

    pub fn cached_artists_page_matching(
        &self,
        album_artist: bool,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Artist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        let settings = load_settings_for_saved(&self.store, &saved);
        self.store
            .with_store(|store| {
                store.load_artists_matching(&saved.server.id, album_artist, query, offset, limit)
            })
            .map(|mut page| {
                external_metadata::normalize_artists(&mut page.items, &settings);
                page
            })
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

    pub fn cached_genres_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Genre>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        self.store
            .with_store(|store| store.load_genres_matching(&saved.server.id, query, offset, limit))
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

    pub fn cached_playlists_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<rufin_provider::PagedResponse<Playlist>, String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(rufin_provider::PagedResponse::new(Vec::new(), 0));
        };
        self.store.with_store(|store| {
            store.load_playlists_matching(&saved.server.id, query, offset, limit)
        })
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
            let playback_snapshot = playback_snapshot_from_queue(
                queue.as_ref(),
                settings.auto_dj_enabled,
                &settings.playback,
            );
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
                external_scrobble_state: Arc::new(Mutex::new(ExternalScrobbleState::default())),
                events,
                sync_in_flight: Arc::new(Mutex::new(HashSet::new())),
                home_refresh_in_flight: Arc::new(Mutex::new(HashSet::new())),
                playlist_refresh_in_flight: Arc::new(Mutex::new(HashSet::new())),
                explore_prefetch_in_flight: Arc::new(Mutex::new(HashSet::new())),
                cover_in_flight: Arc::new(Mutex::new(HashSet::new())),
                external_cover_prefetch_in_flight: Arc::new(Mutex::new(HashSet::new())),
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
        let playback_snapshot = playback_snapshot_from_queue(
            queue.as_ref(),
            settings.auto_dj_enabled,
            &settings.playback,
        );
        let controller = Self {
            store,
            runtime,
            secrets: platform_secret_store(),
            queue: Arc::new(Mutex::new(queue)),
            playback: Arc::new(Mutex::new(playback_backend(false))),
            playback_snapshot: Arc::new(Mutex::new(playback_snapshot.clone())),
            pending_seek: Arc::new(Mutex::new(None)),
            auto_dj_enabled: Arc::new(Mutex::new(settings.auto_dj_enabled)),
            last_progress_snapshot: Arc::new(Mutex::new(None)),
            last_report_snapshot: Arc::new(Mutex::new(None)),
            external_scrobble_state: Arc::new(Mutex::new(ExternalScrobbleState::default())),
            events,
            sync_in_flight: Arc::new(Mutex::new(HashSet::new())),
            home_refresh_in_flight: Arc::new(Mutex::new(HashSet::new())),
            playlist_refresh_in_flight: Arc::new(Mutex::new(HashSet::new())),
            explore_prefetch_in_flight: Arc::new(Mutex::new(HashSet::new())),
            cover_in_flight: Arc::new(Mutex::new(HashSet::new())),
            external_cover_prefetch_in_flight: Arc::new(Mutex::new(HashSet::new())),
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
                volume: settings.playback.volume,
                muted: settings.playback.muted,
                ..PlaybackSnapshot::default()
            })),
            pending_seek: Arc::new(Mutex::new(None)),
            auto_dj_enabled: Arc::new(Mutex::new(settings.auto_dj_enabled)),
            last_progress_snapshot: Arc::new(Mutex::new(None)),
            last_report_snapshot: Arc::new(Mutex::new(None)),
            external_scrobble_state: Arc::new(Mutex::new(ExternalScrobbleState::default())),
            events,
            sync_in_flight: Arc::new(Mutex::new(HashSet::new())),
            home_refresh_in_flight: Arc::new(Mutex::new(HashSet::new())),
            playlist_refresh_in_flight: Arc::new(Mutex::new(HashSet::new())),
            explore_prefetch_in_flight: Arc::new(Mutex::new(HashSet::new())),
            cover_in_flight: Arc::new(Mutex::new(HashSet::new())),
            external_cover_prefetch_in_flight: Arc::new(Mutex::new(HashSet::new())),
            cover_slots: Arc::new((Mutex::new(0), Condvar::new())),
        };
        (
            controller,
            receiver,
            snapshot,
            None,
            PlaybackSnapshot {
                auto_dj_enabled: settings.auto_dj_enabled,
                volume: settings.playback.volume,
                muted: settings.playback.muted,
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
        platform_secret_store()
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

    #[cfg(test)]
    pub fn resync_active_server(&self) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_sync(saved);
        } else {
            let _sent = self.events.send(ControllerEvent::Error(
                "No active music server is saved.".to_string(),
            ));
        }
    }

    pub fn resync_server(&self, server_id: ServerId) {
        let saved = self
            .store
            .with_store(|store| {
                Ok(store
                    .list_servers()?
                    .into_iter()
                    .find(|saved| saved.server.id == server_id))
            })
            .unwrap_or(None);
        if let Some(saved) = saved {
            self.start_sync(saved);
        } else {
            let _sent = self.events.send(ControllerEvent::Error(
                "The selected server is no longer saved.".to_string(),
            ));
        }
    }

    pub fn refresh_home_sections_without_explore_for_active(&self) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_home_refresh_for_saved(saved, HomeRefreshTarget::WithoutExplore);
        }
    }

    pub fn refresh_home_section_for_active(&self, kind: HomeSectionKind) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_home_refresh_for_saved(saved, HomeRefreshTarget::Section(kind));
        }
    }

    pub fn refresh_playlists_for_active(&self) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_playlist_refresh_for_saved(saved);
        }
    }

    pub fn prefetch_explore_for_active(&self) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_explore_prefetch_for_saved(saved);
        }
    }

    pub fn promote_prefetched_explore_for_active(&self, section: HomeSection) {
        if section.kind != HomeSectionKind::Explore {
            return;
        }
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        let Some(saved) = active else {
            return;
        };
        start_prefetched_home_section_promotion_thread(
            self.store.clone(),
            self.events.clone(),
            saved.server.id,
            section,
        );
    }

    fn start_explore_prefetch_for_saved(&self, saved: SavedServer) {
        start_explore_prefetch_thread(
            ExplorePrefetchContext {
                store: self.store.clone(),
                runtime: Arc::clone(&self.runtime),
                secrets: Arc::clone(&self.secrets),
                events: self.events.clone(),
                sync_in_flight: Arc::clone(&self.sync_in_flight),
                explore_prefetch_in_flight: Arc::clone(&self.explore_prefetch_in_flight),
            },
            saved,
        );
    }

    fn start_home_refresh_for_saved(&self, saved: SavedServer, target: HomeRefreshTarget) {
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
            target,
        );
    }

    fn start_playlist_refresh_for_saved(&self, saved: SavedServer) {
        start_playlist_refresh_thread(
            PlaylistRefreshContext {
                store: self.store.clone(),
                runtime: Arc::clone(&self.runtime),
                secrets: Arc::clone(&self.secrets),
                events: self.events.clone(),
                sync_in_flight: Arc::clone(&self.sync_in_flight),
                playlist_refresh_in_flight: Arc::clone(&self.playlist_refresh_in_flight),
            },
            saved,
        );
    }

    fn sync_context(&self) -> SyncContext {
        SyncContext {
            store: self.store.clone(),
            runtime: Arc::clone(&self.runtime),
            secrets: Arc::clone(&self.secrets),
            events: self.events.clone(),
            sync_in_flight: Arc::clone(&self.sync_in_flight),
            cover_in_flight: Arc::clone(&self.cover_in_flight),
            external_cover_prefetch_in_flight: Arc::clone(&self.external_cover_prefetch_in_flight),
            cover_slots: Arc::clone(&self.cover_slots),
        }
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
            self.persist_playback_settings(|settings| {
                settings.volume = volume;
            });
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
            self.persist_playback_settings(|settings| {
                settings.muted = muted;
            });
            self.update_playback_snapshot(|snapshot| {
                snapshot.muted = muted;
            });
            self.emit_playback_snapshot();
        }
    }

    pub fn update_playback_settings(&self, mut playback_settings: PlaybackSettings) {
        playback_settings.sanitize();
        let mut settings = self.load_settings();
        if settings.playback != playback_settings {
            settings.playback = playback_settings.clone();
            if let Err(error) = self.save_settings(&settings) {
                let _sent = self.events.send(ControllerEvent::Error(error));
                return;
            }
        }
        if let Err(error) =
            self.send_playback_command(PlaybackCommand::UpdateSettings(playback_settings.clone()))
        {
            let _sent = self.events.send(ControllerEvent::Error(error));
        }
        self.update_playback_snapshot(|snapshot| {
            snapshot.volume = playback_settings.volume;
            snapshot.muted = playback_settings.muted;
        });
        self.prepare_next_stream();
        self.emit_playback_snapshot();
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
                PlaybackEvent::PreparedTrackStarted(track) => {
                    self.advance_after_prepared_track_started(track);
                }
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

    #[cfg(test)]
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
                    "No active music server is saved.".to_string(),
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

    pub fn clear_server_cache(&self, server_id: ServerId) {
        let store = self.store.clone();
        let events = self.events.clone();
        let sync_in_flight = Arc::clone(&self.sync_in_flight);
        thread::spawn(move || {
            let saved = match store.with_store(|store| {
                Ok(store
                    .list_servers()?
                    .into_iter()
                    .find(|saved| saved.server.id == server_id))
            }) {
                Ok(Some(saved)) => saved,
                Ok(None) => {
                    let _sent = events.send(ControllerEvent::Error(
                        "The selected server is no longer saved.".to_string(),
                    ));
                    return;
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
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
            emit_snapshot(&store, &events);
        });
    }

    #[cfg(test)]
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

    pub fn forget_server(&self, server_id: ServerId) {
        let store = self.store.clone();
        let events = self.events.clone();
        let secrets = Arc::clone(&self.secrets);
        let queue = Arc::clone(&self.queue);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        let sync_in_flight = Arc::clone(&self.sync_in_flight);
        thread::spawn(move || {
            let saved = match store.with_store(|store| {
                let active_id = store.active_server()?.map(|saved| saved.server.id);
                let saved = store
                    .list_servers()?
                    .into_iter()
                    .find(|saved| saved.server.id == server_id);
                Ok((saved, active_id))
            }) {
                Ok((Some(saved), active_id)) => (saved, active_id),
                Ok((None, _)) => {
                    let _sent = events.send(ControllerEvent::Error(
                        "The selected server is no longer saved.".to_string(),
                    ));
                    return;
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            let (saved, active_id) = saved;
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
            let mut settings = load_settings_from_store(&store);
            if settings.sources.selected
                == Some(LibrarySourceSelection::Server(saved.server.id.clone()))
            {
                settings.sources.selected = None;
                if let Err(error) = store.save_settings(&settings) {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
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
            if active_id.as_ref() == Some(&saved.server.id) {
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
            }
            emit_snapshot(&store, &events);
        });
    }

    #[instrument(skip(self, password), fields(provider = provider.provider_id(), server_url = %server_url, username = %username, trust_invalid_cert = trust_invalid_cert))]
    pub fn login(
        &self,
        provider: StreamingProvider,
        server_url: String,
        username: String,
        password: String,
        trust_invalid_cert: bool,
        local_access_root: Option<PathBuf>,
        path_replace_from: Option<String>,
    ) {
        let sync_context = self.sync_context();
        let store = sync_context.store.clone();
        let runtime = Arc::clone(&sync_context.runtime);
        let secrets = Arc::clone(&sync_context.secrets);
        let events = sync_context.events.clone();
        let queue = Arc::clone(&self.queue);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        thread::spawn(move || {
            let provider_name = provider.title();
            let _sent = events.send(ControllerEvent::LoginStatus(format!(
                "Checking {provider_name} server..."
            )));
            let result = runtime.block_on(login_provider(
                provider,
                server_url,
                username,
                password,
                trust_invalid_cert,
            ));

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
                if let Some(root) = local_access_root.as_ref().and_then(|path| path.to_str()) {
                    store.save_server_local_access(&ServerLocalAccess {
                        server_id: saved.server.id.clone(),
                        root_path: root.to_string(),
                        path_replace_from: trimmed_optional(path_replace_from.as_deref()),
                        path_replace_to: Some(root.to_string()),
                    })?;
                }
                store.set_active_server(&saved.server.id)?;
                Ok(())
            }) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            let mut settings = load_settings_from_store(&store);
            settings.sources.selected =
                Some(LibrarySourceSelection::Server(saved.server.id.clone()));
            settings.migrate_defaults();
            if let Err(error) = store.save_settings(&settings) {
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
                &load_settings_from_store(&store).playback,
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

            start_sync_thread(sync_context, saved);
        });
    }

    pub fn add_local_server(&self, root_path: PathBuf) {
        self.add_local_library_folder_with_selection(root_path, true);
    }

    pub fn add_local_library_folder(&self, root_path: PathBuf) {
        self.add_local_library_folder_with_selection(root_path, false);
    }

    fn add_local_library_folder_with_selection(&self, root_path: PathBuf, select_local: bool) {
        let sync_context = self.sync_context();
        let store = sync_context.store.clone();
        let events = sync_context.events.clone();
        thread::spawn(move || {
            let identity = match LocalProvider::identity_for_root(&root_path) {
                Ok(identity) => identity,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error.to_string()));
                    return;
                }
            };
            let mut settings = load_settings_from_store(&store);
            if !settings
                .sources
                .local_folders
                .iter()
                .any(|folder| folder.path == identity.base_url)
            {
                settings.sources.local_folders.push(LocalLibraryFolder {
                    path: identity.base_url,
                });
            }
            if select_local {
                settings.sources.selected = Some(LibrarySourceSelection::Local);
            }
            settings.migrate_defaults();
            if let Err(error) = store.save_settings(&settings) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            let saved = match ensure_local_source_server(&store) {
                Ok(saved) => saved,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            if select_local
                && let Err(error) =
                    store.with_store(|store| store.set_active_server(&saved.server.id))
            {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            emit_snapshot(&store, &events);
            if select_local {
                start_sync_thread(sync_context, saved);
            }
        });
    }

    pub fn remove_local_library_folder(&self, path: String) {
        let sync_context = self.sync_context();
        let store = sync_context.store.clone();
        let events = sync_context.events.clone();
        thread::spawn(move || {
            let mut settings = load_settings_from_store(&store);
            let before = settings.sources.local_folders.len();
            settings
                .sources
                .local_folders
                .retain(|folder| folder.path != path);
            if settings.sources.local_folders.len() == before {
                return;
            }
            let selected_local = matches!(
                settings.sources.selected,
                Some(LibrarySourceSelection::Local)
            );
            if selected_local && settings.sources.local_folders.is_empty() {
                settings.sources.selected = None;
            }
            settings.migrate_defaults();
            if let Err(error) = store.save_settings(&settings) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            let saved = match ensure_local_source_server(&store) {
                Ok(saved) => saved,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            let result = store.with_store(|store| {
                if selected_local && !settings.sources.local_folders.is_empty() {
                    store.set_active_server(&saved.server.id)?;
                }
                store.clear_library_cache(&saved.server.id)
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            emit_snapshot(&store, &events);
            if selected_local && !settings.sources.local_folders.is_empty() {
                start_sync_thread(sync_context, saved);
            }
        });
    }

    pub fn select_source(&self, source: LibrarySourceSelection) {
        let sync_context = self.sync_context();
        let store = sync_context.store.clone();
        let events = sync_context.events.clone();
        thread::spawn(move || {
            let mut settings = load_settings_from_store(&store);
            settings.sources.selected = Some(source.clone());
            settings.migrate_defaults();
            if let Err(error) = store.save_settings(&settings) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }

            let sync_saved = match source {
                LibrarySourceSelection::Local => {
                    let saved = match ensure_local_source_server(&store) {
                        Ok(saved) => saved,
                        Err(error) => {
                            let _sent = events.send(ControllerEvent::Error(error));
                            return;
                        }
                    };
                    if let Err(error) =
                        store.with_store(|store| store.set_active_server(&saved.server.id))
                    {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                    (!settings.sources.local_folders.is_empty()).then_some(saved)
                }
                LibrarySourceSelection::Server(server_id) => {
                    let saved = match store.with_store(|store| {
                        let saved = store
                            .list_servers()?
                            .into_iter()
                            .find(|saved| saved.server.id == server_id);
                        if saved.is_some() {
                            store.set_active_server(&server_id)?;
                        }
                        Ok(saved)
                    }) {
                        Ok(Some(saved)) => saved,
                        Ok(None) => {
                            let _sent = events.send(ControllerEvent::Error(
                                "The selected source is no longer saved.".to_string(),
                            ));
                            return;
                        }
                        Err(error) => {
                            let _sent = events.send(ControllerEvent::Error(error));
                            return;
                        }
                    };
                    active_server_needs_sync(&store, &saved.server.id).then_some(saved)
                }
            };

            emit_snapshot(&store, &events);
            if let Some(saved) = sync_saved {
                start_sync_thread(sync_context, saved);
            }
        });
    }

    pub fn save_server_local_access(
        &self,
        server_id: ServerId,
        root_path: PathBuf,
        path_replace_from: Option<String>,
        path_replace_to: Option<String>,
    ) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let queue = Arc::clone(&self.queue);
        let playback = Arc::clone(&self.playback);
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(root_path) = root_path.to_str().map(ToString::to_string) else {
                let _sent = events.send(ControllerEvent::Error(
                    "Could not use the selected local folder path.".to_string(),
                ));
                return;
            };
            let path_replace_to =
                trimmed_optional(path_replace_to.as_deref()).unwrap_or_else(|| root_path.clone());
            let matched_server_id = server_id.clone();
            let result = store.with_store(|store| {
                store.save_server_local_access(&ServerLocalAccess {
                    server_id,
                    root_path: root_path.clone(),
                    path_replace_from: trimmed_optional(path_replace_from.as_deref()),
                    path_replace_to: Some(path_replace_to),
                })
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Err(error) =
                runtime.block_on(refresh_local_track_matches(&store, &matched_server_id))
            {
                warn!(%error, "failed to refresh local track matches");
            }
            prepare_next_stream_from_handles(
                store.clone(),
                Arc::clone(&runtime),
                Arc::clone(&secrets),
                Arc::clone(&playback),
                Arc::clone(&queue),
                events.clone(),
            );
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

    pub fn update_server_settings(
        &self,
        server_id: ServerId,
        name: String,
        base_url: String,
        trust_invalid_cert: bool,
    ) {
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            let result = store.with_store(|store| {
                let Some(mut saved) = store
                    .list_servers()?
                    .into_iter()
                    .find(|saved| saved.server.id == server_id)
                else {
                    return Ok(false);
                };
                if saved.server.provider != LOCAL_PROVIDER_ID && base_url.trim().is_empty() {
                    return Ok(false);
                }
                let next_name = name.trim().to_string();
                let next_base_url = if saved.server.provider == LOCAL_PROVIDER_ID {
                    saved.server.base_url.clone()
                } else {
                    base_url.trim().to_string()
                };
                let changed = saved.server.name != next_name
                    || saved.server.base_url != next_base_url
                    || saved.trust_invalid_cert != trust_invalid_cert;
                if !changed {
                    return Ok(false);
                }
                saved.server.name = next_name;
                saved.server.base_url = next_base_url;
                saved.trust_invalid_cert = trust_invalid_cert;
                store.save_server(&saved)?;
                Ok(true)
            });
            match result {
                Ok(true) => {
                    let _sent = events.send(ControllerEvent::LoginStatus(
                        "Server settings saved.".to_string(),
                    ));
                    emit_snapshot(&store, &events);
                }
                Ok(false) => {}
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }
        });
    }

    pub fn clear_server_local_access(&self, server_id: ServerId) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let queue = Arc::clone(&self.queue);
        let playback = Arc::clone(&self.playback);
        let events = self.events.clone();
        thread::spawn(move || {
            if let Err(error) = store.with_store(|store| {
                store.delete_server_local_access(&server_id)?;
                store.delete_track_local_matches(&server_id)
            }) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            prepare_next_stream_from_handles(
                store.clone(),
                Arc::clone(&runtime),
                Arc::clone(&secrets),
                Arc::clone(&playback),
                Arc::clone(&queue),
                events.clone(),
            );
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

    pub fn set_selected_music_folder(&self, server_id: ServerId, folder_id: Option<MusicFolderId>) {
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            if let Err(error) = store.with_store(|store| {
                store.set_selected_music_folder_id(&server_id, folder_id.as_ref())
            }) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
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

    pub fn load_folder_for_active(&self, request_id: u64, path: Vec<FolderPathItem>) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        thread::spawn(move || {
            let result = load_folder_detail(&store, &runtime, &secrets, &path);
            match result {
                Ok(detail) => {
                    let _sent = events.send(ControllerEvent::FolderLoaded {
                        request_id,
                        path,
                        detail,
                    });
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::FolderLoadFailed {
                        request_id,
                        path,
                        error,
                    });
                }
            }
        });
    }

    pub fn search(&self, query: String) {
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            let settings = load_settings_for_active_server(&store);
            let mut snapshot = match load_snapshot(&store) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            if let Some(server) = &snapshot.server {
                match store.with_store(|store| store.search_library(&server.id, &query, 50)) {
                    Ok(mut results) => {
                        external_metadata::normalize_search_results(&mut results, &settings);
                        snapshot.search = results;
                    }
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
                    "No active music server is saved.".to_string(),
                ));
                return;
            };

            if saved.server.provider != "fake" && saved.server.provider != "local" {
                let result =
                    provider_for_saved(&store, &runtime, &secrets, &saved).and_then(|provider| {
                        runtime
                            .block_on(
                                provider
                                    .as_music_provider()
                                    .set_favorite(item_id.clone(), favorite),
                            )
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
                    "No active music server is saved.".to_string(),
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
                        .block_on(
                            provider
                                .as_music_provider()
                                .create_playlist(&name, &track_ids),
                        )
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
                    "No active music server is saved.".to_string(),
                ));
                return;
            };
            if saved.server.provider != "fake" && saved.server.provider != "local" {
                let result =
                    provider_for_saved(&store, &runtime, &secrets, &saved).and_then(|provider| {
                        runtime
                            .block_on(
                                provider
                                    .as_music_provider()
                                    .rename_playlist(&playlist_id, &name),
                            )
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
                    "No active music server is saved.".to_string(),
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

    pub fn request_server_lyrics_for_current(&self) {
        self.request_lyrics_for_current_with_search(true, JellyfinLyricsSearch::ServerOnly);
    }

    pub fn refresh_lyrics_for_current(&self) {
        self.request_lyrics_for_current_with_cache(false);
    }

    fn request_lyrics_for_current_with_cache(&self, use_cache: bool) {
        let settings = load_settings_from_store(&self.store);
        self.request_lyrics_for_current_with_search(
            use_cache,
            lyrics_search_for_settings(&settings),
        );
    }

    fn request_lyrics_for_current_with_search(
        &self,
        use_cache: bool,
        search: JellyfinLyricsSearch,
    ) {
        let Some((server_id, entry, _position)) = self.current_queue_entry() else {
            debug!("lyrics request skipped because the queue has no current track");
            let _sent = self.events.send(ControllerEvent::Lyrics(Box::new(None)));
            return;
        };
        if let Some(lyrics) = local_sidecar_lyrics(&self.store, &server_id, &entry.track_id) {
            debug!(track_id = %entry.track_id, "loaded lyrics from local sidecar");
            let _saved = self
                .store
                .with_store(|store| store.save_lyrics(&server_id, &lyrics));
            let _sent = self
                .events
                .send(ControllerEvent::Lyrics(Box::new(Some(lyrics))));
            return;
        }
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

    pub fn search_lyrics_for_current(&self, artist_name: String, track_name: String) {
        let artist_name = artist_name.trim().to_string();
        let track_name = track_name.trim().to_string();
        if artist_name.is_empty() && track_name.is_empty() {
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
        thread::spawn(move || match lrclib_search(&artist_name, &track_name) {
            Ok(results) => {
                let _sent = events.send(ControllerEvent::LyricsSearchResults {
                    track_id,
                    artist_name,
                    track_name,
                    results,
                });
            }
            Err(error) => {
                let _sent = events.send(ControllerEvent::Error(error));
                let _sent = events.send(ControllerEvent::LyricsSearchResults {
                    track_id,
                    artist_name,
                    track_name,
                    results: Vec::new(),
                });
            }
        });
    }

    pub fn save_lyrics_search_result(
        &self,
        track_id: TrackId,
        result: LyricsSearchResult,
        output_path: Option<PathBuf>,
    ) {
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
        thread::spawn(move || {
            match save_lrclib_result(&store, &server_id, &entry, &result, output_path) {
                Ok((path, lyrics)) => {
                    let _saved = store.with_store(|store| store.save_lyrics(&server_id, &lyrics));
                    let _sent = events.send(ControllerEvent::LyricsSaved { path, lyrics });
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }
        });
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

        let settings = load_settings_for_active_server(&self.store);
        let mut tracks = self
            .store
            .with_store(|store| store.load_tracks(&server_id, 0, AUTO_DJ_LIBRARY_LIMIT))
            .map(|page| page.items)?;
        external_metadata::normalize_tracks(&mut tracks, &settings);
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
        self.prepare_next_stream();
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

    fn persist_playback_settings(&self, update: impl FnOnce(&mut PlaybackSettings)) {
        let mut settings = self.load_settings();
        update(&mut settings.playback);
        settings.playback.sanitize();
        if let Err(error) = self.save_settings(&settings) {
            let _sent = self.events.send(ControllerEvent::Error(error));
        }
    }

    fn start_current_track(&self) {
        let Some((server_id, entry, next_entry, position_seconds, playback_settings)) =
            self.current_playback_request()
        else {
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
            let item = match resolve_prepared_item(
                &store,
                &runtime,
                &secrets,
                &server_id,
                &entry,
                &playback_settings,
            ) {
                Ok(item) => item,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            let next = next_entry.and_then(|entry| {
                match resolve_prepared_item(
                    &store,
                    &runtime,
                    &secrets,
                    &server_id,
                    &entry,
                    &playback_settings,
                ) {
                    Ok(item) => Some(item),
                    Err(error) => {
                        let _sent = events.send(ControllerEvent::Error(error));
                        None
                    }
                }
            });
            let command = PlaybackCommand::PlayPrepared {
                item,
                next,
                start_position_seconds: position_seconds,
                settings: playback_settings,
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

    fn current_playback_request(
        &self,
    ) -> Option<(
        ServerId,
        QueueEntry,
        Option<QueueEntry>,
        u32,
        PlaybackSettings,
    )> {
        let playback_settings = self.load_settings().playback;
        self.queue.lock().ok().and_then(|queue| {
            let queue = queue.as_ref()?;
            let snapshot = queue.snapshot();
            let entry = queue.current()?.clone();
            let next = next_queue_entry_after_current(queue);
            Some((
                snapshot.server_id,
                entry,
                next,
                snapshot.progress_seconds,
                playback_settings,
            ))
        })
    }

    fn prepare_next_stream(&self) {
        prepare_next_stream_from_handles(
            self.store.clone(),
            Arc::clone(&self.runtime),
            Arc::clone(&self.secrets),
            Arc::clone(&self.playback),
            Arc::clone(&self.queue),
            self.events.clone(),
        );
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
        external_scrobbling::report(
            &settings,
            &self.external_scrobble_state,
            kind,
            failed,
            &snapshot,
            &current,
        );
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

    fn advance_after_prepared_track_started(&self, track: PlaybackTrack) {
        self.report_playback(PlaybackReportKind::Stopped, false);
        self.auto_dj_top_up_or_emit_error();
        let mut has_next = false;
        let result = self.with_queue_mut(|queue| {
            has_next = queue.advance_after_end_of_stream().is_some();
            if has_next && queue.current().is_some_and(|entry| entry.track_id != track.id) {
                warn!(
                    expected_track_id = %track.id.as_str(),
                    actual_track_id = queue.current().map(|entry| entry.track_id.as_str()).unwrap_or(""),
                    "prepared playback advanced to a different queue entry"
                );
            }
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        if !has_next {
            self.stop();
            return;
        }
        self.persist_and_emit_queue();
        self.update_playback_snapshot(|snapshot| {
            snapshot.state = PlaybackState::Playing;
            snapshot.position_seconds = 0;
            snapshot.position_millis = 0;
            snapshot.duration_seconds = track.duration_seconds;
            snapshot.buffering_percent = None;
            snapshot.last_error = None;
        });
        self.emit_playback_snapshot();
        self.report_playback(PlaybackReportKind::Started, false);
    }

    fn start_sync(&self, saved: SavedServer) {
        start_sync_thread(self.sync_context(), saved);
    }
}

fn start_sync_thread(context: SyncContext, saved: SavedServer) {
    let server_id = saved.server.id.clone();
    match context.sync_in_flight.lock() {
        Ok(mut running) => {
            if !running.insert(server_id.clone()) {
                let _sent = context.events.send(ControllerEvent::LoginStatus(
                    "Sync already running.".to_string(),
                ));
                return;
            }
        }
        Err(_) => {
            let _sent = context.events.send(ControllerEvent::Error(
                "Sync guard lock was poisoned.".to_string(),
            ));
            return;
        }
    }

    thread::spawn(move || {
        let provider_name = provider_display_name(&saved.server.provider);
        let _sent = context.events.send(ControllerEvent::LoginStatus(format!(
            "Syncing {provider_name} library..."
        )));
        let sync_result = run_sync_job(&context.store, &context.runtime, &context.secrets, &saved);
        if let Ok(mut running) = context.sync_in_flight.lock() {
            running.remove(&server_id);
        }
        match sync_result {
            Ok(()) => {
                covers::start_external_metadata_cover_prefetch_thread(
                    context.store.clone(),
                    Arc::clone(&context.runtime),
                    Arc::clone(&context.secrets),
                    context.events.clone(),
                    Arc::clone(&context.cover_in_flight),
                    Arc::clone(&context.external_cover_prefetch_in_flight),
                    Arc::clone(&context.cover_slots),
                    saved.clone(),
                );
                let _sent = context.events.send(ControllerEvent::LoginStatus(
                    "Library sync complete".to_string(),
                ));
                match load_snapshot(&context.store) {
                    Ok(snapshot) => {
                        let _sent = context
                            .events
                            .send(ControllerEvent::Snapshot(Box::new(snapshot)));
                    }
                    Err(error) => {
                        let _sent = context.events.send(ControllerEvent::Error(error));
                    }
                }
            }
            Err(error) => {
                let _failed = context.store.with_store(|store| {
                    store.fail_sync(&saved.server.id, &error)?;
                    Ok(())
                });
                let _sent = context.events.send(ControllerEvent::Error(error));
            }
        }
    });
}

fn start_home_refresh_thread(
    context: HomeRefreshContext,
    saved: SavedServer,
    target: HomeRefreshTarget,
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
        let result = match target {
            HomeRefreshTarget::Section(kind) => refresh_home_section_for_saved(
                &context.store,
                &context.runtime,
                &context.secrets,
                &saved,
                kind,
            ),
            HomeRefreshTarget::WithoutExplore => refresh_home_sections_without_explore_for_saved(
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
                let _sent = context
                    .events
                    .send(home_refresh_completed_event(target, snapshot));
            }
            Err(error) => {
                warn!(%error, "failed to refresh home sections");
            }
        }
    });
}

fn start_playlist_refresh_thread(context: PlaylistRefreshContext, saved: SavedServer) {
    if saved.server.provider == "fake" || saved.server.provider == LOCAL_PROVIDER_ID {
        return;
    }

    let server_id = saved.server.id.clone();
    if sync_is_running(&context.sync_in_flight, &server_id) {
        return;
    }
    match context.playlist_refresh_in_flight.lock() {
        Ok(mut running) => {
            if !running.insert(server_id.clone()) {
                return;
            }
        }
        Err(_) => {
            let _sent = context.events.send(ControllerEvent::Error(
                "Playlist refresh guard lock was poisoned.".to_string(),
            ));
            return;
        }
    }

    thread::spawn(move || {
        let result =
            refresh_playlists_for_saved(&context.store, &context.runtime, &context.secrets, &saved)
                .and_then(|()| load_snapshot(&context.store).map(Box::new));
        if let Ok(mut running) = context.playlist_refresh_in_flight.lock() {
            running.remove(&server_id);
        }
        match result {
            Ok(snapshot) => {
                let _sent = context.events.send(ControllerEvent::Snapshot(snapshot));
            }
            Err(error) => {
                warn!(%error, "failed to refresh playlists");
            }
        }
    });
}

fn home_refresh_completed_event(
    target: HomeRefreshTarget,
    snapshot: Box<LibrarySnapshot>,
) -> ControllerEvent {
    ControllerEvent::HomeSectionsUpdated {
        snapshot,
        include_explore: matches!(target, HomeRefreshTarget::Section(HomeSectionKind::Explore)),
    }
}

fn start_explore_prefetch_thread(context: ExplorePrefetchContext, saved: SavedServer) {
    if saved.server.provider == "fake" {
        return;
    }

    let server_id = saved.server.id.clone();
    if sync_is_running(&context.sync_in_flight, &server_id) {
        return;
    }
    match context.explore_prefetch_in_flight.lock() {
        Ok(mut running) => {
            if !running.insert(server_id.clone()) {
                return;
            }
        }
        Err(_) => {
            let _sent = context.events.send(ControllerEvent::Error(
                "Explore prefetch guard lock was poisoned.".to_string(),
            ));
            return;
        }
    }

    thread::spawn(move || {
        let result = prefetch_home_section_for_saved(
            &context.store,
            &context.runtime,
            &context.secrets,
            &saved,
            HomeSectionKind::Explore,
        );
        if let Ok(mut running) = context.explore_prefetch_in_flight.lock() {
            running.remove(&server_id);
        }
        match result {
            Ok(section) => {
                let _sent = context
                    .events
                    .send(ControllerEvent::HomeSectionPrefetched { server_id, section });
            }
            Err(error) => {
                warn!(%error, "failed to prefetch Explore section");
            }
        }
    });
}

fn start_prefetched_home_section_promotion_thread(
    store: StoreHandle,
    events: Sender<ControllerEvent>,
    server_id: ServerId,
    section: HomeSection,
) {
    thread::spawn(move || {
        let result = promote_prefetched_home_section(&store, &server_id, &section)
            .and_then(|()| load_snapshot(&store).map(Box::new));
        match result {
            Ok(snapshot) => {
                let _sent = events.send(ControllerEvent::HomeSectionsUpdated {
                    snapshot,
                    include_explore: false,
                });
            }
            Err(error) => {
                warn!(%error, "failed to promote prefetched home section");
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
    let provider = provider_for_saved(store, runtime, secrets, saved)?;
    runtime.block_on(sync_provider(
        store,
        &saved.server.id,
        provider.as_music_provider(),
    ))
}

fn refresh_home_sections_without_explore_for_saved(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
) -> Result<(), String> {
    let provider = provider_for_saved(store, runtime, secrets, saved)?;
    runtime.block_on(refresh_home_sections_without_explore(
        store,
        &saved.server.id,
        provider.as_music_provider(),
    ))
}

fn refresh_playlists_for_saved(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
) -> Result<(), String> {
    let provider = provider_for_saved(store, runtime, secrets, saved)?;
    runtime.block_on(refresh_playlist_pages(
        store,
        &saved.server.id,
        provider.as_music_provider(),
    ))
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
        provider.as_music_provider(),
        kind,
    ))
}

fn prefetch_home_section_for_saved(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
    kind: HomeSectionKind,
) -> Result<HomeSection, String> {
    let provider = provider_for_saved(store, runtime, secrets, saved)?;
    runtime.block_on(prefetch_home_section(
        store,
        &saved.server.id,
        provider.as_music_provider(),
        kind,
    ))
}

#[instrument(skip(store, provider), fields(server_id = %server_id.as_str()))]
async fn sync_provider(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
) -> Result<(), String> {
    let generation = store.with_store(|store| store.begin_sync(server_id))?;
    info!(generation, "started provider cache sync");
    sync_album_pages(store, server_id, provider, generation).await?;
    sync_track_pages(store, server_id, provider, generation).await?;
    sync_music_folders(store, server_id, provider, generation).await?;
    sync_artist_pages(store, server_id, provider, generation, false).await?;
    sync_artist_pages(store, server_id, provider, generation, true).await?;
    sync_genre_pages(store, server_id, provider, generation).await?;
    sync_playlist_pages(store, server_id, provider, generation).await?;
    sync_home_sections(store, server_id, provider, generation).await?;
    store.with_store(|store| store.refresh_library_counts(server_id))?;
    store.with_store(|store| store.complete_sync(server_id, generation))?;
    if let Err(error) = refresh_local_track_matches(store, server_id).await {
        warn!(%error, "failed to refresh local track matches");
    }
    info!(generation, "completed provider cache sync");
    Ok(())
}

async fn sync_album_pages(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
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
    provider: &(impl MusicProvider + ?Sized),
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

async fn sync_music_folders(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
    generation: i64,
) -> Result<(), String> {
    if !provider.capabilities().music_folders {
        return Ok(());
    }
    let folders = provider
        .music_folders()
        .await
        .map_err(|error| error.to_string())?;
    store.with_store(|store| store.upsert_music_folders(server_id, &folders, generation))?;
    for folder in folders {
        let mut offset = 0;
        loop {
            let page = provider
                .tracks_in_music_folder(&folder.id, PagedRequest::new(offset, PAGE_SIZE))
                .await
                .map_err(|error| error.to_string())?;
            store.with_store(|store| store.upsert_tracks(server_id, &page.items, generation))?;
            store.with_store(|store| {
                store.upsert_track_music_folder_memberships(
                    server_id,
                    &folder.id,
                    &page.items,
                    generation,
                )
            })?;
            let item_count = page.items.len();
            offset += item_count;
            if sync_page_finished(item_count, page.total, offset) {
                break;
            }
        }
    }
    Ok(())
}

async fn refresh_local_track_matches(
    store: &StoreHandle,
    server_id: &ServerId,
) -> Result<usize, String> {
    let Some(access) = store.with_store(|store| store.server_local_access(server_id))? else {
        return Ok(0);
    };
    let saved = store
        .with_store(|store| {
            store.list_servers().map(|servers| {
                servers
                    .into_iter()
                    .find(|saved| saved.server.id == *server_id)
            })
        })?
        .ok_or_else(|| "The server is no longer saved.".to_string())?;
    if saved.server.provider == "local" {
        return Ok(0);
    }
    let remote_tracks =
        store.with_store(|store| store.load_tracks_for_local_matching(server_id))?;
    if remote_tracks.is_empty() {
        store.with_store(|store| store.replace_track_local_matches(server_id, &[]))?;
        return Ok(0);
    }
    let local_provider = LocalProvider::from_root(PathBuf::from(&access.root_path))
        .map_err(|error| error.to_string())?;
    let local_tracks = load_all_local_tracks_for_matching(&local_provider).await?;
    let matches = conservative_local_matches(&remote_tracks, &local_tracks);
    let count = matches.len();
    store.with_store(|store| store.replace_track_local_matches(server_id, &matches))?;
    debug!(server_id = %server_id, count, "refreshed local track matches");
    Ok(count)
}

async fn load_all_local_tracks_for_matching(
    provider: &LocalProvider,
) -> Result<Vec<Track>, String> {
    let mut tracks = Vec::new();
    let mut offset = 0;
    loop {
        let page = provider
            .tracks(PagedRequest::new(offset, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        let item_count = page.items.len();
        tracks.extend(page.items);
        offset += item_count;
        if sync_page_finished(item_count, page.total, offset) {
            return Ok(tracks);
        }
    }
}

fn local_access_status_for_server(
    store: &StoreHandle,
    server: &ServerIdentity,
    access: Option<&ServerLocalAccess>,
) -> Result<LocalAccessStatus, String> {
    let Some(access) = access else {
        return Ok(LocalAccessStatus::default());
    };
    if server.provider == "local" {
        return Ok(LocalAccessStatus::default());
    }

    let remote_tracks =
        store.with_store(|store| store.load_tracks_for_local_matching(&server.id))?;
    let metadata_matches = store.with_store(|store| store.track_local_match_paths(&server.id))?;
    let metadata_by_track = metadata_matches
        .into_iter()
        .collect::<HashMap<TrackId, String>>();

    let sample_track = remote_tracks
        .iter()
        .find(|track| {
            track
                .local_path
                .as_deref()
                .is_some_and(|path| !path.trim().is_empty())
                && metadata_by_track.contains_key(&track.id)
        })
        .or_else(|| {
            remote_tracks.iter().find(|track| {
                track
                    .local_path
                    .as_deref()
                    .is_some_and(|path| !path.trim().is_empty())
            })
        });
    let sample_server_path = sample_track.and_then(|track| track.local_path.clone());
    let sample_local_path = sample_track.and_then(|track| {
        metadata_by_track.get(&track.id).cloned().or_else(|| {
            track
                .local_path
                .as_deref()
                .and_then(|raw| potential_local_path_text(raw, access))
        })
    });

    let mut effective_matches = HashSet::<TrackId>::new();
    let mut direct_match_count = 0;
    let mut prefix_match_count = 0;
    for track in &remote_tracks {
        let Some(raw) = track.local_path.as_deref() else {
            continue;
        };
        if map_server_path_to_local(raw, access).is_some() {
            prefix_match_count += 1;
            effective_matches.insert(track.id.clone());
        } else if Path::new(raw).is_absolute() {
            direct_match_count += 1;
            effective_matches.insert(track.id.clone());
        }
    }

    let metadata_match_count = metadata_by_track.len();
    for track_id in metadata_by_track.into_keys() {
        effective_matches.insert(track_id);
    }

    let total_track_count = remote_tracks.len();
    let unmatched_count = total_track_count.saturating_sub(effective_matches.len());
    Ok(LocalAccessStatus {
        sample_server_path,
        sample_local_path,
        direct_match_count,
        prefix_match_count,
        metadata_match_count,
        unmatched_count,
        total_track_count,
    })
}

fn potential_local_path_text(raw: &str, access: &ServerLocalAccess) -> Option<String> {
    if raw.trim().is_empty() {
        return None;
    }
    if let Some(mapped) = map_server_path_to_local(raw, access) {
        return Some(mapped.to_string_lossy().into_owned());
    }
    let direct = Path::new(raw);
    if direct.is_absolute() {
        return Some(direct.to_string_lossy().into_owned());
    }
    None
}

#[derive(Hash, Eq, PartialEq)]
struct LocalMatchKey {
    title: String,
    album: String,
    artist: String,
    disc_number: u16,
    track_number: u16,
}

fn conservative_local_matches(
    remote_tracks: &[Track],
    local_tracks: &[Track],
) -> Vec<(TrackId, String, String)> {
    let mut index = HashMap::<LocalMatchKey, Vec<&Track>>::new();
    for track in local_tracks {
        if track.local_path.is_none() {
            continue;
        }
        index.entry(local_match_key(track)).or_default().push(track);
    }

    let mut matches = Vec::new();
    for remote in remote_tracks {
        let Some(candidates) = index.get(&local_match_key(remote)) else {
            continue;
        };
        let matched = candidates
            .iter()
            .copied()
            .filter(|candidate| {
                durations_close(remote.duration_seconds, candidate.duration_seconds)
            })
            .collect::<Vec<_>>();
        if matched.len() != 1 {
            continue;
        }
        let Some(local_path) = matched[0].local_path.clone() else {
            continue;
        };
        matches.push((remote.id.clone(), local_path, "metadata".to_string()));
    }
    matches
}

fn local_match_key(track: &Track) -> LocalMatchKey {
    LocalMatchKey {
        title: normalize_match_text(&track.title),
        album: normalize_match_text(&track.album),
        artist: normalize_match_text(&track.artist),
        disc_number: track.disc_number,
        track_number: track.track_number,
    }
}

fn durations_close(left: u32, right: u32) -> bool {
    left == 0 || right == 0 || left.abs_diff(right) <= 3
}

fn normalize_match_text(value: &str) -> String {
    let mut normalized = String::new();
    for character in value.chars() {
        if character.is_alphanumeric() {
            normalized.extend(character.to_lowercase());
        } else {
            normalized.push(' ');
        }
    }
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

async fn sync_artist_pages(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
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
    provider: &(impl MusicProvider + ?Sized),
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
    provider: &(impl MusicProvider + ?Sized),
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

async fn refresh_playlist_pages(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
) -> Result<(), String> {
    let generation =
        store.with_store(|store| store.sync_state(server_id).map(|state| state.generation))?;
    let mut playlist_ids = Vec::new();
    let mut offset = 0;
    loop {
        let page = provider
            .playlists(PagedRequest::new(offset, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        for playlist in &page.items {
            playlist_ids.push(playlist.id.clone());
        }
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
            store.with_store(|store| store.prune_playlists_except(server_id, &playlist_ids))?;
            return Ok(());
        }
    }
}

async fn sync_home_sections(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
    generation: i64,
) -> Result<(), String> {
    let sections = provider
        .home_sections()
        .await
        .map_err(|error| error.to_string())?;
    cache_home_sections(store, server_id, &sections, generation)
}

#[cfg(test)]
async fn refresh_home_sections(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
) -> Result<(), String> {
    let generation =
        store.with_store(|store| store.sync_state(server_id).map(|state| state.generation))?;
    let sections = provider
        .home_sections()
        .await
        .map_err(|error| error.to_string())?;
    cache_home_sections(store, server_id, &sections, generation)
}

async fn refresh_home_sections_without_explore(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
) -> Result<(), String> {
    for kind in home_refresh_section_kinds()
        .into_iter()
        .filter(|kind| *kind != HomeSectionKind::Explore)
    {
        refresh_home_section(store, server_id, provider, kind).await?;
    }
    Ok(())
}

async fn refresh_home_section(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
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

async fn prefetch_home_section(
    store: &StoreHandle,
    server_id: &ServerId,
    provider: &(impl MusicProvider + ?Sized),
    kind: HomeSectionKind,
) -> Result<HomeSection, String> {
    let generation =
        store.with_store(|store| store.sync_state(server_id).map(|state| state.generation))?;
    let section = provider
        .home_section(kind)
        .await
        .map_err(|error| error.to_string())?;
    cache_home_section_items(store, server_id, &section, generation)?;
    store
        .with_store(|store| store.upsert_home_section_prefetch(server_id, &section, generation))?;
    Ok(section)
}

fn promote_prefetched_home_section(
    store: &StoreHandle,
    server_id: &ServerId,
    section: &HomeSection,
) -> Result<(), String> {
    let generation =
        store.with_store(|store| store.sync_state(server_id).map(|state| state.generation))?;
    cache_home_section(store, server_id, section, generation)?;
    store.with_store(|store| store.clear_home_section_prefetch(server_id, section.kind))?;
    Ok(())
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

fn home_refresh_section_kinds() -> [HomeSectionKind; 5] {
    [
        HomeSectionKind::Explore,
        HomeSectionKind::MostPlayed,
        HomeSectionKind::NewlyAdded,
        HomeSectionKind::RecentlyPlayed,
        HomeSectionKind::RecentlyReleased,
    ]
}

fn load_snapshot(store: &StoreHandle) -> Result<LibrarySnapshot, String> {
    let source_settings = load_settings_from_store(store);
    let saved_servers = store.with_store(|store| store.list_servers())?;
    let remote_saved_servers = saved_servers
        .iter()
        .filter(|saved| saved.server.provider != LOCAL_PROVIDER_ID)
        .cloned()
        .collect::<Vec<_>>();
    let servers = remote_saved_servers
        .iter()
        .map(|saved| saved.server.clone())
        .collect::<Vec<_>>();
    let server_local_access = remote_saved_servers
        .iter()
        .map(|saved| {
            let access = store.with_store(|store| store.server_local_access(&saved.server.id))?;
            let status = local_access_status_for_server(store, &saved.server, access.as_ref())?;
            let sync_state = store
                .with_store(|store| store.sync_state(&saved.server.id))
                .ok();
            let sync_status = sync_state
                .as_ref()
                .map(sync_status_text)
                .unwrap_or_else(|| "Cached library ready".to_string());
            let cached_album_count = store
                .with_store(|store| {
                    store
                        .load_albums(&saved.server.id, 0, 1)
                        .map(|page| page.total)
                })
                .unwrap_or_default();
            let cached_track_count = store
                .with_store(|store| {
                    store
                        .load_tracks(&saved.server.id, 0, 1)
                        .map(|page| page.total)
                })
                .unwrap_or_default();
            let selected_music_folder_name = store
                .with_store(|store| {
                    let selected = store.selected_music_folder_id(&saved.server.id)?;
                    let folders = store.list_music_folders(&saved.server.id)?;
                    Ok(selected.and_then(|selected| {
                        folders
                            .into_iter()
                            .find(|folder| folder.id == selected)
                            .map(|folder| folder.name)
                    }))
                })
                .unwrap_or_default();
            Ok(ServerLocalAccessSnapshot {
                server_id: saved.server.id.clone(),
                access,
                status,
                selected_music_folder_name,
                username: Some(saved.username.clone()),
                trust_invalid_cert: saved.trust_invalid_cert,
                sync_status,
                cached_album_count,
                cached_track_count,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let selected_source = resolve_selected_source(
        &source_settings,
        &remote_saved_servers,
        store.with_store(|store| store.active_server())?,
    );
    let Some(selected_source) = selected_source else {
        let mut snapshot = LibrarySnapshot::first_run();
        snapshot.servers = servers;
        snapshot.local_folders = source_settings.sources.local_folders.clone();
        snapshot.server_local_access = server_local_access;
        return Ok(snapshot);
    };
    let saved = match &selected_source {
        LibrarySourceSelection::Local => ensure_local_source_server(store)?,
        LibrarySourceSelection::Server(server_id) => remote_saved_servers
            .iter()
            .find(|saved| &saved.server.id == server_id)
            .cloned()
            .ok_or_else(|| "The selected source is no longer saved.".to_string())?,
    };
    store.with_store(|store| store.set_active_server(&saved.server.id))?;
    let local_access = store.with_store(|store| store.server_local_access(&saved.server.id))?;
    let local_access_status =
        local_access_status_for_server(store, &saved.server, local_access.as_ref())?;
    let music_folders = store.with_store(|store| store.list_music_folders(&saved.server.id))?;
    let selected_music_folder_id =
        store.with_store(|store| store.selected_music_folder_id(&saved.server.id))?;
    let metadata_settings = load_settings_for_saved(store, &saved);
    let sync_state = store
        .with_store(|store| store.sync_state(&saved.server.id))
        .ok();
    let mut home_sections = store.with_store(|store| store.load_home_sections(&saved.server.id))?;
    let mut prefetched_explore = store.with_store(|store| {
        store.load_home_section_prefetch(&saved.server.id, HomeSectionKind::Explore)
    })?;
    let album_page =
        store.with_store(|store| store.load_albums(&saved.server.id, 0, SNAPSHOT_GRID_LIMIT))?;
    let track_page =
        store.with_store(|store| store.load_tracks(&saved.server.id, 0, SNAPSHOT_TRACK_LIMIT))?;
    let cached_album_count = album_page.total;
    let cached_track_count = track_page.total;
    let mut albums = album_page.items;
    let mut tracks = track_page.items;
    let artist_page = store
        .with_store(|store| store.load_artists(&saved.server.id, false, 0, SNAPSHOT_GRID_LIMIT))?;
    let album_artist_page = store
        .with_store(|store| store.load_artists(&saved.server.id, true, 0, SNAPSHOT_GRID_LIMIT))?;
    let genre_page =
        store.with_store(|store| store.load_genres(&saved.server.id, 0, SNAPSHOT_GRID_LIMIT))?;
    let playlist_page =
        store.with_store(|store| store.load_playlists(&saved.server.id, 0, SNAPSHOT_GRID_LIMIT))?;
    let cached_artist_count = artist_page.total;
    let cached_album_artist_count = album_artist_page.total;
    let cached_genre_count = genre_page.total;
    let cached_playlist_count = playlist_page.total;
    let mut artists = artist_page.items;
    let mut album_artists = album_artist_page.items;
    let genres = genre_page.items;
    let playlists = playlist_page.items;
    let mut favorites = store.with_store(|store| store.load_favorite_tracks(&saved.server.id))?;
    external_metadata::normalize_home_sections(&mut home_sections, &metadata_settings);
    if let Some(section) = &mut prefetched_explore {
        external_metadata::normalize_home_section(section, &metadata_settings);
    }
    external_metadata::normalize_albums(&mut albums, &metadata_settings);
    external_metadata::normalize_tracks(&mut tracks, &metadata_settings);
    external_metadata::normalize_artists(&mut artists, &metadata_settings);
    external_metadata::normalize_artists(&mut album_artists, &metadata_settings);
    external_metadata::normalize_tracks(&mut favorites, &metadata_settings);
    let status = sync_state
        .as_ref()
        .map(sync_status_text)
        .unwrap_or_else(|| "Cached library ready".to_string());
    let last_error = sync_state.and_then(|state| state.last_error);

    Ok(LibrarySnapshot {
        server: Some(saved.server),
        servers,
        selected_source: Some(selected_source),
        local_folders: source_settings.sources.local_folders,
        server_local_access,
        local_access,
        local_access_status,
        music_folders,
        selected_music_folder_id,
        username: Some(saved.username),
        first_run: false,
        sync_status: status,
        last_error,
        cached_album_count,
        cached_track_count,
        cached_artist_count,
        cached_album_artist_count,
        cached_genre_count,
        cached_playlist_count,
        home_sections,
        prefetched_explore,
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

fn sync_status_text(state: &SyncState) -> String {
    match state.status.as_str() {
        "running" => "Syncing library...".to_string(),
        "error" => "Sync needs attention".to_string(),
        _ => "Cached library ready".to_string(),
    }
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
    let settings = load_settings_for_server(store, server);
    match store.with_store(|store| store.load_queue_snapshot(&server.id)) {
        Ok(Some(mut snapshot)) => {
            external_metadata::normalize_queue_snapshot(&mut snapshot, &settings);
            Some(QueueEngine::restore(snapshot))
        }
        Ok(None) => Some(QueueEngine::new(server.id.clone())),
        Err(error) => {
            warn!(%error, "failed to restore queue snapshot");
            Some(QueueEngine::new(server.id.clone()))
        }
    }
}

fn emit_snapshot(store: &StoreHandle, events: &Sender<ControllerEvent>) {
    match load_snapshot(store) {
        Ok(snapshot) => {
            let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
        }
        Err(error) => {
            let _sent = events.send(ControllerEvent::Error(error));
        }
    }
}

fn resolve_selected_source(
    settings: &AppSettings,
    remote_saved_servers: &[SavedServer],
    active_server: Option<SavedServer>,
) -> Option<LibrarySourceSelection> {
    match &settings.sources.selected {
        Some(LibrarySourceSelection::Local) => return Some(LibrarySourceSelection::Local),
        Some(LibrarySourceSelection::Server(server_id))
            if remote_saved_servers
                .iter()
                .any(|saved| saved.server.id == *server_id) =>
        {
            return Some(LibrarySourceSelection::Server(server_id.clone()));
        }
        _ => {}
    }

    if let Some(saved) = active_server
        && saved.server.provider != LOCAL_PROVIDER_ID
    {
        return Some(LibrarySourceSelection::Server(saved.server.id));
    }
    if !settings.sources.local_folders.is_empty() {
        return Some(LibrarySourceSelection::Local);
    }
    remote_saved_servers
        .first()
        .map(|saved| LibrarySourceSelection::Server(saved.server.id.clone()))
}

fn active_server_needs_sync(store: &StoreHandle, server_id: &ServerId) -> bool {
    store
        .with_store(|store| store.sync_completed_age_seconds(server_id))
        .ok()
        .flatten()
        .is_none_or(|age| age > STARTUP_CACHE_STALE_SECONDS)
}

fn trimmed_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn load_settings_from_store(store: &StoreHandle) -> AppSettings {
    let mut settings = store.load_settings().unwrap_or_default();
    settings.migrate_defaults();
    if migrate_legacy_local_servers_to_settings(store, &mut settings) {
        settings.migrate_defaults();
        if let Err(error) = store.save_settings(&settings) {
            warn!(%error, "failed to persist migrated local source settings");
        }
    }
    settings
}

fn migrate_legacy_local_servers_to_settings(
    store: &StoreHandle,
    settings: &mut AppSettings,
) -> bool {
    if !store.database_exists() {
        return false;
    }

    let Ok(saved_servers) = store.with_store(|store| store.list_servers()) else {
        return false;
    };
    let mut changed = false;
    for saved in saved_servers {
        if saved.server.provider != LOCAL_PROVIDER_ID
            || saved.server.id.as_str() == LOCAL_SOURCE_SERVER_ID
        {
            continue;
        }
        let path = saved.server.base_url.trim();
        if path.is_empty() {
            continue;
        }
        if !settings
            .sources
            .local_folders
            .iter()
            .any(|folder| folder.path == path)
        {
            settings.sources.local_folders.push(LocalLibraryFolder {
                path: path.to_string(),
            });
            changed = true;
        }
        if settings.sources.selected == Some(LibrarySourceSelection::Server(saved.server.id)) {
            settings.sources.selected = Some(LibrarySourceSelection::Local);
            changed = true;
        }
    }
    changed
}

fn local_folder_paths(settings: &AppSettings) -> Vec<PathBuf> {
    settings
        .sources
        .local_folders
        .iter()
        .map(|folder| PathBuf::from(&folder.path))
        .collect()
}

fn local_source_server() -> ServerIdentity {
    ServerIdentity {
        id: ServerId::new(LOCAL_SOURCE_SERVER_ID),
        provider: LOCAL_PROVIDER_ID.to_string(),
        name: "Local".to_string(),
        base_url: String::new(),
    }
}

fn local_source_saved() -> SavedServer {
    SavedServer {
        server: local_source_server(),
        user_id: "local".to_string(),
        username: "Local".to_string(),
        trust_invalid_cert: false,
    }
}

fn ensure_local_source_server(store: &StoreHandle) -> Result<SavedServer, String> {
    let saved = local_source_saved();
    store.with_store(|store| store.save_server(&saved))?;
    Ok(saved)
}

fn load_settings_for_active_server(store: &StoreHandle) -> AppSettings {
    let settings = load_settings_from_store(store);
    match store.with_store(|store| store.active_server()) {
        Ok(Some(saved)) => settings_for_server(settings, &saved.server),
        _ => settings,
    }
}

fn load_settings_for_saved(store: &StoreHandle, saved: &SavedServer) -> AppSettings {
    settings_for_server(load_settings_from_store(store), &saved.server)
}

fn load_settings_for_server(store: &StoreHandle, server: &ServerIdentity) -> AppSettings {
    settings_for_server(load_settings_from_store(store), server)
}

fn settings_for_server(mut settings: AppSettings, server: &ServerIdentity) -> AppSettings {
    if server.provider == "fake" {
        settings.external_metadata_enabled = false;
    }
    settings
}

fn playback_snapshot_from_queue(
    queue: Option<&QueueEngine>,
    auto_dj_enabled: bool,
    playback_settings: &PlaybackSettings,
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
            volume: playback_settings.volume,
            muted: playback_settings.muted,
            repeat_mode: queue.repeat_mode(),
            shuffle_enabled: queue.shuffle().enabled,
            auto_dj_enabled,
            buffering_percent: None,
            last_error: None,
        })
        .unwrap_or_else(|| PlaybackSnapshot {
            auto_dj_enabled,
            volume: playback_settings.volume,
            muted: playback_settings.muted,
            ..PlaybackSnapshot::default()
        })
}

fn next_queue_entry_after_current(queue: &QueueEngine) -> Option<QueueEntry> {
    let mut preview = QueueEngine::restore(queue.snapshot());
    preview.advance_after_end_of_stream().cloned()
}

fn queue_current_matches(
    queue: &Arc<Mutex<Option<QueueEngine>>>,
    current_entry_id: &QueueEntryId,
) -> bool {
    queue
        .lock()
        .ok()
        .and_then(|queue| queue.as_ref().and_then(|queue| queue.current().cloned()))
        .is_some_and(|entry| entry.id == *current_entry_id)
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

fn platform_secret_store() -> Arc<dyn SecretStore> {
    #[cfg(unix)]
    {
        Arc::new(SecretServiceStore::new())
    }
    #[cfg(not(unix))]
    {
        Arc::new(MemorySecretStore::new())
    }
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

fn prepared_item_from_entry(entry: &QueueEntry, stream: StreamDescriptor) -> PreparedPlaybackItem {
    PreparedPlaybackItem::new(playback_track_from_entry(entry), stream)
}

fn resolve_prepared_item(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    server_id: &ServerId,
    entry: &QueueEntry,
    playback_settings: &PlaybackSettings,
) -> Result<PreparedPlaybackItem, String> {
    let stream = resolve_stream(
        store,
        runtime,
        secrets,
        server_id,
        &entry.track_id,
        playback_settings,
    )?;
    Ok(prepared_item_from_entry(entry, stream))
}

fn prepare_next_stream_from_handles(
    store: StoreHandle,
    runtime: Arc<Runtime>,
    secrets: Arc<dyn SecretStore>,
    playback: Arc<Mutex<Box<dyn PlaybackBackend>>>,
    queue: Arc<Mutex<Option<QueueEngine>>>,
    events: Sender<ControllerEvent>,
) {
    let playback_settings = load_settings_from_store(&store).playback;
    let Some((server_id, current_entry_id, next_entry, playback_settings)) =
        next_preload_request_from_queue(&queue, playback_settings)
    else {
        if let Err(error) = playback
            .lock()
            .map_err(|_| "playback lock was poisoned".to_string())
            .and_then(|mut playback| {
                playback
                    .send(PlaybackCommand::PrepareNext(None))
                    .map_err(|error| error.to_string())
            })
        {
            let _sent = events.send(ControllerEvent::Error(error));
        }
        return;
    };

    thread::spawn(move || {
        let prepared = match resolve_prepared_item(
            &store,
            &runtime,
            &secrets,
            &server_id,
            &next_entry,
            &playback_settings,
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
        };
        if !queue_current_matches(&queue, &current_entry_id) {
            return;
        }
        if let Err(error) = playback
            .lock()
            .map_err(|_| "playback lock was poisoned".to_string())
            .and_then(|mut playback| {
                playback
                    .send(PlaybackCommand::PrepareNext(Some(prepared)))
                    .map_err(|error| error.to_string())
            })
        {
            let _sent = events.send(ControllerEvent::Error(error));
        }
    });
}

fn next_preload_request_from_queue(
    queue: &Arc<Mutex<Option<QueueEngine>>>,
    playback_settings: PlaybackSettings,
) -> Option<(ServerId, QueueEntryId, QueueEntry, PlaybackSettings)> {
    queue.lock().ok().and_then(|queue| {
        let queue = queue.as_ref()?;
        let server_id = queue.snapshot().server_id;
        let current_entry_id = queue.current()?.id.clone();
        let next = next_queue_entry_after_current(queue)?;
        Some((server_id, current_entry_id, next, playback_settings))
    })
}

fn resolve_stream(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    server_id: &ServerId,
    track_id: &TrackId,
    playback_settings: &PlaybackSettings,
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
    if saved.server.provider != "local"
        && let Some(local_path) = local_audio_path_for_track(store, server_id, track_id)
    {
        let url = reqwest::Url::from_file_path(&local_path).map_err(|()| {
            format!(
                "Could not turn local track path into a file URI: {}",
                local_path.display()
            )
        })?;
        debug!(
            server_id = %server_id,
            track_id = %track_id.as_str(),
            path = %local_path.display(),
            "resolved remote track to local playback file"
        );
        return Ok(StreamDescriptor::new(url.to_string()));
    }

    let provider = provider_for_saved(store, runtime, secrets, &saved)?;
    runtime
        .block_on(
            provider
                .as_music_provider()
                .stream_with_request(&StreamRequest::new(
                    track_id.clone(),
                    playback_settings.stream_quality,
                )),
        )
        .map_err(|error| error.to_string())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrcLibLyricsDto {
    id: u64,
    #[serde(default, alias = "name")]
    track_name: String,
    #[serde(default)]
    artist_name: String,
    #[serde(default)]
    album_name: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    synced_lyrics: Option<String>,
    #[serde(default)]
    plain_lyrics: Option<String>,
}

impl From<LrcLibLyricsDto> for LyricsSearchResult {
    fn from(value: LrcLibLyricsDto) -> Self {
        Self {
            id: value.id,
            track_name: value.track_name,
            artist_name: value.artist_name,
            album_name: value.album_name.unwrap_or_default(),
            duration_seconds: value.duration.unwrap_or_default().round() as u32,
            synced_lyrics: value.synced_lyrics,
            plain_lyrics: value.plain_lyrics,
        }
    }
}

fn lrclib_search(artist_name: &str, track_name: &str) -> Result<Vec<LyricsSearchResult>, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("Rufin/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| error.to_string())?;
    let mut results = Vec::new();
    let mut seen = HashSet::new();
    let mut had_success = false;
    let mut errors = Vec::new();
    for url in lrclib_search_urls(artist_name, track_name)? {
        match lrclib_fetch_search(&client, url) {
            Ok(batch) => {
                had_success = true;
                for result in batch {
                    if seen.insert(result.id) {
                        results.push(result);
                    }
                }
            }
            Err(error) => errors.push(error),
        }
    }
    if !had_success && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    order_lrclib_results(&mut results, artist_name, track_name);
    Ok(results)
}

fn lrclib_search_urls(artist_name: &str, track_name: &str) -> Result<Vec<reqwest::Url>, String> {
    let artist_name = artist_name.trim();
    let track_name = track_name.trim();
    let mut urls = Vec::new();
    let combined_query = [track_name, artist_name]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if !combined_query.is_empty() {
        let mut url = lrclib_search_base_url()?;
        url.query_pairs_mut().append_pair("q", &combined_query);
        urls.push(url);
    }
    if !track_name.is_empty() {
        let mut url = lrclib_search_base_url()?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("track_name", track_name);
            if !artist_name.is_empty() {
                query.append_pair("artist_name", artist_name);
            }
        }
        urls.push(url);
    }
    Ok(urls)
}

fn lrclib_search_base_url() -> Result<reqwest::Url, String> {
    reqwest::Url::parse("https://lrclib.net/api/search").map_err(|error| error.to_string())
}

fn lrclib_fetch_search(
    client: &reqwest::blocking::Client,
    url: reqwest::Url,
) -> Result<Vec<LyricsSearchResult>, String> {
    let body = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("Lyric search failed: {error}"))?
        .text()
        .map_err(|error| format!("Lyric search failed: {error}"))?;
    parse_lrclib_search_body(&body)
}

fn parse_lrclib_search_body(body: &str) -> Result<Vec<LyricsSearchResult>, String> {
    let values = serde_json::from_str::<Vec<serde_json::Value>>(body)
        .map_err(|error| format!("Lyric search returned invalid data: {error}"))?;
    let mut results = Vec::new();
    for value in values {
        match serde_json::from_value::<LrcLibLyricsDto>(value) {
            Ok(dto) => {
                let result = LyricsSearchResult::from(dto);
                if !result.track_name.trim().is_empty() || !result.artist_name.trim().is_empty() {
                    results.push(result);
                }
            }
            Err(error) => {
                debug!(%error, "skipped invalid LRCLIB search result");
            }
        }
    }
    Ok(results)
}

fn order_lrclib_results(results: &mut [LyricsSearchResult], artist_name: &str, track_name: &str) {
    results.sort_by(|a, b| {
        lrclib_match_score(a, artist_name, track_name)
            .cmp(&lrclib_match_score(b, artist_name, track_name))
            .then_with(|| lrclib_has_synced_lyrics(b).cmp(&lrclib_has_synced_lyrics(a)))
            .then_with(|| lrclib_has_plain_lyrics(b).cmp(&lrclib_has_plain_lyrics(a)))
            .then_with(|| a.track_name.cmp(&b.track_name))
            .then_with(|| a.artist_name.cmp(&b.artist_name))
    });
}

fn lrclib_match_score(result: &LyricsSearchResult, artist_name: &str, track_name: &str) -> u16 {
    text_match_score(track_name, &result.track_name).saturating_mul(2)
        + text_match_score(artist_name, &result.artist_name)
}

fn text_match_score(query: &str, candidate: &str) -> u16 {
    let query = normalize_search_text(query);
    if query.is_empty() {
        return 0;
    }
    let candidate = normalize_search_text(candidate);
    if candidate == query {
        return 0;
    }
    if candidate.contains(&query) || query.contains(&candidate) {
        return 10;
    }
    let query_tokens = query.split_whitespace().collect::<HashSet<_>>();
    if query_tokens.is_empty() {
        return 0;
    }
    let candidate_tokens = candidate.split_whitespace().collect::<HashSet<_>>();
    let matched = query_tokens.intersection(&candidate_tokens).count();
    let missing = query_tokens.len().saturating_sub(matched);
    let extra = candidate_tokens.len().saturating_sub(matched);
    if matched == 0 {
        return 100 + query_tokens.len() as u16 * 10;
    }
    (missing as u16 * 30) + (extra.min(6) as u16 * 4)
}

fn normalize_search_text(value: &str) -> String {
    let mut normalized = String::new();
    for character in value.chars() {
        if character.is_alphanumeric() {
            normalized.extend(character.to_lowercase());
        } else {
            normalized.push(' ');
        }
    }
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn lrclib_has_synced_lyrics(result: &LyricsSearchResult) -> bool {
    result
        .synced_lyrics
        .as_deref()
        .is_some_and(|lyrics| !lyrics.trim().is_empty())
}

fn lrclib_has_plain_lyrics(result: &LyricsSearchResult) -> bool {
    result
        .plain_lyrics
        .as_deref()
        .is_some_and(|lyrics| !lyrics.trim().is_empty())
}

fn save_lrclib_result(
    store: &StoreHandle,
    server_id: &ServerId,
    entry: &QueueEntry,
    result: &LyricsSearchResult,
    output_path: Option<PathBuf>,
) -> Result<(PathBuf, Lyrics), String> {
    let content = lyrics_result_content(result)
        .ok_or_else(|| "Selected lyric result has no lyrics to save.".to_string())?;
    let settings = load_settings_from_store(store);
    let path = output_path
        .or_else(|| {
            local_audio_path_for_track(store, server_id, &entry.track_id)
                .map(|path| path.with_extension("lrc"))
        })
        .map(Ok)
        .unwrap_or_else(|| lyrics_save_path(&entry.title, &settings))?;
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

fn local_sidecar_lyrics(
    store: &StoreHandle,
    server_id: &ServerId,
    track_id: &TrackId,
) -> Option<Lyrics> {
    let audio_path = local_audio_path_for_track(store, server_id, track_id)?;
    let path = audio_path.with_extension("lrc");
    let content = fs::read_to_string(path).ok()?;
    let lines = content
        .lines()
        .filter_map(lyric_line_from_text)
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| Lyrics {
        track_id: track_id.clone(),
        source: rufin_provider::LyricsSource::Local,
        lines,
    })
}

fn local_audio_path_for_track(
    store: &StoreHandle,
    server_id: &ServerId,
    track_id: &TrackId,
) -> Option<PathBuf> {
    let saved = store
        .with_store(|store| {
            store.list_servers().map(|servers| {
                servers
                    .into_iter()
                    .find(|saved| saved.server.id == *server_id)
            })
        })
        .ok()
        .flatten()?;
    let raw = store
        .with_store(|store| store.track_local_path(server_id, track_id))
        .ok()
        .flatten();
    if saved.server.provider == "local" {
        let direct = PathBuf::from(raw?);
        return direct.is_file().then_some(direct);
    }
    let access = store
        .with_store(|store| store.server_local_access(server_id))
        .ok()
        .flatten()?;
    if let Some(matched) = store
        .with_store(|store| store.track_local_match_path(server_id, track_id))
        .ok()
        .flatten()
    {
        let matched = PathBuf::from(matched);
        if matched.is_file() {
            return Some(matched);
        }
    }
    let raw = raw?;
    let direct = PathBuf::from(&raw);
    if direct.is_file() {
        return Some(direct);
    }
    let mapped = map_server_path_to_local(&raw, &access)?;
    mapped.is_file().then_some(mapped)
}

fn map_server_path_to_local(raw: &str, access: &ServerLocalAccess) -> Option<PathBuf> {
    let replace_to = access
        .path_replace_to
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&access.root_path);
    if let Some(prefix) = access
        .path_replace_from
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && raw.starts_with(prefix)
    {
        let suffix = raw[prefix.len()..].trim_start_matches(['/', '\\']);
        return Some(PathBuf::from(replace_to).join(path_from_server_suffix(suffix)));
    }
    let raw_path = Path::new(raw);
    if raw_path.is_relative() {
        return Some(PathBuf::from(replace_to).join(raw_path));
    }
    None
}

fn path_from_server_suffix(suffix: &str) -> PathBuf {
    suffix
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect::<PathBuf>()
}

fn lyrics_save_path(track_title: &str, settings: &AppSettings) -> Result<PathBuf, String> {
    let user_dirs = directories::UserDirs::new()
        .ok_or_else(|| "Could not find the user home directory.".to_string())?;
    let base = settings
        .lyrics_export_folder
        .as_ref()
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            user_dirs
                .audio_dir()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| user_dirs.home_dir().join("Music"))
        });
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
        rufin_provider::LyricsSource::Local => true,
        rufin_provider::LyricsSource::Server => true,
        rufin_provider::LyricsSource::Remote => !matches!(search, JellyfinLyricsSearch::ServerOnly),
    }
}

fn provider_for_saved(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
) -> Result<LoadedProvider, String> {
    let _unused = runtime;
    if saved.server.provider == LOCAL_PROVIDER_ID
        && saved.server.id.as_str() == LOCAL_SOURCE_SERVER_ID
    {
        let settings = load_settings_from_store(store);
        return LocalProvider::from_roots_with_identity(
            local_folder_paths(&settings),
            saved.server.clone(),
        )
        .map(LoadedProvider::Local)
        .map_err(|error| error.to_string());
    }
    if saved.server.provider == LOCAL_PROVIDER_ID {
        let session = SavedProviderSession {
            server: saved.server.clone(),
            user_id: saved.user_id.clone(),
            username: saved.username.clone(),
            trust_invalid_cert: saved.trust_invalid_cert,
            access_token: String::new(),
        };
        return provider_from_saved(session).map_err(|error| error.to_string());
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
    provider_from_saved(session).map_err(|error| error.to_string())
}

fn load_folder_detail(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    path: &[FolderPathItem],
) -> Result<FolderDetail, String> {
    let saved = store
        .with_store(|store| store.active_server())?
        .ok_or_else(|| "No active server.".to_string())?;
    let selected_music_folder_id =
        store.with_store(|store| store.selected_music_folder_id(&saved.server.id))?;
    let settings = load_settings_for_saved(store, &saved);
    let provider = provider_for_saved(store, runtime, secrets, &saved)?;
    let music_provider = provider.as_music_provider();
    if !music_provider.capabilities().folder_browsing {
        return Err("folder browsing is not supported by the active provider.".to_string());
    }
    let folder_id = path.last().map(|entry| &entry.id);
    let mut detail = runtime
        .block_on(music_provider.folder(folder_id, selected_music_folder_id.as_ref()))
        .map_err(|error| error.to_string())?;
    external_metadata::normalize_tracks(&mut detail.tracks, &settings);
    Ok(detail)
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
            .block_on(
                provider
                    .as_music_provider()
                    .remove_playlist_entries(&before.playlist.id, &removed),
            )
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
            .block_on(
                provider
                    .as_music_provider()
                    .add_playlist_tracks(&before.playlist.id, &added),
            )
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
                .block_on(provider.as_music_provider().move_playlist_entry(
                    &before.playlist.id,
                    &entry.entry_id,
                    new_index,
                ))
                .map_err(|error| error.to_string())?;
        }
    }

    runtime
        .block_on(
            provider
                .as_music_provider()
                .playlist_detail(&before.playlist.id),
        )
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
        if saved.server.provider == "fake" || saved.server.provider == "local" {
            return;
        }
        let result = provider_for_saved(&store, &runtime, &secrets, &saved).and_then(|provider| {
            runtime
                .block_on(provider.as_music_provider().report_playback(report))
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

fn data_dir() -> Option<PathBuf> {
    ProjectDirs::from("io.github", "screwys", "Rufin").map(|dirs| dirs.data_dir().to_path_buf())
}

fn config_dir() -> Option<PathBuf> {
    ProjectDirs::from("io.github", "screwys", "Rufin").map(|dirs| dirs.config_dir().to_path_buf())
}

fn cache_dir() -> Option<PathBuf> {
    ProjectDirs::from("io.github", "screwys", "Rufin").map(|dirs| dirs.cache_dir().to_path_buf())
}

fn restrict_settings_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
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
        AppController, ControllerEvent, DATABASE_FILE_NAME, LOCAL_SOURCE_SERVER_ID,
        LibrarySnapshot, PendingSeek, RandomPlayAction, RandomPlayRequest, SETTINGS_FILE_NAME,
        SNAPSHOT_GRID_LIMIT, SNAPSHOT_TRACK_LIMIT, StoreHandle, auto_dj_candidates,
        home_refresh_completed_event, load_settings_from_store, load_snapshot,
        playback_snapshot_from_queue, prefetch_home_section, promote_prefetched_home_section,
        refresh_home_section, refresh_home_sections, refresh_home_sections_without_explore,
        refresh_playlist_pages, restore_queue, seek_position_is_stale, sync_page_finished,
        sync_provider,
    };
    use crate::external_scrobbling::ExternalScrobbleState;
    use rufin_core::{
        AlbumId, AppSettings, ArtistCredit, ArtistId, HomeSection, HomeSectionKind, ImageRef,
        LibrarySourceSelection, LocalLibraryFolder, PlaybackSettings, Playlist, PlaylistId,
        QueueEngine, RepeatMode, ServerId, ServerIdentity, ThemePreference, Track, TrackId,
    };
    use rufin_playback::{
        PlaybackBackend, PlaybackCommand, PlaybackError, PlaybackEvent, PlaybackState,
        PlaybackTrack,
    };
    use rufin_provider::{
        FavoriteItemId, LyricLine, Lyrics, LyricsSource, MusicProvider, PagedRequest, PlayedFilter,
        PlaylistEntry,
    };
    use rufin_provider_local::LOCAL_PROVIDER_ID;
    use rufin_secrets::{MemorySecretStore, SecretStore};
    use rufin_store::{CoverCacheEntry, SavedServer, ServerLocalAccess};
    use rufin_test_support::{FakeProvider, FakeScale};
    use tokio::runtime::Runtime;

    use crate::providers::JellyfinLyricsSearch;

    struct StalePositionAfterSeekBackend {
        stale_millis: u64,
        events: Vec<PlaybackEvent>,
    }

    struct RecordingPlaybackBackend {
        commands: Arc<Mutex<Vec<PlaybackCommand>>>,
        events: Vec<PlaybackEvent>,
    }

    impl RecordingPlaybackBackend {
        fn new(commands: Arc<Mutex<Vec<PlaybackCommand>>>) -> Self {
            Self {
                commands,
                events: Vec::new(),
            }
        }
    }

    impl PlaybackBackend for RecordingPlaybackBackend {
        fn send(&mut self, command: PlaybackCommand) -> Result<(), PlaybackError> {
            self.commands
                .lock()
                .expect("commands")
                .push(command.clone());
            match command {
                PlaybackCommand::Play { .. } | PlaybackCommand::PlayPrepared { .. } => {
                    self.events
                        .push(PlaybackEvent::StateChanged(PlaybackState::Playing));
                }
                PlaybackCommand::PrepareNext(_) => {}
                PlaybackCommand::SetVolume(volume) => {
                    self.events.push(PlaybackEvent::VolumeChanged {
                        volume,
                        muted: false,
                    });
                }
                PlaybackCommand::SetMuted(muted) => {
                    self.events
                        .push(PlaybackEvent::VolumeChanged { volume: 1.0, muted });
                }
                _ => {}
            }
            Ok(())
        }

        fn drain_events(&mut self) -> Vec<PlaybackEvent> {
            std::mem::take(&mut self.events)
        }
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
    fn source_selection_is_saved_separately_from_playback_queue() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let server_id = snapshot.server.as_ref().expect("server").id.clone();
        let first = snapshot.tracks[0].clone();
        let second = snapshot.tracks[1].clone();

        controller.play_tracks_now(vec![first.clone(), second]);
        let queue = wait_for_queue(&events).expect("queue");
        assert_eq!(queue.entries[0].track_id, first.id);
        let _playback = wait_for_playback_state(&controller, &events, PlaybackState::Playing);

        controller.select_source(LibrarySourceSelection::Local);
        let local_snapshot = wait_for_snapshot(&events);

        assert_eq!(
            local_snapshot.selected_source,
            Some(LibrarySourceSelection::Local)
        );
        assert_eq!(
            controller.load_settings().sources.selected,
            Some(LibrarySourceSelection::Local)
        );
        assert_eq!(
            controller
                .queue
                .lock()
                .expect("queue")
                .as_ref()
                .expect("queue")
                .snapshot()
                .entries[0]
                .track_id,
            first.id
        );
        assert_eq!(
            controller
                .playback_snapshot
                .lock()
                .expect("playback")
                .current
                .as_ref()
                .expect("current")
                .track_id,
            first.id
        );

        controller.select_source(LibrarySourceSelection::Server(server_id.clone()));
        let server_snapshot = wait_for_snapshot(&events);

        assert_eq!(
            server_snapshot.selected_source,
            Some(LibrarySourceSelection::Server(server_id.clone()))
        );
        assert_eq!(
            controller.load_settings().sources.selected,
            Some(LibrarySourceSelection::Server(server_id))
        );
    }

    #[test]
    fn local_source_snapshot_loads_configured_folders() {
        let store = StoreHandle::open_memory().expect("memory store");
        let root = unique_test_dir("local-source-snapshot");
        fs::create_dir_all(&root).expect("create root");
        let mut settings = AppSettings::default();
        settings.sources.selected = Some(LibrarySourceSelection::Local);
        settings.sources.local_folders = vec![LocalLibraryFolder {
            path: root.to_string_lossy().into_owned(),
        }];
        store.save_settings(&settings).expect("save settings");

        let snapshot = load_snapshot(&store).expect("load snapshot");

        assert!(!snapshot.first_run);
        assert_eq!(
            snapshot.selected_source,
            Some(LibrarySourceSelection::Local)
        );
        assert_eq!(
            snapshot.server.expect("server").id.as_str(),
            LOCAL_SOURCE_SERVER_ID
        );
        assert_eq!(snapshot.local_folders, settings.sources.local_folders);
        let _cleanup = fs::remove_dir_all(root);
    }

    #[test]
    fn local_folder_preferences_add_preserves_remote_source_selection() {
        let store = StoreHandle::open_memory().expect("memory store");
        let saved = saved_server();
        let root = unique_test_dir("add-local-folder-preserve-source");
        fs::create_dir_all(&root).expect("create root");
        let mut settings = AppSettings::default();
        settings.sources.selected = Some(LibrarySourceSelection::Server(saved.server.id.clone()));
        store.save_settings(&settings).expect("save settings");
        store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&saved.server.id)
            })
            .expect("save server");
        let (controller, events) = controller_from_store_for_test(store);

        controller.add_local_library_folder(root.clone());
        let snapshot = wait_for_snapshot(&events);

        assert_eq!(
            snapshot.selected_source,
            Some(LibrarySourceSelection::Server(saved.server.id.clone()))
        );
        assert_eq!(snapshot.local_folders.len(), 1);
        let active = controller
            .store
            .with_store(|store| store.active_server())
            .expect("active server")
            .expect("active server");
        assert_eq!(active.server.id, saved.server.id);
        let _cleanup = fs::remove_dir_all(root);
    }

    #[test]
    fn local_folder_preferences_remove_preserves_remote_source_selection() {
        let store = StoreHandle::open_memory().expect("memory store");
        let saved = saved_server();
        let root = unique_test_dir("remove-local-folder-preserve-source");
        fs::create_dir_all(&root).expect("create root");
        let path = root.to_string_lossy().into_owned();
        let mut settings = AppSettings::default();
        settings.sources.selected = Some(LibrarySourceSelection::Server(saved.server.id.clone()));
        settings.sources.local_folders = vec![LocalLibraryFolder { path: path.clone() }];
        store.save_settings(&settings).expect("save settings");
        store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&saved.server.id)
            })
            .expect("save server");
        let (controller, events) = controller_from_store_for_test(store);

        controller.remove_local_library_folder(path);
        let snapshot = wait_for_snapshot(&events);

        assert_eq!(
            snapshot.selected_source,
            Some(LibrarySourceSelection::Server(saved.server.id.clone()))
        );
        assert!(snapshot.local_folders.is_empty());
        let active = controller
            .store
            .with_store(|store| store.active_server())
            .expect("active server")
            .expect("active server");
        assert_eq!(active.server.id, saved.server.id);
        let _cleanup = fs::remove_dir_all(root);
    }

    #[test]
    fn update_server_settings_persists_editable_fields() {
        let (controller, events, _snapshot, _queue, _player) =
            AppController::bootstrap_memory_for_test();
        let server_id = ServerId::new("server:editable");
        controller
            .store
            .with_store(|store| {
                store.save_server(&SavedServer {
                    server: ServerIdentity {
                        id: server_id.clone(),
                        provider: "jellyfin".to_string(),
                        name: "Old name".to_string(),
                        base_url: "http://old.example.test".to_string(),
                    },
                    user_id: "user-id".to_string(),
                    username: "listener".to_string(),
                    trust_invalid_cert: false,
                })?;
                store.set_active_server(&server_id)
            })
            .expect("save server");

        controller.update_server_settings(
            server_id.clone(),
            "Edited server".to_string(),
            "https://media.example.test".to_string(),
            true,
        );

        assert_eq!(wait_for_status(&events), "Server settings saved.");
        let snapshot = wait_for_snapshot(&events);
        let edited = snapshot
            .servers
            .iter()
            .find(|server| server.id == server_id)
            .expect("edited server");
        assert_eq!(edited.name, "Edited server");
        assert_eq!(edited.base_url, "https://media.example.test");
        let saved = controller
            .store
            .with_store(|store| store.list_servers())
            .expect("load saved servers")
            .into_iter()
            .find(|saved| saved.server.id == server_id)
            .expect("edited saved server");
        assert!(saved.trust_invalid_cert);
    }

    #[test]
    fn legacy_local_server_roots_migrate_to_local_source_settings() {
        let store = StoreHandle::open_memory().expect("memory store");
        let root = unique_test_dir("legacy-local-source");
        fs::create_dir_all(&root).expect("create root");
        let legacy = SavedServer {
            server: ServerIdentity {
                id: ServerId::new("local:server:legacy"),
                provider: LOCAL_PROVIDER_ID.to_string(),
                name: "Old Local".to_string(),
                base_url: root.to_string_lossy().into_owned(),
            },
            user_id: "local".to_string(),
            username: "Local".to_string(),
            trust_invalid_cert: false,
        };
        store
            .with_store(|store| {
                store.save_server(&legacy)?;
                store.set_active_server(&legacy.server.id)
            })
            .expect("save legacy server");
        let mut settings = AppSettings::default();
        settings.sources.selected = Some(LibrarySourceSelection::Server(legacy.server.id.clone()));
        store.save_settings(&settings).expect("save settings");

        let snapshot = load_snapshot(&store).expect("load snapshot");
        let migrated = load_settings_from_store(&store);

        assert_eq!(
            snapshot.selected_source,
            Some(LibrarySourceSelection::Local)
        );
        assert_eq!(
            snapshot.server.expect("server").id.as_str(),
            LOCAL_SOURCE_SERVER_ID
        );
        assert_eq!(
            migrated.sources.local_folders,
            vec![LocalLibraryFolder {
                path: root.to_string_lossy().into_owned()
            }]
        );
        assert_eq!(
            migrated.sources.selected,
            Some(LibrarySourceSelection::Local)
        );
        assert!(snapshot.servers.is_empty());
        let _cleanup = fs::remove_dir_all(root);
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
            SNAPSHOT_GRID_LIMIT.min(FakeScale::Small.album_count())
        );
        assert_eq!(
            snapshot.tracks.len(),
            SNAPSHOT_TRACK_LIMIT.min(FakeScale::Small.track_count())
        );
        assert_eq!(snapshot.cached_album_count, FakeScale::Small.album_count());
        assert_eq!(snapshot.cached_track_count, FakeScale::Small.track_count());
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
        assert_eq!(snapshot.albums.len(), SNAPSHOT_GRID_LIMIT);
        assert_eq!(snapshot.tracks.len(), 2_000);
        assert_eq!(snapshot.cached_album_count, 1_000);
        assert_eq!(snapshot.cached_track_count, 2_000);
    }

    #[test]
    fn provider_sync_caches_all_track_pages() {
        let runtime = Runtime::new().expect("runtime");
        let store = StoreHandle::open_memory().expect("memory store");
        let provider = FakeProvider::new(FakeScale::Small);
        let server_id = provider.identity().server.id.clone();
        let saved = SavedServer {
            server: provider.identity().server.clone(),
            user_id: "fake-user".to_string(),
            username: "fake".to_string(),
            trust_invalid_cert: false,
        };

        store
            .with_store(|store| store.save_server(&saved))
            .expect("save server");

        runtime
            .block_on(sync_provider(&store, &server_id, &provider))
            .expect("sync provider");

        let first_page = store
            .with_store(|store| store.load_tracks(&server_id, 0, 1))
            .expect("load first track page");
        let final_page = store
            .with_store(|store| {
                store.load_tracks(&server_id, FakeScale::Small.track_count() - 1, 10)
            })
            .expect("load final track page");

        assert_eq!(first_page.total, FakeScale::Small.track_count());
        assert_eq!(final_page.total, FakeScale::Small.track_count());
        assert_eq!(final_page.items.len(), 1);
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
    fn playlist_refresh_replaces_cached_list_without_full_sync() {
        let runtime = Runtime::new().expect("runtime");
        let store = StoreHandle::open_memory().expect("memory store");
        let provider = FakeProvider::new(FakeScale::Small);
        let saved = SavedServer {
            server: provider.identity().server.clone(),
            user_id: "fake-user".to_string(),
            username: "fake".to_string(),
            trust_invalid_cert: false,
        };
        let stale_track = runtime
            .block_on(provider.tracks(PagedRequest::new(0, 1)))
            .expect("stale track page")
            .items
            .into_iter()
            .next()
            .expect("stale track");
        let stale_playlist = Playlist {
            id: PlaylistId::new("fake:playlist:stale"),
            name: "Old Playlist".to_string(),
            track_count: 1,
            duration_seconds: stale_track.duration_seconds,
            image_ref: stale_track.image_ref.clone(),
        };
        let stale_entry = PlaylistEntry {
            entry_id: "old-playlist-entry".to_string(),
            track: stale_track.clone(),
        };

        store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&saved.server.id)?;
                store.upsert_tracks(&saved.server.id, std::slice::from_ref(&stale_track), 0)?;
                store.upsert_playlists(
                    &saved.server.id,
                    std::slice::from_ref(&stale_playlist),
                    0,
                )?;
                store.upsert_playlist_entries(
                    &saved.server.id,
                    &stale_playlist.id,
                    std::slice::from_ref(&stale_entry),
                    0,
                )?;
                Ok(())
            })
            .expect("seed stale playlists");

        let before = store
            .with_store(|store| store.load_playlists(&saved.server.id, 0, 10))
            .expect("load stale playlists");
        assert_eq!(before.total, 1);
        assert_eq!(before.items[0].id, stale_playlist.id);

        runtime
            .block_on(refresh_playlist_pages(&store, &saved.server.id, &provider))
            .expect("refresh playlists");

        let after = store
            .with_store(|store| store.load_playlists(&saved.server.id, 0, 10))
            .expect("load refreshed playlists");
        let detail = store
            .with_store(|store| store.load_playlist_detail(&saved.server.id, &PlaylistId::fake(1)))
            .expect("load playlist detail")
            .expect("playlist detail");
        let sync_state = store
            .with_store(|store| store.sync_state(&saved.server.id))
            .expect("sync state");

        assert!(after.total > 1);
        assert!(
            !after
                .items
                .iter()
                .any(|playlist| playlist.id == stale_playlist.id)
        );
        assert!(
            after
                .items
                .iter()
                .any(|playlist| playlist.id == PlaylistId::fake(1))
        );
        assert!(!detail.entries.is_empty());
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
        let mut expected_track = stale_track;
        let expected_credit = ArtistCredit {
            id: expected_track.artist_id.clone().expect("artist id"),
            name: expected_track.artist.clone(),
        };
        expected_track.artist_credits = vec![expected_credit];
        assert_eq!(after[1].tracks, vec![expected_track]);
    }

    #[test]
    fn home_section_refresh_uses_home_update_event() {
        let event = home_refresh_completed_event(
            super::HomeRefreshTarget::Section(HomeSectionKind::MostPlayed),
            Box::new(LibrarySnapshot::first_run()),
        );

        assert!(matches!(
            event,
            ControllerEvent::HomeSectionsUpdated {
                include_explore: false,
                ..
            }
        ));

        let event = home_refresh_completed_event(
            super::HomeRefreshTarget::Section(HomeSectionKind::Explore),
            Box::new(LibrarySnapshot::first_run()),
        );

        assert!(matches!(
            event,
            ControllerEvent::HomeSectionsUpdated {
                include_explore: true,
                ..
            }
        ));
    }

    #[test]
    fn home_refresh_without_explore_leaves_explore_cache_unchanged() {
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
                            tracks: vec![stale_track],
                        },
                    ],
                    0,
                )?;
                Ok(())
            })
            .expect("seed stale home sections");

        runtime
            .block_on(refresh_home_sections_without_explore(
                &store,
                &saved.server.id,
                &provider,
            ))
            .expect("refresh non-Explore home sections");

        let after = store
            .with_store(|store| store.load_home_sections(&saved.server.id))
            .expect("load refreshed home sections");

        assert_eq!(after[0].kind, HomeSectionKind::Explore);
        assert_eq!(after[0].albums[0].id, stale_album.id);
        assert_eq!(after[1].kind, HomeSectionKind::MostPlayed);
        assert_eq!(after[1].tracks[0].id, TrackId::fake(1));
    }

    #[test]
    fn explore_prefetch_promotes_only_when_requested() {
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

        store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&saved.server.id)?;
                store.upsert_albums(&saved.server.id, std::slice::from_ref(&stale_album), 0)?;
                store.upsert_home_section(
                    &saved.server.id,
                    &HomeSection {
                        kind: HomeSectionKind::Explore,
                        albums: vec![stale_album.clone()],
                        tracks: Vec::new(),
                    },
                    0,
                )?;
                Ok(())
            })
            .expect("seed stale Explore");

        let prefetched = runtime
            .block_on(prefetch_home_section(
                &store,
                &saved.server.id,
                &provider,
                HomeSectionKind::Explore,
            ))
            .expect("prefetch Explore");
        let visible_before = store
            .with_store(|store| store.load_home_sections(&saved.server.id))
            .expect("load visible sections");

        assert_eq!(visible_before[0].albums[0].id, stale_album.id);
        assert_eq!(prefetched.albums[0].id, AlbumId::fake(1));
        assert!(
            store
                .with_store(|store| {
                    store.load_home_section_prefetch(&saved.server.id, HomeSectionKind::Explore)
                })
                .expect("load prefetched Explore")
                .is_some()
        );

        promote_prefetched_home_section(&store, &saved.server.id, &prefetched)
            .expect("promote prefetched Explore");

        let visible_after = store
            .with_store(|store| store.load_home_sections(&saved.server.id))
            .expect("load promoted sections");
        assert_eq!(visible_after[0].albums[0].id, AlbumId::fake(1));
        assert!(
            store
                .with_store(|store| {
                    store.load_home_section_prefetch(&saved.server.id, HomeSectionKind::Explore)
                })
                .expect("load cleared prefetched Explore")
                .is_none()
        );
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
    fn external_cached_cover_reuses_available_size() {
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
            "rufin-external-cover-{}-{}.jpg",
            std::process::id(),
            "cached"
        ));
        fs::write(&path, [1_u8, 2, 3]).expect("write cover");
        let image_ref = ImageRef::new(
            "external:album:Example%20Artist:Example%20Album",
            Some("external-v1-test".to_string()),
        );

        controller
            .store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&server_id)?;
                store.save_cover_cache_entry(&CoverCacheEntry {
                    server_id: server_id.clone(),
                    item_id: image_ref.item_id.clone(),
                    image_tag: "external-v1-test".to_string(),
                    size: 256,
                    path: path.to_string_lossy().to_string(),
                })
            })
            .expect("seed cover cache");

        assert_eq!(
            controller.cached_cover_path(&image_ref, 512),
            Some(path.clone())
        );
        assert_eq!(
            controller.cached_cover_path(&image_ref, 96),
            Some(path.clone())
        );
        let _cleanup = fs::remove_file(path);
    }

    #[test]
    fn provider_cached_cover_reuses_available_size() {
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
            "rufin-provider-cover-{}-{}.jpg",
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

        assert_eq!(
            controller.cached_cover_path(&image_ref, 512),
            Some(path.clone())
        );
        assert_eq!(
            controller.cached_cover_path(&image_ref, 96),
            Some(path.clone())
        );
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
        assert_eq!(queue.entries.len(), 1 + super::AUTO_DJ_ITEM_COUNT);
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
            1 + super::AUTO_DJ_ITEM_COUNT
        );
    }

    #[test]
    fn play_tracks_prepares_next_stream_for_backend() {
        let (controller, _events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let commands = Arc::new(Mutex::new(Vec::new()));
        *controller.playback.lock().expect("playback") =
            Box::new(RecordingPlaybackBackend::new(Arc::clone(&commands)));
        let first = snapshot.tracks[0].clone();
        let second = snapshot.tracks[1].clone();

        controller.play_tracks_now(vec![first.clone(), second.clone()]);

        let command = wait_for_recorded_command(&commands, |command| {
            matches!(command, PlaybackCommand::PlayPrepared { .. })
        });
        let PlaybackCommand::PlayPrepared { item, next, .. } = command else {
            panic!("expected prepared play command");
        };
        assert_eq!(item.track.id, first.id);
        assert_eq!(next.expect("next").track.id, second.id);
    }

    #[test]
    fn local_access_changes_reprepare_next_stream_for_backend() {
        let (controller, _events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let commands = Arc::new(Mutex::new(Vec::new()));
        *controller.playback.lock().expect("playback") =
            Box::new(RecordingPlaybackBackend::new(Arc::clone(&commands)));
        let server_id = snapshot.server.as_ref().expect("server").id.clone();
        let first = snapshot.tracks[0].clone();
        let second = snapshot.tracks[1].clone();

        controller.play_tracks_now(vec![first, second.clone()]);
        let _play = wait_for_recorded_command(&commands, |command| {
            matches!(command, PlaybackCommand::PlayPrepared { .. })
        });
        commands.lock().expect("commands").clear();

        let root = unique_test_dir("reprepare-local-access");
        fs::create_dir_all(&root).expect("create root");
        controller.save_server_local_access(
            server_id.clone(),
            root.clone(),
            Some("/server/music".to_string()),
            Some(root.to_string_lossy().into_owned()),
        );

        let command = wait_for_recorded_command(&commands, |command| {
            matches!(command, PlaybackCommand::PrepareNext(Some(_)))
        });
        let PlaybackCommand::PrepareNext(Some(item)) = command else {
            panic!("expected prepared next command");
        };
        assert_eq!(item.track.id, second.id);
        commands.lock().expect("commands").clear();

        controller.clear_server_local_access(server_id);

        let command = wait_for_recorded_command(&commands, |command| {
            matches!(command, PlaybackCommand::PrepareNext(Some(_)))
        });
        let PlaybackCommand::PrepareNext(Some(item)) = command else {
            panic!("expected prepared next command");
        };
        assert_eq!(item.track.id, second.id);
        let _cleanup = fs::remove_dir_all(root);
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
    fn manual_next_at_queue_end_wraps_to_first_track() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let first = snapshot.tracks[0].clone();
        let second = snapshot.tracks[1].clone();

        controller.toggle_auto_dj();
        let _playback = wait_for_playback_auto_dj(&events, false);
        controller.play_tracks_now(vec![first.clone(), second.clone()]);
        let _queue = wait_for_queue(&events).expect("queue");
        controller.next_track();
        let _queue = wait_for_queue(&events).expect("next queue");
        controller.seek_millis(12_000);
        let _playback = wait_for_playback_position(&events, 12_000);

        controller.next_track();

        let playback = wait_for_playback_position(&events, 0);
        assert_eq!(playback.current.expect("current").track_id, first.id);
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
    fn cycle_repeat_uses_all_one_off_order() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));

        controller.play_now(snapshot.tracks[0].clone());
        let _queue = wait_for_queue(&events).expect("queue");

        controller.cycle_repeat();
        let queue = wait_for_queue(&events).expect("repeat one");
        assert_eq!(queue.repeat_mode, RepeatMode::One);

        controller.cycle_repeat();
        let queue = wait_for_queue(&events).expect("repeat off");
        assert_eq!(queue.repeat_mode, RepeatMode::Off);

        controller.cycle_repeat();
        let queue = wait_for_queue(&events).expect("repeat all");
        assert_eq!(queue.repeat_mode, RepeatMode::All);
    }

    #[test]
    fn path_settings_round_trip_uses_config_file_without_sqlite() {
        let dir = unique_test_dir("settings-round-trip");
        let settings_path = dir.join("config").join(SETTINGS_FILE_NAME);
        let database_path = dir.join(DATABASE_FILE_NAME);
        let store = StoreHandle::Path {
            database_path: database_path.clone(),
            settings_path: settings_path.clone(),
        };
        let settings = AppSettings {
            theme_preference: ThemePreference::Dark,
            auto_dj_enabled: true,
            ..AppSettings::default()
        };

        store.save_settings(&settings).expect("save settings");

        assert_eq!(load_settings_from_store(&store), settings);
        assert!(settings_path.exists());
        assert!(!database_path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(&settings_path)
                    .expect("settings metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let _cleanup = fs::remove_dir_all(dir);
    }

    #[test]
    fn toggle_auto_dj_persists_and_emits_playback_state() {
        let (controller, events, _snapshot, _queue, player) =
            AppController::bootstrap(Some(FakeScale::Small));

        assert!(player.auto_dj_enabled);

        controller.toggle_auto_dj();

        let playback = wait_for_playback_auto_dj(&events, false);
        assert!(!playback.auto_dj_enabled);
        assert!(!controller.load_settings().auto_dj_enabled);
    }

    #[test]
    fn random_play_now_replaces_queue_and_starts_first_random_track() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let expected = random_track_ids(&snapshot.tracks, 3);

        controller.play_random_tracks(random_request(RandomPlayAction::PlayNow, 3));

        let queue = wait_for_queue(&events).expect("random queue");
        assert_eq!(queue.current_index, Some(0));
        assert_eq!(
            queue
                .entries
                .iter()
                .map(|entry| entry.track_id.clone())
                .collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn random_play_next_inserts_tracks_after_current() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let first = snapshot.tracks[0].clone();
        let second = snapshot.tracks[1].clone();
        let expected_random = random_track_ids(&snapshot.tracks, 2);

        controller.play_tracks_now(vec![first.clone(), second.clone()]);
        let _queue = wait_for_queue(&events).expect("initial queue");
        controller.play_random_tracks(random_request(RandomPlayAction::PlayNext, 2));

        let queue = wait_for_queue(&events).expect("random next queue");
        let ids = queue
            .entries
            .iter()
            .map(|entry| entry.track_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(queue.current_index, Some(0));
        assert_eq!(ids[0], first.id);
        assert_eq!(&ids[1..3], expected_random.as_slice());
        assert_eq!(ids[3], second.id);
    }

    #[test]
    fn random_add_last_appends_tracks_without_replacing_current() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let first = snapshot.tracks[0].clone();
        let second = snapshot.tracks[1].clone();
        let expected_random = random_track_ids(&snapshot.tracks, 2);

        controller.play_tracks_now(vec![first.clone(), second.clone()]);
        let _queue = wait_for_queue(&events).expect("initial queue");
        controller.play_random_tracks(random_request(RandomPlayAction::AddLast, 2));

        let queue = wait_for_queue(&events).expect("random append queue");
        let ids = queue
            .entries
            .iter()
            .map(|entry| entry.track_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(queue.current_index, Some(0));
        assert_eq!(ids[0], first.id);
        assert_eq!(ids[1], second.id);
        assert_eq!(&ids[2..4], expected_random.as_slice());
    }

    #[test]
    fn auto_dj_tops_up_low_queue_from_cached_library() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let first = snapshot.tracks[0].clone();

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
    fn prepared_track_started_advances_queue_without_restarting_playback() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let commands = Arc::new(Mutex::new(Vec::new()));
        *controller.playback.lock().expect("playback") =
            Box::new(RecordingPlaybackBackend::new(Arc::clone(&commands)));
        let first = snapshot.tracks[0].clone();
        let second = snapshot.tracks[1].clone();

        controller.play_tracks_now(vec![first, second.clone()]);
        let _initial_queue = wait_for_queue(&events).expect("initial queue");
        let _command = wait_for_recorded_command(&commands, |command| {
            matches!(command, PlaybackCommand::PlayPrepared { .. })
        });
        commands.lock().expect("commands").clear();

        controller.advance_after_prepared_track_started(PlaybackTrack {
            id: second.id.clone(),
            title: second.title.clone(),
            artist: second.artist.clone(),
            album: second.album.clone(),
            duration_seconds: second.duration_seconds,
        });
        let queue = wait_for_queue(&events).expect("queue");

        assert_eq!(
            queue.entries[queue.current_index.expect("current")].track_id,
            second.id
        );
        assert!(
            commands
                .lock()
                .expect("commands")
                .iter()
                .all(|command| !matches!(
                    command,
                    PlaybackCommand::Play { .. } | PlaybackCommand::PlayPrepared { .. }
                ))
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
    fn server_lyrics_request_ignores_cached_remote_lyrics() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let track = snapshot.tracks[0].clone();
        controller.play_now(track.clone());
        let _playback = wait_for_playback_state(&controller, &events, PlaybackState::Playing);
        let server_id = controller
            .store
            .with_store(|store| store.active_server())
            .expect("load active server")
            .expect("active server")
            .server
            .id;
        let remote_lyrics = Lyrics {
            track_id: track.id,
            source: LyricsSource::Remote,
            lines: vec![LyricLine {
                text: "cached remote line".to_string(),
                start_millis: None,
            }],
        };
        controller
            .store
            .with_store(|store| store.save_lyrics(&server_id, &remote_lyrics))
            .expect("save remote lyrics");

        controller.request_server_lyrics_for_current();

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
        let path = super::lyrics_save_path("Song Title", &AppSettings::default())
            .expect("lyrics save path");
        let path = path.to_string_lossy();

        assert!(path.contains("Music") || path.contains("music"));
        assert!(path.ends_with("Song Title.lrc"));
    }

    #[test]
    fn lyrics_save_path_uses_configured_export_folder() {
        let settings = AppSettings {
            lyrics_export_folder: Some("/tmp/rufin-lyrics".to_string()),
            ..AppSettings::default()
        };

        let path = super::lyrics_save_path("Song Title", &settings).expect("lyrics save path");

        assert_eq!(path, PathBuf::from("/tmp/rufin-lyrics/Song Title.lrc"));
    }

    #[test]
    fn local_sidecar_lyrics_use_same_stem_as_audio_file() {
        let store = StoreHandle::open_memory().expect("memory store");
        let saved = self::saved_server();
        let dir = self::unique_test_dir("local-sidecar");
        fs::create_dir_all(&dir).expect("create dir");
        let generation = store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&saved.server.id)?;
                store.save_server_local_access(&ServerLocalAccess {
                    server_id: saved.server.id.clone(),
                    root_path: dir.to_string_lossy().into_owned(),
                    path_replace_from: None,
                    path_replace_to: Some(dir.to_string_lossy().into_owned()),
                })?;
                store.begin_sync(&saved.server.id)
            })
            .expect("begin sync");
        let audio = dir.join("07 I'm feeling lucky.flac");
        let lrc = dir.join("07 I'm feeling lucky.lrc");
        fs::write(&audio, []).expect("audio");
        fs::write(&lrc, "[00:01.00]line one").expect("lrc");
        let mut track = restored_track();
        track.local_path = Some(audio.to_string_lossy().into_owned());
        store
            .with_store(|store| store.upsert_tracks(&saved.server.id, &[track.clone()], generation))
            .expect("upsert track");

        let lyrics = super::local_sidecar_lyrics(&store, &saved.server.id, &track.id)
            .expect("sidecar lyrics");

        assert_eq!(lyrics.source, LyricsSource::Local);
        assert_eq!(lyrics.lines[0].text, "line one");
        assert_eq!(lyrics.lines[0].start_millis, Some(1_000));
        let _cleanup = fs::remove_dir_all(dir);
    }

    #[test]
    fn mapped_local_audio_path_uses_server_prefix_replacement() {
        let store = StoreHandle::open_memory().expect("memory store");
        let saved = self::saved_server();
        let generation = store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.save_server_local_access(&ServerLocalAccess {
                    server_id: saved.server.id.clone(),
                    root_path: "/unused".to_string(),
                    path_replace_from: Some("/server/music".to_string()),
                    path_replace_to: Some(
                        self::unique_test_dir("mapped-audio")
                            .to_string_lossy()
                            .into_owned(),
                    ),
                })?;
                store.begin_sync(&saved.server.id)
            })
            .expect("begin sync");
        let root = store
            .with_store(|store| store.server_local_access(&saved.server.id))
            .expect("access")
            .expect("access")
            .path_replace_to
            .expect("replace to");
        let root = PathBuf::from(root);
        let audio = root.join("Album/Track.flac");
        fs::create_dir_all(audio.parent().expect("parent")).expect("create dir");
        fs::write(&audio, []).expect("audio");
        let mut track = restored_track();
        track.local_path = Some("/server/music/Album/Track.flac".to_string());
        store
            .with_store(|store| store.upsert_tracks(&saved.server.id, &[track.clone()], generation))
            .expect("upsert track");

        let mapped = super::local_audio_path_for_track(&store, &saved.server.id, &track.id)
            .expect("mapped path");

        assert_eq!(mapped, audio);
        let _cleanup = fs::remove_dir_all(root);
    }

    #[test]
    fn remote_local_audio_path_requires_configured_access() {
        let store = StoreHandle::open_memory().expect("memory store");
        let saved = self::saved_server();
        let generation = store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&saved.server.id)?;
                store.begin_sync(&saved.server.id)
            })
            .expect("begin sync");
        let dir = self::unique_test_dir("remote-no-local-access");
        fs::create_dir_all(&dir).expect("create dir");
        let audio = dir.join("Track.flac");
        fs::write(&audio, []).expect("audio");
        let mut track = restored_track();
        track.local_path = Some(audio.to_string_lossy().into_owned());
        store
            .with_store(|store| store.upsert_tracks(&saved.server.id, &[track.clone()], generation))
            .expect("upsert track");

        let mapped = super::local_audio_path_for_track(&store, &saved.server.id, &track.id);

        assert_eq!(mapped, None);
        let _cleanup = fs::remove_dir_all(dir);
    }

    #[test]
    fn resolve_stream_prefers_local_file_for_remote_server_with_access() {
        let store = StoreHandle::open_memory().expect("memory store");
        let saved = self::saved_server();
        let root = self::unique_test_dir("local-playback-stream");
        let audio = root.join("Album/Track.flac");
        fs::create_dir_all(audio.parent().expect("parent")).expect("create dir");
        fs::write(&audio, []).expect("audio");
        let generation = store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&saved.server.id)?;
                store.save_server_local_access(&ServerLocalAccess {
                    server_id: saved.server.id.clone(),
                    root_path: root.to_string_lossy().into_owned(),
                    path_replace_from: Some("/server/music".to_string()),
                    path_replace_to: Some(root.to_string_lossy().into_owned()),
                })?;
                store.begin_sync(&saved.server.id)
            })
            .expect("begin sync");
        let mut track = restored_track();
        track.local_path = Some("/server/music/Album/Track.flac".to_string());
        store
            .with_store(|store| store.upsert_tracks(&saved.server.id, &[track.clone()], generation))
            .expect("upsert track");
        let runtime = Arc::new(Runtime::new().expect("runtime"));
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());

        let stream = super::resolve_stream(
            &store,
            &runtime,
            &secrets,
            &saved.server.id,
            &track.id,
            &PlaybackSettings::default(),
        )
        .expect("stream");

        assert!(stream.uri().starts_with("file://"));
        assert!(stream.uri().contains("Track.flac"));
        let _cleanup = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_stream_uses_cached_local_match_without_server_path() {
        let store = StoreHandle::open_memory().expect("memory store");
        let saved = self::saved_server();
        let root = self::unique_test_dir("cached-local-match-stream");
        let audio = root.join("Album/Track.flac");
        fs::create_dir_all(audio.parent().expect("parent")).expect("create dir");
        fs::write(&audio, []).expect("audio");
        let generation = store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&saved.server.id)?;
                store.save_server_local_access(&ServerLocalAccess {
                    server_id: saved.server.id.clone(),
                    root_path: root.to_string_lossy().into_owned(),
                    path_replace_from: None,
                    path_replace_to: Some(root.to_string_lossy().into_owned()),
                })?;
                store.begin_sync(&saved.server.id)
            })
            .expect("begin sync");
        let track = restored_track();
        store
            .with_store(|store| {
                store.upsert_tracks(&saved.server.id, std::slice::from_ref(&track), generation)?;
                store.replace_track_local_matches(
                    &saved.server.id,
                    &[(
                        track.id.clone(),
                        audio.to_string_lossy().into_owned(),
                        "metadata".to_string(),
                    )],
                )
            })
            .expect("seed track");
        let runtime = Arc::new(Runtime::new().expect("runtime"));
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());

        let stream = super::resolve_stream(
            &store,
            &runtime,
            &secrets,
            &saved.server.id,
            &track.id,
            &PlaybackSettings::default(),
        )
        .expect("stream");

        assert!(stream.uri().starts_with("file://"));
        assert!(stream.uri().contains("Track.flac"));
        let _cleanup = fs::remove_dir_all(root);
    }

    #[test]
    fn relative_local_audio_path_uses_configured_local_prefix() {
        let store = StoreHandle::open_memory().expect("memory store");
        let saved = self::saved_server();
        let scan_root = self::unique_test_dir("relative-scan-root");
        let local_root = self::unique_test_dir("relative-local-prefix");
        let audio = local_root.join("Album/Track.flac");
        fs::create_dir_all(audio.parent().expect("parent")).expect("create dir");
        fs::write(&audio, []).expect("audio");
        let generation = store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&saved.server.id)?;
                store.save_server_local_access(&ServerLocalAccess {
                    server_id: saved.server.id.clone(),
                    root_path: scan_root.to_string_lossy().into_owned(),
                    path_replace_from: None,
                    path_replace_to: Some(local_root.to_string_lossy().into_owned()),
                })?;
                store.begin_sync(&saved.server.id)
            })
            .expect("begin sync");
        let mut track = restored_track();
        track.local_path = Some("Album/Track.flac".to_string());
        store
            .with_store(|store| store.upsert_tracks(&saved.server.id, &[track.clone()], generation))
            .expect("upsert track");

        let mapped = super::local_audio_path_for_track(&store, &saved.server.id, &track.id)
            .expect("mapped path");

        assert_eq!(mapped, audio);
        let _cleanup = fs::remove_dir_all(scan_root);
        let _cleanup = fs::remove_dir_all(local_root);
    }

    #[test]
    fn snapshot_local_access_status_counts_cached_mapping_candidates() {
        let store = StoreHandle::open_memory().expect("memory store");
        let saved = self::saved_server();
        let root = self::unique_test_dir("local-access-status");
        let local_prefix = root.join("mapped");
        let direct_audio = root.join("Direct.flac");
        let prefix_audio = local_prefix.join("Album/Mapped.flac");
        let metadata_audio = root.join("Metadata.flac");
        fs::create_dir_all(prefix_audio.parent().expect("parent")).expect("create mapped dir");
        fs::write(&direct_audio, []).expect("direct audio");
        fs::write(&prefix_audio, []).expect("prefix audio");
        fs::write(&metadata_audio, []).expect("metadata audio");
        let generation = store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.set_active_server(&saved.server.id)?;
                store.save_server_local_access(&ServerLocalAccess {
                    server_id: saved.server.id.clone(),
                    root_path: root.to_string_lossy().into_owned(),
                    path_replace_from: Some("/server/music".to_string()),
                    path_replace_to: Some(local_prefix.to_string_lossy().into_owned()),
                })?;
                store.begin_sync(&saved.server.id)
            })
            .expect("begin sync");
        let mut direct = restored_track();
        direct.id = TrackId::new("jellyfin:track:direct");
        direct.title = "Direct".to_string();
        direct.local_path = Some(direct_audio.to_string_lossy().into_owned());
        let mut prefix = restored_track();
        prefix.id = TrackId::new("jellyfin:track:prefix");
        prefix.title = "Prefix".to_string();
        prefix.local_path = Some("/server/music/Album/Mapped.flac".to_string());
        let mut metadata = restored_track();
        metadata.id = TrackId::new("jellyfin:track:metadata");
        metadata.title = "Metadata".to_string();
        let mut unmatched = restored_track();
        unmatched.id = TrackId::new("jellyfin:track:unmatched");
        unmatched.title = "Unmatched".to_string();
        unmatched.local_path = Some("/server/music/Album/Missing.flac".to_string());
        store
            .with_store(|store| {
                store.upsert_tracks(
                    &saved.server.id,
                    &[direct, prefix, metadata.clone(), unmatched],
                    generation,
                )?;
                store.replace_track_local_matches(
                    &saved.server.id,
                    &[(
                        metadata.id.clone(),
                        metadata_audio.to_string_lossy().into_owned(),
                        "metadata".to_string(),
                    )],
                )
            })
            .expect("seed tracks");

        let snapshot = super::load_snapshot(&store).expect("load snapshot");

        assert_eq!(snapshot.local_access_status.total_track_count, 4);
        assert_eq!(snapshot.local_access_status.direct_match_count, 1);
        assert_eq!(snapshot.local_access_status.prefix_match_count, 2);
        assert_eq!(snapshot.local_access_status.metadata_match_count, 1);
        assert_eq!(snapshot.local_access_status.unmatched_count, 0);
        assert!(snapshot.local_access_status.sample_server_path.is_some());
        let _cleanup = fs::remove_dir_all(root);
    }

    #[test]
    fn conservative_local_matches_only_accept_unique_duration_matches() {
        let album = AlbumId::fake(1);
        let mut remote = restored_track();
        remote.album_id = album.clone();
        remote.title = "First Motion".to_string();
        remote.album = "Blue Rooms".to_string();
        remote.artist = "Astral Kin".to_string();
        remote.duration_seconds = 210;
        remote.disc_number = 1;
        remote.track_number = 7;

        let mut local = remote.clone();
        local.id = TrackId::new("local:track:one");
        local.local_path = Some("/home/me/Music/Blue Rooms/07 First Motion.flac".to_string());
        local.duration_seconds = 212;

        let matches = super::conservative_local_matches(&[remote.clone()], &[local.clone()]);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].0, remote.id);
        assert_eq!(
            matches[0].1,
            "/home/me/Music/Blue Rooms/07 First Motion.flac"
        );

        let local_one = local.clone();
        let mut duplicate = local;
        duplicate.id = TrackId::new("local:track:two");
        duplicate.local_path = Some("/home/me/Music/Other/07 First Motion.flac".to_string());

        assert!(super::conservative_local_matches(&[remote], &[local_one, duplicate]).is_empty());
    }

    #[test]
    fn snapshot_includes_active_server_local_access() {
        let store = StoreHandle::open_memory().expect("memory store");
        let saved = self::saved_server();
        let access = ServerLocalAccess {
            server_id: saved.server.id.clone(),
            root_path: "/home/demo/Music".to_string(),
            path_replace_from: Some("/server/music".to_string()),
            path_replace_to: Some("/home/demo/Music".to_string()),
        };
        store
            .with_store(|store| {
                store.save_server(&saved)?;
                store.save_server_local_access(&access)?;
                store.set_active_server(&saved.server.id)
            })
            .expect("save server");

        let snapshot = super::load_snapshot(&store).expect("load snapshot");

        assert_eq!(snapshot.local_access, Some(access));
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
    fn lrclib_duration_accepts_fractional_seconds() {
        let json = r#"{
            "id": 7,
            "trackName": "Imagine",
            "artistName": "John Lennon",
            "albumName": "Imagine",
            "duration": 185.0,
            "plainLyrics": "line",
            "syncedLyrics": null
        }"#;

        let dto =
            serde_json::from_str::<super::LrcLibLyricsDto>(json).expect("deserialize lrclib dto");
        let result = super::LyricsSearchResult::from(dto);

        assert_eq!(result.duration_seconds, 185);
        assert_eq!(result.track_name, "Imagine");
        assert_eq!(result.artist_name, "John Lennon");
    }

    #[test]
    fn lrclib_manual_search_uses_combined_query_first() {
        let urls = super::lrclib_search_urls("joy", "feel my soul").expect("lrclib search urls");
        let query_pairs = urls[0]
            .query_pairs()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<Vec<_>>();

        assert_eq!(
            query_pairs,
            vec![("q".to_string(), "feel my soul joy".to_string())]
        );
    }

    #[test]
    fn lrclib_search_body_decodes_feel_my_soul_result() {
        let json = r#"[{
            "id": 9386114,
            "name": "feel my soul",
            "artistName": "joy",
            "albumName": "feel my soul",
            "duration": 223.0,
            "plainLyrics": "plain line",
            "syncedLyrics": "[00:01.00]synced line",
            "lyricsfile": null
        }]"#;

        let results = super::parse_lrclib_search_body(json).expect("parse lrclib response");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, 9_386_114);
        assert_eq!(results[0].track_name, "feel my soul");
        assert_eq!(results[0].artist_name, "joy");
        assert_eq!(results[0].duration_seconds, 223);
        assert!(results[0].synced_lyrics.is_some());
        assert!(results[0].plain_lyrics.is_some());
    }

    #[test]
    fn lrclib_results_prefer_matching_title_over_album_hit() {
        let mut results = vec![
            super::LyricsSearchResult {
                id: 1,
                track_name: "Crippled Inside".to_string(),
                artist_name: "John Lennon".to_string(),
                album_name: "Imagine".to_string(),
                duration_seconds: 233,
                synced_lyrics: Some("[00:01.00]line".to_string()),
                plain_lyrics: Some("line".to_string()),
            },
            super::LyricsSearchResult {
                id: 2,
                track_name: "Imagine".to_string(),
                artist_name: "John Lennon".to_string(),
                album_name: "Lennon".to_string(),
                duration_seconds: 185,
                synced_lyrics: None,
                plain_lyrics: Some("line".to_string()),
            },
        ];

        super::order_lrclib_results(&mut results, "John Lennon", "Imagine");

        assert_eq!(results[0].track_name, "Imagine");
    }

    #[test]
    fn controller_events_are_sendable() {
        fn assert_send<T: Send>() {}
        assert_send::<ControllerEvent>();
    }

    #[test]
    fn provider_not_found_cover_errors_are_classified() {
        assert!(super::covers::is_provider_not_found_error(
            "provider item was not found"
        ));
        assert!(!super::covers::is_provider_not_found_error(
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
        let playback_snapshot = playback_snapshot_from_queue(
            queue.as_ref(),
            settings.auto_dj_enabled,
            &settings.playback,
        );
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
            external_scrobble_state: Arc::new(Mutex::new(ExternalScrobbleState::default())),
            events,
            sync_in_flight: Arc::new(Mutex::new(HashSet::new())),
            home_refresh_in_flight: Arc::new(Mutex::new(HashSet::new())),
            playlist_refresh_in_flight: Arc::new(Mutex::new(HashSet::new())),
            explore_prefetch_in_flight: Arc::new(Mutex::new(HashSet::new())),
            cover_in_flight: Arc::new(Mutex::new(HashSet::new())),
            external_cover_prefetch_in_flight: Arc::new(Mutex::new(HashSet::new())),
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
            artist_credits: Vec::new(),
            album_artist_credits: Vec::new(),
            album: "Album".to_string(),
            year: 2026,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 180,
            favorite: false,
            disc_number: 1,
            track_number: 1,
            image_ref: None,
            genres: Vec::new(),
            local_path: None,
        }
    }

    fn saved_server() -> SavedServer {
        SavedServer {
            server: ServerIdentity {
                id: ServerId::new("jellyfin:server:test"),
                provider: "jellyfin".to_string(),
                name: "Test Server".to_string(),
                base_url: "https://music.example".to_string(),
            },
            user_id: "user".to_string(),
            username: "demo".to_string(),
            trust_invalid_cert: false,
        }
    }

    fn unique_test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rufin-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ))
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
            artist_credits: Vec::new(),
            album_artist_credits: Vec::new(),
            album: "Album".to_string(),
            year: 2026,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 180,
            favorite: false,
            disc_number: 1,
            track_number: number as u16,
            image_ref: None,
            genres: genres.iter().map(|genre| genre.to_string()).collect(),
            local_path: None,
        }
    }

    fn wait_for_snapshot(events: &Receiver<ControllerEvent>) -> LibrarySnapshot {
        loop {
            match events
                .recv_timeout(Duration::from_secs(5))
                .expect("controller event")
            {
                ControllerEvent::Snapshot(snapshot)
                | ControllerEvent::HomeSectionsUpdated { snapshot, .. } => return *snapshot,
                ControllerEvent::Queue(_)
                | ControllerEvent::FavoriteChanged { .. }
                | ControllerEvent::Playback(_)
                | ControllerEvent::Lyrics(_)
                | ControllerEvent::LyricsSearchResults { .. }
                | ControllerEvent::LyricsSaved { .. }
                | ControllerEvent::FolderLoaded { .. }
                | ControllerEvent::FolderLoadFailed { .. }
                | ControllerEvent::HomeSectionPrefetched { .. }
                | ControllerEvent::ServerDiscovery { .. }
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
                | ControllerEvent::HomeSectionsUpdated { .. }
                | ControllerEvent::Queue(_)
                | ControllerEvent::Playback(_)
                | ControllerEvent::Lyrics(_)
                | ControllerEvent::LyricsSearchResults { .. }
                | ControllerEvent::LyricsSaved { .. }
                | ControllerEvent::FolderLoaded { .. }
                | ControllerEvent::FolderLoadFailed { .. }
                | ControllerEvent::HomeSectionPrefetched { .. }
                | ControllerEvent::ServerDiscovery { .. }
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
                | ControllerEvent::HomeSectionsUpdated { .. }
                | ControllerEvent::FavoriteChanged { .. }
                | ControllerEvent::Queue(_)
                | ControllerEvent::Playback(_)
                | ControllerEvent::Lyrics(_)
                | ControllerEvent::LyricsSearchResults { .. }
                | ControllerEvent::LyricsSaved { .. }
                | ControllerEvent::FolderLoaded { .. }
                | ControllerEvent::FolderLoadFailed { .. }
                | ControllerEvent::HomeSectionPrefetched { .. }
                | ControllerEvent::ServerDiscovery { .. }
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
                | ControllerEvent::HomeSectionsUpdated { .. }
                | ControllerEvent::FavoriteChanged { .. }
                | ControllerEvent::Playback(_)
                | ControllerEvent::LoginStatus(_)
                | ControllerEvent::Lyrics(_)
                | ControllerEvent::LyricsSearchResults { .. }
                | ControllerEvent::LyricsSaved { .. }
                | ControllerEvent::FolderLoaded { .. }
                | ControllerEvent::FolderLoadFailed { .. }
                | ControllerEvent::HomeSectionPrefetched { .. }
                | ControllerEvent::ServerDiscovery { .. }
                | ControllerEvent::CoverReady { .. } => {}
                ControllerEvent::Error(error) => panic!("controller error: {error}"),
            }
        }
    }

    fn random_request(action: RandomPlayAction, limit: usize) -> RandomPlayRequest {
        RandomPlayRequest {
            action,
            limit,
            min_year: None,
            max_year: None,
            genre_id: None,
            genre_name: None,
            played_filter: PlayedFilter::All,
        }
    }

    fn random_track_ids(tracks: &[Track], limit: usize) -> Vec<TrackId> {
        let mut ids = tracks
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>();
        ids.sort_by_key(|id| id.as_str().to_string());
        ids.truncate(limit);
        ids
    }

    fn wait_for_cover_ready(events: &Receiver<ControllerEvent>, expected_key: &str) -> PathBuf {
        loop {
            match events
                .recv_timeout(Duration::from_secs(5))
                .expect("controller event")
            {
                ControllerEvent::CoverReady { key, path } if key == expected_key => return path,
                ControllerEvent::Snapshot(_)
                | ControllerEvent::HomeSectionsUpdated { .. }
                | ControllerEvent::FavoriteChanged { .. }
                | ControllerEvent::Queue(_)
                | ControllerEvent::Playback(_)
                | ControllerEvent::LoginStatus(_)
                | ControllerEvent::Lyrics(_)
                | ControllerEvent::LyricsSearchResults { .. }
                | ControllerEvent::LyricsSaved { .. }
                | ControllerEvent::FolderLoaded { .. }
                | ControllerEvent::FolderLoadFailed { .. }
                | ControllerEvent::HomeSectionPrefetched { .. }
                | ControllerEvent::ServerDiscovery { .. }
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
                | ControllerEvent::HomeSectionsUpdated { .. }
                | ControllerEvent::FavoriteChanged { .. }
                | ControllerEvent::Queue(_)
                | ControllerEvent::Playback(_)
                | ControllerEvent::LoginStatus(_)
                | ControllerEvent::LyricsSearchResults { .. }
                | ControllerEvent::LyricsSaved { .. }
                | ControllerEvent::FolderLoaded { .. }
                | ControllerEvent::FolderLoadFailed { .. }
                | ControllerEvent::HomeSectionPrefetched { .. }
                | ControllerEvent::ServerDiscovery { .. }
                | ControllerEvent::CoverReady { .. } => {}
                ControllerEvent::Error(error) => panic!("controller error: {error}"),
            }
        }
    }

    fn wait_for_recorded_command(
        commands: &Arc<Mutex<Vec<PlaybackCommand>>>,
        predicate: impl Fn(&PlaybackCommand) -> bool,
    ) -> PlaybackCommand {
        for _ in 0..50 {
            if let Some(command) = commands
                .lock()
                .expect("commands")
                .iter()
                .find(|command| predicate(command))
                .cloned()
            {
                return command;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("timed out waiting for playback command");
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
                    | ControllerEvent::FolderLoaded { .. }
                    | ControllerEvent::FolderLoadFailed { .. }
                    | ControllerEvent::HomeSectionPrefetched { .. }
                    | ControllerEvent::ServerDiscovery { .. }
                    | ControllerEvent::CoverReady { .. } => {}
                    ControllerEvent::Snapshot(_)
                    | ControllerEvent::HomeSectionsUpdated { .. }
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
                | ControllerEvent::FolderLoaded { .. }
                | ControllerEvent::FolderLoadFailed { .. }
                | ControllerEvent::HomeSectionPrefetched { .. }
                | ControllerEvent::ServerDiscovery { .. }
                | ControllerEvent::CoverReady { .. } => {}
                ControllerEvent::Snapshot(_)
                | ControllerEvent::HomeSectionsUpdated { .. }
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
                | ControllerEvent::FolderLoaded { .. }
                | ControllerEvent::FolderLoadFailed { .. }
                | ControllerEvent::HomeSectionPrefetched { .. }
                | ControllerEvent::ServerDiscovery { .. }
                | ControllerEvent::CoverReady { .. } => {}
                ControllerEvent::Snapshot(_)
                | ControllerEvent::HomeSectionsUpdated { .. }
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
                    | ControllerEvent::FolderLoaded { .. }
                    | ControllerEvent::FolderLoadFailed { .. }
                    | ControllerEvent::HomeSectionPrefetched { .. }
                    | ControllerEvent::ServerDiscovery { .. }
                    | ControllerEvent::CoverReady { .. } => {}
                    ControllerEvent::Snapshot(_)
                    | ControllerEvent::HomeSectionsUpdated { .. }
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
