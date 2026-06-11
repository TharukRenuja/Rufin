use super::*;

use super::{
    AppController, ControllerEvent, LOCAL_SOURCE_SERVER_ID, LibrarySnapshot, LibrarySyncStatus,
    LoginActivationContext, LoginActivationRequest, SNAPSHOT_GRID_LIMIT, SNAPSHOT_TRACK_LIMIT,
    StoreHandle, activate_logged_in_server, activate_with_token, home_refresh_completed_event,
    load_runtime_snapshot, load_snapshot, prefetch_home_section, promote_prefetched_home_section,
    refresh_home_section, refresh_home_sections, refresh_home_sections_without_explore,
    refresh_playlist_pages, sync_local_provider_with_events, sync_page_finished, sync_provider,
    sync_provider_outcome, sync_provider_with_events,
};
use rufin_core::{
    Album, AlbumId, AppSettings, ArtistCredit, Genre, GenreId, HomeSection, HomeSectionKind,
    LibrarySourceSelection, LocalLibraryFolder, Playlist, PlaylistId, ServerId, ServerIdentity,
    Track, TrackId,
};
use rufin_playback::{
    PlaybackBackend, PlaybackCommand, PlaybackError, PlaybackEvent, PlaybackState,
};
use rufin_provider::{MusicProvider, PagedRequest, PlaylistEntry, ProviderSession};
use rufin_secrets::{MemorySecretStore, SecretStore};
use rufin_store::SavedServer;
use rufin_test_support::{FakeProvider, FakeScale};
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;
struct SaveFailingSecretStore;
impl SecretStore for SaveFailingSecretStore {
    fn save_secret(
        &self,
        _key: &rufin_secrets::SecretKey,
        _secret: &str,
    ) -> rufin_secrets::SecretResult<()> {
        Err(rufin_secrets::SecretError::Backend(
            "save failed".to_string(),
        ))
    }

    fn load_secret(
        &self,
        _key: &rufin_secrets::SecretKey,
    ) -> rufin_secrets::SecretResult<Option<String>> {
        Ok(None)
    }

    fn delete_secret(&self, _key: &rufin_secrets::SecretKey) -> rufin_secrets::SecretResult<()> {
        Ok(())
    }
}

#[test]
pub(in crate::controller) fn startup_jellyfin_saved() {
    let store = StoreHandle::open_memory().expect("open memory store");

    let first =
        ensure_device_id(&store, || Ok("rufin-install-one".to_string())).expect("first device id");
    let second =
        ensure_device_id(&store, || Ok("rufin-install-two".to_string())).expect("second device id");

    assert_eq!(first, "rufin-install-one");
    assert_eq!(second, first);
    assert_eq!(
        store
            .load_settings()
            .expect("load settings")
            .jellyfin_device_id,
        "rufin-install-one"
    );
}

pub(in crate::controller) struct RecordingPlaybackBackend {
    commands: Arc<Mutex<Vec<PlaybackCommand>>>,
    events: Vec<PlaybackEvent>,
}
impl RecordingPlaybackBackend {
    pub(in crate::controller) fn new(commands: Arc<Mutex<Vec<PlaybackCommand>>>) -> Self {
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
#[test]
pub(in crate::controller) fn startup_server_state() {
    let (_controller, _events, snapshot, queue, player) =
        AppController::bootstrap_memory_for_test();
    assert!(snapshot.first_run);
    assert!(snapshot.server.is_none());
    assert!(queue.is_none());
    assert_eq!(player.state, PlaybackState::Stopped);
}
#[test]
pub(in crate::controller) fn startup_activate_source() {
    let (controller, events, snapshot, _queue, _player) =
        AppController::bootstrap_with_fake(FakeScale::Small);
    let server_id = snapshot.server.as_ref().expect("server").id.clone();
    let first = snapshot.tracks[0].clone();
    let second = snapshot.tracks[1].clone();
    controller.play_tracks_now(vec![first.clone(), second]);
    let queue = wait_for_queue(&events).expect("queue");
    assert_eq!(queue.entries[0].track_id, first.id);
    let _playback = wait_for_playback_state(&controller, &events, PlaybackState::Playing);
    controller.select_source(LibrarySourceSelection::Local);
    let local_queue = wait_for_queue(&events).expect("local queue");
    assert_eq!(local_queue.server_id.as_str(), LOCAL_SOURCE_SERVER_ID);
    assert!(local_queue.entries.is_empty());
    let local_playback = wait_for_playback_state(&controller, &events, PlaybackState::Stopped);
    assert!(local_playback.current.is_none());
    let local_snapshot = wait_for_snapshot(&events);
    assert_eq!(
        local_snapshot.selected_source,
        Some(LibrarySourceSelection::Local)
    );
    assert_eq!(
        controller.load_settings().sources.selected,
        Some(LibrarySourceSelection::Local)
    );
    controller.select_source(LibrarySourceSelection::Server(server_id.clone()));
    let restored_queue = wait_for_queue(&events).expect("restored server queue");
    assert_eq!(restored_queue.server_id, server_id);
    assert_eq!(restored_queue.entries[0].track_id, first.id);
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
pub(in crate::controller) fn startup_init_queue() {
    let (controller, events, _snapshot, initial_queue, _player) =
        AppController::bootstrap_memory_for_test();
    assert!(initial_queue.is_none());
    let root = unique_test_dir("first-run-local-queue");
    fs::create_dir_all(&root).expect("create root");
    controller.add_local_server(root.clone());
    let queue = wait_for_queue(&events).expect("local queue");
    assert_eq!(queue.server_id.as_str(), LOCAL_SOURCE_SERVER_ID);
    let snapshot = wait_for_snapshot(&events);
    assert_eq!(
        snapshot.selected_source,
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
            .server_id
            .as_str(),
        LOCAL_SOURCE_SERVER_ID
    );
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn startup_accept_folders() {
    let (controller, events, _snapshot, initial_queue, _player) =
        AppController::bootstrap_memory_for_test();
    assert!(initial_queue.is_none());
    let first = unique_test_dir("first-run-local-folder-one");
    let second = unique_test_dir("first-run-local-folder-two");
    fs::create_dir_all(&first).expect("create first root");
    fs::create_dir_all(&second).expect("create second root");
    controller.add_local_server_folders(vec![first.clone(), second.clone()]);
    let queue = wait_for_queue(&events).expect("local queue");
    assert_eq!(queue.server_id.as_str(), LOCAL_SOURCE_SERVER_ID);
    let snapshot = wait_for_snapshot(&events);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Local)
    );
    assert_eq!(
        snapshot.local_folders,
        vec![
            LocalLibraryFolder {
                path: first.to_string_lossy().into_owned()
            },
            LocalLibraryFolder {
                path: second.to_string_lossy().into_owned()
            }
        ]
    );
    let _cleanup_first = fs::remove_dir_all(first);
    let _cleanup_second = fs::remove_dir_all(second);
}
#[test]
pub(in crate::controller) fn startup_activate_token() {
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let server_id = ServerId::new("jellyfin:server:new");
    let session = ProviderSession {
        server: ServerIdentity {
            id: server_id.clone(),
            provider: "jellyfin".to_string(),
            name: "New Server".to_string(),
            base_url: "https://library.example.test".to_string(),
        },
        user_id: "user-id".to_string(),
        username: "listener".to_string(),
        access_token: "token".to_string(),
        device_id: Some("rufin-install-one".to_string()),
    };
    activate_logged_in_server(
        &LoginActivationContext {
            store: &controller.store,
            queue: &controller.queue,
            playback_request_generation: &controller.playback_request_generation,
            playback: &controller.playback,
            playback_snapshot: &controller.playback_snapshot,
            auto_dj_enabled: &controller.auto_dj_enabled,
            events: &controller.events,
        },
        LoginActivationRequest {
            session: &session,
            trust_invalid_cert: false,
            local_access_root: None,
            path_replace_from: None,
        },
    )
    .expect("activate logged-in server");
    let queue = wait_for_queue(&events).expect("server queue");
    assert_eq!(queue.server_id, server_id);
    let snapshot = wait_for_snapshot(&events);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Server(server_id.clone()))
    );
    assert_eq!(
        snapshot.server.as_ref().map(|server| server.id.clone()),
        Some(server_id.clone())
    );
    assert_eq!(
        controller
            .secrets
            .load_token(&server_id)
            .expect("load token"),
        None
    );
}
#[test]
pub(in crate::controller) fn startup_persist_server() {
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let secrets: Arc<dyn SecretStore> = Arc::new(SaveFailingSecretStore);
    let server_id = ServerId::new("jellyfin:server:new");
    let session = ProviderSession {
        server: ServerIdentity {
            id: server_id,
            provider: "jellyfin".to_string(),
            name: "New Server".to_string(),
            base_url: "https://library.example.test".to_string(),
        },
        user_id: "user-id".to_string(),
        username: "listener".to_string(),
        access_token: "token".to_string(),
        device_id: Some("rufin-install-one".to_string()),
    };
    let error = activate_with_token(
        &LoginActivationContext {
            store: &controller.store,
            queue: &controller.queue,
            playback_request_generation: &controller.playback_request_generation,
            playback: &controller.playback,
            playback_snapshot: &controller.playback_snapshot,
            auto_dj_enabled: &controller.auto_dj_enabled,
            events: &controller.events,
        },
        &secrets,
        LoginActivationRequest {
            session: &session,
            trust_invalid_cert: false,
            local_access_root: None,
            path_replace_from: None,
        },
    )
    .expect_err("token save should fail");

    assert!(error.contains("save failed"));
    assert_eq!(
        controller
            .store
            .with_store(|store| store.active_server())
            .expect("active server"),
        None
    );
    assert!(
        controller
            .store
            .with_store(|store| store.list_servers())
            .expect("servers")
            .is_empty()
    );
    assert!(events.try_recv().is_err());
}

