use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use directories::ProjectDirs;
use rufin_core::{Album, Artist, Genre, HomeSection, Playlist, ServerId, ServerIdentity, Track};
use rufin_provider::{
    LoginRequest, MusicProvider, PagedRequest, SavedProviderSession, SearchResults,
};
use rufin_provider_jellyfin::JellyfinProvider;
use rufin_secrets::{MemorySecretStore, SecretServiceStore, SecretStore};
use rufin_store::{SavedServer, Store, StoreError};
use rufin_test_support::{FakeProvider, FakeScale};
use tokio::runtime::Runtime;
use tracing::{info, instrument, warn};

const PAGE_SIZE: usize = 500;

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
    LoginStatus(String),
    Error(String),
}

#[derive(Clone)]
pub struct AppController {
    store: StoreHandle,
    runtime: Arc<Runtime>,
    secrets: Arc<dyn SecretStore>,
    events: Sender<ControllerEvent>,
    sync_in_flight: Arc<Mutex<HashSet<ServerId>>>,
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
    pub fn bootstrap(
        fake_scale: Option<FakeScale>,
    ) -> (Self, Receiver<ControllerEvent>, LibrarySnapshot) {
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
            let controller = Self {
                store,
                runtime,
                secrets: Arc::new(MemorySecretStore::new()),
                events,
                sync_in_flight: Arc::new(Mutex::new(HashSet::new())),
            };
            return (controller, receiver, snapshot);
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
        let controller = Self {
            store,
            runtime,
            secrets: Arc::new(SecretServiceStore::new()),
            events,
            sync_in_flight: Arc::new(Mutex::new(HashSet::new())),
        };
        (controller, receiver, snapshot)
    }

    #[cfg(test)]
    fn bootstrap_memory_for_test() -> (Self, Receiver<ControllerEvent>, LibrarySnapshot) {
        let (events, receiver) = channel();
        let runtime = Runtime::new()
            .map(Arc::new)
            .unwrap_or_else(|error| panic!("failed to create Tokio runtime: {error}"));
        let store = StoreHandle::open_memory()
            .unwrap_or_else(|error| panic!("failed to open memory store: {error}"));
        let snapshot = load_snapshot(&store).unwrap_or_else(|error| {
            panic!("failed to load memory snapshot: {error}");
        });
        let controller = Self {
            store,
            runtime,
            secrets: Arc::new(MemorySecretStore::new()),
            events,
            sync_in_flight: Arc::new(Mutex::new(HashSet::new())),
        };
        (controller, receiver, snapshot)
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
        offset += page.items.len();
        if offset >= page.total || page.items.is_empty() {
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
        offset += page.items.len();
        if offset >= page.total || page.items.is_empty() {
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
        offset += page.items.len();
        if offset >= page.total || page.items.is_empty() {
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
        offset += page.items.len();
        if offset >= page.total || page.items.is_empty() {
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
        offset += page.items.len();
        if offset >= page.total || page.items.is_empty() {
            return Ok(());
        }
    }
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

        store.with_store(|store| {
            store.upsert_albums(&server.id, &albums.items, generation)?;
            store.upsert_tracks(&server.id, &tracks.items, generation)?;
            store.upsert_artists(&server.id, &artists.items, false, generation)?;
            store.upsert_artists(&server.id, &album_artists.items, true, generation)?;
            store.upsert_genres(&server.id, &genres.items, generation)?;
            store.upsert_playlists(&server.id, &playlists.items, generation)?;
            store.complete_sync(&server.id, generation)?;
            Ok(())
        })
    })?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    use super::{AppController, ControllerEvent, LibrarySnapshot};
    use rufin_test_support::FakeScale;

    #[test]
    fn no_server_bootstrap_enters_first_run_state() {
        let (_controller, _events, snapshot) = AppController::bootstrap_memory_for_test();

        assert!(snapshot.first_run);
        assert!(snapshot.server.is_none());
    }

    #[test]
    fn fake_bootstrap_routes_data_through_store_cache() {
        let (_controller, _events, snapshot) = AppController::bootstrap(Some(FakeScale::Small));

        assert!(!snapshot.first_run);
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
    fn large_fake_bootstrap_seeds_visible_cache_window() {
        let (_controller, _events, snapshot) = AppController::bootstrap(Some(FakeScale::Large));

        assert!(!snapshot.first_run);
        assert_eq!(snapshot.albums.len(), 500);
        assert_eq!(snapshot.tracks.len(), 1_000);
    }

    #[test]
    fn clear_cache_emits_empty_active_server_snapshot() {
        let (controller, events, snapshot) = AppController::bootstrap(Some(FakeScale::Small));
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
    fn forget_server_emits_first_run_and_deletes_token() {
        let (controller, events, snapshot) = AppController::bootstrap(Some(FakeScale::Small));
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
        let (controller, events, snapshot) = AppController::bootstrap(Some(FakeScale::Small));
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
    fn controller_events_are_sendable() {
        fn assert_send<T: Send>() {}
        assert_send::<ControllerEvent>();
    }

    fn wait_for_snapshot(events: &Receiver<ControllerEvent>) -> LibrarySnapshot {
        loop {
            match events
                .recv_timeout(Duration::from_secs(5))
                .expect("controller event")
            {
                ControllerEvent::Snapshot(snapshot) => return *snapshot,
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
                ControllerEvent::Snapshot(_) => {}
                ControllerEvent::Error(error) => panic!("controller error: {error}"),
            }
        }
    }
}
