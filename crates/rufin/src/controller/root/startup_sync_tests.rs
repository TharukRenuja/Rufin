use super::*;

use super::{
    AppController, ControllerEvent, LOCAL_SOURCE_IDENTITY_ID, LibrarySnapshot, StoreHandle,
    home_refresh_completed_event, load_runtime_snapshot, load_snapshot,
    promote_prefetched_home_section, sync_local_source_outcome, sync_local_source_with_events,
};
use domain::{
    AlbumId, AppSettings, Genre, GenreId, HomeSection, HomeSectionKind, ImageRef,
    LibrarySourceSelection, LocalLibraryFolder, Playlist, PlaylistId, SourceId, SourceIdentity,
    TrackId,
};
use library::{SavedSource, SourceLocalAccess};
use playback::PlaybackState;
use rusqlite::Connection;
use secrets::{MemorySecretStore, SecretStore};
use source::{PlaylistEntry, SourceObjectChanges};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::thread;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;

#[test]
pub(in crate::controller) fn startup_jellyfin_saved() {
    let store = StoreHandle::open_memory().expect("open memory store");

    let first = crate::sources::ensure_jellyfin_device_id(&store).expect("first device id");
    let second = crate::sources::ensure_jellyfin_device_id(&store).expect("second device id");

    assert!(first.starts_with("rufin-"));
    assert_eq!(second, first);
    assert_eq!(store.load_settings().jellyfin_device_id, first);
}

#[test]
pub(in crate::controller) fn selecting_jellyfin_preserves_generated_device_id() {
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let saved = saved_source();
    seed_cached_library(
        &controller.store,
        &saved,
        &[remote_album_with_image_ref(provider_cover_ref())],
        &[],
        &[],
    );
    controller
        .secrets
        .save_token(&saved.source.id, "token")
        .expect("save token");

    controller.select_source(LibrarySourceSelection::Source(saved.source.id.clone()));

    assert_eq!(
        wait_for_source_selection(&events),
        LibrarySourceSelection::Source(saved.source.id)
    );
    let _queue = wait_for_queue(&events);
    let _snapshot = wait_for_snapshot(&events);
    let device_id = controller.load_settings().jellyfin_device_id;
    assert!(device_id.starts_with("rufin-"));
}

#[test]
pub(in crate::controller) fn startup_server_state() {
    let (_controller, _events, snapshot, queue, player) =
        AppController::bootstrap_memory_for_test();
    assert!(snapshot.first_run);
    assert!(snapshot.source.is_none());
    assert!(queue.is_none());
    assert_eq!(player.state, PlaybackState::Stopped);
}
#[test]
pub(in crate::controller) fn startup_activate_source() {
    let store = StoreHandle::open_memory().expect("memory store");
    let remote = saved_source();
    let album = remote_album_with_image_ref(provider_cover_ref());
    let mut track = library_track(
        1,
        album.artist_id.clone(),
        album.id.clone(),
        &album.artist,
        &[],
    );
    track.id = TrackId::new("jellyfin:track:one");
    track.album = album.title.clone();
    track.image_ref = album.image_ref.clone();
    seed_cached_library(
        &store,
        &remote,
        std::slice::from_ref(&album),
        std::slice::from_ref(&track),
        &[],
    );
    store
        .with_store(|store| {
            let mut queue = QueueEngine::new(remote.source.id.clone());
            queue.play_now(&track);
            store.save_queue_snapshot(&queue.snapshot())
        })
        .expect("save remote queue");
    let root = unique_test_dir("source-activation-local");
    fs::create_dir_all(&root).expect("create local root");
    let mut settings = store.load_settings();
    settings.sources.selected = Some(LibrarySourceSelection::Source(remote.source.id.clone()));
    settings.sources.local_folders = vec![LocalLibraryFolder {
        path: root.to_string_lossy().into_owned(),
    }];
    settings.private_mode = true;
    settings.seekbar_waveform_enabled = false;
    store.save_settings(&settings).expect("save settings");
    let (controller, events) = controller_from_store_for_test(store);
    controller
        .secrets
        .save_token(&remote.source.id, "test-token")
        .expect("save token");
    let playback_commands = Arc::new(Mutex::new(Vec::new()));
    *controller.playback.lock().expect("playback") = Box::new(RecordingPlaybackBackend::new(
        Arc::clone(&playback_commands),
    ));

    controller.start_current_track();
    let play_command = wait_for_recorded_command(&playback_commands, |command| {
        matches!(command, PlaybackCommand::PlayPrepared { .. })
    });
    let PlaybackCommand::PlayPrepared { item, .. } = play_command else {
        panic!("expected prepared remote playback");
    };
    assert_eq!(item.track.id, track.id);
    let remote_playback = wait_for_playback_state(&controller, &events, PlaybackState::Playing);
    assert_eq!(
        remote_playback
            .current
            .as_ref()
            .map(|entry| &entry.track_id),
        Some(&track.id)
    );

    controller.select_source(LibrarySourceSelection::Local);
    assert_eq!(
        wait_for_source_selection(&events),
        LibrarySourceSelection::Local
    );
    let local_queue = wait_for_queue(&events).expect("local queue");
    assert_eq!(local_queue.source_id.as_str(), LOCAL_SOURCE_IDENTITY_ID);
    assert!(local_queue.entries.is_empty());
    assert_eq!(
        wait_for_recorded_command(&playback_commands, |command| {
            matches!(command, PlaybackCommand::Stop)
        }),
        PlaybackCommand::Stop
    );
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
    controller.select_source(LibrarySourceSelection::Source(remote.source.id.clone()));
    assert_eq!(
        wait_for_source_selection(&events),
        LibrarySourceSelection::Source(remote.source.id.clone())
    );
    let restored_queue = wait_for_queue(&events).expect("restored server queue");
    assert_eq!(restored_queue.source_id, remote.source.id);
    assert_eq!(restored_queue.entries[0].track_id, track.id);
    let server_snapshot = wait_for_snapshot(&events);
    assert_eq!(
        server_snapshot.selected_source,
        Some(LibrarySourceSelection::Source(remote.source.id.clone()))
    );
    assert_eq!(
        controller.load_settings().sources.selected,
        Some(LibrarySourceSelection::Source(remote.source.id))
    );
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn startup_init_queue() {
    let (controller, events, _snapshot, initial_queue, _player) =
        AppController::bootstrap_memory_for_test();
    assert!(initial_queue.is_none());
    let root = unique_test_dir("first-run-local-queue");
    fs::create_dir_all(&root).expect("create root");
    crate::sources::configure_local_source(
        &controller,
        crate::sources::LocalFolderHostInput {
            roots: vec![root.clone()],
        },
    );
    let queue = wait_for_queue(&events).expect("local queue");
    assert_eq!(queue.source_id.as_str(), LOCAL_SOURCE_IDENTITY_ID);
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
            .source_id
            .as_str(),
        LOCAL_SOURCE_IDENTITY_ID
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
    crate::sources::configure_local_source(
        &controller,
        crate::sources::LocalFolderHostInput {
            roots: vec![first.clone(), second.clone()],
        },
    );
    let queue = wait_for_queue(&events).expect("local queue");
    assert_eq!(queue.source_id.as_str(), LOCAL_SOURCE_IDENTITY_ID);
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
        snapshot.source.expect("server").id.as_str(),
        LOCAL_SOURCE_IDENTITY_ID
    );
    assert_eq!(snapshot.local_folders, settings.sources.local_folders);
    let active = store
        .with_store(|store| store.active_source())
        .expect("active server");
    assert!(active.is_none());
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn snapshot_does_not_replace_unconfigured_local_selection() {
    let store = StoreHandle::open_memory().expect("memory store");
    let remote = saved_source();
    let local = local_source_saved();
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&remote)?;
            store.save_source(&local)?;
            store.set_active_source(&local.source.id)
        })
        .expect("seed servers");

    let snapshot = load_snapshot(&store).expect("load snapshot");

    assert_eq!(snapshot.selected_source, None);
    assert!(snapshot.source.is_none());
    assert!(snapshot.first_run);
    assert_eq!(snapshot.sources, vec![remote.source.clone()]);
    assert!(snapshot.local_folders.is_empty());
    let active = store
        .with_store(|store| store.active_source())
        .expect("active server")
        .expect("active server");
    assert_eq!(active.source.id, local.source.id);
}

#[test]
pub(in crate::controller) fn snapshot_projects_selection_without_committing_it() {
    let store = StoreHandle::open_memory().expect("memory store");
    let active_saved = saved_source();
    let mut selected_saved = saved_source();
    selected_saved.source.id = SourceId::new("jellyfin:server:selected");
    selected_saved.source.name = "Selected Server".to_string();
    selected_saved.source.base_url = "https://selected.example.test".to_string();
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Source(
        selected_saved.source.id.clone(),
    ));
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&active_saved)?;
            store.save_source(&selected_saved)?;
            store.set_active_source(&active_saved.source.id)
        })
        .expect("save servers");

    let snapshot = load_snapshot(&store).expect("load snapshot");

    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Source(
            selected_saved.source.id.clone()
        ))
    );
    assert_eq!(
        snapshot.source.as_ref().map(|server| server.id.clone()),
        Some(selected_saved.source.id.clone())
    );
    let active_after = store
        .with_store(|store| store.active_source())
        .expect("active server")
        .expect("active server");
    assert_eq!(active_after.source.id, active_saved.source.id);
}

#[test]
pub(in crate::controller) fn startup_local_access_status_reuse() {
    let store = StoreHandle::open_memory().expect("memory store");
    let active_saved = saved_source();
    let mut other_saved = saved_source();
    other_saved.source.id = SourceId::new("jellyfin:server:other");
    other_saved.source.name = "Other Server".to_string();
    other_saved.source.base_url = "https://other.example.test".to_string();
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Source(
        active_saved.source.id.clone(),
    ));
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&active_saved)?;
            store.save_source(&other_saved)?;
            store.set_active_source(&active_saved.source.id)?;
            store.save_source_local_access(&SourceLocalAccess {
                source_id: active_saved.source.id.clone(),
                root_path: "/home/demo/Music".to_string(),
                path_replace_from: Some("/server/music".to_string()),
                path_replace_to: Some("/home/demo/Music".to_string()),
            })?;
            store.save_source_local_access(&SourceLocalAccess {
                source_id: other_saved.source.id.clone(),
                root_path: "/home/demo/Other".to_string(),
                path_replace_from: Some("/other/music".to_string()),
                path_replace_to: Some("/home/demo/Other".to_string()),
            })?;
            let generation = store.begin_sync(&active_saved.source.id)?;
            let mut track = library_track(
                1,
                Some(ArtistId::fake(1)),
                AlbumId::fake(1),
                "Example Artist",
                &[],
            );
            track.local_path = Some("/server/music/Album/Track.flac".to_string());
            store.upsert_tracks(&active_saved.source.id, &[track], generation)
        })
        .expect("seed servers");

    let snapshot = load_snapshot(&store).expect("load snapshot");

    assert_eq!(snapshot.source_local_access.len(), 2);
    let active_summary = snapshot
        .source_local_access
        .iter()
        .find(|summary| summary.source_id == active_saved.source.id)
        .expect("active summary");
    assert_eq!(snapshot.local_access, active_summary.access);
    assert_eq!(snapshot.local_access_status, active_summary.status);
    assert_eq!(snapshot.local_access_status.prefix_match_count, 1);
}

#[test]
pub(in crate::controller) fn startup_missing_token_reconnects_saved_remote() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = saved_source();
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source.id)
        })
        .expect("save server");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());

    let snapshot = load_runtime_snapshot(&store, &secrets).expect("load runtime snapshot");

    assert!(snapshot.first_run);
    assert_eq!(
        snapshot.source.as_ref().map(|server| server.id.clone()),
        Some(saved.source.id.clone())
    );
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Source(saved.source.id.clone()))
    );
}