#[test]
pub(in crate::controller) fn startup_persist_server_token_in_foreground_store() {
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let server_id = ServerId::new("jellyfin:server:foreground");
    let session = ProviderSession {
        server: ServerIdentity {
            id: server_id.clone(),
            provider: "jellyfin".to_string(),
            name: "Foreground Server".to_string(),
            base_url: "https://library.example.test".to_string(),
        },
        user_id: "user-id".to_string(),
        username: "listener".to_string(),
        access_token: "token".to_string(),
        device_id: Some("rufin-install-one".to_string()),
    };

    activate_with_token(
        &LoginActivationContext {
            store: &controller.store,
            queue: &controller.queue,
            playback_request_generation: &controller.playback_request_generation,
            playback: &controller.playback,
            playback_snapshot: &controller.playback_snapshot,
            auto_dj_enabled: &controller.auto_dj_enabled,
            events: &controller.events,
        },
        &controller.secrets,
        LoginActivationRequest {
            session: &session,
            trust_invalid_cert: false,
            local_access_root: None,
            path_replace_from: None,
        },
    )
    .expect("activate with token");

    assert_eq!(
        controller
            .secrets
            .load_token(&server_id)
            .expect("load token"),
        Some("token".to_string())
    );
    let queue = wait_for_queue(&events).expect("server queue");
    assert_eq!(queue.server_id, server_id);
}
#[test]
pub(in crate::controller) fn startup_load_folders() {
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
    let active = store
        .with_store(|store| store.active_server())
        .expect("active server")
        .expect("active server");
    assert_eq!(active.server.id.as_str(), LOCAL_SOURCE_SERVER_ID);
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn startup_load_source() {
    let store = StoreHandle::open_memory().expect("memory store");
    let active_saved = saved_server();
    let mut selected_saved = saved_server();
    selected_saved.server.id = ServerId::new("jellyfin:server:selected");
    selected_saved.server.name = "Selected Server".to_string();
    selected_saved.server.base_url = "https://selected.example.test".to_string();
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Server(
        selected_saved.server.id.clone(),
    ));
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_server(&active_saved)?;
            store.save_server(&selected_saved)?;
            store.set_active_server(&active_saved.server.id)
        })
        .expect("save servers");

    let snapshot = load_snapshot(&store).expect("load snapshot");

    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Server(
            selected_saved.server.id.clone()
        ))
    );
    assert_eq!(
        snapshot.server.as_ref().map(|server| server.id.clone()),
        Some(selected_saved.server.id.clone())
    );
    let active_after = store
        .with_store(|store| store.active_server())
        .expect("active server")
        .expect("active server");
    assert_eq!(active_after.server.id, selected_saved.server.id);
}

#[test]
pub(in crate::controller) fn startup_missing_token_reconnects_saved_remote() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = saved_server();
    store
        .with_store(|store| {
            store.save_server(&saved)?;
            store.set_active_server(&saved.server.id)
        })
        .expect("save server");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());

    let snapshot = load_runtime_snapshot(&store, &secrets).expect("load runtime snapshot");

    assert!(snapshot.first_run);
    assert_eq!(
        snapshot.server.as_ref().map(|server| server.id.clone()),
        Some(saved.server.id.clone())
    );
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Server(saved.server.id.clone()))
    );
    assert_eq!(snapshot.username.as_deref(), Some(saved.username.as_str()));
    assert_eq!(
        snapshot.sync_status,
        "Connect once more to continue using this server."
    );
    assert!(snapshot.last_error.is_none());
}

#[test]
pub(in crate::controller) fn startup_config_token_keeps_saved_remote_active() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = saved_server();
    store
        .with_store(|store| {
            store.save_server(&saved)?;
            store.set_active_server(&saved.server.id)
        })
        .expect("save server");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
    secrets
        .save_token(&saved.server.id, "cached-session-token")
        .expect("save token");

    let snapshot = load_runtime_snapshot(&store, &secrets).expect("load runtime snapshot");

    assert!(!snapshot.first_run);
    assert_eq!(
        snapshot.server.as_ref().map(|server| server.id.clone()),
        Some(saved.server.id.clone())
    );
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Server(saved.server.id))
    );
}

