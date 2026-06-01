use super::*;

use super::{
    AppController, ControllerEvent, LOCAL_SOURCE_SERVER_ID, LibrarySnapshot,
    LoginActivationContext, LoginActivationRequest, SNAPSHOT_GRID_LIMIT, SNAPSHOT_TRACK_LIMIT,
    StoreHandle, activate_logged_in_server, home_refresh_completed_event, load_snapshot,
    prefetch_home_section, promote_prefetched_home_section, refresh_home_section,
    refresh_home_sections, refresh_home_sections_without_explore, refresh_playlist_pages,
    save_token_and_activate_logged_in_server, sync_page_finished, sync_provider,
};
use rufin_core::{
    AlbumId, AppSettings, ArtistCredit, HomeSection, HomeSectionKind, LibrarySourceSelection,
    LocalLibraryFolder, Playlist, PlaylistId, ServerId, ServerIdentity, TrackId,
};
use rufin_playback::{
    PlaybackBackend, PlaybackCommand, PlaybackError, PlaybackEvent, PlaybackState,
};
use rufin_provider::{MusicProvider, PagedRequest, PlaylistEntry, ProviderSession};
use rufin_secrets::SecretStore;
use rufin_store::SavedServer;
use rufin_test_support::{FakeProvider, FakeScale};
use std::fs;
use std::sync::{Arc, Mutex};
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
pub(in crate::controller) fn jellyfin_device_id_is_generated_once_and_saved() {
    let store = StoreHandle::open_memory().expect("open memory store");

    let first =
        ensure_jellyfin_device_id_with_generator(&store, || Ok("rufin-install-one".to_string()))
            .expect("first device id");
    let second =
        ensure_jellyfin_device_id_with_generator(&store, || Ok("rufin-install-two".to_string()))
            .expect("second device id");

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
pub(in crate::controller) fn no_server_bootstrap_enters_first_run_state() {
    let (_controller, _events, snapshot, queue, player) =
        AppController::bootstrap_memory_for_test();
    assert!(snapshot.first_run);
    assert!(snapshot.server.is_none());
    assert!(queue.is_none());
    assert_eq!(player.state, PlaybackState::Stopped);
}
#[test]
pub(in crate::controller) fn source_selection_activates_queue_for_selected_source() {
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
pub(in crate::controller) fn first_run_local_server_initializes_active_queue() {
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
pub(in crate::controller) fn first_run_local_server_accepts_multiple_folders() {
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
pub(in crate::controller) fn activate_logged_in_server_selects_server_without_saving_token() {
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
pub(in crate::controller) fn token_save_failure_does_not_persist_empty_server() {
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
    let error = save_token_and_activate_logged_in_server(
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
pub(in crate::controller) fn local_source_snapshot_loads_configured_folders() {
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
pub(in crate::controller) fn snapshot_load_reconciles_active_server_to_selected_remote_source() {
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
pub(in crate::controller) fn local_folder_preferences_add_selects_local_source_and_syncs() {
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
    assert_eq!(wait_for_status(&events), "Syncing Local library...");
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn selecting_fresh_local_source_reuses_cached_library() {
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
    assert_eq!(snapshot.sync_status, "Cached library ready");
    let state = controller
        .store
        .with_store(|store| store.sync_state(&local.server.id))
        .expect("sync state");
    assert_eq!(state.status, "idle");
    assert_eq!(state.generation, generation);

    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        match events.recv_timeout(Duration::from_millis(25)) {
            Ok(ControllerEvent::LoginStatus(status)) => {
                panic!("unexpected local sync status: {status}");
            }
            Ok(ControllerEvent::Snapshot(snapshot)) => {
                assert_ne!(snapshot.sync_status, "Syncing library...");
            }
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn selecting_local_source_with_missing_artwork_reuses_cached_rows() {
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
    assert_eq!(snapshot.sync_status, "Cached library ready");
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn local_folder_preferences_remove_preserves_remote_source_selection() {
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
pub(in crate::controller) fn update_server_settings_persists_editable_fields() {
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
pub(in crate::controller) fn unchanged_server_settings_emit_visible_status() {
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
pub(in crate::controller) fn fake_bootstrap_routes_data_through_store_cache() {
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
pub(in crate::controller) fn sync_pages_continue_when_total_is_unknown() {
    assert!(!sync_page_finished(500, 0, 500));
    assert!(sync_page_finished(120, 0, 620));
    assert!(!sync_page_finished(120, 1_000, 620));
    assert!(sync_page_finished(500, 1_000, 1_000));
}
#[test]
pub(in crate::controller) fn large_fake_bootstrap_seeds_visible_cache_window() {
    let (_controller, _events, snapshot, _queue, _player) =
        AppController::bootstrap_with_fake(FakeScale::Large);
    assert!(!snapshot.first_run);
    assert_eq!(snapshot.albums.len(), SNAPSHOT_GRID_LIMIT);
    assert_eq!(snapshot.tracks.len(), 2_000);
    assert_eq!(snapshot.cached_album_count, 1_000);
    assert_eq!(snapshot.cached_track_count, 2_000);
}
#[test]
pub(in crate::controller) fn provider_sync_caches_all_track_pages() {
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
pub(in crate::controller) fn local_source_with_missing_artwork_does_not_resync_cached_rows() {
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

    assert!(initial_cover_cache_required(&store, &local.server.id));
    assert!(!active_server_needs_sync(&store, &local.server.id));
    let readiness = active_source_readiness(&store, &local.server.id).expect("readiness");
    assert!(!readiness.artwork_fresh);
    assert_eq!(
        readiness.prefetch_required_reason,
        Some(SyncRequiredReason::LocalArtworkMissing)
    );
}
#[test]
pub(in crate::controller) fn startup_readiness_does_not_scan_local_artwork_cache() {
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
pub(in crate::controller) fn local_source_sync_skips_artwork_cache_when_cover_file_exists() {
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
pub(in crate::controller) fn local_source_thumbnail_only_cache_still_needs_grid_artwork() {
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

    assert!(initial_cover_cache_required(&store, &local.server.id));
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
pub(in crate::controller) fn local_startup_sync_policy_ignores_remote_age_when_artwork_is_ready() {
    let stale_age = Some(STARTUP_CACHE_STALE_SECONDS + 60);

    let local_ready = source_sync_readiness(SourceSyncReadinessInput {
        provider: LOCAL_PROVIDER_ID,
        cached_item_count: 42,
        sync_status: Some("idle"),
        sync_completed_age_seconds: stale_age,
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
pub(in crate::controller) fn local_home_section_cache_discards_cross_source_items() {
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
pub(in crate::controller) fn local_snapshot_reuses_album_image_for_stale_track_image_refs() {
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
    store
        .with_store(|store| {
            store.save_server(&local)?;
            store.set_active_server(&local.server.id)?;
            let generation = store.begin_sync(&local.server.id)?;
            store.upsert_albums(&local.server.id, std::slice::from_ref(&album), generation)?;
            store.upsert_tracks(
                &local.server.id,
                &[favorite_track.clone(), tracks[1].clone()],
                generation,
            )?;
            store.upsert_home_sections(
                &local.server.id,
                &[HomeSection {
                    kind: HomeSectionKind::MostPlayed,
                    albums: Vec::new(),
                    tracks: vec![favorite_track.clone()],
                }],
                generation,
            )?;
            store.complete_sync(&local.server.id, generation)
        })
        .expect("seed local cache");

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
}
#[test]
pub(in crate::controller) fn local_cached_tracks_page_reuses_album_image_for_stale_track_image_refs()
 {
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
    controller
        .store
        .with_store(|store| {
            store.save_server(&local)?;
            store.set_active_server(&local.server.id)?;
            let generation = store.begin_sync(&local.server.id)?;
            store.upsert_albums(&local.server.id, std::slice::from_ref(&album), generation)?;
            store.upsert_tracks(&local.server.id, &tracks, generation)?;
            store.complete_sync(&local.server.id, generation)
        })
        .expect("seed local cache");

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
pub(in crate::controller) fn remote_snapshot_discards_local_provider_image_refs() {
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
    assert_not_local_provider_image_ref(snapshot.albums[0].image_ref.as_ref());
    assert_eq!(snapshot.home_sections.len(), 1);
    assert_not_local_provider_image_ref(snapshot.home_sections[0].albums[0].image_ref.as_ref());
}
#[test]
pub(in crate::controller) fn local_snapshot_discards_external_metadata_image_refs() {
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
pub(in crate::controller) fn local_snapshot_does_not_reuse_external_album_image_for_tracks() {
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
pub(in crate::controller) fn remote_cached_album_page_discards_local_provider_image_refs() {
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
    assert_not_local_provider_image_ref(page.items[0].image_ref.as_ref());
}
#[test]
pub(in crate::controller) fn local_cached_tracks_page_discards_external_album_fallback_image_refs()
{
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
pub(in crate::controller) fn cached_remote_sync_skips_initial_artwork_cache() {
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
fn assert_not_local_provider_image_ref(image_ref: Option<&ImageRef>) {
    assert!(
        !image_ref.is_some_and(|image_ref| image_ref.item_id.starts_with("local:cover:")),
        "remote cached reads must not expose local provider image refs"
    );
}
#[test]
pub(in crate::controller) fn home_refresh_replaces_cached_sections_without_full_sync() {
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
pub(in crate::controller) fn playlist_refresh_replaces_cached_list_without_full_sync() {
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
pub(in crate::controller) fn home_section_refresh_replaces_only_selected_section() {
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
pub(in crate::controller) fn home_section_refresh_uses_home_update_event() {
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
pub(in crate::controller) fn in_flight_permit_suppresses_duplicates_until_release() {
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
pub(in crate::controller) fn in_flight_guards_keep_poisoned_locks_blocking() {
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
pub(in crate::controller) fn home_refresh_without_explore_leaves_explore_cache_unchanged() {
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
pub(in crate::controller) fn explore_prefetch_promotes_only_when_requested() {
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
pub(in crate::controller) fn clear_cache_emits_empty_active_server_snapshot() {
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