#[test]
pub(in crate::controller) fn startup_unknown_selected_source_remains_recoverable() {
    let store = StoreHandle::open_memory().expect("memory store");
    let mut saved = saved_source();
    saved.source.id = SourceId::new("removed-provider:server");
    saved.source.kind = "removed-provider".to_string();
    saved.source.name = "Removed Provider".to_string();
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source.id)
        })
        .expect("save unsupported source");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());

    let snapshot = load_runtime_snapshot(&store, &secrets).expect("load runtime snapshot");

    assert!(snapshot.first_run);
    assert_eq!(snapshot.sources, vec![saved.source.clone()]);
    assert_eq!(snapshot.source, Some(saved.source.clone()));
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Source(saved.source.id))
    );
}

#[test]
pub(in crate::controller) fn selecting_unknown_source_restores_committed_selection() {
    let store = StoreHandle::open_memory().expect("memory store");
    let active = saved_source();
    let mut unsupported = saved_source();
    unsupported.source.id = SourceId::new("removed-provider:server");
    unsupported.source.kind = "removed-provider".to_string();
    unsupported.source.name = "Removed Provider".to_string();
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Source(active.source.id.clone()));
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&active)?;
            store.save_source(&unsupported)?;
            store.set_active_source(&active.source.id)
        })
        .expect("save sources");
    let (controller, events) = controller_from_store_for_test(store);
    controller
        .secrets
        .save_token(&active.source.id, "token")
        .expect("save active token");

    controller.select_source(LibrarySourceSelection::Source(
        unsupported.source.id.clone(),
    ));

    assert_eq!(
        wait_for_source_selection(&events),
        LibrarySourceSelection::Source(unsupported.source.id.clone())
    );
    let snapshot = wait_for_snapshot(&events);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Source(active.source.id.clone()))
    );
    let error = events.recv_timeout(Duration::from_secs(1)).expect("error");
    assert!(matches!(
        error,
        ControllerEvent::SourceTransitionFailed {
            source_id: Some(source_id),
            error,
        } if source_id == unsupported.source.id
            && error == "Saved source type is no longer supported."
    ));
    assert_eq!(
        current_active_source(&controller.active_source)
            .expect("active source")
            .identity
            .id,
        active.source.id
    );
}

#[test]
pub(in crate::controller) fn startup_config_token_keeps_saved_remote_active() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = saved_source();
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source.id)
        })
        .expect("save server");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
    secrets
        .save_token(&saved.source.id, "cached-session-token")
        .expect("save token");

    let snapshot = load_runtime_snapshot(&store, &secrets).expect("load runtime snapshot");

    assert!(!snapshot.first_run);
    assert_eq!(
        snapshot.source.as_ref().map(|server| server.id.clone()),
        Some(saved.source.id.clone())
    );
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Source(saved.source.id))
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
        snapshot.source.expect("server").id.as_str(),
        LOCAL_SOURCE_IDENTITY_ID
    );
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn startup_add_syncs() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = saved_source();
    let root = unique_test_dir("add-local-folder-select-source");
    fs::create_dir_all(&root).expect("create root");
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Source(saved.source.id.clone()));
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source.id)
        })
        .expect("save server");
    let (controller, events) = controller_from_store_for_test(store);
    controller.add_local_library_folder(root.clone());
    let queue = wait_for_queue(&events).expect("local queue");
    assert_eq!(queue.source_id.as_str(), LOCAL_SOURCE_IDENTITY_ID);
    let snapshot = wait_for_snapshot(&events);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Local)
    );
    assert_eq!(snapshot.local_folders.len(), 1);
    let active = controller
        .store
        .with_store(|store| store.active_source())
        .expect("active server")
        .expect("active server");
    assert_eq!(active.source.id.as_str(), LOCAL_SOURCE_IDENTITY_ID);
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn startup_reuse_cache() {
    let store = StoreHandle::open_memory().expect("memory store");
    let remote = saved_source();
    let local = local_source_saved();
    let root = unique_test_dir("stale-local-source-selection");
    fs::create_dir_all(&root).expect("create root");
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Source(remote.source.id.clone()));
    settings.sources.local_folders = vec![LocalLibraryFolder {
        path: root.to_string_lossy().into_owned(),
    }];
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&remote)?;
            store.save_source(&local)?;
            store.set_active_source(&remote.source.id)?;
            let generation = store.begin_sync(&local.source.id)?;
            commit_cached_library(
                store,
                &local.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![local_album_with_image_ref(ImageRef::new(
                        "local:cover:file%3A%2F%2Fmissing-cover",
                        None,
                    ))],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed local cache");
    let (controller, events) = controller_from_store_for_test(store);

    controller.select_source(LibrarySourceSelection::Local);

    let queue = wait_for_queue(&events).expect("local queue");
    assert_eq!(queue.source_id.as_str(), LOCAL_SOURCE_IDENTITY_ID);
    let snapshot = wait_for_snapshot(&events);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Local)
    );
    assert_eq!(snapshot.cached_album_count, 1);
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn startup_disk_store_waits_for_short_write_lock() {
    let (store, store_root) = disk_store_for_test("startup-disk-lock");
    let local = local_source_saved();
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.set_active_source(&local.source.id)
        })
        .expect("seed active server");
    let database_path = disk_store_database_path(&store);
    let lock = Connection::open(database_path).expect("open lock connection");
    lock.execute_batch("BEGIN IMMEDIATE")
        .expect("hold write lock");
    let writer = {
        let store = store.clone();
        let source_id = local.source.id.clone();
        thread::spawn(move || store.with_store(|store| store.begin_sync(&source_id)))
    };

    thread::sleep(Duration::from_millis(50));
    lock.execute_batch("COMMIT").expect("release write lock");

    let generation = writer
        .join()
        .expect("join writer")
        .expect("begin sync after lock release");
    assert_eq!(generation, 1);
    assert_eq!(generation, 1);
    let _cleanup = fs::remove_dir_all(store_root);
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
            store.save_source(&local)?;
            store.set_active_source(&local.source.id)
        })
        .expect("save local server");
    let runtime = Runtime::new().expect("runtime");
    let (events, _receiver) = channel();
    let cold = LocalSource::from_roots_with_identity(vec![root.clone()], local.source.clone())
        .expect("cold local provider");
    runtime
        .block_on(sync_local_source_with_events(
            &store,
            &local.source.id,
            &cold,
            events.clone(),
        ))
        .expect("cold local sync");
    assert_eq!(
        store
            .with_store(|store| store.load_tracks(&local.source.id, 0, 10))
            .expect("cold tracks")
            .total,
        2
    );
    let (seed_generation, seed_revision) = store
        .with_store(|store| {
            Ok((
                store.begin_sync(&local.source.id)?,
                store.source_cache_revision(&local.source.id)?,
            ))
        })
        .expect("begin manifest seed");
    let mut committed_manifest = store
        .with_store(|store| store.load_local_manifest(&local.source.id))
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
            store.upsert_tracks(&local.source.id, &committed_tracks, seed_generation)?;
            let current_album_ids = store
                .load_albums(&local.source.id, 0, 100)?
                .items
                .into_iter()
                .map(|album| album.id)
                .collect();
            let current_artist_ids = store
                .load_artists(&local.source.id, false, 0, 100)?
                .items
                .into_iter()
                .map(|artist| artist.id)
                .collect();
            let current_album_artist_ids = store
                .load_artists(&local.source.id, true, 0, 100)?
                .items
                .into_iter()
                .map(|artist| artist.id)
                .collect();
            let current_genre_ids = store
                .load_genres(&local.source.id, 0, 100)?
                .items
                .into_iter()
                .map(|genre| genre.id)
                .collect();
            store
                .commit_local_library_delta(
                    &local.source.id,
                    seed_generation,
                    seed_revision,
                    true,
                    library::LocalLibraryDelta {
                        current_album_ids,
                        current_artist_ids,
                        current_album_artist_ids,
                        current_genre_ids,
                        manifest: library::LocalManifestDelta {
                            upserted_entries: committed_manifest.clone(),
                            ..library::LocalManifestDelta::default()
                        },
                        ..library::LocalLibraryDelta::default()
                    },
                )
                .map(|_| ())
        })
        .expect("seed committed genres");
    fs::remove_file(&removed).expect("remove audio");
    let manifest = store
        .with_store(|store| store.load_local_manifest(&local.source.id))
        .expect("manifest");
    let warm = LocalSource::from_roots_with_manifest_cache(
        vec![root.clone()],
        local.source.clone(),
        manifest,
    )
    .expect("warm local provider");
    assert_eq!(warm.manifest_scan().entries.len(), 1);
    assert_eq!(warm.manifest_scan().deleted_track_ids.len(), 1);

    runtime
        .block_on(sync_local_source_with_events(
            &store,
            &local.source.id,
            &warm,
            events,
        ))
        .expect("warm local sync");

    let tracks = store
        .with_store(|store| store.load_tracks(&local.source.id, 0, 10))
        .expect("warm tracks");
    assert_eq!(tracks.total, 1);
    assert_eq!(tracks.items[0].title, "Kept");
    let retained_path = store
        .with_store(|store| store.track_local_path(&local.source.id, &tracks.items[0].id))
        .expect("retained path");
    assert_eq!(
        retained_path.as_deref(),
        Some(kept.to_string_lossy().as_ref())
    );
    let genres = store
        .with_store(|store| store.load_genres(&local.source.id, 0, 10))
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
pub(in crate::controller) fn local_sync_delta_matches_manifest_change() {
    for changed in [false, true] {
        let label = if changed {
            "local-changed-post-sync"
        } else {
            "local-noop-post-sync"
        };
        let (store, local, root, _generation) = seed_cached_local_source(label);
        if changed {
            fs::write(root.join("Artist").join("Album").join("Second.mp3"), []).expect("audio");
        }
        let manifest = store
            .with_store(|store| store.load_local_manifest(&local.source.id))
            .expect("manifest");
        let warm = LocalSource::from_roots_with_manifest_cache(
            vec![root.clone()],
            local.source.clone(),
            manifest,
        )
        .expect("warm local provider");
        assert_eq!(warm.manifest_scan().library_changed, changed, "{label}");
        let runtime = Runtime::new().expect("runtime");
        let revision = store
            .with_store(|store| store.source_cache_revision(&local.source.id))
            .expect("cache revision");

        let outcome = runtime
            .block_on(sync_local_source_outcome(&store, &local.source.id, &warm))
            .expect("local sync");

        assert_eq!(outcome.delta.is_empty(), !changed, "{label}");
        assert_eq!(
            store
                .with_store(|store| store.source_cache_revision(&local.source.id))
                .expect("updated cache revision"),
            revision + 1,
            "{label}"
        );
        let _cleanup = fs::remove_dir_all(root);
    }
}

#[test]
pub(in crate::controller) fn local_object_scope_ignores_unrelated_path_and_commits_audio() {
    let (store, local, root, _generation) = seed_cached_local_source("local-object-scope");
    let manifest = store
        .with_store(|store| store.load_local_manifest(&local.source.id))
        .expect("cached manifest");
    let roots: crate::sources::LocalRootsLoader = {
        let root = root.clone();
        Arc::new(move || vec![root.clone()])
    };
    let full_scan_used = Arc::new(AtomicBool::new(false));
    let load: crate::sources::LocalLoader = {
        let root = root.clone();
        let identity = local.source.clone();
        let full_scan_used = Arc::clone(&full_scan_used);
        Arc::new(move |progress, cancellation| {
            full_scan_used.store(true, Ordering::Relaxed);
            LocalSource::from_roots_with_manifest_scan(
                vec![root.clone()],
                identity.clone(),
                manifest.clone(),
                progress,
                || cancellation.is_cancelled(),
            )
        })
    };
    let sync = local_sync_operation(local.source.id.clone(), local.source.clone(), load, roots);
    let runtime = Runtime::new().expect("runtime");
    let cancellation = library_sync::CancellationToken::new();
    let mut progress = |_| {};
    let revision = store
        .with_store(|store| store.source_cache_revision(&local.source.id))
        .expect("cache revision");
    let unrelated = root.join("notes.txt");
    fs::write(&unrelated, "not library data").expect("unrelated file");
    let generation = store
        .with_store(|store| store.begin_sync(&local.source.id))
        .expect("begin unrelated object sync");
    let scope = library_sync::ReconcileScope::objects(SourceObjectChanges::new([unrelated
        .to_string_lossy()
        .into_owned()]));

    let ignored = sync(
        &store,
        &runtime,
        &scope,
        generation,
        &mut progress,
        &cancellation,
    )
    .expect("sync unrelated path");

    assert_eq!(ignored, library_sync::SyncOutcome::Ignored);
    assert!(!full_scan_used.load(Ordering::Relaxed));
    assert_eq!(
        store
            .with_store(|store| store.source_cache_revision(&local.source.id))
            .expect("unchanged cache revision"),
        revision
    );

    let added = root.join("Artist").join("Album").join("Second.mp3");
    fs::write(&added, []).expect("added audio");
    let generation = store
        .with_store(|store| store.begin_sync(&local.source.id))
        .expect("begin audio object sync");
    let scope = library_sync::ReconcileScope::objects(SourceObjectChanges::new([added
        .to_string_lossy()
        .into_owned()]));

    let outcome = sync(
        &store,
        &runtime,
        &scope,
        generation,
        &mut progress,
        &cancellation,
    )
    .expect("sync changed path");
    let library_sync::SyncOutcome::Committed(commit) = outcome else {
        panic!("expected local commit");
    };

    assert!(!full_scan_used.load(Ordering::Relaxed));
    assert_eq!(commit.delta.tracks.added.len(), 1);
    let tracks = store
        .with_store(|store| store.load_tracks(&local.source.id, 0, 10))
        .expect("load local tracks");
    assert_eq!(tracks.total, 2);
    assert!(tracks.items.iter().any(|track| track.title == "Second"));
    let _cleanup = fs::remove_dir_all(root);
}