#[test]
pub(in crate::controller) fn startup_local_source_does_not_require_secret() {
    let store = StoreHandle::open_memory().expect("memory store");
    let root = unique_test_dir("local-source-runtime-snapshot");
    fs::create_dir_all(&root).expect("create root");
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    settings.sources.local_folders = vec![LocalLibraryFolder {
        path: root.to_string_lossy().into_owned(),
    }];
    store.save_settings(&settings).expect("save settings");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());

    let snapshot = load_runtime_snapshot(&store, &secrets).expect("load runtime snapshot");

    assert!(!snapshot.first_run);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Local)
    );
    assert_eq!(
        snapshot.server.expect("server").id.as_str(),
        LOCAL_SOURCE_SERVER_ID
    );
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn startup_add_syncs() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = saved_server();
    let root = unique_test_dir("add-local-folder-select-source");
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
    let queue = wait_for_queue(&events).expect("local queue");
    assert_eq!(queue.server_id.as_str(), LOCAL_SOURCE_SERVER_ID);
    let snapshot = wait_for_snapshot(&events);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Local)
    );
    assert_eq!(snapshot.local_folders.len(), 1);
    let active = controller
        .store
        .with_store(|store| store.active_server())
        .expect("active server")
        .expect("active server");
    assert_eq!(active.server.id.as_str(), LOCAL_SOURCE_SERVER_ID);
    assert_eq!(wait_for_status(&events), "Syncing Local library…");
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn startup_start_refresh() {
    let store = StoreHandle::open_memory().expect("memory store");
    let remote = saved_server();
    let local = local_source_saved();
    let root = unique_test_dir("fresh-local-source-selection");
    fs::create_dir_all(&root).expect("create root");
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Server(remote.server.id.clone()));
    settings.sources.local_folders = vec![LocalLibraryFolder {
        path: root.to_string_lossy().into_owned(),
    }];
    store.save_settings(&settings).expect("save settings");
    let generation = store
        .with_store(|store| {
            store.save_server(&remote)?;
            store.save_server(&local)?;
            store.set_active_server(&remote.server.id)?;
            let generation = store.begin_sync(&local.server.id)?;
            store.complete_sync(&local.server.id, generation)?;
            Ok(generation)
        })
        .expect("seed fresh local sync");
    let (controller, events) = controller_from_store_for_test(store);

    controller.select_source(LibrarySourceSelection::Local);

    let queue = wait_for_queue(&events).expect("local queue");
    assert_eq!(queue.server_id.as_str(), LOCAL_SOURCE_SERVER_ID);
    let snapshot = wait_for_snapshot(&events);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Local)
    );
    assert_eq!(snapshot.sync_status, "Syncing library…");
    assert_eq!(wait_for_status(&events), "Syncing Local library…");
    let completed_snapshot = wait_for_snapshot(&events);
    assert_eq!(
        completed_snapshot.selected_source,
        Some(LibrarySourceSelection::Local)
    );
    assert_eq!(completed_snapshot.sync_status, "Cached library ready");
    let state = controller
        .store
        .with_store(|store| store.sync_state(&local.server.id))
        .expect("sync state");
    assert_eq!(state.status, "idle");
    assert_eq!(state.generation, generation);
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn startup_reuse_cache() {
    let store = StoreHandle::open_memory().expect("memory store");
    let remote = saved_server();
    let local = local_source_saved();
    let root = unique_test_dir("stale-local-source-selection");
    fs::create_dir_all(&root).expect("create root");
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Server(remote.server.id.clone()));
    settings.sources.local_folders = vec![LocalLibraryFolder {
        path: root.to_string_lossy().into_owned(),
    }];
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_server(&remote)?;
            store.save_server(&local)?;
            store.set_active_server(&remote.server.id)?;
            let generation = store.begin_sync(&local.server.id)?;
            store.upsert_albums(
                &local.server.id,
                &[local_album_with_image_ref(ImageRef::new(
                    "local:cover:file%3A%2F%2Fmissing-cover",
                    None,
                ))],
                generation,
            )?;
            store.complete_sync(&local.server.id, generation)
        })
        .expect("seed local cache");
    let (controller, events) = controller_from_store_for_test(store);

    controller.select_source(LibrarySourceSelection::Local);

    let queue = wait_for_queue(&events).expect("local queue");
    assert_eq!(queue.server_id.as_str(), LOCAL_SOURCE_SERVER_ID);
    let snapshot = wait_for_snapshot(&events);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Local)
    );
    assert_eq!(snapshot.sync_status, "Syncing library…");
    assert_eq!(snapshot.cached_album_count, 1);
    assert_eq!(wait_for_status(&events), "Syncing Local library…");
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn startup_login_sync_emits_snapshot() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let root = unique_test_dir("login-sync-snapshot");
    fs::create_dir_all(&root).expect("create root");
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    settings.sources.local_folders = vec![LocalLibraryFolder {
        path: root.to_string_lossy().into_owned(),
    }];
    store.save_settings(&settings).expect("save settings");
    seed_cached_library(
        &store,
        &local,
        &[local_album_with_image_ref(ImageRef::new(
            "local:cover:file%3A%2F%2Flogin-sync-cover",
            None,
        ))],
        &[],
        &[],
    );
    let (controller, events) = controller_from_store_for_test(store);

    start_login_sync_thread(controller.sync_context(), local);

    let snapshot = wait_for_snapshot(&events);
    assert!(!snapshot.first_run);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Local)
    );
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn startup_source_sync_running_emits_snapshot() {
    let store = StoreHandle::open_memory().expect("memory store");
    let remote = saved_server();
    let local = local_source_saved();
    let root = unique_test_dir("running-local-source-selection");
    fs::create_dir_all(&root).expect("create root");
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Server(remote.server.id.clone()));
    settings.sources.local_folders = vec![LocalLibraryFolder {
        path: root.to_string_lossy().into_owned(),
    }];
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_server(&remote)?;
            store.save_server(&local)?;
            store.set_active_server(&remote.server.id)?;
            let generation = store.begin_sync(&local.server.id)?;
            store.upsert_albums(
                &local.server.id,
                &[local_album_with_image_ref(ImageRef::new(
                    "local:cover:file%3A%2F%2Fmissing-cover",
                    None,
                ))],
                generation,
            )?;
            store.complete_sync(&local.server.id, generation)
        })
        .expect("seed local cache");
    let (controller, events) = controller_from_store_for_test(store);
    let _permit = controller
        .sync_in_flight
        .acquire(local.server.id.clone())
        .expect("sync guard")
        .expect("sync permit");

    controller.select_source(LibrarySourceSelection::Local);

    let snapshot = wait_for_snapshot(&events);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Local)
    );
    assert_eq!(snapshot.cached_album_count, 1);
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn startup_track_deleted() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let root = unique_test_dir("local-delta-prune");
    fs::create_dir_all(&root).expect("create root");
    let kept = root.join("Album/Kept.mp3");
    let removed = root.join("Album/Removed.mp3");
    fs::create_dir_all(kept.parent().expect("parent")).expect("create album dir");
    fs::write(&kept, []).expect("kept audio");
    fs::write(&removed, []).expect("removed audio");
    store
        .with_store(|store| {
            store.save_server(&local)?;
            store.set_active_server(&local.server.id)
        })
        .expect("save local server");
    let runtime = Runtime::new().expect("runtime");
    let (events, _receiver) = channel();
    let cold = LocalProvider::from_roots_with_identity(vec![root.clone()], local.server.clone())
        .expect("cold local provider");
    runtime
        .block_on(sync_local_provider_with_events(
            &store,
            &local.server.id,
            &cold,
            events.clone(),
        ))
        .expect("cold local sync");
    assert_eq!(
        store
            .with_store(|store| store.load_tracks(&local.server.id, 0, 10))
            .expect("cold tracks")
            .total,
        2
    );
    let committed_generation = store
        .with_store(|store| {
            store
                .sync_state(&local.server.id)
                .map(|state| state.generation)
        })
        .expect("committed generation");
    let mut committed_manifest = store
        .with_store(|store| store.load_local_manifest(&local.server.id))
        .expect("committed manifest");
    for entry in &mut committed_manifest {
        entry.track.genres = vec!["Example".to_string()];
        entry.metadata_hash = format!("metadata:{}", entry.track.id.as_str());
        entry.search_hash = format!("search:{}", entry.track.id.as_str());
    }
    let committed_tracks = committed_manifest
        .iter()
        .map(|entry| entry.track.clone())
        .collect::<Vec<_>>();
    store
        .with_store(|store| {
            store.upsert_tracks(&local.server.id, &committed_tracks, committed_generation)?;
            store.replace_local_manifest(
                &local.server.id,
                committed_generation,
                &committed_manifest,
            )
        })
        .expect("seed committed genres");
    fs::remove_file(&removed).expect("remove audio");
    let manifest = store
        .with_store(|store| store.load_local_manifest(&local.server.id))
        .expect("manifest");
    let warm = LocalProvider::from_roots_with_manifest_cache(
        vec![root.clone()],
        local.server.clone(),
        manifest,
    )
    .expect("warm local provider");
    assert_eq!(warm.manifest_scan().retained_track_ids.len(), 1);
    assert_eq!(warm.manifest_scan().deleted_track_ids.len(), 1);

    runtime
        .block_on(sync_local_provider_with_events(
            &store,
            &local.server.id,
            &warm,
            events,
        ))
        .expect("warm local sync");

    let tracks = store
        .with_store(|store| store.load_tracks(&local.server.id, 0, 10))
        .expect("warm tracks");
    assert_eq!(tracks.total, 1);
    assert_eq!(tracks.items[0].title, "Kept");
    let retained_path = store
        .with_store(|store| store.track_local_path(&local.server.id, &tracks.items[0].id))
        .expect("retained path");
    assert_eq!(
        retained_path.as_deref(),
        Some(kept.to_string_lossy().as_ref())
    );
    let genres = store
        .with_store(|store| store.load_genres(&local.server.id, 0, 10))
        .expect("genres");
    let genre = genres
        .items
        .iter()
        .find(|genre| genre.name == "Example")
        .expect("retained genre");
    assert_eq!(genre.track_count, 1);
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn startup_advance_generation() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let root = unique_test_dir("local-no-change-generation");
    let album_dir = root.join("Artist").join("Album");
    fs::create_dir_all(&album_dir).expect("create album dir");
    fs::write(album_dir.join("cover.jpg"), [1_u8]).expect("cover");
    fs::write(album_dir.join("Track.mp3"), []).expect("audio");
    store
        .with_store(|store| {
            store.save_server(&local)?;
            store.set_active_server(&local.server.id)
        })
        .expect("save local server");
    let runtime = Runtime::new().expect("runtime");
    let (events, _receiver) = channel();
    let cold = LocalProvider::from_roots_with_identity(vec![root.clone()], local.server.clone())
        .expect("cold local provider");
    runtime
        .block_on(sync_local_provider_with_events(
            &store,
            &local.server.id,
            &cold,
            events.clone(),
        ))
        .expect("cold local sync");
    let committed_generation = store
        .with_store(|store| {
            store
                .sync_state(&local.server.id)
                .map(|state| state.generation)
        })
        .expect("committed generation");
    let mut manifest = store
        .with_store(|store| store.load_local_manifest(&local.server.id))
        .expect("manifest");
    for entry in &mut manifest {
        entry.track.genres = vec!["Example".to_string()];
        entry.metadata_hash = format!("metadata:{}", entry.track.id.as_str());
        entry.search_hash = format!("search:{}", entry.track.id.as_str());
    }
    let committed_tracks = manifest
        .iter()
        .map(|entry| entry.track.clone())
        .collect::<Vec<_>>();
    let committed_genres = [Genre {
        id: GenreId::new("local:genre:example"),
        name: "Example".to_string(),
        album_count: 1,
        track_count: 1,
        image_refs: Vec::new(),
        image_ref: None,
    }];
    store
        .with_store(|store| {
            store.upsert_tracks(&local.server.id, &committed_tracks, committed_generation)?;
            store.upsert_genres(&local.server.id, &committed_genres, committed_generation)?;
            store.replace_local_manifest(&local.server.id, committed_generation, &manifest)?;
            store.complete_sync(&local.server.id, committed_generation)?;
            Ok(())
        })
        .expect("seed committed genre");
    let cached_genres = store
        .with_store(|store| store.load_genres(&local.server.id, 0, 10))
        .expect("cached genres");
    assert_eq!(cached_genres.total, 1);
    assert!(
        cached_genres.items[0].image_ref.is_some(),
        "genre should have a derived collection cover ref"
    );
    let warm = LocalProvider::from_roots_with_manifest_cache(
        vec![root.clone()],
        local.server.clone(),
        manifest,
    )
    .expect("warm local provider");
    assert!(!warm.manifest_scan().library_changed);

    runtime
        .block_on(sync_local_provider_with_events(
            &store,
            &local.server.id,
            &warm,
            events,
        ))
        .expect("warm local sync");

    let state = store
        .with_store(|store| store.sync_state(&local.server.id))
        .expect("sync state");
    assert_eq!(state.status, "idle");
    assert_eq!(state.generation, committed_generation);
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn startup_cached_local_noop_uses_status() {
    let (store, local, root, generation) = seed_cached_local_source("local-noop-status");
    let (controller, events) = controller_from_store_for_test(store);

    controller.start_sync(local);

    let status = wait_for_sync_status_without_snapshot(&events, "Cached library ready");
    assert_eq!(status.last_error, None);
    assert!(status.delta.is_empty());
    let state = controller
        .store
        .with_store(|store| store.sync_state(&status.server_id))
        .expect("final sync state");
    assert_eq!(state.generation, generation);
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn startup_cached_local_delta_uses_status_delta() {
    let (store, local, root, _generation) = seed_cached_local_source("local-delta-snapshot");
    let album_dir = root.join("Artist").join("Album");
    fs::write(album_dir.join("Second.mp3"), []).expect("audio");
    let (controller, events) = controller_from_store_for_test(store);

    controller.start_sync(local);

    let status = wait_for_sync_status_without_snapshot(&events, "Cached library ready");
    assert!(!status.delta.tracks.added.is_empty());
    assert_eq!(status.counts.tracks, 2);
    let _cleanup = fs::remove_dir_all(root);
}

fn seed_cached_local_source(label: &str) -> (StoreHandle, SavedServer, PathBuf, i64) {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let root = unique_test_dir(label);
    let album_dir = root.join("Artist").join("Album");
    fs::create_dir_all(&album_dir).expect("create album dir");
    fs::write(album_dir.join("Track.mp3"), []).expect("audio");
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    settings.sources.local_folders = vec![LocalLibraryFolder {
        path: root.to_string_lossy().into_owned(),
    }];
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_server(&local)?;
            store.set_active_server(&local.server.id)
        })
        .expect("save local server");
    let runtime = Runtime::new().expect("runtime");
    let (seed_events, _seed_receiver) = channel();
    let cold = LocalProvider::from_roots_with_identity(vec![root.clone()], local.server.clone())
        .expect("cold local provider");
    runtime
        .block_on(sync_local_provider_with_events(
            &store,
            &local.server.id,
            &cold,
            seed_events,
        ))
        .expect("cold local sync");
    let generation = store
        .with_store(|store| {
            store
                .sync_state(&local.server.id)
                .map(|state| state.generation)
        })
        .expect("sync state");
    (store, local, root, generation)
}
#[test]
pub(in crate::controller) fn startup_change_audio() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let root = unique_test_dir("local-artist-artwork-delta");
    let artist_dir = root.join("Unknown Artist");
    let album_dir = artist_dir.join("Album");
    fs::create_dir_all(&album_dir).expect("create album dir");
    let artist_image = artist_dir.join("artist.jpg");
    fs::write(&artist_image, [1_u8]).expect("artist image");
    fs::write(album_dir.join("Track.mp3"), []).expect("audio");
    store
        .with_store(|store| {
            store.save_server(&local)?;
            store.set_active_server(&local.server.id)
        })
        .expect("save local server");
    let runtime = Runtime::new().expect("runtime");
    let (events, _receiver) = channel();
    let cold = LocalProvider::from_roots_with_identity(vec![root.clone()], local.server.clone())
        .expect("cold local provider");
    runtime
        .block_on(sync_local_provider_with_events(
            &store,
            &local.server.id,
            &cold,
            events.clone(),
        ))
        .expect("cold local sync");
    let cold_tag = store
        .with_store(|store| store.load_artists(&local.server.id, false, 0, 10))
        .expect("cold artists")
        .items
        .into_iter()
        .next()
        .and_then(|artist| artist.image_ref.and_then(|image_ref| image_ref.tag))
        .expect("cold artist tag");
    fs::write(&artist_image, [1_u8, 2_u8]).expect("replace artist image");
    let manifest = store
        .with_store(|store| store.load_local_manifest(&local.server.id))
        .expect("manifest");
    let warm = LocalProvider::from_roots_with_manifest_cache(
        vec![root.clone()],
        local.server.clone(),
        manifest,
    )
    .expect("warm local provider");
    assert!(!warm.manifest_scan().library_changed);

    runtime
        .block_on(sync_local_provider_with_events(
            &store,
            &local.server.id,
            &warm,
            events,
        ))
        .expect("warm local sync");

    let warm_tag = store
        .with_store(|store| store.load_artists(&local.server.id, false, 0, 10))
        .expect("warm artists")
        .items
        .into_iter()
        .next()
        .and_then(|artist| artist.image_ref.and_then(|image_ref| image_ref.tag))
        .expect("warm artist tag");
    assert_ne!(cold_tag, warm_tag);
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn startup_preserve_selection() {
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
pub(in crate::controller) fn startup_removing_cache() {
    let store = StoreHandle::open_memory().expect("memory store");
    let remote = saved_server();
    let local = local_source_saved();
    let first = unique_test_dir("remove-inactive-local-first");
    let second = unique_test_dir("remove-inactive-local-second");
    fs::create_dir_all(&first).expect("create first root");
    fs::create_dir_all(&second).expect("create second root");
    fs::write(first.join("Removed.mp3"), []).expect("first audio");
    fs::write(second.join("Remaining.mp3"), []).expect("second audio");
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Server(remote.server.id.clone()));
    settings.sources.local_folders = vec![
        LocalLibraryFolder {
            path: first.to_string_lossy().into_owned(),
        },
        LocalLibraryFolder {
            path: second.to_string_lossy().into_owned(),
        },
    ];
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_server(&remote)?;
            store.save_server(&local)?;
            store.set_active_server(&remote.server.id)
        })
        .expect("save servers");
    let runtime = Runtime::new().expect("runtime");
    let (seed_events, _seed_receiver) = channel();
    let cold = LocalProvider::from_roots_with_identity(
        vec![first.clone(), second.clone()],
        local.server.clone(),
    )
    .expect("cold local provider");
    runtime
        .block_on(sync_local_provider_with_events(
            &store,
            &local.server.id,
            &cold,
            seed_events,
        ))
        .expect("seed local sync");
    assert_eq!(
        store
            .with_store(|store| store.load_tracks(&local.server.id, 0, 10))
            .expect("seed tracks")
            .total,
        2
    );
    let (controller, events) = controller_from_store_for_test(store);

    controller.remove_local_library_folder(first.to_string_lossy().into_owned());

    let snapshot = wait_for_snapshot(&events);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Server(remote.server.id.clone()))
    );
    assert_eq!(wait_for_status(&events), "Syncing Local library…");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let total = controller
            .store
            .with_store(|store| store.load_tracks(&local.server.id, 0, 10))
            .expect("poll tracks")
            .total;
        if total == 1 {
            break;
        }
        assert!(Instant::now() < deadline, "local sync did not prune root");
        thread::sleep(Duration::from_millis(25));
    }
    let tracks = controller
        .store
        .with_store(|store| store.load_tracks(&local.server.id, 0, 10))
        .expect("remaining tracks");
    assert_eq!(tracks.total, 1);
    assert_eq!(tracks.items[0].title, "Remaining");
    let active = controller
        .store
        .with_store(|store| store.active_server())
        .expect("active server")
        .expect("active server");
    assert_eq!(active.server.id, remote.server.id);
    let _cleanup_first = fs::remove_dir_all(first);
    let _cleanup_second = fs::remove_dir_all(second);
}

#[test]
pub(in crate::controller) fn startup_record_state() {
    let store = StoreHandle::open_memory().expect("memory store");
    let remote = saved_server();
    let failing = SavedServer {
        server: ServerIdentity {
            id: ServerId::new("unsupported:inactive-sync"),
            provider: "unsupported".to_string(),
            name: "Inactive Sync".to_string(),
            base_url: "https://music.example".to_string(),
        },
        user_id: "user".to_string(),
        username: "demo".to_string(),
        trust_invalid_cert: false,
    };
    store
        .with_store(|store| {
            store.save_server(&remote)?;
            store.save_server(&failing)?;
            store.set_active_server(&remote.server.id)
        })
        .expect("seed servers");
    let (controller, _events) = controller_from_store_for_test(store);

    controller.start_sync(failing.clone());

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = controller
            .store
            .with_store(|store| store.sync_state(&failing.server.id))
            .expect("sync state");
        if state.status == "error" {
            assert_eq!(
                state.last_error.as_deref(),
                Some("No saved token found for the active server.")
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "inactive failed sync stayed {}",
            state.status
        );
        thread::sleep(Duration::from_millis(25));
    }
    let active = controller
        .store
        .with_store(|store| store.active_server())
        .expect("active server")
        .expect("active server");
    assert_eq!(active.server.id, remote.server.id);
}
#[test]
pub(in crate::controller) fn startup_cached_sync_error_uses_status() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = saved_server();
    let album = remote_album_with_image_ref(ImageRef::new("album:one", Some("tag".to_string())));
    let track = local_track_with_image_ref(
        1,
        &album,
        ImageRef::new("track:one", Some("tag".to_string())),
    );
    seed_cached_library(&store, &saved, &[album], &[track], &[]);
    let (controller, events) = controller_from_store_for_test(store);

    controller.start_sync(saved);

    let status = wait_for_sync_status_without_snapshot(&events, "Action failed");
    assert_eq!(status.sync_status, "Action failed");
    assert_eq!(
        status.last_error.as_deref(),
        Some("No saved token found for the active server.")
    );
}

fn wait_for_sync_status_without_snapshot(
    events: &std::sync::mpsc::Receiver<ControllerEvent>,
    expected_status: &str,
) -> LibrarySyncStatus {
    loop {
        match events
            .recv_timeout(Duration::from_secs(5))
            .expect("controller event")
        {
            ControllerEvent::LibrarySyncStatus(status) if status.sync_status == expected_status => {
                return *status;
            }
            ControllerEvent::Snapshot(_) => panic!("cached same-source sync emitted snapshot"),
            ControllerEvent::LibrarySyncStatus(_)
            | ControllerEvent::LibraryDelta(_)
            | ControllerEvent::HomeSectionsUpdated { .. }
            | ControllerEvent::PlaylistChanged { .. }
            | ControllerEvent::SmartPlaylistChanged { .. }
            | ControllerEvent::FavoriteChanged { .. }
            | ControllerEvent::Queue(_)
            | ControllerEvent::Playback(_)
            | ControllerEvent::Visualizer(_)
            | ControllerEvent::Lyrics(_)
            | ControllerEvent::LyricsSearchResults { .. }
            | ControllerEvent::LyricsSearchFailed { .. }
            | ControllerEvent::LyricsSaved { .. }
            | ControllerEvent::FolderLoaded { .. }
            | ControllerEvent::FolderLoadFailed { .. }
            | ControllerEvent::HomeSectionPrefetched { .. }
            | ControllerEvent::ServerDiscovery { .. }
            | ControllerEvent::CoverReady { .. }
            | ControllerEvent::CoverUnavailable { .. }
            | ControllerEvent::LoginStatus(_) => {}
            ControllerEvent::Error(error) => panic!("controller error: {error}"),
        }
    }
}