fn seed_cached_local_source(label: &str) -> (StoreHandle, SavedSource, PathBuf, i64) {
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
            store.save_source(&local)?;
            store.set_active_source(&local.source.id)
        })
        .expect("save local server");
    let runtime = Runtime::new().expect("runtime");
    let (seed_events, _seed_receiver) = channel();
    let cold = LocalSource::from_roots_with_identity(vec![root.clone()], local.source.clone())
        .expect("cold local provider");
    runtime
        .block_on(sync_local_source_with_events(
            &store,
            &local.source.id,
            &cold,
            seed_events,
        ))
        .expect("cold local sync");
    let generation = store
        .with_store(|store| {
            store
                .sync_state(&local.source.id)
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
            store.save_source(&local)?;
            store.set_active_source(&local.source.id)
        })
        .expect("save local server");
    let runtime = Runtime::new().expect("runtime");
    let (events, _receiver) = channel();
    let cold = LocalSource::from_roots_with_identity(vec![root.clone()], local.source.clone())
        .expect("cold local provider");
    runtime
        .block_on(sync_local_source_with_events(
            &store,
            &local.source.id,
            &cold,
            events.clone(),
        ))
        .expect("cold local sync");
    let cold_tag = store
        .with_store(|store| store.load_artists(&local.source.id, false, 0, 10))
        .expect("cold artists")
        .items
        .into_iter()
        .next()
        .and_then(|artist| artist.image_ref.and_then(|image_ref| image_ref.tag))
        .expect("cold artist tag");
    fs::write(&artist_image, [1_u8, 2_u8]).expect("replace artist image");
    let manifest = store
        .with_store(|store| store.load_local_manifest(&local.source.id))
        .expect("manifest");
    let warm = LocalSource::from_roots_with_manifest_cache(
        vec![root.clone()],
        local.source.clone(),
        manifest,
    )
    .expect("warm local provider");
    assert!(!warm.manifest_scan().library_changed);

    runtime
        .block_on(sync_local_source_with_events(
            &store,
            &local.source.id,
            &warm,
            events,
        ))
        .expect("warm local sync");

    let warm_tag = store
        .with_store(|store| store.load_artists(&local.source.id, false, 0, 10))
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
    let saved = saved_source();
    let root = unique_test_dir("remove-local-folder-preserve-source");
    fs::create_dir_all(&root).expect("create root");
    let path = root.to_string_lossy().into_owned();
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Source(saved.source.id.clone()));
    settings.sources.local_folders = vec![LocalLibraryFolder { path: path.clone() }];
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source.id)
        })
        .expect("save server");
    let (controller, events) = controller_from_store_for_test(store);
    controller.remove_local_library_folder(path);
    let snapshot = wait_for_snapshot(&events);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Source(saved.source.id.clone()))
    );
    assert!(snapshot.local_folders.is_empty());
    let active = controller
        .store
        .with_store(|store| store.active_source())
        .expect("active server")
        .expect("active server");
    assert_eq!(active.source.id, saved.source.id);
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn removing_final_selected_local_root_deactivates_source() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let remote = saved_source();
    let root = unique_test_dir("remove-final-selected-local-root");
    fs::create_dir_all(&root).expect("create root");
    let path = root.to_string_lossy().into_owned();
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    settings.sources.local_folders = vec![LocalLibraryFolder { path: path.clone() }];
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.save_source(&remote)?;
            store.set_active_source(&local.source.id)
        })
        .expect("save local source");
    let (controller, events) = controller_from_store_for_test(store);
    *controller.queue.lock().expect("queue") = Some(QueueEngine::new(local.source.id.clone()));

    controller.remove_local_library_folder(path);

    let snapshot = wait_for_snapshot(&events);
    assert_eq!(snapshot.selected_source, None);
    assert_eq!(snapshot.sources, vec![remote.source]);
    assert!(snapshot.local_folders.is_empty());
    assert!(
        controller
            .store
            .with_store(|store| store.active_source())
            .expect("active source")
            .is_none()
    );
    assert!(current_active_source(&controller.active_source).is_none());
    assert!(controller.queue.lock().expect("queue").is_none());
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn newer_local_removal_supersedes_pending_source_selection() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let remote = saved_source();
    let root = unique_test_dir("source-transition-local-removal");
    fs::create_dir_all(&root).expect("create root");
    let path = root.to_string_lossy().into_owned();
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    settings.sources.local_folders = vec![LocalLibraryFolder { path: path.clone() }];
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.save_source(&remote)?;
            store.set_active_source(&local.source.id)
        })
        .expect("save sources");
    let (controller, events) = controller_from_store_for_test(store);
    controller
        .secrets
        .save_token(&remote.source.id, "token")
        .expect("save token");

    let transition_lock = controller
        .source_transitions
        .commit
        .lock()
        .expect("transition lock");
    controller.select_source(LibrarySourceSelection::Source(remote.source.id.clone()));
    controller.remove_local_library_folder(path);
    drop(transition_lock);

    let snapshot = wait_for_snapshot(&events);
    assert_eq!(snapshot.selected_source, None);
    assert_eq!(snapshot.sources, vec![remote.source]);
    assert!(
        controller
            .store
            .with_store(|store| store.active_source())
            .expect("active source")
            .is_none()
    );
    assert!(current_active_source(&controller.active_source).is_none());
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn startup_removing_cache() {
    let store = StoreHandle::open_memory().expect("memory store");
    let remote = saved_source();
    let local = local_source_saved();
    let first = unique_test_dir("remove-inactive-local-first");
    let second = unique_test_dir("remove-inactive-local-second");
    fs::create_dir_all(&first).expect("create first root");
    fs::create_dir_all(&second).expect("create second root");
    fs::write(first.join("Removed.mp3"), []).expect("first audio");
    fs::write(second.join("Remaining.mp3"), []).expect("second audio");
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Source(remote.source.id.clone()));
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
            store.save_source(&remote)?;
            store.save_source(&local)?;
            store.set_active_source(&remote.source.id)
        })
        .expect("save servers");
    let runtime = Runtime::new().expect("runtime");
    let (seed_events, _seed_receiver) = channel();
    let cold = LocalSource::from_roots_with_identity(
        vec![first.clone(), second.clone()],
        local.source.clone(),
    )
    .expect("cold local provider");
    runtime
        .block_on(sync_local_source_with_events(
            &store,
            &local.source.id,
            &cold,
            seed_events,
        ))
        .expect("seed local sync");
    assert_eq!(
        store
            .with_store(|store| store.load_tracks(&local.source.id, 0, 10))
            .expect("seed tracks")
            .total,
        2
    );
    let (controller, events) = controller_from_store_for_test(store);

    controller.remove_local_library_folder(first.to_string_lossy().into_owned());

    let snapshot = wait_for_snapshot(&events);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Source(remote.source.id.clone()))
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let total = controller
            .store
            .with_store(|store| store.load_tracks(&local.source.id, 0, 10))
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
        .with_store(|store| store.load_tracks(&local.source.id, 0, 10))
        .expect("remaining tracks");
    assert_eq!(tracks.total, 1);
    assert_eq!(tracks.items[0].title, "Remaining");
    let active = controller
        .store
        .with_store(|store| store.active_source())
        .expect("active server")
        .expect("active server");
    assert_eq!(active.source.id, remote.source.id);
    let _cleanup_first = fs::remove_dir_all(first);
    let _cleanup_second = fs::remove_dir_all(second);
}

#[test]
pub(in crate::controller) fn inactive_manual_sync_failure_keeps_the_active_source() {
    let store = StoreHandle::open_memory().expect("memory store");
    let remote = saved_source();
    let failing = SavedSource {
        source: SourceIdentity {
            id: SourceId::new("unsupported:inactive-sync"),
            kind: "unsupported".to_string(),
            name: "Inactive Sync".to_string(),
            base_url: "https://music.example".to_string(),
        },
        user_id: "user".to_string(),
        username: "demo".to_string(),
        trust_invalid_cert: false,
        use_jellyfin_instant_mix: false,
    };
    store
        .with_store(|store| {
            store.save_source(&remote)?;
            store.save_source(&failing)?;
            store.set_active_source(&remote.source.id)
        })
        .expect("seed sources");
    let (controller, events) = controller_from_store_for_test(store);
    controller.request_manual_source_sync(failing.source.id.clone());
    let running = wait_for_source_sync_change(&events, &failing.source.id);
    assert_eq!(running.phase, library_sync::SyncPhase::Running);
    assert!(running.manual);
    let failed = wait_for_source_sync_change(&events, &failing.source.id);
    assert_eq!(failed.epoch, running.epoch);
    assert_eq!(failed.phase, library_sync::SyncPhase::Failed);
    assert!(failed.manual);
    let active = controller
        .store
        .with_store(|store| store.active_source())
        .expect("active source")
        .expect("active source");
    assert_eq!(active.source.id, remote.source.id);
}
#[test]
pub(in crate::controller) fn failed_manual_sync_keeps_the_cache() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = saved_source();
    let album = remote_album_with_image_ref(ImageRef::new("album:one", Some("tag".to_string())));
    let track = local_track_with_image_ref(
        1,
        &album,
        ImageRef::new("track:one", Some("tag".to_string())),
    );
    seed_cached_library(&store, &saved, &[album], &[track], &[]);
    let revision = store
        .with_store(|store| store.source_cache_revision(&saved.source.id))
        .expect("cache revision");
    let cached = store
        .with_store(|store| store.load_tracks(&saved.source.id, 0, 10))
        .expect("cached tracks");
    let (controller, events) = controller_from_store_for_test(store);
    *controller.active_source.write().expect("active source") = None;
    controller.request_manual_source_sync(saved.source.id.clone());
    let failure = wait_for_sync_failure(&events, &saved.source.id);
    assert_eq!(failure, "No saved token found for the active server.");
    assert_eq!(
        controller
            .store
            .with_store(|store| store.source_cache_revision(&saved.source.id))
            .expect("final cache revision"),
        revision
    );
    assert_eq!(
        controller
            .store
            .with_store(|store| store.load_tracks(&saved.source.id, 0, 10))
            .expect("final cached tracks"),
        cached
    );
}