#[test]
pub(in crate::controller) fn startup_persist_field() {
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
        "http://old.example.test".to_string(),
        "listener".to_string(),
        String::new(),
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
    assert_eq!(edited.base_url, "http://old.example.test");
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
pub(in crate::controller) fn startup_emit_status() {
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let server_id = ServerId::new("server:unchanged");
    controller
        .store
        .with_store(|store| {
            store.save_server(&SavedServer {
                server: ServerIdentity {
                    id: server_id.clone(),
                    provider: "jellyfin".to_string(),
                    name: "Saved server".to_string(),
                    base_url: "http://server.example.test".to_string(),
                },
                user_id: "user-id".to_string(),
                username: "listener".to_string(),
                trust_invalid_cert: false,
            })?;
            store.set_active_server(&server_id)
        })
        .expect("save server");

    controller.update_server_settings(
        server_id,
        "Saved server".to_string(),
        "http://server.example.test".to_string(),
        "listener".to_string(),
        String::new(),
        false,
    );

    assert_eq!(wait_for_status(&events), "No changes to save.");
}
#[test]
pub(in crate::controller) fn startup_store_cache() {
    let (_controller, _events, snapshot, queue, player) =
        AppController::bootstrap_with_fake(FakeScale::Small);
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
pub(in crate::controller) fn startup_sync_total() {
    assert!(!sync_page_finished(500, 0, 500));
    assert!(sync_page_finished(120, 0, 620));
    assert!(!sync_page_finished(120, 1_000, 620));
    assert!(sync_page_finished(500, 1_000, 1_000));
}
#[test]
pub(in crate::controller) fn startup_large_window() {
    let (_controller, _events, snapshot, _queue, _player) =
        AppController::bootstrap_with_fake(FakeScale::Large);
    assert!(!snapshot.first_run);
    assert_eq!(snapshot.albums.len(), SNAPSHOT_GRID_LIMIT);
    assert_eq!(snapshot.tracks.len(), 2_000);
    assert_eq!(snapshot.cached_album_count, 1_000);
    assert_eq!(snapshot.cached_track_count, 2_000);
}
#[test]
pub(in crate::controller) fn startup_track_page() {
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
        .with_store(|store| store.load_tracks(&server_id, FakeScale::Small.track_count() - 1, 10))
        .expect("load final track page");
    assert_eq!(first_page.total, FakeScale::Small.track_count());
    assert_eq!(final_page.total, FakeScale::Small.track_count());
    assert_eq!(final_page.items.len(), 1);
}
#[test]
pub(in crate::controller) fn startup_emit_timing() {
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
    let (events, receiver) = channel();

    runtime
        .block_on(sync_provider_with_events(
            &store, &server_id, &provider, events,
        ))
        .expect("sync provider");

    let statuses = receiver
        .try_iter()
        .filter_map(|event| match event {
            ControllerEvent::LoginStatus(status) => Some(status),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(statuses.iter().any(|status| status.contains(
        "Caching library… This may take some time. Cached tracks page 5/5 for Fake Library (Music Server), 2,400/2,400 fetched, 2,400 cached"
    )));
    assert!(
        statuses
            .iter()
            .any(|status| status.contains("elapsed") && status.contains("Finalizing cache"))
    );
}
#[test]
pub(in crate::controller) fn startup_cache_total() {
    let (events, receiver) = channel();
    let mut progress =
        SyncProgressReporter::new(Some(events), "Local Music".to_string(), "Local".to_string());

    progress.page_written(SyncPageProgress {
        collection: SyncCollection::Tracks,
        page_number: 2,
        fetched: 620,
        written: 620,
        total: None,
        finished: true,
        fetch_elapsed: Duration::from_millis(25),
        write_elapsed: Duration::from_millis(10),
    });

    let status = wait_for_status(&receiver);
    assert!(status.contains(
        "Caching library… This may take some time. Cached tracks page 2 for Local Music (Local)"
    ));
    assert!(status.contains("620 fetched, 620 cached"));
    assert!(!status.contains("620/"));
}
#[test]
pub(in crate::controller) fn startup_local_cache() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let image_ref = ImageRef::new("local:cover:file%3A%2F%2Fexample-cover", None);
    store
        .with_store(|store| {
            store.save_server(&local)?;
            let generation = store.begin_sync(&local.server.id)?;
            store.upsert_albums(
                &local.server.id,
                &[local_album_with_image_ref(image_ref.clone())],
                generation,
            )?;
            store.complete_sync(&local.server.id, generation)
        })
        .expect("seed local cache");

    assert!(!initial_cover_cache_required(&store, &local.server.id));
    assert!(!active_server_needs_sync(&store, &local.server.id));
    let readiness = active_source_readiness(&store, &local.server.id).expect("readiness");
    assert!(!readiness.artwork_fresh);
    assert_eq!(
        readiness.prefetch_required_reason,
        Some(SyncRequiredReason::LocalArtworkMissing)
    );
}
#[test]
pub(in crate::controller) fn startup_readiness_cache() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let image_ref = ImageRef::new("local:cover:file%3A%2F%2Fmissing-startup-cover", None);
    store
        .with_store(|store| {
            store.save_server(&local)?;
            let generation = store.begin_sync(&local.server.id)?;
            store.upsert_albums(
                &local.server.id,
                &[local_album_with_image_ref(image_ref)],
                generation,
            )?;
            store.complete_sync(&local.server.id, generation)
        })
        .expect("seed local cache");

    let readiness = active_source_startup_readiness(&store, &local.server.id).expect("readiness");

    assert!(readiness.metadata_fresh);
    assert!(readiness.artwork_fresh);
    assert_eq!(readiness.prefetch_required_reason, None);
    assert_eq!(readiness.startup_delay_ms, None);
}
#[test]
pub(in crate::controller) fn warm_cache_schedule() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let root = unique_test_dir("local-warm-manifest-refresh");
    fs::create_dir_all(&root).expect("create root");
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    settings.sources.local_folders = vec![LocalLibraryFolder {
        path: root.to_string_lossy().into_owned(),
    }];
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_server(&local)?;
            store.set_active_server(&local.server.id)?;
            let generation = store.begin_sync(&local.server.id)?;
            store.upsert_albums(
                &local.server.id,
                &[local_album_with_image_ref(ImageRef::new(
                    "local:cover:file%3A%2F%2Fwarm-cover",
                    None,
                ))],
                generation,
            )?;
            store.complete_sync(&local.server.id, generation)
        })
        .expect("seed local cache");

    let readiness = active_source_startup_readiness(&store, &local.server.id).expect("readiness");

    assert_eq!(
        readiness.sync_required_reason,
        Some(SyncRequiredReason::LocalManifestRefresh)
    );
    assert_eq!(readiness.startup_delay_ms, Some(8_000));
    assert!(readiness.metadata_fresh);
    assert!(readiness.artwork_fresh);
    assert!(active_server_needs_sync(&store, &local.server.id));
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn empty_cache_schedule() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let root = unique_test_dir("local-empty-manifest-refresh");
    fs::create_dir_all(&root).expect("create root");
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    settings.sources.local_folders = vec![LocalLibraryFolder {
        path: root.to_string_lossy().into_owned(),
    }];
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_server(&local)?;
            store.set_active_server(&local.server.id)?;
            let generation = store.begin_sync(&local.server.id)?;
            store.complete_sync(&local.server.id, generation)
        })
        .expect("seed empty local cache");

    let readiness = active_source_startup_readiness(&store, &local.server.id).expect("readiness");

    assert_eq!(
        readiness.sync_required_reason,
        Some(SyncRequiredReason::LocalManifestRefresh)
    );
    assert_eq!(readiness.startup_delay_ms, Some(8_000));
    assert!(readiness.metadata_fresh);
    assert!(readiness.artwork_fresh);
    assert!(active_server_needs_sync(&store, &local.server.id));
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn startup_local_refresh() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    store
        .with_store(|store| {
            store.save_server(&local)?;
            let generation = store.begin_sync(&local.server.id)?;
            store.upsert_albums(
                &local.server.id,
                &[local_album_with_image_ref(ImageRef::new(
                    "local:cover:file%3A%2F%2Fwarm-cover",
                    None,
                ))],
                generation,
            )?;
            store.complete_sync(&local.server.id, generation)
        })
        .expect("seed local cache");

    let readiness = active_source_startup_readiness(&store, &local.server.id).expect("readiness");

    assert_eq!(readiness.sync_required_reason, None);
    assert_eq!(readiness.startup_delay_ms, None);
    assert!(!active_server_needs_sync(&store, &local.server.id));
}
#[test]
pub(in crate::controller) fn startup_local_exists() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let image_ref = ImageRef::new("local:cover:file%3A%2F%2Fcached-cover", None);
    let album = local_album_with_image_ref(image_ref.clone());
    let stale_track = local_track_with_image_ref(
        1,
        &album,
        ImageRef::new("local:cover:embedded%3A%2Fmusic%2Fstale.flac", None),
    );
    let root = unique_test_dir("local-cover-cache-ready");
    fs::create_dir_all(&root).expect("create cache dir");
    let cover_path = root.join("cover.jpg");
    fs::write(&cover_path, [0xff_u8, 0xd8, 0xff, 0xd9]).expect("write cover file");
    store
        .with_store(|store| {
            store.save_server(&local)?;
            let generation = store.begin_sync(&local.server.id)?;
            store.upsert_albums(&local.server.id, std::slice::from_ref(&album), generation)?;
            store.upsert_tracks(&local.server.id, &[stale_track], generation)?;
            store.save_cover_cache_entry(&CoverCacheEntry {
                server_id: local.server.id.clone(),
                item_id: image_ref.item_id.clone(),
                image_tag: IMAGE_TAG_UNTAGGED.to_string(),
                size: 256,
                path: cover_path.to_string_lossy().to_string(),
            })?;
            store.complete_sync(&local.server.id, generation)
        })
        .expect("save local server");

    assert!(!initial_cover_cache_required(&store, &local.server.id));
    assert!(!active_server_needs_sync(&store, &local.server.id));
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn startup_local_artwork() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let image_ref = ImageRef::new("local:cover:file%3A%2F%2Fthumbnail-only-cover", None);
    let album = local_album_with_image_ref(image_ref.clone());
    let root = unique_test_dir("local-thumbnail-only-cover-cache");
    fs::create_dir_all(&root).expect("create cache dir");
    let cover_path = root.join("cover.jpg");
    fs::write(&cover_path, [0xff_u8, 0xd8, 0xff, 0xd9]).expect("write cover file");
    store
        .with_store(|store| {
            store.save_server(&local)?;
            let generation = store.begin_sync(&local.server.id)?;
            store.upsert_albums(&local.server.id, std::slice::from_ref(&album), generation)?;
            store.save_cover_cache_entry(&CoverCacheEntry {
                server_id: local.server.id.clone(),
                item_id: image_ref.item_id.clone(),
                image_tag: IMAGE_TAG_UNTAGGED.to_string(),
                size: 96,
                path: cover_path.to_string_lossy().to_string(),
            })?;
            store.complete_sync(&local.server.id, generation)
        })
        .expect("save local server");

    assert!(!initial_cover_cache_required(&store, &local.server.id));
    assert!(!active_server_needs_sync(&store, &local.server.id));
    let readiness = active_source_readiness(&store, &local.server.id).expect("readiness");
    assert!(!readiness.artwork_fresh);
    assert_eq!(
        readiness.prefetch_required_reason,
        Some(SyncRequiredReason::LocalArtworkMissing)
    );
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn startup_ignore_ready() {
    let stale_age = Some(STARTUP_CACHE_STALE_SECONDS + 60);

    let local_ready = source_sync_readiness(SourceSyncReadinessInput {
        provider: LOCAL_PROVIDER_ID,
        cached_item_count: 42,
        sync_status: Some("idle"),
        sync_completed_age_seconds: stale_age,
        local_library_configured: false,
        local_artwork_missing: false,
    });
    assert_eq!(local_ready.sync_required_reason, None);
    assert_eq!(local_ready.startup_delay_ms, None);
    assert!(local_ready.metadata_fresh);
    assert!(local_ready.artwork_fresh);

    let remote_stale = source_sync_readiness(SourceSyncReadinessInput {
        provider: "jellyfin",
        cached_item_count: 42,
        sync_status: Some("idle"),
        sync_completed_age_seconds: stale_age,
        local_library_configured: false,
        local_artwork_missing: false,
    });
    assert_eq!(
        remote_stale.sync_required_reason,
        Some(SyncRequiredReason::RemoteCacheStale)
    );
    assert_eq!(remote_stale.startup_delay_ms, Some(8_000));

    let local_missing_artwork = source_sync_readiness(SourceSyncReadinessInput {
        provider: LOCAL_PROVIDER_ID,
        cached_item_count: 42,
        sync_status: Some("idle"),
        sync_completed_age_seconds: stale_age,
        local_library_configured: false,
        local_artwork_missing: true,
    });
    assert_eq!(local_missing_artwork.sync_required_reason, None);
    assert_eq!(
        local_missing_artwork.prefetch_required_reason,
        Some(SyncRequiredReason::LocalArtworkMissing)
    );
    assert_eq!(local_missing_artwork.startup_delay_ms, None);
    assert!(local_missing_artwork.metadata_fresh);
    assert!(!local_missing_artwork.artwork_fresh);
}
#[test]
pub(in crate::controller) fn startup_local_source() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let local_image_ref = ImageRef::new("local:cover:file%3A%2F%2Fsection-cover", None);
    let remote_image_ref = ImageRef::new("jellyfin:album:remote", None);
    let local_album = local_album_with_image_ref(local_image_ref.clone());
    let mut remote_album = local_album_with_image_ref(remote_image_ref);
    remote_album.id = AlbumId::new("jellyfin:album:remote");
    let generation = store
        .with_store(|store| {
            store.save_server(&local)?;
            store.begin_sync(&local.server.id)
        })
        .expect("begin sync");

    cache_home_section(
        &store,
        &local.server.id,
        &HomeSection {
            kind: HomeSectionKind::Explore,
            albums: vec![remote_album, local_album],
            tracks: Vec::new(),
        },
        generation,
    )
    .expect("cache home section");

    let sections = store
        .with_store(|store| store.load_home_sections(&local.server.id))
        .expect("load home sections");
    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0].albums.len(), 1);
    assert_eq!(sections[0].albums[0].id.as_str(), "local:album:one");
    assert_eq!(
        sections[0].albums[0]
            .image_ref
            .as_ref()
            .map(|image_ref| image_ref.item_id.as_str()),
        Some(local_image_ref.item_id.as_str())
    );
}
#[test]
pub(in crate::controller) fn snapshot_reuse_album() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let album_image_ref = ImageRef::new("local:cover:file%3A%2F%2Falbum-cover", None);
    let album = local_album_with_image_ref(album_image_ref.clone());
    let tracks = [
        local_track_with_image_ref(
            1,
            &album,
            ImageRef::new("local:cover:embedded%3A%2Fmusic%2Fone.flac", None),
        ),
        local_track_with_image_ref(
            2,
            &album,
            ImageRef::new("local:cover:embedded%3A%2Fmusic%2Ftwo.flac", None),
        ),
    ];
    let mut favorite_track = tracks[0].clone();
    favorite_track.favorite = true;
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    seed_cached_library(
        &store,
        &local,
        std::slice::from_ref(&album),
        &[favorite_track.clone(), tracks[1].clone()],
        &[HomeSection {
            kind: HomeSectionKind::MostPlayed,
            albums: Vec::new(),
            tracks: vec![favorite_track],
        }],
    );

    let snapshot = load_snapshot(&store).expect("load snapshot");

    assert!(
        snapshot
            .tracks
            .iter()
            .all(|track| track.image_ref.as_ref() == Some(&album_image_ref))
    );
    assert_eq!(
        snapshot.home_sections[0].tracks[0].image_ref.as_ref(),
        Some(&album_image_ref)
    );
    assert_eq!(
        snapshot.favorites[0].image_ref.as_ref(),
        Some(&album_image_ref)
    );
    let (controller, _events) = controller_from_store_for_test(store);
    let favorites = controller
        .cached_favorite_tracks()
        .expect("cached favorite tracks");
    assert_eq!(favorites.len(), 1);
    assert_eq!(favorites[0].image_ref.as_ref(), Some(&album_image_ref));
}
#[test]
pub(in crate::controller) fn startup_track_cards() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let track_image_ref = ImageRef::new("local:cover:embedded%3A%2Fmusic%2Fpayload.flac", None);
    let mut album =
        local_album_with_image_ref(ImageRef::new("local:cover:file%3A%2F%2Funused", None));
    album.image_ref = None;
    let track = local_track_with_image_ref(1, &album, track_image_ref.clone());
    let mut section_album = album.clone();
    section_album.image_ref = Some(ImageRef::new(
        "local:cover:embedded%3A%2Fmusic%2Fstale-payload.flac",
        None,
    ));
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    seed_cached_library(
        &store,
        &local,
        std::slice::from_ref(&album),
        std::slice::from_ref(&track),
        &[],
    );
    let mut section = HomeSection {
        kind: HomeSectionKind::Explore,
        albums: vec![section_album],
        tracks: vec![track],
    };

    home_image_refs(&store, &local, &mut section).expect("normalize home section");

    assert_eq!(section.albums[0].image_ref.as_ref(), Some(&track_image_ref));
    assert_eq!(section.tracks[0].image_ref.as_ref(), Some(&track_image_ref));
}
#[test]
pub(in crate::controller) fn stale_track_images() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let local = local_source_saved();
    let album_image_ref = ImageRef::new("local:cover:file%3A%2F%2Falbum-cover", None);
    let album = local_album_with_image_ref(album_image_ref.clone());
    let tracks = vec![
        local_track_with_image_ref(
            1,
            &album,
            ImageRef::new("local:cover:embedded%3A%2Fmusic%2Fone.flac", None),
        ),
        local_track_with_image_ref(
            2,
            &album,
            ImageRef::new("local:cover:embedded%3A%2Fmusic%2Ftwo.flac", None),
        ),
    ];
    seed_cached_library(
        &controller.store,
        &local,
        std::slice::from_ref(&album),
        &tracks,
        &[],
    );

    let page = controller
        .cached_tracks_page(0, 10)
        .expect("cached tracks page");

    assert!(
        page.items
            .iter()
            .all(|track| track.image_ref.as_ref() == Some(&album_image_ref))
    );
}

#[test]
pub(in crate::controller) fn auto_dj_candidate() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let local = local_source_saved();
    let album_image_ref = ImageRef::new("local:cover:file%3A%2F%2Fauto-dj-album", None);
    let album = local_album_with_image_ref(album_image_ref.clone());
    let tracks = (1..=7)
        .map(|number| {
            local_track_with_image_ref(
                number,
                &album,
                ImageRef::new(
                    format!("local:cover:embedded%3A%2Fmusic%2Fauto-dj-{number}.flac"),
                    None,
                ),
            )
        })
        .collect::<Vec<_>>();
    seed_cached_library(
        &controller.store,
        &local,
        std::slice::from_ref(&album),
        &tracks,
        &[],
    );
    let mut queue = QueueEngine::new(local.server.id.clone());
    queue.play_now(&tracks[0]);
    *controller.queue.lock().expect("queue") = Some(queue);
    *controller.auto_dj_enabled.lock().expect("auto dj") = true;

    assert!(controller.refill_auto_dj_queue());

    let queue = controller.queue.lock().expect("queue");
    let queue = queue.as_ref().expect("queue");
    assert_eq!(queue.entries().len(), 1 + super::AUTO_DJ_ITEM_COUNT);
    assert!(
        queue
            .entries()
            .iter()
            .skip(1)
            .all(|entry| entry.image_ref.as_ref() == Some(&album_image_ref))
    );
}