fn wait_for_library_commit(
    events: &std::sync::mpsc::Receiver<ControllerEvent>,
    source_id: &SourceId,
) -> Box<LibraryCommitUpdate> {
    loop {
        match events
            .recv_timeout(Duration::from_secs(5))
            .expect("controller event")
        {
            ControllerEvent::LibraryCommitted(update) if update.commit.source_id == *source_id => {
                return update;
            }
            ControllerEvent::SourceSyncChanged(state)
                if state.source_id == *source_id
                    && state.phase == library_sync::SyncPhase::Failed =>
            {
                panic!("source sync failed: {:?}", state.failure)
            }
            ControllerEvent::Error(error) => panic!("controller error: {error}"),
            _ => {}
        }
    }
}
fn wait_for_sync_failure(
    events: &std::sync::mpsc::Receiver<ControllerEvent>,
    source_id: &SourceId,
) -> String {
    loop {
        match events
            .recv_timeout(Duration::from_secs(5))
            .expect("controller event")
        {
            ControllerEvent::SourceSyncChanged(state)
                if state.source_id == *source_id
                    && state.phase == library_sync::SyncPhase::Failed =>
            {
                return state.failure.expect("typed sync failure");
            }
            ControllerEvent::LibraryCommitted(update) if update.commit.source_id == *source_id => {
                panic!("source sync unexpectedly committed")
            }
            ControllerEvent::Error(error) => panic!("controller error: {error}"),
            _ => {}
        }
    }
}

fn wait_for_source_sync_change(
    events: &std::sync::mpsc::Receiver<ControllerEvent>,
    source_id: &SourceId,
) -> library_sync::SourceSyncChanged {
    loop {
        match events
            .recv_timeout(Duration::from_secs(5))
            .expect("controller event")
        {
            ControllerEvent::SourceSyncChanged(state) if state.source_id == *source_id => {
                return state;
            }
            ControllerEvent::LibraryCommitted(update) if update.commit.source_id == *source_id => {
                panic!("source sync unexpectedly committed")
            }
            ControllerEvent::Error(error) => panic!("controller error: {error}"),
            _ => {}
        }
    }
}

#[test]
pub(in crate::controller) fn startup_emit_status() {
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let source_id = SourceId::new("server:unchanged");
    controller
        .store
        .with_store(|store| {
            store.save_source(&SavedSource {
                source: SourceIdentity {
                    id: source_id.clone(),
                    kind: "jellyfin".to_string(),
                    name: "Saved server".to_string(),
                    base_url: "http://server.example.test".to_string(),
                },
                user_id: "user-id".to_string(),
                username: "listener".to_string(),
                trust_invalid_cert: false,
                use_jellyfin_instant_mix: false,
            })?;
            store.set_active_source(&source_id)
        })
        .expect("save server");
    controller
        .secrets
        .save_token(&source_id, "test-token")
        .expect("save token");
    crate::sources::update_jellyfin_settings(
        &controller,
        crate::sources::JellyfinSettingsInput {
            credentials: crate::sources::CredentialSettingsInput {
                source_id,
                name: "Saved server".to_string(),
                base_url: "http://server.example.test".to_string(),
                username: "listener".to_string(),
                password: String::new(),
                trust_invalid_cert: false,
            },
            use_instant_mix: false,
        },
    );

    assert_eq!(wait_for_notice(&events), SourceNotice::NoChanges);
}
#[test]
pub(in crate::controller) fn active_local_sync_updates_manifest_delta() {
    let (store, local, root, _generation) = seed_cached_local_source("local-active-sync");
    let album_dir = root.join("Artist").join("Album");
    fs::write(album_dir.join("Second.mp3"), []).expect("audio");
    let (controller, events) = controller_from_store_for_test(store);
    controller.request_manual_source_sync(local.source.id.clone());
    let update = wait_for_library_commit(&events, &local.source.id);
    assert!(!update.commit.delta.tracks.added.is_empty());
    let Some(Ok(LibraryCommitProjection::Current { counts, .. })) = update.projection else {
        panic!("expected current library projection")
    };
    assert_eq!(counts.tracks, 2);
    let tracks = controller
        .store
        .with_store(|store| store.load_tracks(&local.source.id, 0, 10))
        .expect("load local tracks")
        .items;
    assert_eq!(tracks.len(), 2);
    let _cleanup = fs::remove_dir_all(root);
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

    for track in &snapshot.tracks {
        let expected = tracks
            .iter()
            .find(|candidate| candidate.id == track.id)
            .expect("snapshot track");
        assert_eq!(track.image_ref.as_ref(), expected.image_ref.as_ref());
    }
    assert_eq!(
        snapshot.home_sections[0].tracks[0].image_ref.as_ref(),
        tracks[0].image_ref.as_ref()
    );
    assert_eq!(
        snapshot.favorites[0].image_ref.as_ref(),
        tracks[0].image_ref.as_ref()
    );
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

    for track in &page.items {
        let expected = tracks
            .iter()
            .find(|candidate| candidate.id == track.id)
            .expect("cached track");
        assert_eq!(track.image_ref.as_ref(), expected.image_ref.as_ref());
    }
}

#[test]
pub(in crate::controller) fn auto_dj_candidate() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = local_source_saved();
    let artist_id = ArtistId::new("local:artist:auto-dj");
    let tracks = (1..=7)
        .map(|number| {
            let mut track = library_track(
                number,
                Some(artist_id.clone()),
                AlbumId::new(format!("local:album:{number}")),
                "Auto DJ Artist",
                &[],
            );
            track.id = TrackId::new(format!("local:track:{number}"));
            track.image_ref = Some(ImageRef::new(
                format!("local:cover:embedded%3A%2Fmusic%2F{number}.flac"),
                None,
            ));
            track.local_path = Some(format!("/music/{number}.flac"));
            track.source_format = Some("flac".to_string());
            track
        })
        .collect::<Vec<_>>();
    seed_cached_library(&store, &saved, &[], &tracks, &[]);
    let (controller, _events) = controller_from_store_for_test(store);
    let mut queue = QueueEngine::new(saved.source.id);
    queue.play_now(&tracks[0]);
    *controller.queue.lock().expect("queue") = Some(queue);
    *controller.auto_dj_enabled.lock().expect("auto dj") = true;

    assert!(controller.auto_dj_topup());

    let queue = controller.queue.lock().expect("queue");
    let queue = queue.as_ref().expect("queue");
    assert_eq!(queue.entries().len(), 1 + super::AUTO_DJ_ITEM_COUNT);
    for entry in queue.entries().iter().skip(1) {
        let track = tracks
            .iter()
            .find(|track| track.id == entry.track_id)
            .expect("auto dj track");
        assert_eq!(entry.image_ref.as_ref(), track.image_ref.as_ref());
        assert!(matches!(
            entry.origin.as_ref(),
            Some(domain::QueueEntryOrigin::AutoDj { .. })
        ));
    }
}

#[test]
pub(in crate::controller) fn auto_dj_falls_back_to_random_when_radio_is_empty() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = local_source_saved();
    let tracks = (1..=7)
        .map(|number| {
            let mut track = library_track(
                number,
                Some(ArtistId::new(format!("local:artist:{number}"))),
                AlbumId::new(format!("local:album:{number}")),
                &format!("Artist {number}"),
                &[],
            );
            track.id = TrackId::new(format!("local:track:{number}"));
            track.local_path = Some(format!("/music/{number}.flac"));
            track.source_format = Some("flac".to_string());
            track
        })
        .collect::<Vec<_>>();
    seed_cached_library(&store, &saved, &[], &tracks, &[]);
    let (controller, _events) = controller_from_store_for_test(store);
    let mut queue = QueueEngine::new(saved.source.id.clone());
    queue.play_now(&tracks[0]);
    *controller.queue.lock().expect("queue") = Some(queue);
    *controller.auto_dj_enabled.lock().expect("auto dj") = true;

    assert!(controller.auto_dj_topup());

    let queue = controller.queue.lock().expect("queue");
    let queue = queue.as_ref().expect("queue");
    assert_eq!(queue.entries().len(), 1 + super::AUTO_DJ_ITEM_COUNT);
    let appended = queue
        .entries()
        .iter()
        .skip(1)
        .map(|entry| entry.track_id.clone())
        .collect::<HashSet<_>>();
    assert_eq!(appended.len(), super::AUTO_DJ_ITEM_COUNT);
    assert!(!appended.contains(&tracks[0].id));
    let fallback_ids = tracks
        .iter()
        .skip(1)
        .map(|track| track.id.clone())
        .collect::<HashSet<_>>();
    assert!(appended.is_subset(&fallback_ids));
    assert!(queue.entries().iter().skip(1).all(|entry| matches!(
        entry.origin.as_ref(),
        Some(domain::QueueEntryOrigin::AutoDj { .. })
    )));
}

#[test]
pub(in crate::controller) fn restored_queue_reuse() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let album_image_ref = ImageRef::new("local:cover:file%3A%2F%2Falbum-cover", None);
    let album = local_album_with_image_ref(album_image_ref.clone());
    let track_image_ref = ImageRef::new("local:cover:embedded%3A%2Fmusic%2Fone.flac", None);
    let track = local_track_with_image_ref(1, &album, track_image_ref.clone());
    seed_cached_library(
        &store,
        &local,
        std::slice::from_ref(&album),
        std::slice::from_ref(&track),
        &[],
    );
    store
        .with_store(|store| {
            let mut queue = QueueEngine::new(local.source.id.clone());
            queue.play_now(&track);
            store.save_queue_snapshot(&queue.snapshot())
        })
        .expect("seed local queue");

    let restored = restore_queue(&store, Some(&local.source)).expect("restore queue");
    let queue = restored.snapshot();
    assert_eq!(queue.entries[0].image_ref.as_ref(), Some(&track_image_ref));

    let playback =
        playback_snapshot_from_queue(Some(&restored), false, &PlaybackSettings::default());
    assert_eq!(
        playback.current.expect("current").image_ref.as_ref(),
        Some(&track_image_ref)
    );
}

#[test]
pub(in crate::controller) fn restored_queue_uses_canonical_external_album_ref() {
    let store = StoreHandle::open_memory().expect("memory store");
    let remote = saved_source();
    let album_ref = external_metadata::external_album_image_ref(
        "未来古代楽団",
        "忘れじの言の葉/エデンの揺り籃",
    )
    .expect("album ref");
    let weak_ref = external_metadata::external_album_image_ref(
        "未来古代楽団, 安次嶺希和子",
        "忘れじの言の葉/エデンの揺り籃",
    )
    .expect("track artist ref");
    let mut album = remote_album_with_image_ref(album_ref.clone());
    album.title = "忘れじの言の葉/エデンの揺り籃".to_string();
    album.artist = "未来古代楽団".to_string();
    let mut first = library_track(
        1,
        Some(ArtistId::new("jellyfin:artist:first")),
        album.id.clone(),
        "未来古代楽団, 安次嶺希和子",
        &[],
    );
    first.title = "忘れじの言の葉".to_string();
    first.album = album.title.clone();
    first.image_ref = Some(weak_ref);
    let mut next = library_track(
        2,
        Some(ArtistId::new("jellyfin:artist:album")),
        album.id.clone(),
        "未来古代楽団",
        &[],
    );
    next.title = "エデンの揺り籃".to_string();
    next.album = album.title.clone();
    next.image_ref = Some(album_ref.clone());
    let mut settings = AppSettings {
        external_metadata_enabled: true,
        ..AppSettings::default()
    };
    settings.sources.selected = Some(LibrarySourceSelection::Source(remote.source.id.clone()));
    store.save_settings(&settings).expect("save settings");
    seed_cached_library(
        &store,
        &remote,
        std::slice::from_ref(&album),
        &[first.clone(), next],
        &[],
    );
    store
        .with_store(|store| {
            let mut queue = QueueEngine::new(remote.source.id.clone());
            queue.play_now(&first);
            store.save_queue_snapshot(&queue.snapshot())
        })
        .expect("seed remote queue");

    let restored = restore_queue(&store, Some(&remote.source)).expect("restore queue");
    let queue = restored.snapshot();
    assert_eq!(queue.entries[0].image_ref.as_ref(), Some(&album_ref));

    let playback =
        playback_snapshot_from_queue(Some(&restored), false, &PlaybackSettings::default());
    assert_eq!(
        playback.current.expect("current").image_ref.as_ref(),
        Some(&album_ref)
    );
}

#[test]
pub(in crate::controller) fn restored_queue_refreshes_stale_ref_from_track_projection() {
    let store = StoreHandle::open_memory().expect("memory store");
    let remote = saved_source();
    let album_ref = ImageRef::new(
        "navidrome:cover:al-album-one",
        Some("album-tag".to_string()),
    );
    let stale_queue_ref = ImageRef::new(
        "navidrome:cover:mf-stale-track-one",
        Some("stale-tag".to_string()),
    );
    let album = remote_album_with_image_ref(album_ref.clone());
    let mut track = library_track(
        1,
        Some(ArtistId::new("jellyfin:artist:one")),
        album.id.clone(),
        "Example Artist",
        &[],
    );
    track.album = album.title.clone();
    track.image_ref = Some(album_ref.clone());
    seed_cached_library(
        &store,
        &remote,
        std::slice::from_ref(&album),
        std::slice::from_ref(&track),
        &[],
    );
    store
        .with_store(|store| {
            let mut queue = QueueEngine::new(remote.source.id.clone());
            queue.play_now(&track);
            let mut snapshot = queue.snapshot();
            snapshot.entries[0].image_ref = Some(stale_queue_ref);
            store.save_queue_snapshot(&snapshot)
        })
        .expect("seed stale remote queue");

    let restored = restore_queue(&store, Some(&remote.source)).expect("restore queue");
    let queue = restored.snapshot();
    assert_eq!(queue.entries[0].image_ref.as_ref(), Some(&album_ref));

    let persisted = store
        .with_store(|store| store.load_queue_snapshot(&remote.source.id))
        .expect("load persisted queue")
        .expect("persisted queue");
    assert_eq!(persisted.entries[0].image_ref.as_ref(), Some(&album_ref));
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
    let mut queue = QueueEngine::new(local.source.id.clone());
    queue.play_now(&track);
    *controller.queue.lock().expect("queue") = Some(queue);
    controller.sync_playback_snapshot_from_queue();

    super::refresh_queue_refs(&controller, &local);

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
    let mut queue = QueueEngine::new(local.source.id.clone());
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

    super::snapshot_queue_refs(&controller, &local, original_snapshot);

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
    let remote = saved_source();
    let local_image_ref = ImageRef::new("local:cover:embedded%3A%2Fmusic%2Ftrack.flac", None);
    let remote_album = remote_album_with_image_ref(local_image_ref);
    store
        .with_store(|store| {
            store.save_source(&remote)?;
            store.set_active_source(&remote.source.id)?;
            let generation = store.begin_sync(&remote.source.id)?;
            commit_cached_library(
                store,
                &remote.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![remote_album.clone()],
                    home_sections: vec![HomeSection {
                        kind: HomeSectionKind::Explore,
                        albums: vec![remote_album],
                        tracks: Vec::new(),
                    }],
                    ..CachedLibraryObservation::default()
                },
            )
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
            store.save_source(&local)?;
            store.set_active_source(&local.source.id)?;
            let generation = store.begin_sync(&local.source.id)?;
            commit_cached_library(
                store,
                &local.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![local_album.clone()],
                    home_sections: vec![HomeSection {
                        kind: HomeSectionKind::Explore,
                        albums: vec![local_album],
                        tracks: Vec::new(),
                    }],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed local cache");

    let snapshot = load_snapshot(&store).expect("load snapshot");

    assert_eq!(snapshot.albums.len(), 1);
    assert!(snapshot.albums[0].image_ref.is_none());
    assert_eq!(snapshot.home_sections.len(), 1);
    assert!(snapshot.home_sections[0].albums[0].image_ref.is_none());
}

#[test]
pub(in crate::controller) fn snapshot_keeps_cached_external_art_in_private_mode() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let external_image_ref = ImageRef::new("external:album:Example%20Artist:Example%20Album", None);
    let local_album = local_album_with_image_ref(external_image_ref.clone());
    let mut settings = AppSettings {
        external_metadata_enabled: true,
        private_mode: true,
        ..AppSettings::default()
    };
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.set_active_source(&local.source.id)?;
            let generation = store.begin_sync(&local.source.id)?;
            commit_cached_library(
                store,
                &local.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![local_album.clone()],
                    home_sections: vec![HomeSection {
                        kind: HomeSectionKind::Explore,
                        albums: vec![local_album],
                        tracks: Vec::new(),
                    }],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed local cache");

    let snapshot = load_snapshot(&store).expect("load snapshot");

    assert_eq!(snapshot.albums.len(), 1);
    assert_eq!(
        snapshot.albums[0].image_ref.as_ref(),
        Some(&external_image_ref)
    );
    assert_eq!(snapshot.home_sections.len(), 1);
    assert_eq!(
        snapshot.home_sections[0].albums[0].image_ref.as_ref(),
        Some(&external_image_ref)
    );
}

#[test]
pub(in crate::controller) fn local_routes_select_external_mbid_art() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let mut album =
        local_album_with_image_ref(ImageRef::new("local:cover:file%3A%2F%2Funused", None));
    album.image_ref = None;
    album.musicbrainz_release_group_id = Some("group-one".to_string());
    let mut track = library_track(
        1,
        Some(ArtistId::new("local:artist:one")),
        album.id.clone(),
        "Example Artist",
        &[],
    );
    track.id = TrackId::new("local:track:one");
    track.album = album.title.clone();
    let mut missing_identity_album = album.clone();
    missing_identity_album.id = AlbumId::new("local:album:no-mbid");
    missing_identity_album.title = "Unknown Album".to_string();
    missing_identity_album.artist = "Unknown Artist".to_string();
    missing_identity_album.musicbrainz_release_group_id = None;
    missing_identity_album.musicbrainz_album_id = None;
    let mut settings = AppSettings {
        external_metadata_enabled: true,
        ..AppSettings::default()
    };
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    seed_cached_library(
        &store,
        &local,
        &[album.clone(), missing_identity_album],
        std::slice::from_ref(&track),
        &[],
    );
    let expected = external_metadata::external_album_identity_image_ref(&album)
        .expect("external release-group ref");
    let (controller, _events) = controller_from_store_for_test(store);

    let albums = controller
        .cached_albums_page(0, 10)
        .expect("cached albums")
        .items;
    let selected_album = albums
        .iter()
        .find(|candidate| candidate.id == album.id)
        .expect("selected album");
    assert_eq!(selected_album.image_ref.as_ref(), Some(&expected));
    let missing_identity = albums
        .iter()
        .find(|candidate| candidate.id == AlbumId::new("local:album:no-mbid"))
        .expect("album without mbid");
    assert!(missing_identity.image_ref.is_none());

    let tracks = controller
        .cached_tracks_page(0, 10)
        .expect("cached tracks")
        .items;
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].image_ref.as_ref(), Some(&expected));
}

#[test]
pub(in crate::controller) fn local_artist_grids_use_detail_selected_mbid_fallback_art() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let mut album =
        local_album_with_image_ref(ImageRef::new("local:cover:file%3A%2F%2Funused", None));
    album.image_ref = None;
    album.musicbrainz_release_group_id = Some("group-one".to_string());
    let artist_id = ArtistId::new("local:artist:external-fallback");
    album.artist_id = Some(artist_id.clone());
    let mut track = library_track(
        1,
        Some(artist_id.clone()),
        album.id.clone(),
        "External Fallback Artist",
        &[],
    );
    track.id = TrackId::new("local:track:external-fallback");
    track.album = album.title.clone();
    let artist = domain::Artist {
        id: artist_id.clone(),
        name: "External Fallback Artist".to_string(),
        album_count: 1,
        track_count: 1,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        musicbrainz_artist_id: None,
        image_ref: None,
    };
    let mut settings = AppSettings {
        external_metadata_enabled: true,
        ..AppSettings::default()
    };
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.set_active_source(&local.source.id)?;
            let generation = store.begin_sync(&local.source.id)?;
            commit_cached_library(
                store,
                &local.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![album.clone()],
                    tracks: vec![track.clone()],
                    artists: vec![artist.clone()],
                    album_artists: vec![artist],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed local cache");
    let expected = external_metadata::external_album_identity_image_ref(&album)
        .expect("external release-group ref");
    let (controller, _events) = controller_from_store_for_test(store);

    let detail = controller
        .cached_artist_detail(&artist_id)
        .expect("artist detail")
        .expect("artist detail row");
    let artists = controller
        .cached_artists_page(false, 0, 10)
        .expect("cached artists")
        .items;
    let album_artists = controller
        .cached_artists_page(true, 0, 10)
        .expect("cached album artists")
        .items;

    assert_eq!(detail.artist.image_ref.as_ref(), Some(&expected));
    assert_eq!(artists.len(), 1);
    assert_eq!(artists[0].image_ref, detail.artist.image_ref);
    assert_eq!(album_artists.len(), 1);
    assert_eq!(album_artists[0].image_ref, detail.artist.image_ref);

    let snapshot = load_snapshot(&controller.store).expect("startup snapshot");
    assert_eq!(snapshot.artists.len(), 1);
    assert_eq!(snapshot.artists[0].image_ref, detail.artist.image_ref);
    assert_eq!(snapshot.album_artists.len(), 1);
    assert_eq!(snapshot.album_artists[0].image_ref, detail.artist.image_ref);
}

#[test]
pub(in crate::controller) fn local_genre_grid_uses_cached_mbid_album_fallback_art() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let mut album =
        local_album_with_image_ref(ImageRef::new("local:cover:file%3A%2F%2Funused", None));
    album.image_ref = None;
    album.musicbrainz_release_group_id = Some("441f9fa7-4c22-4b0f-a363-ba6fa6b04ded".to_string());
    album.genres = vec!["Example Genre".to_string()];
    let mut track = library_track(
        1,
        album.artist_id.clone(),
        album.id.clone(),
        "Example Artist",
        &["Example Genre"],
    );
    track.album = album.title.clone();
    let mut settings = AppSettings {
        external_metadata_enabled: true,
        ..AppSettings::default()
    };
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    let genre = Genre {
        id: GenreId::new("local:genre:example"),
        name: "Example Genre".to_string(),
        album_count: 1,
        track_count: 1,
        duration_seconds: 180,
        image_refs: Vec::new(),
        image_ref: None,
    };
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.set_active_source(&local.source.id)?;
            let generation = store.begin_sync(&local.source.id)?;
            commit_cached_library(
                store,
                &local.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![album.clone()],
                    tracks: vec![track],
                    genres: vec![genre],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed genre cache");
    let expected = external_metadata::external_album_identity_image_ref(&album)
        .expect("external release-group ref");
    let root = unique_test_dir("genre-mbid-cover-cache");
    fs::create_dir_all(&root).expect("create cache dir");
    let cover_path = root.join("cover.jpg");
    fs::write(&cover_path, [0xff_u8, 0xd8, 0xff, 0xd9]).expect("write cover");
    store
        .with_store(|store| {
            store.save_cover_cache_entry(&CoverCacheEntry {
                source_id: local.source.id.clone(),
                item_id: expected.item_id.clone(),
                image_tag: expected
                    .tag
                    .clone()
                    .unwrap_or_else(|| IMAGE_TAG_UNTAGGED.to_string()),
                size: 256,
                path: cover_path.to_string_lossy().to_string(),
            })
        })
        .expect("seed cover cache");
    let (controller, _events) = controller_from_store_for_test(store);

    let genres = controller
        .cached_genres_page(0, 10)
        .expect("cached genres")
        .items;
    let snapshot = load_snapshot(&controller.store).expect("startup snapshot");

    assert_eq!(genres.len(), 1);
    assert_eq!(genres[0].image_ref.as_ref(), Some(&expected));
    assert_eq!(snapshot.genres.len(), 1);
    assert_eq!(snapshot.genres[0].image_ref.as_ref(), Some(&expected));
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn local_source_art_wins_over_external_mbid_art() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let local_ref = ImageRef::new("local:cover:file%3A%2F%2Fsource-cover", None);
    let mut album = local_album_with_image_ref(local_ref.clone());
    album.musicbrainz_release_group_id = Some("group-one".to_string());
    let mut settings = AppSettings {
        external_metadata_enabled: true,
        ..AppSettings::default()
    };
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    seed_cached_library(&store, &local, std::slice::from_ref(&album), &[], &[]);
    let (controller, _events) = controller_from_store_for_test(store);

    let albums = controller
        .cached_albums_page(0, 10)
        .expect("cached albums")
        .items;

    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].image_ref.as_ref(), Some(&local_ref));
}