#[test]
pub(in crate::controller) fn restored_queue_reuse() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let album_image_ref = ImageRef::new("local:cover:file%3A%2F%2Falbum-cover", None);
    let album = local_album_with_image_ref(album_image_ref.clone());
    let track = local_track_with_image_ref(
        1,
        &album,
        ImageRef::new("local:cover:embedded%3A%2Fmusic%2Fone.flac", None),
    );
    seed_cached_library(
        &store,
        &local,
        std::slice::from_ref(&album),
        std::slice::from_ref(&track),
        &[],
    );
    store
        .with_store(|store| {
            let mut queue = QueueEngine::new(local.server.id.clone());
            queue.play_now(&track);
            store.save_queue_snapshot(&queue.snapshot())
        })
        .expect("seed local queue");

    let restored = restore_queue(&store, Some(&local.server)).expect("restore queue");
    let queue = restored.snapshot();
    assert_eq!(queue.entries[0].image_ref.as_ref(), Some(&album_image_ref));

    let playback =
        playback_snapshot_from_queue(Some(&restored), false, &PlaybackSettings::default());
    assert_eq!(
        playback.current.expect("current").image_ref.as_ref(),
        Some(&album_image_ref)
    );
}

#[test]
pub(in crate::controller) fn sync_refreshes_queue() {
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let local = local_source_saved();
    let old_image_ref = ImageRef::new(
        "local:cover:file%3A%2F%2Factive-album-cover",
        Some("old-cover".to_string()),
    );
    let new_image_ref = ImageRef::new(
        "local:cover:file%3A%2F%2Factive-album-cover",
        Some("new-cover".to_string()),
    );
    let old_album = local_album_with_image_ref(old_image_ref.clone());
    let new_album = local_album_with_image_ref(new_image_ref.clone());
    let track = local_track_with_image_ref(1, &old_album, old_image_ref);
    seed_cached_library(
        &controller.store,
        &local,
        std::slice::from_ref(&new_album),
        std::slice::from_ref(&track),
        &[],
    );
    let mut queue = QueueEngine::new(local.server.id.clone());
    queue.play_now(&track);
    *controller.queue.lock().expect("queue") = Some(queue);
    controller.sync_playback_snapshot_from_queue();

    super::refresh_queue_refs(&controller.sync_context(), &local);

    let queue = wait_for_queue(&events).expect("queue");
    assert_eq!(queue.entries[0].image_ref.as_ref(), Some(&new_image_ref));
    let playback = controller
        .playback_snapshot
        .lock()
        .expect("playback")
        .clone();
    assert_eq!(
        playback.current.expect("current").image_ref.as_ref(),
        Some(&new_image_ref)
    );
}

#[test]
pub(in crate::controller) fn startup_change_progress() {
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let local = local_source_saved();
    let old_image_ref = ImageRef::new(
        "local:cover:file%3A%2F%2Fprogress-album-cover",
        Some("old-cover".to_string()),
    );
    let new_image_ref = ImageRef::new(
        "local:cover:file%3A%2F%2Fprogress-album-cover",
        Some("new-cover".to_string()),
    );
    let old_album = local_album_with_image_ref(old_image_ref.clone());
    let new_album = local_album_with_image_ref(new_image_ref.clone());
    let track = local_track_with_image_ref(1, &old_album, old_image_ref);
    seed_cached_library(
        &controller.store,
        &local,
        std::slice::from_ref(&new_album),
        std::slice::from_ref(&track),
        &[],
    );
    let mut queue = QueueEngine::new(local.server.id.clone());
    queue.play_now(&track);
    *controller.queue.lock().expect("queue") = Some(queue);
    let original_snapshot = controller
        .queue
        .lock()
        .expect("queue")
        .as_ref()
        .expect("queue")
        .snapshot();
    controller
        .queue
        .lock()
        .expect("queue")
        .as_mut()
        .expect("queue")
        .set_progress_seconds(17);

    super::snapshot_queue_refs(&controller.sync_context(), &local, original_snapshot);

    let queue = wait_for_queue(&events).expect("queue");
    assert_eq!(queue.progress_seconds, 17);
    assert_eq!(queue.entries[0].image_ref.as_ref(), Some(&new_image_ref));
    let playback = controller
        .playback_snapshot
        .lock()
        .expect("playback")
        .clone();
    assert_eq!(playback.position_seconds, 17);
    assert_eq!(
        playback.current.expect("current").image_ref.as_ref(),
        Some(&new_image_ref)
    );
}

#[test]
pub(in crate::controller) fn snapshot_discards_image() {
    let store = StoreHandle::open_memory().expect("memory store");
    let remote = saved_server();
    let local_image_ref = ImageRef::new("local:cover:embedded%3A%2Fmusic%2Ftrack.flac", None);
    let remote_album = remote_album_with_image_ref(local_image_ref);
    store
        .with_store(|store| {
            store.save_server(&remote)?;
            store.set_active_server(&remote.server.id)?;
            let generation = store.begin_sync(&remote.server.id)?;
            store.upsert_albums(
                &remote.server.id,
                std::slice::from_ref(&remote_album),
                generation,
            )?;
            store.upsert_home_sections(
                &remote.server.id,
                &[HomeSection {
                    kind: HomeSectionKind::Explore,
                    albums: vec![remote_album],
                    tracks: Vec::new(),
                }],
                generation,
            )?;
            store.complete_sync(&remote.server.id, generation)
        })
        .expect("seed remote cache");

    let snapshot = load_snapshot(&store).expect("load snapshot");

    assert_eq!(snapshot.albums.len(), 1);
    startup_assert_ref(snapshot.albums[0].image_ref.as_ref());
    assert_eq!(snapshot.home_sections.len(), 1);
    startup_assert_ref(snapshot.home_sections[0].albums[0].image_ref.as_ref());
}
#[test]
pub(in crate::controller) fn snapshot_discards_external() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let external_image_ref = ImageRef::new("external:album:Example%20Artist:Example%20Album", None);
    let local_album = local_album_with_image_ref(external_image_ref);
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_server(&local)?;
            store.set_active_server(&local.server.id)?;
            let generation = store.begin_sync(&local.server.id)?;
            store.upsert_albums(
                &local.server.id,
                std::slice::from_ref(&local_album),
                generation,
            )?;
            store.upsert_home_sections(
                &local.server.id,
                &[HomeSection {
                    kind: HomeSectionKind::Explore,
                    albums: vec![local_album],
                    tracks: Vec::new(),
                }],
                generation,
            )?;
            store.complete_sync(&local.server.id, generation)
        })
        .expect("seed local cache");

    let snapshot = load_snapshot(&store).expect("load snapshot");

    assert_eq!(snapshot.albums.len(), 1);
    assert!(snapshot.albums[0].image_ref.is_none());
    assert_eq!(snapshot.home_sections.len(), 1);
    assert!(snapshot.home_sections[0].albums[0].image_ref.is_none());
}
#[test]
pub(in crate::controller) fn startup_track_image() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let external_image_ref = ImageRef::new("external:album:Example%20Artist:Example%20Album", None);
    let album = local_album_with_image_ref(external_image_ref);
    let mut track = library_track(
        1,
        Some(ArtistId::new("local:artist:one")),
        album.id.clone(),
        "Example Artist",
        &[],
    );
    track.id = TrackId::new("local:track:one");
    track.album = album.title.clone();
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_server(&local)?;
            store.set_active_server(&local.server.id)?;
            let generation = store.begin_sync(&local.server.id)?;
            store.upsert_albums(&local.server.id, std::slice::from_ref(&album), generation)?;
            store.upsert_tracks(&local.server.id, std::slice::from_ref(&track), generation)?;
            store.complete_sync(&local.server.id, generation)
        })
        .expect("seed local cache");

    let snapshot = load_snapshot(&store).expect("load snapshot");

    assert_eq!(snapshot.tracks.len(), 1);
    assert!(snapshot.tracks[0].image_ref.is_none());
}
#[test]
pub(in crate::controller) fn cache_album_page() {
    let store = StoreHandle::open_memory().expect("memory store");
    let remote = saved_server();
    let local_image_ref = ImageRef::new("local:cover:embedded%3A%2Fmusic%2Ftrack.flac", None);
    let remote_album = remote_album_with_image_ref(local_image_ref);
    store
        .with_store(|store| {
            store.save_server(&remote)?;
            store.set_active_server(&remote.server.id)?;
            let generation = store.begin_sync(&remote.server.id)?;
            store.upsert_albums(
                &remote.server.id,
                std::slice::from_ref(&remote_album),
                generation,
            )?;
            store.complete_sync(&remote.server.id, generation)?;
            Ok(())
        })
        .expect("seed remote cache");
    let (controller, _events) = controller_from_store_for_test(store);

    let page = controller
        .cached_albums_page(0, 10)
        .expect("load cached albums");

    assert_eq!(page.items.len(), 1);
    startup_assert_ref(page.items[0].image_ref.as_ref());
}
#[test]
pub(in crate::controller) fn external_album_refs() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let local = local_source_saved();
    let external_image_ref = ImageRef::new("external:album:Example%20Artist:Example%20Album", None);
    let album = local_album_with_image_ref(external_image_ref);
    let mut track = library_track(
        1,
        Some(ArtistId::new("local:artist:one")),
        album.id.clone(),
        "Example Artist",
        &[],
    );
    track.id = TrackId::new("local:track:one");
    track.album = album.title.clone();
    controller
        .store
        .with_store(|store| {
            store.save_server(&local)?;
            store.set_active_server(&local.server.id)?;
            let generation = store.begin_sync(&local.server.id)?;
            store.upsert_albums(&local.server.id, std::slice::from_ref(&album), generation)?;
            store.upsert_tracks(&local.server.id, std::slice::from_ref(&track), generation)?;
            store.complete_sync(&local.server.id, generation)
        })
        .expect("seed local cache");

    let page = controller
        .cached_tracks_page(0, 10)
        .expect("cached tracks page");

    assert_eq!(page.items.len(), 1);
    assert!(page.items[0].image_ref.is_none());
}
#[test]
pub(in crate::controller) fn startup_remote_cache() {
    let runtime = Runtime::new().expect("runtime");
    let store = StoreHandle::open_memory().expect("memory store");
    let provider = FakeProvider::new(FakeScale::Small);
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
        .block_on(sync_provider(&store, &saved.server.id, &provider))
        .expect("sync remote cache");

    assert!(!initial_cover_cache_required(&store, &saved.server.id));
}
#[test]
pub(in crate::controller) fn startup_remote_sync_detects_noop_and_delta() {
    let runtime = Runtime::new().expect("runtime");
    let store = StoreHandle::open_memory().expect("memory store");
    let provider = FakeProvider::new(FakeScale::Small);
    let saved = SavedServer {
        server: provider.identity().server.clone(),
        user_id: "fake-user".to_string(),
        username: "fake".to_string(),
        trust_invalid_cert: false,
    };
    store
        .with_store(|store| {
            store.save_server(&saved)?;
            store.set_active_server(&saved.server.id)
        })
        .expect("save server");
    runtime
        .block_on(sync_provider(&store, &saved.server.id, &provider))
        .expect("seed remote cache");

    let noop = runtime
        .block_on(sync_provider_outcome(&store, &saved.server.id, &provider))
        .expect("same remote sync");
    assert!(noop.delta.is_empty());

    let mut stale_album = runtime
        .block_on(provider.albums(PagedRequest::new(0, 1)))
        .expect("provider albums")
        .items
        .into_iter()
        .next()
        .expect("album");
    stale_album.title = "Stale Album Title".to_string();
    let generation = store
        .with_store(|store| {
            store
                .sync_state(&saved.server.id)
                .map(|state| state.generation)
        })
        .expect("sync state");
    store
        .with_store(|store| {
            store.upsert_albums(
                &saved.server.id,
                std::slice::from_ref(&stale_album),
                generation,
            )
        })
        .expect("seed stale album");

    let changed = runtime
        .block_on(sync_provider_outcome(&store, &saved.server.id, &provider))
        .expect("changed remote sync");
    assert!(changed.delta.albums.fields.contains(&stale_album.id));
}
fn seed_cached_library(
    store: &StoreHandle,
    saved: &SavedServer,
    albums: &[Album],
    tracks: &[Track],
    home_sections: &[HomeSection],
) {
    store
        .with_store(|store| {
            store.save_server(saved)?;
            store.set_active_server(&saved.server.id)?;
            let generation = store.begin_sync(&saved.server.id)?;
            if !albums.is_empty() {
                store.upsert_albums(&saved.server.id, albums, generation)?;
            }
            if !tracks.is_empty() {
                store.upsert_tracks(&saved.server.id, tracks, generation)?;
            }
            if !home_sections.is_empty() {
                store.upsert_home_sections(&saved.server.id, home_sections, generation)?;
            }
            store.complete_sync(&saved.server.id, generation)
        })
        .expect("seed library cache");
}

fn local_album_with_image_ref(image_ref: ImageRef) -> Album {
    Album {
        id: AlbumId::new("local:album:one"),
        title: "Example Album".to_string(),
        artist: "Example Artist".to_string(),
        artist_id: Some(ArtistId::new("local:artist:one")),
        album_artist_credits: Vec::new(),
        artist_credits: Vec::new(),
        year: 2026,
        release_date: None,
        date_added: None,
        last_played: None,
        play_count: None,
        user_rating: None,
        track_count: 1,
        duration_seconds: 180,
        favorite: false,
        color_seed: 1,
        image_ref: Some(image_ref),
        genres: Vec::new(),
        release_types: Vec::new(),
        is_compilation: None,
        musicbrainz_album_id: None,
        musicbrainz_release_group_id: None,
    }
}
fn local_track_with_image_ref(number: u32, album: &Album, image_ref: ImageRef) -> Track {
    let mut track = library_track(
        number,
        Some(ArtistId::new("local:artist:one")),
        album.id.clone(),
        "Example Artist",
        &[],
    );
    track.id = TrackId::new(format!("local:track:{number}"));
    track.album = album.title.clone();
    track.image_ref = Some(image_ref);
    track
}
fn remote_album_with_image_ref(image_ref: ImageRef) -> Album {
    let mut album = local_album_with_image_ref(image_ref);
    album.id = AlbumId::new("jellyfin:album:one");
    album.artist_id = Some(ArtistId::new("jellyfin:artist:one"));
    album
}
fn startup_assert_ref(image_ref: Option<&ImageRef>) {
    assert!(
        !image_ref.is_some_and(|image_ref| image_ref.item_id.starts_with("local:cover:")),
        "remote cached reads must not expose local provider image refs"
    );
}
#[test]
pub(in crate::controller) fn home_refresh_replace() {
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
pub(in crate::controller) fn playlist_refresh_replace() {
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
        image_refs: Vec::new(),
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
            store.upsert_playlists(&saved.server.id, std::slice::from_ref(&stale_playlist), 0)?;
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
pub(in crate::controller) fn startup_replace_section() {
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
        musicbrainz_artist_id: None,
    };
    expected_track.artist_credits = vec![expected_credit];
    assert_eq!(after[1].tracks, vec![expected_track]);
}
#[test]
pub(in crate::controller) fn startup_update_event() {
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
pub(in crate::controller) fn startup_suppress_release() {
    let guards = InFlightGuards::new("Test");
    let server_id = ServerId::new("test-server");
    let permit = guards
        .acquire(server_id.clone())
        .expect("guard lock")
        .expect("first permit");

    assert!(guards.contains_or_blocked(&server_id));
    assert!(
        guards
            .acquire(server_id.clone())
            .expect("duplicate guard lock")
            .is_none()
    );

    drop(permit);

    assert!(!guards.contains_or_blocked(&server_id));
    assert!(
        guards
            .acquire(server_id)
            .expect("guard lock after release")
            .is_some()
    );
}
#[test]
pub(in crate::controller) fn startup_keep_blocking() {
    let guards = InFlightGuards::new("Test");
    let poisoned = guards.clone();
    let _panic = std::thread::spawn(move || {
        let _running = poisoned.inner.lock().expect("guard lock");
        panic!("poison in-flight guard");
    })
    .join();

    assert!(guards.contains_or_blocked(&ServerId::new("test-server")));
    let error = match guards.acquire(ServerId::new("another-test-server")) {
        Ok(_) => panic!("poisoned guard accepted a permit"),
        Err(error) => error,
    };
    assert_eq!(error, "Test guard lock was poisoned.");
}
#[test]
pub(in crate::controller) fn startup_home_unchanged() {
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
pub(in crate::controller) fn startup_promote_prefetch() {
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
pub(in crate::controller) fn startup_emit_snapshot() {
    let (controller, events, snapshot, _queue, _player) =
        AppController::bootstrap_with_fake(FakeScale::Small);
    let server = snapshot.server.expect("server");
    controller.clear_active_server_cache();
    let snapshot = wait_for_snapshot(&events);
    assert!(!snapshot.first_run);
    assert_eq!(snapshot.server.expect("server").id, server.id);
    assert!(snapshot.albums.is_empty());
    assert!(snapshot.tracks.is_empty());
    assert!(snapshot.search.albums.is_empty());
}