#[test]
pub(in crate::controller) fn album_artist_grid_uses_track_album_fallback_art() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let fallback_ref = ImageRef::new("local:cover:file%3A%2F%2Fcompilation-cover", None);
    let mut album = local_album_with_image_ref(fallback_ref.clone());
    album.id = AlbumId::new("local:album:compilation");
    album.title = "Compilation Album".to_string();
    album.artist = "Compilation Curator".to_string();
    album.artist_id = Some(ArtistId::new("local:artist:curator"));
    let artist_id = ArtistId::new("local:artist:guest");
    let mut track = library_track(
        1,
        Some(artist_id.clone()),
        album.id.clone(),
        "Guest Artist",
        &[],
    );
    track.id = TrackId::new("local:track:guest");
    track.album = album.title.clone();
    let album_artist = domain::Artist {
        id: artist_id,
        name: "Guest Artist".to_string(),
        album_count: 1,
        track_count: 1,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        musicbrainz_artist_id: None,
        image_ref: None,
    };
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.set_active_source(&local.source.id)?;
            let generation = store.begin_sync(&local.source.id)?;
            commit_cached_library(
                store,
                &local.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![album],
                    tracks: vec![track],
                    album_artists: vec![album_artist],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed local cache");
    let (controller, _events) = controller_from_store_for_test(store);

    let artists = controller
        .cached_artists_page(true, 0, 10)
        .expect("cached album artists")
        .items;

    let guest = artists
        .iter()
        .find(|artist| artist.name == "Guest Artist")
        .expect("guest album artist");
    assert_eq!(guest.image_ref.as_ref(), Some(&fallback_ref));
}

#[test]
pub(in crate::controller) fn album_projection_binds_track_fallback_art_before_route_read() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = SavedSource {
        source: SourceIdentity {
            id: SourceId::new("remote:server:album-binding"),
            kind: "subsonic".to_string(),
            name: "Remote Library".to_string(),
            base_url: "https://library.example.test".to_string(),
        },
        user_id: "user".to_string(),
        username: "listener".to_string(),
        trust_invalid_cert: false,
        use_jellyfin_instant_mix: false,
    };
    let fallback_ref = ImageRef::new(
        "remote:track:source-track-cover",
        Some("source-tag".to_string()),
    );
    let mut album = local_album_with_image_ref(fallback_ref.clone());
    album.id = AlbumId::new("remote:album:missing-source-cover");
    album.image_ref = None;
    let mut track = library_track(
        1,
        album.artist_id.clone(),
        album.id.clone(),
        "Representative Artist",
        &[],
    );
    track.id = TrackId::new("remote:track:representative");
    track.album = album.title.clone();
    track.image_ref = Some(fallback_ref.clone());
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source.id)?;
            let generation = store.begin_sync(&saved.source.id)?;
            commit_cached_library(
                store,
                &saved.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![album.clone()],
                    tracks: vec![track],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed remote cache");

    let raw_refs = store
        .with_store(|store| store.load_raw_album_image_refs(&saved.source.id))
        .expect("raw album refs");
    let selected = store
        .with_store(|store| store.load_albums(&saved.source.id, 0, 10))
        .expect("selected albums");

    assert_eq!(raw_refs.get(&album.id), Some(&None));
    assert_eq!(selected.items[0].image_ref.as_ref(), Some(&fallback_ref));
}

#[test]
pub(in crate::controller) fn genre_projection_uses_bound_album_track_fallback_art() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = SavedSource {
        source: SourceIdentity {
            id: SourceId::new("remote:server:genre-binding"),
            kind: "subsonic".to_string(),
            name: "Remote Library".to_string(),
            base_url: "https://library.example.test".to_string(),
        },
        user_id: "user".to_string(),
        username: "listener".to_string(),
        trust_invalid_cert: false,
        use_jellyfin_instant_mix: false,
    };
    let fallback_ref = ImageRef::new(
        "remote:track:genre-track-cover",
        Some("source-tag".to_string()),
    );
    let genre = Genre {
        id: GenreId::new("remote:genre:example"),
        name: "Example Genre".to_string(),
        album_count: 1,
        track_count: 1,
        duration_seconds: 180,
        image_refs: Vec::new(),
        image_ref: None,
    };
    let mut album = local_album_with_image_ref(fallback_ref.clone());
    album.id = AlbumId::new("remote:album:genre-binding");
    album.image_ref = None;
    album.genres = vec![genre.name.clone()];
    let mut track = library_track(
        1,
        album.artist_id.clone(),
        album.id.clone(),
        "Representative Artist",
        &[&genre.name],
    );
    track.id = TrackId::new("remote:track:genre-binding");
    track.album = album.title.clone();
    track.image_ref = Some(fallback_ref.clone());
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source.id)?;
            let generation = store.begin_sync(&saved.source.id)?;
            commit_cached_library(
                store,
                &saved.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![album],
                    tracks: vec![track],
                    genres: vec![genre],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed remote cache");

    let genres = store
        .with_store(|store| store.load_genres(&saved.source.id, 0, 10))
        .expect("load genres")
        .items;

    assert_eq!(genres.len(), 1);
    assert_eq!(genres[0].image_refs, vec![fallback_ref.clone()]);
    assert_eq!(genres[0].image_ref.as_ref(), Some(&fallback_ref));
}

#[test]
pub(in crate::controller) fn track_projection_binds_album_fallback_art_before_route_read() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = SavedSource {
        source: SourceIdentity {
            id: SourceId::new("remote:server:track-binding"),
            kind: "subsonic".to_string(),
            name: "Remote Library".to_string(),
            base_url: "https://library.example.test".to_string(),
        },
        user_id: "user".to_string(),
        username: "listener".to_string(),
        trust_invalid_cert: false,
        use_jellyfin_instant_mix: false,
    };
    let album_ref = ImageRef::new(
        "remote:album:source-album-cover",
        Some("source-tag".to_string()),
    );
    let mut album = local_album_with_image_ref(album_ref.clone());
    album.id = AlbumId::new("remote:album:track-binding");
    let mut track = library_track(
        1,
        album.artist_id.clone(),
        album.id.clone(),
        "Representative Artist",
        &[],
    );
    track.id = TrackId::new("remote:track:missing-cover");
    track.album = album.title.clone();
    track.image_ref = None;
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source.id)?;
            let generation = store.begin_sync(&saved.source.id)?;
            commit_cached_library(
                store,
                &saved.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![album],
                    tracks: vec![track.clone()],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed remote cache");

    let raw_refs = store
        .with_store(|store| store.load_raw_track_image_refs(&saved.source.id))
        .expect("raw track refs");
    let selected = store
        .with_store(|store| store.load_tracks(&saved.source.id, 0, 10))
        .expect("selected tracks");

    assert_eq!(raw_refs.get(&track.id), Some(&None));
    assert_eq!(selected.items[0].image_ref.as_ref(), Some(&album_ref));
}

#[test]
pub(in crate::controller) fn album_projection_binds_mbid_art_before_route_read() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let mut album =
        local_album_with_image_ref(ImageRef::new("local:cover:file%3A%2F%2Funused", None));
    album.image_ref = None;
    album.musicbrainz_release_group_id = Some("441f9fa7-4c22-4b0f-a363-ba6fa6b04ded".to_string());
    let expected = external_metadata::external_album_identity_image_ref(&album)
        .expect("external release-group ref");
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.set_active_source(&local.source.id)?;
            let generation = store.begin_sync(&local.source.id)?;
            commit_cached_library(
                store,
                &local.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![album.clone()],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed local cache");

    let raw_refs = store
        .with_store(|store| store.load_raw_album_image_refs(&local.source.id))
        .expect("raw album refs");
    let selected = store
        .with_store(|store| store.load_albums(&local.source.id, 0, 10))
        .expect("selected albums");

    assert_eq!(raw_refs.get(&album.id), Some(&None));
    assert_eq!(selected.items[0].image_ref.as_ref(), Some(&expected));
}

#[test]
pub(in crate::controller) fn genre_projection_uses_bound_mbid_album_art() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let genre = Genre {
        id: GenreId::new("local:genre:mbid-binding"),
        name: "Example Genre".to_string(),
        album_count: 1,
        track_count: 1,
        duration_seconds: 180,
        image_refs: Vec::new(),
        image_ref: None,
    };
    let mut album =
        local_album_with_image_ref(ImageRef::new("local:cover:file%3A%2F%2Funused", None));
    album.id = AlbumId::new("local:album:mbid-genre-binding");
    album.image_ref = None;
    album.musicbrainz_release_group_id = Some("441f9fa7-4c22-4b0f-a363-ba6fa6b04ded".to_string());
    album.genres = vec![genre.name.clone()];
    let expected = external_metadata::external_album_identity_image_ref(&album)
        .expect("external release-group ref");
    let mut track = library_track(
        1,
        album.artist_id.clone(),
        album.id.clone(),
        "Representative Artist",
        &[&genre.name],
    );
    track.id = TrackId::new("local:track:mbid-genre-binding");
    track.album = album.title.clone();
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.set_active_source(&local.source.id)?;
            let generation = store.begin_sync(&local.source.id)?;
            commit_cached_library(
                store,
                &local.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![album.clone()],
                    tracks: vec![track],
                    genres: vec![genre],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed local cache");

    let genres = store
        .with_store(|store| store.load_genres(&local.source.id, 0, 10))
        .expect("load genres")
        .items;

    assert_eq!(genres.len(), 1);
    assert_eq!(genres[0].image_refs, vec![expected.clone()]);
    assert_eq!(genres[0].image_ref.as_ref(), Some(&expected));
}

#[test]
pub(in crate::controller) fn genre_route_consumes_bound_mbid_art_without_cache() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let mut settings = AppSettings {
        external_metadata_enabled: true,
        ..AppSettings::default()
    };
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    let genre = Genre {
        id: GenreId::new("local:genre:mbid-route-binding"),
        name: "Example Genre".to_string(),
        album_count: 1,
        track_count: 1,
        duration_seconds: 180,
        image_refs: Vec::new(),
        image_ref: None,
    };
    let mut album =
        local_album_with_image_ref(ImageRef::new("local:cover:file%3A%2F%2Funused", None));
    album.id = AlbumId::new("local:album:mbid-route-binding");
    album.image_ref = None;
    album.musicbrainz_release_group_id = Some("441f9fa7-4c22-4b0f-a363-ba6fa6b04ded".to_string());
    album.genres = vec![genre.name.clone()];
    let expected = external_metadata::external_album_identity_image_ref(&album)
        .expect("external release-group ref");
    let mut track = library_track(
        1,
        album.artist_id.clone(),
        album.id.clone(),
        "Representative Artist",
        &[&genre.name],
    );
    track.id = TrackId::new("local:track:mbid-route-binding");
    track.album = album.title.clone();
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.set_active_source(&local.source.id)?;
            let generation = store.begin_sync(&local.source.id)?;
            commit_cached_library(
                store,
                &local.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![album],
                    tracks: vec![track],
                    genres: vec![genre],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed local cache");
    let (controller, _events) = controller_from_store_for_test(store);

    let genres = controller
        .cached_genres_page(0, 10)
        .expect("cached genres")
        .items;

    assert_eq!(genres.len(), 1);
    assert_eq!(genres[0].image_refs, vec![expected.clone()]);
    assert_eq!(genres[0].image_ref.as_ref(), Some(&expected));
}

#[test]
pub(in crate::controller) fn genre_route_keeps_cached_mbid_art_in_private_mode() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let mut settings = AppSettings {
        external_metadata_enabled: true,
        private_mode: true,
        ..AppSettings::default()
    };
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    let genre = Genre {
        id: GenreId::new("local:genre:private-mbid-binding"),
        name: "Example Genre".to_string(),
        album_count: 1,
        track_count: 1,
        duration_seconds: 180,
        image_refs: Vec::new(),
        image_ref: None,
    };
    let mut album =
        local_album_with_image_ref(ImageRef::new("local:cover:file%3A%2F%2Funused", None));
    album.id = AlbumId::new("local:album:private-mbid-binding");
    album.image_ref = None;
    album.musicbrainz_release_group_id = Some("441f9fa7-4c22-4b0f-a363-ba6fa6b04ded".to_string());
    album.genres = vec![genre.name.clone()];
    let expected = external_metadata::external_album_identity_image_ref(&album)
        .expect("external release-group ref");
    let mut track = library_track(
        1,
        album.artist_id.clone(),
        album.id.clone(),
        "Representative Artist",
        &[&genre.name],
    );
    track.id = TrackId::new("local:track:private-mbid-binding");
    track.album = album.title.clone();
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.set_active_source(&local.source.id)?;
            let generation = store.begin_sync(&local.source.id)?;
            commit_cached_library(
                store,
                &local.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![album.clone()],
                    tracks: vec![track],
                    genres: vec![genre],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed local cache");
    let raw_refs = store
        .with_store(|store| store.load_raw_album_image_refs(&local.source.id))
        .expect("raw album refs");
    let selected = store
        .with_store(|store| store.load_albums(&local.source.id, 0, 10))
        .expect("selected albums");
    assert_eq!(raw_refs.get(&album.id), Some(&None));
    assert_eq!(selected.items[0].image_ref.as_ref(), Some(&expected));
    let (controller, _events) = controller_from_store_for_test(store);

    let genres = controller
        .cached_genres_page(0, 10)
        .expect("cached genres")
        .items;

    assert_eq!(genres.len(), 1);
    assert_eq!(genres[0].image_refs, vec![expected.clone()]);
    assert_eq!(genres[0].image_ref.as_ref(), Some(&expected));
}

#[test]
pub(in crate::controller) fn artist_grid_uses_track_fallback_art() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let fallback_ref = ImageRef::new("local:cover:embedded%3A%2F%2Ftrack-cover", None);
    let mut album = local_album_with_image_ref(fallback_ref.clone());
    album.image_ref = None;
    let artist_id = ArtistId::new("local:artist:track-cover");
    album.artist_id = Some(artist_id.clone());
    let mut track = library_track(
        1,
        Some(artist_id.clone()),
        album.id.clone(),
        "Track Cover Artist",
        &[],
    );
    track.id = TrackId::new("local:track:track-cover");
    track.album = album.title.clone();
    track.image_ref = Some(fallback_ref.clone());
    let artist = domain::Artist {
        id: artist_id,
        name: "Track Cover Artist".to_string(),
        album_count: 1,
        track_count: 1,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        musicbrainz_artist_id: None,
        image_ref: None,
    };
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.set_active_source(&local.source.id)?;
            let generation = store.begin_sync(&local.source.id)?;
            commit_cached_library(
                store,
                &local.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![album],
                    tracks: vec![track],
                    artists: vec![artist],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed local cache");
    let (controller, _events) = controller_from_store_for_test(store);

    let artists = controller
        .cached_artists_page(false, 0, 10)
        .expect("cached artists")
        .items;

    assert_eq!(artists.len(), 1);
    assert_eq!(artists[0].image_ref.as_ref(), Some(&fallback_ref));
}

#[test]
pub(in crate::controller) fn startup_snapshot_includes_artist_grid_fallback_art() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let album_ref = ImageRef::new("local:cover:file%3A%2F%2Fguest-album-cover", None);
    let track_ref = ImageRef::new("local:cover:embedded%3A%2F%2Fsinger-track-cover", None);
    let mut guest_album = local_album_with_image_ref(album_ref.clone());
    guest_album.id = AlbumId::new("local:album:guest");
    guest_album.title = "Guest Album".to_string();
    guest_album.artist = "Album Curator".to_string();
    guest_album.artist_id = Some(ArtistId::new("local:artist:curator"));
    let guest_id = ArtistId::new("local:artist:guest-snapshot");
    let mut guest_track = library_track(
        1,
        Some(guest_id.clone()),
        guest_album.id.clone(),
        "Guest Snapshot",
        &[],
    );
    guest_track.id = TrackId::new("local:track:guest-snapshot");
    guest_track.album = guest_album.title.clone();
    let mut singer_album = local_album_with_image_ref(track_ref.clone());
    singer_album.id = AlbumId::new("local:album:singer");
    singer_album.image_ref = None;
    let singer_id = ArtistId::new("local:artist:singer-snapshot");
    singer_album.artist_id = Some(singer_id.clone());
    let mut singer_track = library_track(
        2,
        Some(singer_id.clone()),
        singer_album.id.clone(),
        "Singer Snapshot",
        &[],
    );
    singer_track.id = TrackId::new("local:track:singer-snapshot");
    singer_track.album = singer_album.title.clone();
    singer_track.image_ref = Some(track_ref.clone());
    let guest_album_artist = domain::Artist {
        id: guest_id,
        name: "Guest Snapshot".to_string(),
        album_count: 1,
        track_count: 1,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        musicbrainz_artist_id: None,
        image_ref: None,
    };
    let singer = domain::Artist {
        id: singer_id,
        name: "Singer Snapshot".to_string(),
        album_count: 1,
        track_count: 1,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        musicbrainz_artist_id: None,
        image_ref: None,
    };
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.set_active_source(&local.source.id)?;
            let generation = store.begin_sync(&local.source.id)?;
            commit_cached_library(
                store,
                &local.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![guest_album, singer_album],
                    tracks: vec![guest_track, singer_track],
                    artists: vec![singer],
                    album_artists: vec![guest_album_artist],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed local cache");

    let snapshot = load_snapshot(&store).expect("load snapshot");

    let artist = snapshot
        .artists
        .iter()
        .find(|artist| artist.name == "Singer Snapshot")
        .expect("artist snapshot row");
    assert_eq!(artist.image_ref.as_ref(), Some(&track_ref));
    let album_artist = snapshot
        .album_artists
        .iter()
        .find(|artist| artist.name == "Guest Snapshot")
        .expect("album artist snapshot row");
    assert_eq!(album_artist.image_ref.as_ref(), Some(&album_ref));
}

#[test]
pub(in crate::controller) fn startup_track_image() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let external_image_ref = ImageRef::new("external:album:Example%20Artist:Example%20Album", None);
    let album = local_album_with_image_ref(external_image_ref.clone());
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
            store.save_source(&local)?;
            store.set_active_source(&local.source.id)?;
            let generation = store.begin_sync(&local.source.id)?;
            commit_cached_library(
                store,
                &local.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![album],
                    tracks: vec![track],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed local cache");

    let snapshot = load_snapshot(&store).expect("load snapshot");
    let expected = external_metadata::external_album_image_ref("Example Artist", "Example Album")
        .expect("external album ref");

    assert_eq!(snapshot.tracks.len(), 1);
    assert_eq!(snapshot.tracks[0].image_ref.as_ref(), Some(&expected));
}
#[test]
pub(in crate::controller) fn cache_album_page() {
    let store = StoreHandle::open_memory().expect("memory store");
    let remote = saved_source();
    let local_image_ref = ImageRef::new("local:cover:embedded%3A%2Fmusic%2Ftrack.flac", None);
    let remote_album = remote_album_with_image_ref(local_image_ref);
    store
        .with_store(|store| {
            store.save_source(&remote)?;
            store.set_active_source(&remote.source.id)?;
            let generation = store.begin_sync(&remote.source.id)?;
            commit_cached_library(
                store,
                &remote.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![remote_album],
                    ..CachedLibraryObservation::default()
                },
            )?;
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
    let album = local_album_with_image_ref(external_image_ref.clone());
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
            store.save_source(&local)?;
            store.set_active_source(&local.source.id)?;
            let generation = store.begin_sync(&local.source.id)?;
            commit_cached_library(
                store,
                &local.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![album],
                    tracks: vec![track],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed local cache");

    let page = controller
        .cached_tracks_page(0, 10)
        .expect("cached tracks page");
    let expected = external_metadata::external_album_image_ref("Example Artist", "Example Album")
        .expect("external album ref");

    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].image_ref.as_ref(), Some(&expected));
}
#[test]
pub(in crate::controller) fn startup_remote_cache() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = saved_source();
    let album = remote_album_with_image_ref(provider_cover_ref());
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            let generation = store.begin_sync(&saved.source.id)?;
            commit_cached_library(
                store,
                &saved.source.id,
                generation,
                CachedLibraryObservation {
                    albums: vec![album.clone()],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed remote cache");
    let albums = store
        .with_store(|store| store.load_albums(&saved.source.id, 0, 1))
        .expect("load remote cache");
    let cached_album = albums.items.first().expect("cached album");

    assert_eq!(albums.total, 1);
    assert_eq!(cached_album.id, album.id);
    assert_eq!(cached_album.image_ref.as_ref(), album.image_ref.as_ref());
}

fn startup_assert_ref(image_ref: Option<&ImageRef>) {
    assert!(
        !image_ref.is_some_and(|image_ref| image_ref.item_id.starts_with("local:cover:")),
        "remote cached reads must not expose local provider image refs"
    );
}
#[test]
pub(in crate::controller) fn home_refresh_replace() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = saved_source();
    let mut stale_album = remote_album_with_image_ref(ImageRef::new(
        "jellyfin:cover:stale",
        Some("stale".to_string()),
    ));
    stale_album.id = AlbumId::new("jellyfin:album:stale");
    let fresh_album = remote_album_with_image_ref(provider_cover_ref());
    let mut stale_track = library_track(
        9,
        stale_album.artist_id.clone(),
        stale_album.id.clone(),
        &stale_album.artist,
        &[],
    );
    stale_track.id = TrackId::new("jellyfin:track:stale");
    stale_track.album = stale_album.title.clone();
    let mut fresh_track = library_track(
        1,
        fresh_album.artist_id.clone(),
        fresh_album.id.clone(),
        &fresh_album.artist,
        &[],
    );
    fresh_track.id = TrackId::new("jellyfin:track:fresh");
    fresh_track.album = fresh_album.title.clone();
    let stale_sections = [
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
    ];
    let fresh_sections = [
        HomeSection {
            kind: HomeSectionKind::Explore,
            albums: vec![fresh_album.clone()],
            tracks: Vec::new(),
        },
        HomeSection {
            kind: HomeSectionKind::MostPlayed,
            albums: Vec::new(),
            tracks: vec![fresh_track.clone()],
        },
    ];
    seed_cached_library(
        &store,
        &saved,
        std::slice::from_ref(&stale_album),
        std::slice::from_ref(&stale_track),
        &stale_sections,
    );
    let before = store
        .with_store(|store| store.load_home_sections(&saved.source.id))
        .expect("load stale home sections");
    let before_sync = store
        .with_store(|store| store.sync_state(&saved.source.id))
        .expect("sync state before refresh");
    assert_eq!(before[0].albums[0].id, stale_album.id);
    assert_eq!(before[1].tracks[0].id, stale_track.id);

    store
        .with_store(|store| {
            store.upsert_albums(
                &saved.source.id,
                std::slice::from_ref(&fresh_album),
                before_sync.generation,
            )?;
            store.upsert_tracks(
                &saved.source.id,
                std::slice::from_ref(&fresh_track),
                before_sync.generation,
            )?;
            store.upsert_home_sections(&saved.source.id, &fresh_sections, before_sync.generation)
        })
        .expect("replace home sections");

    let after = store
        .with_store(|store| store.load_home_sections(&saved.source.id))
        .expect("load refreshed home sections");
    let sync_state = store
        .with_store(|store| store.sync_state(&saved.source.id))
        .expect("sync state");
    assert_eq!(after[0].kind, HomeSectionKind::Explore);
    assert_eq!(after[0].albums[0].id, fresh_album.id);
    assert_eq!(after[1].kind, HomeSectionKind::MostPlayed);
    assert_eq!(after[1].tracks[0].id, fresh_track.id);
    assert_eq!(sync_state.generation, before_sync.generation);
    assert_eq!(sync_state.status, before_sync.status);
}
#[test]
pub(in crate::controller) fn playlist_refresh_replace() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = saved_source();
    let album = remote_album_with_image_ref(provider_cover_ref());
    let mut stale_track = library_track(
        1,
        album.artist_id.clone(),
        album.id.clone(),
        &album.artist,
        &[],
    );
    stale_track.id = TrackId::new("jellyfin:track:stale-playlist");
    stale_track.album = album.title.clone();
    let mut fresh_track = stale_track.clone();
    fresh_track.id = TrackId::new("jellyfin:track:fresh-playlist");
    fresh_track.title = "Fresh Playlist Track".to_string();
    let stale_playlist = Playlist {
        id: PlaylistId::new("jellyfin:playlist:stale"),
        name: "Old Playlist".to_string(),
        owner: Some(SourceFeatureOwner::Native),
        track_count: 1,
        duration_seconds: stale_track.duration_seconds,
        top_genres: Vec::new(),
        image_refs: Vec::new(),
        image_ref: stale_track.image_ref.clone(),
    };
    let stale_entry = PlaylistEntry {
        entry_id: "old-playlist-entry".to_string(),
        track: stale_track.clone(),
    };
    let fresh_playlist = Playlist {
        id: PlaylistId::new("jellyfin:playlist:fresh"),
        name: "Fresh Playlist".to_string(),
        owner: Some(SourceFeatureOwner::Native),
        track_count: 1,
        duration_seconds: fresh_track.duration_seconds,
        top_genres: Vec::new(),
        image_refs: Vec::new(),
        image_ref: fresh_track.image_ref.clone(),
    };
    let fresh_entry = PlaylistEntry {
        entry_id: "fresh-playlist-entry".to_string(),
        track: fresh_track.clone(),
    };
    let sync_artist = Artist {
        id: album.artist_id.clone().expect("album artist"),
        name: album.artist.clone(),
        album_count: 1,
        track_count: 1,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        musicbrainz_artist_id: None,
        image_ref: None,
    };
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source.id)?;
            let generation = store.begin_sync(&saved.source.id)?;
            commit_cached_library(
                store,
                &saved.source.id,
                generation,
                CachedLibraryObservation {
                    tracks: vec![stale_track.clone()],
                    playlists: vec![PlaylistDetail {
                        playlist: stale_playlist.clone(),
                        tracks: vec![stale_track.clone()],
                        entries: vec![stale_entry.clone()],
                    }],
                    ..CachedLibraryObservation::default()
                },
            )?;
            Ok(())
        })
        .expect("seed stale playlists");
    let before = store
        .with_store(|store| store.load_playlists(&saved.source.id, 0, 10))
        .expect("load stale playlists");
    assert_eq!(before.total, 1);
    assert_eq!(before.items[0].id, stale_playlist.id);
    let before_sync = store
        .with_store(|store| store.sync_state(&saved.source.id))
        .expect("sync state before playlist refresh");

    let delta = store
        .with_store_session(|store| {
            let generation = store
                .begin_sync(&saved.source.id)
                .map_err(|error| error.to_string())?;
            let base_cache_revision = store
                .source_cache_revision(&saved.source.id)
                .map_err(|error| error.to_string())?;
            store
                .commit_library_sync(
                    &saved.source.id,
                    generation,
                    base_cache_revision,
                    library::LibrarySync {
                        albums: vec![album.clone()],
                        tracks: vec![fresh_track.clone()],
                        artists: vec![sync_artist.clone()],
                        album_artists: vec![sync_artist.clone()],
                        genres: Vec::new(),
                        playlists: vec![PlaylistDetail {
                            playlist: fresh_playlist.clone(),
                            tracks: vec![fresh_track.clone()],
                            entries: vec![fresh_entry.clone()],
                        }],
                        home_sections: Vec::new(),
                        mappings: vec![
                            SourceObjectMapping {
                                source_object_id: album.id.as_str().to_string(),
                                entity_kind: SourceEntityKind::Album,
                                entity_id: album.id.as_str().to_string(),
                            },
                            SourceObjectMapping {
                                source_object_id: fresh_track.id.as_str().to_string(),
                                entity_kind: SourceEntityKind::Track,
                                entity_id: fresh_track.id.as_str().to_string(),
                            },
                            SourceObjectMapping {
                                source_object_id: sync_artist.id.as_str().to_string(),
                                entity_kind: SourceEntityKind::Artist,
                                entity_id: sync_artist.id.as_str().to_string(),
                            },
                            SourceObjectMapping {
                                source_object_id: sync_artist.id.as_str().to_string(),
                                entity_kind: SourceEntityKind::AlbumArtist,
                                entity_id: sync_artist.id.as_str().to_string(),
                            },
                            SourceObjectMapping {
                                source_object_id: fresh_playlist.id.as_str().to_string(),
                                entity_kind: SourceEntityKind::Playlist,
                                entity_id: fresh_playlist.id.as_str().to_string(),
                            },
                        ],
                        coverage: library::SyncCoverage::All {
                            music_folders: Vec::new(),
                        },
                        local_access: None,
                    },
                )
                .map(|commit| commit.delta)
                .map_err(|error| error.to_string())
        })
        .expect("replace authoritative playlists");

    let after = store
        .with_store(|store| store.load_playlists(&saved.source.id, 0, 10))
        .expect("load refreshed playlists");
    let detail = store
        .with_store(|store| store.load_playlist_detail(&saved.source.id, &fresh_playlist.id))
        .expect("load playlist detail")
        .expect("playlist detail");
    let sync_state = store
        .with_store(|store| store.sync_state(&saved.source.id))
        .expect("sync state");
    assert_eq!(after.total, 1);
    assert_eq!(after.items[0].id, fresh_playlist.id);
    assert_eq!(detail.entries.len(), 1);
    assert_eq!(detail.entries[0].entry_id, fresh_entry.entry_id);
    assert_eq!(detail.entries[0].track.id, fresh_track.id);
    assert!(delta.playlists.added.contains(&fresh_playlist.id));
    assert!(delta.playlists.deleted.contains(&stale_playlist.id));
    assert!(sync_state.generation > before_sync.generation);
    assert_eq!(sync_state.status, "idle");
}
#[test]
pub(in crate::controller) fn startup_replace_section() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = saved_source();
    let mut stale_album = remote_album_with_image_ref(ImageRef::new(
        "jellyfin:cover:stale-section",
        Some("stale-section".to_string()),
    ));
    stale_album.id = AlbumId::new("jellyfin:album:stale-section");
    let fresh_album = remote_album_with_image_ref(provider_cover_ref());
    let mut stale_track = library_track(
        9,
        stale_album.artist_id.clone(),
        stale_album.id.clone(),
        &stale_album.artist,
        &[],
    );
    stale_track.id = TrackId::new("jellyfin:track:stale-section");
    stale_track.album = stale_album.title.clone();
    let mut fresh_track = library_track(
        1,
        fresh_album.artist_id.clone(),
        fresh_album.id.clone(),
        &fresh_album.artist,
        &[],
    );
    fresh_track.id = TrackId::new("jellyfin:track:fresh-section");
    fresh_track.album = fresh_album.title.clone();
    let stale_sections = [
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
    ];
    seed_cached_library(
        &store,
        &saved,
        &[stale_album.clone(), fresh_album.clone()],
        &[stale_track.clone(), fresh_track.clone()],
        &stale_sections,
    );
    let (generation, cache_revision) = store
        .with_store(|store| {
            store
                .sync_state(&saved.source.id)
                .map(|state| (state.generation, state.cache_revision))
        })
        .expect("section generation");

    cache_home_section(
        &store,
        &saved.source.id,
        &HomeSection {
            kind: HomeSectionKind::Explore,
            albums: vec![fresh_album.clone()],
            tracks: Vec::new(),
        },
        generation,
        cache_revision,
    )
    .expect("replace Explore");
    let after = store
        .with_store(|store| store.load_home_sections(&saved.source.id))
        .expect("load Explore replacement");
    assert_eq!(after[0].kind, HomeSectionKind::Explore);
    assert_eq!(after[0].albums[0].id, fresh_album.id);
    assert_eq!(after[1].kind, HomeSectionKind::MostPlayed);
    assert_eq!(after[1].tracks[0].id, stale_track.id);

    let cache_revision = store
        .with_store(|store| store.source_cache_revision(&saved.source.id))
        .expect("Home cache revision");
    cache_home_section(
        &store,
        &saved.source.id,
        &HomeSection {
            kind: HomeSectionKind::MostPlayed,
            albums: Vec::new(),
            tracks: vec![fresh_track.clone()],
        },
        generation,
        cache_revision,
    )
    .expect("replace Most Played");
    let after = store
        .with_store(|store| store.load_home_sections(&saved.source.id))
        .expect("load Most Played replacement");
    assert_eq!(after[0].kind, HomeSectionKind::Explore);
    assert_eq!(after[0].albums[0].id, fresh_album.id);
    assert_eq!(after[1].kind, HomeSectionKind::MostPlayed);
    assert_eq!(after[1].tracks[0].id, fresh_track.id);
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
pub(in crate::controller) fn startup_promote_prefetch() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = saved_source();
    let mut stale_album = remote_album_with_image_ref(ImageRef::new(
        "jellyfin:cover:stale-prefetch",
        Some("stale-prefetch".to_string()),
    ));
    stale_album.id = AlbumId::new("jellyfin:album:stale-prefetch");
    let fresh_album = remote_album_with_image_ref(provider_cover_ref());
    let visible = HomeSection {
        kind: HomeSectionKind::Explore,
        albums: vec![stale_album.clone()],
        tracks: Vec::new(),
    };
    let prefetched = HomeSection {
        kind: HomeSectionKind::Explore,
        albums: vec![fresh_album.clone()],
        tracks: Vec::new(),
    };
    seed_cached_library(
        &store,
        &saved,
        &[stale_album.clone(), fresh_album.clone()],
        &[],
        std::slice::from_ref(&visible),
    );
    let (generation, cache_revision) = store
        .with_store(|store| {
            store
                .sync_state(&saved.source.id)
                .map(|state| (state.generation, state.cache_revision))
        })
        .expect("prefetch generation");
    store
        .with_store(|store| {
            store.save_home_section_prefetch(
                &saved.source.id,
                generation,
                cache_revision,
                &prefetched,
            )
        })
        .expect("stage prefetched Explore");
    let visible_before = store
        .with_store(|store| store.load_home_sections(&saved.source.id))
        .expect("load visible sections");
    assert_eq!(visible_before[0].albums[0].id, stale_album.id);
    assert_eq!(prefetched.albums[0].id, fresh_album.id);
    assert!(
        store
            .with_store(|store| {
                store.load_home_section_prefetch(&saved.source.id, HomeSectionKind::Explore)
            })
            .expect("load prefetched Explore")
            .is_some()
    );
    promote_prefetched_home_section(&store, &saved.source.id, &prefetched)
        .expect("promote prefetched Explore");
    let visible_after = store
        .with_store(|store| store.load_home_sections(&saved.source.id))
        .expect("load promoted sections");
    assert_eq!(visible_after[0].albums[0].id, fresh_album.id);
    assert!(
        store
            .with_store(|store| {
                store.load_home_section_prefetch(&saved.source.id, HomeSectionKind::Explore)
            })
            .expect("load cleared prefetched Explore")
            .is_none()
    );
}
#[test]
pub(in crate::controller) fn startup_emit_snapshot() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = saved_source();
    let album = remote_album_with_image_ref(provider_cover_ref());
    let mut track = library_track(
        1,
        album.artist_id.clone(),
        album.id.clone(),
        &album.artist,
        &[],
    );
    track.id = TrackId::new("jellyfin:track:clear-cache");
    track.album = album.title.clone();
    seed_cached_library(
        &store,
        &saved,
        std::slice::from_ref(&album),
        std::slice::from_ref(&track),
        &[],
    );
    let (controller, events) = controller_from_store_for_test(store);

    controller.clear_active_source_cache();

    let snapshot = wait_for_snapshot(&events);
    assert!(!snapshot.first_run);
    assert_eq!(snapshot.source.expect("server").id, saved.source.id);
    assert!(snapshot.albums.is_empty());
    assert!(snapshot.tracks.is_empty());
}
