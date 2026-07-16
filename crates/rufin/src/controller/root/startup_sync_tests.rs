use super::*;
use crate::controller::root::controller_bootstrap::bootstrap_memory_for_test;
use async_channel::unbounded;

use super::{
    LOCAL_SOURCE_IDENTITY_ID, ProductReceivers, StoreHandle, load_runtime_source_presentation,
    load_source_presentation, save_home_section_projection, sync_local_source_outcome,
    sync_local_source_with_events,
};
use library::{
    AlbumId, Genre, GenreId, HomeSection, HomeSectionKind, ImageRef, Playlist, PlaylistDetail,
    PlaylistEntry, PlaylistId, SourceId, TrackId,
};
use library::{SourceLocalAccess, StoredSource};
use rusqlite::Connection;
use secrets::{MemorySecretStore, SecretStore};
use sources::{
    CredentialSourceConfig, SourceObjectChanges, jellyfin::JellyfinSourceConfig,
    local::LocalSourceConfig, subsonic::SubsonicSourceConfig,
};
use sources::{LibrarySourceSelection, LocalLibraryFolder, SourceIdentity};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;

fn jellyfin_identity(saved: &StoredSource) -> SourceIdentity {
    JellyfinSourceConfig::from_stored(saved)
        .expect("decode Jellyfin source")
        .credentials
        .source
}

fn local_identity(saved: &StoredSource) -> SourceIdentity {
    LocalSourceConfig::from_stored(saved)
        .expect("decode local source")
        .source
}

#[test]
pub(in crate::controller) fn startup_jellyfin_saved() {
    let store = StoreHandle::open_memory().expect("open memory store");

    let first = crate::source_setup::ensure_jellyfin_device_id(&store).expect("first device id");
    let second = crate::source_setup::ensure_jellyfin_device_id(&store).expect("second device id");

    assert!(first.starts_with("rufin-"));
    assert_eq!(second, first);
    assert_eq!(store.load_settings().jellyfin_device_id, first);
}

#[test]
pub(in crate::controller) fn selecting_jellyfin_preserves_generated_device_id() {
    let (owners, events, _snapshot, _playback) = bootstrap_memory_for_test();
    let saved = saved_source();
    seed_cached_library(
        &owners.source.store,
        &saved,
        &[remote_album_with_image_ref(provider_cover_ref())],
        &[],
        &[],
    );
    owners
        .source
        .secrets
        .save_token(saved.source_id.as_str(), "token")
        .expect("save token");

    owners
        .source
        .select_source(LibrarySourceSelection::Source(saved.source_id.clone()));

    assert_eq!(
        wait_for_source_selection(&events),
        LibrarySourceSelection::Source(saved.source_id)
    );
    let _snapshot = wait_for_source_presentation(&events);
    let device_id = owners.settings.load_settings().jellyfin_device_id;
    assert!(device_id.starts_with("rufin-"));
}

#[test]
pub(in crate::controller) fn startup_server_state() {
    let (_owners, _events, snapshot, playback) = bootstrap_memory_for_test();
    assert!(snapshot.first_run);
    assert!(snapshot.source.is_none());
    assert!(playback.is_none());
}
#[test]
pub(in crate::controller) fn startup_init_queue() {
    let (owners, events, _snapshot, initial_playback) = bootstrap_memory_for_test();
    assert!(initial_playback.is_none());
    let root = unique_test_dir("first-run-local-queue");
    fs::create_dir_all(&root).expect("create root");
    crate::source_setup::configure_source(
        &owners.source,
        sources::SourceSetupInput::Local(sources::LocalFolderHostInput {
            roots: vec![root.clone()],
        }),
    );
    let playback = wait_for_playback_projection(&events);
    assert_eq!(
        playback.view.transport.source_id.as_str(),
        LOCAL_SOURCE_IDENTITY_ID
    );
    assert_eq!(playback.view.queue.total, 0);
    let snapshot = wait_for_source_presentation(&events);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Local)
    );
    assert_eq!(
        owners
            .playback
            .playback_product()
            .expect("playback product")
            .sequence_snapshot()
            .expect("sequence")
            .source_id()
            .as_str(),
        LOCAL_SOURCE_IDENTITY_ID
    );
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn startup_accept_folders() {
    let (owners, events, _snapshot, initial_playback) = bootstrap_memory_for_test();
    assert!(initial_playback.is_none());
    let first = unique_test_dir("first-run-local-folder-one");
    let second = unique_test_dir("first-run-local-folder-two");
    fs::create_dir_all(&first).expect("create first root");
    fs::create_dir_all(&second).expect("create second root");
    crate::source_setup::configure_source(
        &owners.source,
        sources::SourceSetupInput::Local(sources::LocalFolderHostInput {
            roots: vec![first.clone(), second.clone()],
        }),
    );
    let playback = wait_for_playback_projection(&events);
    assert_eq!(
        playback.view.transport.source_id.as_str(),
        LOCAL_SOURCE_IDENTITY_ID
    );
    let snapshot = wait_for_source_presentation(&events);
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
    let mut settings = StoredSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    settings.sources.local_folders = vec![LocalLibraryFolder {
        path: root.to_string_lossy().into_owned(),
    }];
    store.save_settings(&settings).expect("save settings");
    let snapshot = load_source_presentation(&store).expect("load snapshot");
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
    let remote_identity = jellyfin_identity(&remote);
    let local = local_source_saved();
    let mut settings = StoredSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&remote)?;
            store.save_source(&local)?;
            store.set_active_source(&local.source_id)
        })
        .expect("seed servers");

    let snapshot = load_source_presentation(&store).expect("load snapshot");

    assert_eq!(snapshot.selected_source, None);
    assert!(snapshot.source.is_none());
    assert!(snapshot.first_run);
    assert_eq!(snapshot.sources, vec![remote_identity]);
    assert!(snapshot.local_folders.is_empty());
    let active = store
        .with_store(|store| store.active_source())
        .expect("active server")
        .expect("active server");
    assert_eq!(active.source_id, local.source_id);
}

#[test]
pub(in crate::controller) fn snapshot_projects_selection_without_committing_it() {
    let store = StoreHandle::open_memory().expect("memory store");
    let active_saved = saved_source();
    let mut selected_saved = saved_source();
    selected_saved.source_id = SourceId::new("jellyfin:server:selected");
    selected_saved.name = "Selected Server".to_string();
    let mut selected_config =
        JellyfinSourceConfig::from_stored(&selected_saved).expect("decode selected source");
    selected_config.credentials.source.base_url = "https://selected.example.test".to_string();
    selected_saved = selected_config.into_stored();
    let mut settings = StoredSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Source(
        selected_saved.source_id.clone(),
    ));
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&active_saved)?;
            store.save_source(&selected_saved)?;
            store.set_active_source(&active_saved.source_id)
        })
        .expect("save servers");

    let snapshot = load_source_presentation(&store).expect("load snapshot");

    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Source(
            selected_saved.source_id.clone()
        ))
    );
    assert_eq!(
        snapshot.source.as_ref().map(|server| server.id.clone()),
        Some(selected_saved.source_id.clone())
    );
    let active_after = store
        .with_store(|store| store.active_source())
        .expect("active server")
        .expect("active server");
    assert_eq!(active_after.source_id, active_saved.source_id);
}

#[test]
pub(in crate::controller) fn startup_local_access_status_reuse() {
    let store = StoreHandle::open_memory().expect("memory store");
    let active_saved = saved_source();
    let mut other_saved = saved_source();
    other_saved.source_id = SourceId::new("jellyfin:server:other");
    other_saved.name = "Other Server".to_string();
    let mut other_config =
        JellyfinSourceConfig::from_stored(&other_saved).expect("decode other source");
    other_config.credentials.source.base_url = "https://other.example.test".to_string();
    other_saved = other_config.into_stored();
    let mut settings = StoredSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Source(
        active_saved.source_id.clone(),
    ));
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&active_saved)?;
            store.save_source(&other_saved)?;
            store.set_active_source(&active_saved.source_id)?;
            store.save_source_local_access(&SourceLocalAccess {
                source_id: active_saved.source_id.clone(),
                root_path: "/home/demo/Music".to_string(),
                path_replace_from: Some("/server/music".to_string()),
                path_replace_to: Some("/home/demo/Music".to_string()),
            })?;
            store.save_source_local_access(&SourceLocalAccess {
                source_id: other_saved.source_id.clone(),
                root_path: "/home/demo/Other".to_string(),
                path_replace_from: Some("/other/music".to_string()),
                path_replace_to: Some("/home/demo/Other".to_string()),
            })?;
            let generation = store.begin_sync(&active_saved.source_id)?;
            let mut track = library_track(
                1,
                Some(ArtistId::fake(1)),
                AlbumId::fake(1),
                "Example Artist",
                &[],
            );
            track.local_path = Some("/server/music/Album/Track.flac".to_string());
            store.upsert_tracks(&active_saved.source_id, &[track], generation)
        })
        .expect("seed servers");

    let snapshot = load_source_presentation(&store).expect("load snapshot");

    assert_eq!(snapshot.source_local_access.len(), 2);
    let active_summary = snapshot
        .source_local_access
        .iter()
        .find(|summary| summary.source_id == active_saved.source_id)
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
            store.set_active_source(&saved.source_id)
        })
        .expect("save server");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());

    let snapshot =
        load_runtime_source_presentation(&store, &secrets).expect("load runtime snapshot");

    assert!(snapshot.first_run);
    assert_eq!(
        snapshot.source.as_ref().map(|server| server.id.clone()),
        Some(saved.source_id.clone())
    );
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Source(saved.source_id.clone()))
    );
}

#[test]
pub(in crate::controller) fn startup_unknown_selected_source_remains_recoverable() {
    let store = StoreHandle::open_memory().expect("memory store");
    let mut saved = saved_source();
    saved.source_id = SourceId::new("removed-provider:server");
    saved.kind = "removed-provider".to_string();
    saved.name = "Removed Provider".to_string();
    let expected_source = SourceIdentity {
        id: saved.source_id.clone(),
        kind: saved.kind.clone(),
        name: saved.name.clone(),
        base_url: String::new(),
    };
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source_id)
        })
        .expect("save unsupported source");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());

    let snapshot =
        load_runtime_source_presentation(&store, &secrets).expect("load runtime snapshot");

    assert!(snapshot.first_run);
    assert_eq!(snapshot.sources, vec![expected_source.clone()]);
    assert_eq!(snapshot.source, Some(expected_source));
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Source(saved.source_id))
    );
}

#[test]
pub(in crate::controller) fn selecting_unknown_source_restores_committed_selection() {
    let store = StoreHandle::open_memory().expect("memory store");
    let active = saved_source();
    let mut unsupported = saved_source();
    unsupported.source_id = SourceId::new("removed-provider:server");
    unsupported.kind = "removed-provider".to_string();
    unsupported.name = "Removed Provider".to_string();
    let mut settings = StoredSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Source(active.source_id.clone()));
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&active)?;
            store.save_source(&unsupported)?;
            store.set_active_source(&active.source_id)
        })
        .expect("save sources");
    let (owners, events) = owners_from_store_for_test(store);
    owners
        .source
        .secrets
        .save_token(active.source_id.as_str(), "token")
        .expect("save active token");

    owners.source.select_source(LibrarySourceSelection::Source(
        unsupported.source_id.clone(),
    ));

    assert_eq!(
        wait_for_source_selection(&events),
        LibrarySourceSelection::Source(unsupported.source_id.clone())
    );
    let snapshot = wait_for_source_presentation(&events);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Source(active.source_id.clone()))
    );
    let error = wait_for_typed_event(
        &events.source_transition_failure,
        Duration::from_secs(1),
        "error",
    );
    assert!(matches!(
        error,
        sources::SourceTransitionFailed {
            source_id: Some(source_id),
            error,
        } if source_id == unsupported.source_id
            && error == "Saved source type is no longer supported."
    ));
    assert_eq!(
        current_active_source(&owners.source.active_source)
            .expect("active source")
            .identity
            .id,
        active.source_id
    );
}

#[test]
pub(in crate::controller) fn startup_config_token_keeps_saved_remote_active() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = saved_source();
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source_id)
        })
        .expect("save server");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
    secrets
        .save_token(saved.source_id.as_str(), "cached-session-token")
        .expect("save token");

    let snapshot =
        load_runtime_source_presentation(&store, &secrets).expect("load runtime snapshot");

    assert!(!snapshot.first_run);
    assert_eq!(
        snapshot.source.as_ref().map(|server| server.id.clone()),
        Some(saved.source_id.clone())
    );
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Source(saved.source_id))
    );
}

#[test]
pub(in crate::controller) fn startup_local_source_does_not_require_secret() {
    let store = StoreHandle::open_memory().expect("memory store");
    let root = unique_test_dir("local-source-runtime-snapshot");
    fs::create_dir_all(&root).expect("create root");
    let mut settings = StoredSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    settings.sources.local_folders = vec![LocalLibraryFolder {
        path: root.to_string_lossy().into_owned(),
    }];
    store.save_settings(&settings).expect("save settings");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());

    let snapshot =
        load_runtime_source_presentation(&store, &secrets).expect("load runtime snapshot");

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
    let mut settings = StoredSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Source(saved.source_id.clone()));
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source_id)
        })
        .expect("save server");
    let (owners, events) = owners_from_store_for_test(store);
    owners.source.add_local_library_folder(root.clone());
    let playback = wait_for_playback_projection(&events);
    assert_eq!(
        playback.view.transport.source_id.as_str(),
        LOCAL_SOURCE_IDENTITY_ID
    );
    let snapshot = wait_for_source_presentation(&events);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Local)
    );
    assert_eq!(snapshot.local_folders.len(), 1);
    let active = owners
        .source
        .store
        .with_store(|store| store.active_source())
        .expect("active server")
        .expect("active server");
    assert_eq!(active.source_id.as_str(), LOCAL_SOURCE_IDENTITY_ID);
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn startup_reuse_cache() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let root = unique_test_dir("startup-reuse-cache");
    fs::create_dir_all(&root).expect("create root");
    let mut settings = StoredSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    settings.sources.local_folders = vec![LocalLibraryFolder {
        path: root.to_string_lossy().into_owned(),
    }];
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.set_active_source(&local.source_id)?;
            let generation = store.begin_sync(&local.source_id)?;
            commit_cached_library(
                store,
                &local.source_id,
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
    let (owners, _events) = owners_from_store_for_test(store);

    let playback = owners
        .playback
        .playback_product()
        .and_then(|product| product.initial_projection())
        .expect("restore local playback");
    assert_eq!(
        playback.view.transport.source_id.as_str(),
        LOCAL_SOURCE_IDENTITY_ID
    );
    assert_eq!(
        owners
            .library
            .library_query(SourceId::new(LOCAL_SOURCE_IDENTITY_ID))
            .albums_page(0, 1)
            .expect("local albums")
            .total,
        1
    );
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn startup_disk_store_waits_for_short_write_lock() {
    let (store, store_root) = disk_store_for_test("startup-disk-lock");
    let local = local_source_saved();
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.set_active_source(&local.source_id)
        })
        .expect("seed active server");
    let database_path = disk_store_database_path(&store);
    let lock = Connection::open(database_path).expect("open lock connection");
    lock.execute_batch("BEGIN IMMEDIATE")
        .expect("hold write lock");
    let writer = {
        let store = store.clone();
        let source_id = local.source_id.clone();
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
            store.set_active_source(&local.source_id)
        })
        .expect("save local server");
    let runtime = Runtime::new().expect("runtime");
    let (events, _receiver) = unbounded();
    let local_identity = local_identity(&local);
    let cold = LocalSource::from_roots_with_identity(vec![root.clone()], local_identity.clone())
        .expect("cold local provider");
    runtime
        .block_on(sync_local_source_with_events(
            &store,
            &local.source_id,
            &cold,
            events.clone(),
        ))
        .expect("cold local sync");
    assert_eq!(
        store
            .with_store(|store| store.load_tracks(&local.source_id, 0, 10))
            .expect("cold tracks")
            .total,
        2
    );
    let (seed_generation, seed_revision) = store
        .with_store(|store| {
            Ok((
                store.begin_sync(&local.source_id)?,
                store.source_cache_revision(&local.source_id)?,
            ))
        })
        .expect("begin manifest seed");
    let mut committed_manifest = store
        .with_store(|store| store.load_local_manifest(&local.source_id))
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
            store.upsert_tracks(&local.source_id, &committed_tracks, seed_generation)?;
            let current_album_ids = store
                .load_albums(&local.source_id, 0, 100)?
                .items
                .into_iter()
                .map(|album| album.id)
                .collect();
            let current_artist_ids = store
                .load_artists(&local.source_id, false, 0, 100)?
                .items
                .into_iter()
                .map(|artist| artist.id)
                .collect();
            let current_album_artist_ids = store
                .load_artists(&local.source_id, true, 0, 100)?
                .items
                .into_iter()
                .map(|artist| artist.id)
                .collect();
            let current_genre_ids = store
                .load_genres(&local.source_id, 0, 100)?
                .items
                .into_iter()
                .map(|genre| genre.id)
                .collect();
            store
                .commit_local_library_delta(
                    &local.source_id,
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
        .with_store(|store| store.load_local_manifest(&local.source_id))
        .expect("manifest");
    let warm =
        LocalSource::from_roots_with_manifest_cache(vec![root.clone()], local_identity, manifest)
            .expect("warm local provider");
    assert_eq!(warm.manifest_scan().entries.len(), 1);
    assert_eq!(warm.manifest_scan().deleted_track_ids.len(), 1);

    runtime
        .block_on(sync_local_source_with_events(
            &store,
            &local.source_id,
            &warm,
            events,
        ))
        .expect("warm local sync");

    let tracks = store
        .with_store(|store| store.load_tracks(&local.source_id, 0, 10))
        .expect("warm tracks");
    assert_eq!(tracks.total, 1);
    assert_eq!(tracks.items[0].title, "Kept");
    let retained_path = store
        .with_store(|store| store.track_local_path(&local.source_id, &tracks.items[0].id))
        .expect("retained path");
    assert_eq!(
        retained_path.as_deref(),
        Some(kept.to_string_lossy().as_ref())
    );
    let genres = store
        .with_store(|store| store.load_genres(&local.source_id, 0, 10))
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
            .with_store(|store| store.load_local_manifest(&local.source_id))
            .expect("manifest");
        let local_identity = local_identity(&local);
        let warm = LocalSource::from_roots_with_manifest_cache(
            vec![root.clone()],
            local_identity,
            manifest,
        )
        .expect("warm local provider");
        assert_eq!(warm.manifest_scan().library_changed, changed, "{label}");
        let runtime = Runtime::new().expect("runtime");
        let revision = store
            .with_store(|store| store.source_cache_revision(&local.source_id))
            .expect("cache revision");

        let outcome = runtime
            .block_on(sync_local_source_outcome(&store, &local.source_id, &warm))
            .expect("local sync");

        assert_eq!(outcome.delta.is_empty(), !changed, "{label}");
        assert_eq!(
            store
                .with_store(|store| store.source_cache_revision(&local.source_id))
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
        .with_store(|store| store.load_local_manifest(&local.source_id))
        .expect("cached manifest");
    let roots: crate::source_setup::LocalRootsLoader = {
        let root = root.clone();
        Arc::new(move || vec![root.clone()])
    };
    let full_scan_used = Arc::new(AtomicBool::new(false));
    let local_identity = local_identity(&local);
    let load: crate::source_setup::LocalLoader = {
        let root = root.clone();
        let identity = local_identity.clone();
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
    let sync = local_sync_operation(local.source_id.clone(), local_identity, load, roots);
    let runtime = Runtime::new().expect("runtime");
    let cancellation = library_sync::CancellationToken::new();
    let mut progress = |_| {};
    let revision = store
        .with_store(|store| store.source_cache_revision(&local.source_id))
        .expect("cache revision");
    let unrelated = root.join("notes.txt");
    fs::write(&unrelated, "not library data").expect("unrelated file");
    let generation = store
        .with_store(|store| store.begin_sync(&local.source_id))
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
            .with_store(|store| store.source_cache_revision(&local.source_id))
            .expect("unchanged cache revision"),
        revision
    );

    let added = root.join("Artist").join("Album").join("Second.mp3");
    fs::write(&added, []).expect("added audio");
    let generation = store
        .with_store(|store| store.begin_sync(&local.source_id))
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
        .with_store(|store| store.load_tracks(&local.source_id, 0, 10))
        .expect("load local tracks");
    assert_eq!(tracks.total, 2);
    assert!(tracks.items.iter().any(|track| track.title == "Second"));
    let _cleanup = fs::remove_dir_all(root);
}

fn seed_cached_local_source(label: &str) -> (StoreHandle, StoredSource, PathBuf, i64) {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let root = unique_test_dir(label);
    let album_dir = root.join("Artist").join("Album");
    fs::create_dir_all(&album_dir).expect("create album dir");
    fs::write(album_dir.join("Track.mp3"), []).expect("audio");
    let mut settings = StoredSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    settings.sources.local_folders = vec![LocalLibraryFolder {
        path: root.to_string_lossy().into_owned(),
    }];
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.set_active_source(&local.source_id)
        })
        .expect("save local server");
    let runtime = Runtime::new().expect("runtime");
    let (seed_events, _seed_receiver) = unbounded();
    let cold = LocalSource::from_roots_with_identity(vec![root.clone()], local_identity(&local))
        .expect("cold local provider");
    runtime
        .block_on(sync_local_source_with_events(
            &store,
            &local.source_id,
            &cold,
            seed_events,
        ))
        .expect("cold local sync");
    let generation = store
        .with_store(|store| {
            store
                .sync_state(&local.source_id)
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
            store.set_active_source(&local.source_id)
        })
        .expect("save local server");
    let runtime = Runtime::new().expect("runtime");
    let (events, _receiver) = unbounded();
    let local_identity = local_identity(&local);
    let cold = LocalSource::from_roots_with_identity(vec![root.clone()], local_identity.clone())
        .expect("cold local provider");
    runtime
        .block_on(sync_local_source_with_events(
            &store,
            &local.source_id,
            &cold,
            events.clone(),
        ))
        .expect("cold local sync");
    let cold_tag = store
        .with_store(|store| store.load_artists(&local.source_id, false, 0, 10))
        .expect("cold artists")
        .items
        .into_iter()
        .next()
        .and_then(|artist| artist.image_ref.and_then(|image_ref| image_ref.tag))
        .expect("cold artist tag");
    fs::write(&artist_image, [1_u8, 2_u8]).expect("replace artist image");
    let manifest = store
        .with_store(|store| store.load_local_manifest(&local.source_id))
        .expect("manifest");
    let warm =
        LocalSource::from_roots_with_manifest_cache(vec![root.clone()], local_identity, manifest)
            .expect("warm local provider");
    assert!(!warm.manifest_scan().library_changed);

    runtime
        .block_on(sync_local_source_with_events(
            &store,
            &local.source_id,
            &warm,
            events,
        ))
        .expect("warm local sync");

    let warm_tag = store
        .with_store(|store| store.load_artists(&local.source_id, false, 0, 10))
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
    let mut settings = StoredSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Source(saved.source_id.clone()));
    settings.sources.local_folders = vec![LocalLibraryFolder { path: path.clone() }];
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source_id)
        })
        .expect("save server");
    let (owners, events) = owners_from_store_for_test(store);
    owners.source.remove_local_library_folder(path);
    let snapshot = wait_for_source_presentation(&events);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Source(saved.source_id.clone()))
    );
    assert!(snapshot.local_folders.is_empty());
    let active = owners
        .source
        .store
        .with_store(|store| store.active_source())
        .expect("active server")
        .expect("active server");
    assert_eq!(active.source_id, saved.source_id);
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
    let mut settings = StoredSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    settings.sources.local_folders = vec![LocalLibraryFolder { path: path.clone() }];
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.save_source(&remote)?;
            store.set_active_source(&local.source_id)
        })
        .expect("save local source");
    let (owners, events) = owners_from_store_for_test(store);

    owners.source.remove_local_library_folder(path);

    let snapshot = wait_for_source_presentation(&events);
    assert_eq!(snapshot.selected_source, None);
    assert_eq!(snapshot.sources, vec![jellyfin_identity(&remote)]);
    assert!(snapshot.local_folders.is_empty());
    assert!(
        owners
            .source
            .store
            .with_store(|store| store.active_source())
            .expect("active source")
            .is_none()
    );
    assert!(current_active_source(&owners.source.active_source).is_none());
    assert!(owners.playback.playback_product_if_present().is_none());
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
    let mut settings = StoredSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    settings.sources.local_folders = vec![LocalLibraryFolder { path: path.clone() }];
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.save_source(&remote)?;
            store.set_active_source(&local.source_id)
        })
        .expect("save sources");
    let (owners, events) = owners_from_store_for_test(store);
    owners
        .source
        .secrets
        .save_token(remote.source_id.as_str(), "token")
        .expect("save token");

    let transition_lock = owners
        .source
        .source_transitions
        .commit
        .lock()
        .expect("transition lock");
    owners
        .source
        .select_source(LibrarySourceSelection::Source(remote.source_id.clone()));
    owners.source.remove_local_library_folder(path);
    drop(transition_lock);

    let snapshot = wait_for_source_presentation(&events);
    assert_eq!(snapshot.selected_source, None);
    assert_eq!(snapshot.sources, vec![jellyfin_identity(&remote)]);
    assert!(
        owners
            .source
            .store
            .with_store(|store| store.active_source())
            .expect("active source")
            .is_none()
    );
    assert!(current_active_source(&owners.source.active_source).is_none());
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
    let mut settings = StoredSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Source(remote.source_id.clone()));
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
            store.set_active_source(&remote.source_id)
        })
        .expect("save servers");
    let runtime = Runtime::new().expect("runtime");
    let (seed_events, _seed_receiver) = unbounded();
    let cold = LocalSource::from_roots_with_identity(
        vec![first.clone(), second.clone()],
        local_identity(&local),
    )
    .expect("cold local provider");
    runtime
        .block_on(sync_local_source_with_events(
            &store,
            &local.source_id,
            &cold,
            seed_events,
        ))
        .expect("seed local sync");
    assert_eq!(
        store
            .with_store(|store| store.load_tracks(&local.source_id, 0, 10))
            .expect("seed tracks")
            .total,
        2
    );
    let (owners, events) = owners_from_store_for_test(store);

    owners
        .source
        .remove_local_library_folder(first.to_string_lossy().into_owned());

    let snapshot = wait_for_source_presentation(&events);
    assert_eq!(
        snapshot.selected_source,
        Some(LibrarySourceSelection::Source(remote.source_id.clone()))
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let total = owners
            .source
            .store
            .with_store(|store| store.load_tracks(&local.source_id, 0, 10))
            .expect("poll tracks")
            .total;
        if total == 1 {
            break;
        }
        assert!(Instant::now() < deadline, "local sync did not prune root");
        thread::sleep(Duration::from_millis(25));
    }
    let tracks = owners
        .source
        .store
        .with_store(|store| store.load_tracks(&local.source_id, 0, 10))
        .expect("remaining tracks");
    assert_eq!(tracks.total, 1);
    assert_eq!(tracks.items[0].title, "Remaining");
    let active = owners
        .source
        .store
        .with_store(|store| store.active_source())
        .expect("active server")
        .expect("active server");
    assert_eq!(active.source_id, remote.source_id);
    let _cleanup_first = fs::remove_dir_all(first);
    let _cleanup_second = fs::remove_dir_all(second);
}

#[test]
pub(in crate::controller) fn inactive_manual_sync_failure_keeps_the_active_source() {
    let store = StoreHandle::open_memory().expect("memory store");
    let remote = saved_source();
    let mut failing = remote.clone();
    failing.source_id = SourceId::new("unsupported:inactive-sync");
    failing.kind = "unsupported".to_string();
    failing.name = "Inactive Sync".to_string();
    store
        .with_store(|store| {
            store.save_source(&remote)?;
            store.save_source(&failing)?;
            store.set_active_source(&remote.source_id)
        })
        .expect("seed sources");
    let (owners, events) = owners_from_store_for_test(store);
    owners
        .source
        .request_manual_source_sync(failing.source_id.clone());
    let running = wait_for_source_sync_change(&events, &failing.source_id);
    assert_eq!(running.phase, library_sync::SyncPhase::Running);
    assert!(running.manual);
    let failed = wait_for_source_sync_change(&events, &failing.source_id);
    assert_eq!(failed.epoch, running.epoch);
    assert_eq!(failed.phase, library_sync::SyncPhase::Failed);
    assert!(failed.manual);
    let active = owners
        .source
        .store
        .with_store(|store| store.active_source())
        .expect("active source")
        .expect("active source");
    assert_eq!(active.source_id, remote.source_id);
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
        .with_store(|store| store.source_cache_revision(&saved.source_id))
        .expect("cache revision");
    let cached = store
        .with_store(|store| store.load_tracks(&saved.source_id, 0, 10))
        .expect("cached tracks");
    let (owners, events) = owners_from_store_for_test(store);
    *owners.source.active_source.write().expect("active source") = None;
    owners
        .source
        .request_manual_source_sync(saved.source_id.clone());
    let failure = wait_for_sync_failure(&events, &saved.source_id);
    assert_eq!(failure, "No saved token found for the active server.");
    assert_eq!(
        owners
            .source
            .store
            .with_store(|store| store.source_cache_revision(&saved.source_id))
            .expect("final cache revision"),
        revision
    );
    assert_eq!(
        owners
            .source
            .store
            .with_store(|store| store.load_tracks(&saved.source_id, 0, 10))
            .expect("final cached tracks"),
        cached
    );
}

fn wait_for_library_commit(
    events: &ProductReceivers,
    source_id: &SourceId,
) -> (library_sync::LibraryCommitted, bool) {
    loop {
        match wait_for_typed_event(
            &events.library_sync,
            Duration::from_secs(5),
            "controller event",
        ) {
            library_sync::LibrarySyncEvent::Committed { update, manual }
                if update.source_id == *source_id =>
            {
                return (update, manual);
            }
            library_sync::LibrarySyncEvent::SyncChanged(state)
                if state.source_id == *source_id
                    && state.phase == library_sync::SyncPhase::Failed =>
            {
                panic!("source sync failed: {:?}", state.failure)
            }
            _ => {}
        }
    }
}
fn wait_for_sync_failure(events: &ProductReceivers, source_id: &SourceId) -> String {
    loop {
        match wait_for_typed_event(
            &events.library_sync,
            Duration::from_secs(5),
            "controller event",
        ) {
            library_sync::LibrarySyncEvent::SyncChanged(state)
                if state.source_id == *source_id
                    && state.phase == library_sync::SyncPhase::Failed =>
            {
                return state.failure.expect("typed sync failure");
            }
            library_sync::LibrarySyncEvent::Committed { update, .. }
                if update.source_id == *source_id =>
            {
                panic!("source sync unexpectedly committed")
            }
            _ => {}
        }
    }
}

fn wait_for_source_sync_change(
    events: &ProductReceivers,
    source_id: &SourceId,
) -> library_sync::SourceSyncChanged {
    loop {
        match wait_for_typed_event(
            &events.library_sync,
            Duration::from_secs(5),
            "controller event",
        ) {
            library_sync::LibrarySyncEvent::SyncChanged(state) if state.source_id == *source_id => {
                return state;
            }
            library_sync::LibrarySyncEvent::Committed { update, .. }
                if update.source_id == *source_id =>
            {
                panic!("source sync unexpectedly committed")
            }
            _ => {}
        }
    }
}

#[test]
pub(in crate::controller) fn startup_emit_status() {
    let (owners, events, _snapshot, _playback) = bootstrap_memory_for_test();
    let source_id = SourceId::new("server:unchanged");
    owners
        .source
        .store
        .with_store(|store| {
            store.save_source(
                &JellyfinSourceConfig {
                    credentials: CredentialSourceConfig {
                        source: SourceIdentity {
                            id: source_id.clone(),
                            kind: "jellyfin".to_string(),
                            name: "Saved server".to_string(),
                            base_url: "http://server.example.test".to_string(),
                        },
                        user_id: "user-id".to_string(),
                        username: "listener".to_string(),
                        trust_invalid_cert: false,
                    },
                    use_instant_mix: false,
                }
                .into_stored(),
            )?;
            store.set_active_source(&source_id)
        })
        .expect("save server");
    owners
        .source
        .secrets
        .save_token(source_id.as_str(), "test-token")
        .expect("save token");
    crate::source_setup::update_source(
        &owners.source,
        sources::SourceSettingsInput::Jellyfin(sources::JellyfinSettingsInput {
            credentials: sources::CredentialSettingsInput {
                source_id,
                name: "Saved server".to_string(),
                base_url: "http://server.example.test".to_string(),
                username: "listener".to_string(),
                password: String::new(),
                trust_invalid_cert: false,
            },
            use_instant_mix: false,
        }),
    );

    assert_eq!(wait_for_notice(&events), SourceNotice::NoChanges);
}
#[test]
pub(in crate::controller) fn active_local_sync_updates_manifest_delta() {
    let (store, local, root, _generation) = seed_cached_local_source("local-active-sync");
    let album_dir = root.join("Artist").join("Album");
    fs::write(album_dir.join("Second.mp3"), []).expect("audio");
    let (owners, events) = owners_from_store_for_test(store);
    owners
        .source
        .request_manual_source_sync(local.source_id.clone());
    let (update, manual) = wait_for_library_commit(&events, &local.source_id);
    assert!(manual);
    assert!(!update.delta.tracks.added.is_empty());
    let idle = wait_for_source_sync_change(&events, &local.source_id);
    assert_eq!(idle.phase, library_sync::SyncPhase::Idle);
    let tracks = owners
        .source
        .store
        .with_store(|store| store.load_tracks(&local.source_id, 0, 10))
        .expect("load local tracks")
        .items;
    assert_eq!(tracks.len(), 2);
    let _cleanup = fs::remove_dir_all(root);
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
    let mut settings = StoredSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    seed_cached_library(
        &store,
        &local,
        std::slice::from_ref(&album),
        std::slice::from_ref(&track),
        &[],
    );
    let section = HomeSection {
        kind: HomeSectionKind::Explore,
        albums: vec![section_album],
        tracks: vec![track],
    };

    let section = store
        .with_store(|store| {
            store.save_home_section_prefetch(&local.source_id, &section)?;
            store.load_home_section_prefetch(&local.source_id, HomeSectionKind::Explore)
        })
        .expect("reload projected Home section")
        .expect("saved Home section");

    assert_eq!(section.albums[0].image_ref.as_ref(), Some(&track_image_ref));
    assert_eq!(section.tracks[0].image_ref.as_ref(), Some(&track_image_ref));
}
#[test]
pub(in crate::controller) fn stale_track_images() {
    let (owners, _events, _snapshot, _playback) = bootstrap_memory_for_test();
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
        &owners.source.store,
        &local,
        std::slice::from_ref(&album),
        &tracks,
        &[],
    );

    let page = owners
        .library
        .library_query(local.source_id.clone())
        .tracks_page(library::TrackSort::Title, false, 0, 10)
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
    let album_artist = library::Artist {
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
        representative_albums: Vec::new(),
    };
    let mut settings = StoredSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.set_active_source(&local.source_id)?;
            let generation = store.begin_sync(&local.source_id)?;
            commit_cached_library(
                store,
                &local.source_id,
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
    let (owners, _events) = owners_from_store_for_test(store);

    let artists = owners
        .library
        .library_query(local.source_id.clone())
        .artists_page(true, 0, 10)
        .expect("cached album artists")
        .items;

    let guest = artists
        .iter()
        .find(|artist| artist.name == "Guest Artist")
        .expect("guest album artist");
    assert_eq!(
        guest
            .representative_albums
            .first()
            .and_then(|album| album.image_ref.as_ref()),
        Some(&fallback_ref)
    );
}

#[test]
pub(in crate::controller) fn album_projection_binds_track_fallback_art_before_route_read() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = SubsonicSourceConfig {
        credentials: CredentialSourceConfig {
            source: SourceIdentity {
                id: SourceId::new("remote:server:album-binding"),
                kind: "subsonic".to_string(),
                name: "Remote Library".to_string(),
                base_url: "https://library.example.test".to_string(),
            },
            user_id: "user".to_string(),
            username: "listener".to_string(),
            trust_invalid_cert: false,
        },
    }
    .into_stored();
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
            store.set_active_source(&saved.source_id)?;
            let generation = store.begin_sync(&saved.source_id)?;
            commit_cached_library(
                store,
                &saved.source_id,
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
        .with_store(|store| store.load_raw_album_image_refs(&saved.source_id))
        .expect("raw album refs");
    let selected = store
        .with_store(|store| store.load_albums(&saved.source_id, 0, 10))
        .expect("selected albums");

    assert_eq!(raw_refs.get(&album.id), Some(&None));
    assert_eq!(selected.items[0].image_ref.as_ref(), Some(&fallback_ref));
}

#[test]
pub(in crate::controller) fn genre_projection_derives_live_album_track_art() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = SubsonicSourceConfig {
        credentials: CredentialSourceConfig {
            source: SourceIdentity {
                id: SourceId::new("remote:server:genre-binding"),
                kind: "subsonic".to_string(),
                name: "Remote Library".to_string(),
                base_url: "https://library.example.test".to_string(),
            },
            user_id: "user".to_string(),
            username: "listener".to_string(),
            trust_invalid_cert: false,
        },
    }
    .into_stored();
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
        image_ref: None,
        representative_albums: Vec::new(),
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
            store.set_active_source(&saved.source_id)?;
            let generation = store.begin_sync(&saved.source_id)?;
            commit_cached_library(
                store,
                &saved.source_id,
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
        .with_store(|store| store.load_genres(&saved.source_id, 0, 10))
        .expect("load genres")
        .items;

    assert_eq!(genres.len(), 1);
    assert_eq!(
        genres[0]
            .representative_albums
            .first()
            .and_then(|album| album.image_ref.as_ref()),
        Some(&fallback_ref)
    );
}

#[test]
pub(in crate::controller) fn track_projection_binds_album_fallback_art_before_route_read() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = SubsonicSourceConfig {
        credentials: CredentialSourceConfig {
            source: SourceIdentity {
                id: SourceId::new("remote:server:track-binding"),
                kind: "subsonic".to_string(),
                name: "Remote Library".to_string(),
                base_url: "https://library.example.test".to_string(),
            },
            user_id: "user".to_string(),
            username: "listener".to_string(),
            trust_invalid_cert: false,
        },
    }
    .into_stored();
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
            store.set_active_source(&saved.source_id)?;
            let generation = store.begin_sync(&saved.source_id)?;
            commit_cached_library(
                store,
                &saved.source_id,
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
        .with_store(|store| store.load_raw_track_image_refs(&saved.source_id))
        .expect("raw track refs");
    let selected = store
        .with_store(|store| store.load_tracks(&saved.source_id, 0, 10))
        .expect("selected tracks");

    assert_eq!(raw_refs.get(&track.id), Some(&None));
    assert_eq!(selected.items[0].image_ref, None);
    assert_eq!(
        selected.items[0]
            .album_artwork
            .as_ref()
            .and_then(|artwork| artwork.image_ref.as_ref()),
        Some(&album_ref)
    );
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
    let artist = library::Artist {
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
        representative_albums: Vec::new(),
    };
    let mut settings = StoredSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.set_active_source(&local.source_id)?;
            let generation = store.begin_sync(&local.source_id)?;
            commit_cached_library(
                store,
                &local.source_id,
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
    let (owners, _events) = owners_from_store_for_test(store);

    let artists = owners
        .library
        .library_query(local.source_id.clone())
        .artists_page(false, 0, 10)
        .expect("cached artists")
        .items;

    assert_eq!(artists.len(), 1);
    assert_eq!(
        artists[0]
            .representative_albums
            .first()
            .and_then(|album| album.image_ref.as_ref()),
        Some(&fallback_ref)
    );
}

#[test]
pub(in crate::controller) fn startup_query_includes_artist_grid_fallback_art() {
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
    let guest_album_artist = library::Artist {
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
        representative_albums: Vec::new(),
    };
    let singer = library::Artist {
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
        representative_albums: Vec::new(),
    };
    let mut settings = StoredSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    store.save_settings(&settings).expect("save settings");
    store
        .with_store(|store| {
            store.save_source(&local)?;
            store.set_active_source(&local.source_id)?;
            let generation = store.begin_sync(&local.source_id)?;
            commit_cached_library(
                store,
                &local.source_id,
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

    let query = store.library_access().query(local.source_id);
    let artist = query
        .artists_page(false, 0, 10)
        .expect("artists")
        .items
        .into_iter()
        .find(|artist| artist.name == "Singer Snapshot")
        .expect("artist snapshot row");
    assert_eq!(
        artist
            .representative_albums
            .first()
            .and_then(|album| album.image_ref.as_ref()),
        Some(&track_ref)
    );
    let album_artist = query
        .artists_page(true, 0, 10)
        .expect("album artists")
        .items
        .into_iter()
        .find(|artist| artist.name == "Guest Snapshot")
        .expect("album artist snapshot row");
    assert_eq!(
        album_artist
            .representative_albums
            .first()
            .and_then(|album| album.image_ref.as_ref()),
        Some(&album_ref)
    );
}

#[test]
pub(in crate::controller) fn startup_remote_cache() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = saved_source();
    let album = remote_album_with_image_ref(provider_cover_ref());
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            let generation = store.begin_sync(&saved.source_id)?;
            commit_cached_library(
                store,
                &saved.source_id,
                generation,
                CachedLibraryObservation {
                    albums: vec![album.clone()],
                    ..CachedLibraryObservation::default()
                },
            )
        })
        .expect("seed remote cache");
    let albums = store
        .with_store(|store| store.load_albums(&saved.source_id, 0, 1))
        .expect("load remote cache");
    let cached_album = albums.items.first().expect("cached album");

    assert_eq!(albums.total, 1);
    assert_eq!(cached_album.id, album.id);
    assert_eq!(cached_album.image_ref.as_ref(), album.image_ref.as_ref());
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
        .with_store(|store| store.load_home_sections(&saved.source_id))
        .expect("load stale home sections");
    let before_sync = store
        .with_store(|store| store.sync_state(&saved.source_id))
        .expect("sync state before refresh");
    assert_eq!(before[0].albums[0].id, stale_album.id);
    assert_eq!(before[1].tracks[0].id, stale_track.id);

    store
        .with_store(|store| {
            store.upsert_albums(
                &saved.source_id,
                std::slice::from_ref(&fresh_album),
                before_sync.generation,
            )?;
            store.upsert_tracks(
                &saved.source_id,
                std::slice::from_ref(&fresh_track),
                before_sync.generation,
            )?;
            store.upsert_home_sections(&saved.source_id, &fresh_sections, before_sync.generation)
        })
        .expect("replace home sections");

    let after = store
        .with_store(|store| store.load_home_sections(&saved.source_id))
        .expect("load refreshed home sections");
    let sync_state = store
        .with_store(|store| store.sync_state(&saved.source_id))
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
        image_ref: stale_track.image_ref.clone(),
        representative_albums: Vec::new(),
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
        image_ref: fresh_track.image_ref.clone(),
        representative_albums: Vec::new(),
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
        representative_albums: Vec::new(),
    };
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source_id)?;
            let generation = store.begin_sync(&saved.source_id)?;
            commit_cached_library(
                store,
                &saved.source_id,
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
        .with_store(|store| store.load_playlists(&saved.source_id, 0, 10))
        .expect("load stale playlists");
    assert_eq!(before.total, 1);
    assert_eq!(before.items[0].id, stale_playlist.id);
    let before_sync = store
        .with_store(|store| store.sync_state(&saved.source_id))
        .expect("sync state before playlist refresh");

    let delta = store
        .with_store_session(|store| {
            let generation = store
                .begin_sync(&saved.source_id)
                .map_err(|error| error.to_string())?;
            let base_sync_input_revision = store
                .source_sync_input_revision(&saved.source_id)
                .map_err(|error| error.to_string())?;
            store
                .commit_library_sync(
                    &saved.source_id,
                    generation,
                    base_sync_input_revision,
                    library::LibrarySync {
                        albums: vec![album.clone()],
                        tracks: vec![fresh_track.clone()],
                        artists: vec![sync_artist.clone()],
                        album_artists: vec![sync_artist.clone()],
                        genres: Vec::new(),
                        playlists: vec![library::PlaylistSnapshot {
                            playlist: fresh_playlist.clone(),
                            entries: vec![library::PlaylistEntryKey {
                                entry_id: fresh_entry.entry_id.clone(),
                                track_id: fresh_entry.track.id.clone(),
                            }],
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
        .with_store(|store| store.load_playlists(&saved.source_id, 0, 10))
        .expect("load refreshed playlists");
    let detail = store
        .with_store(|store| store.load_playlist_detail(&saved.source_id, &fresh_playlist.id))
        .expect("load playlist detail")
        .expect("playlist detail");
    let sync_state = store
        .with_store(|store| store.sync_state(&saved.source_id))
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
    cache_home_section(
        &store,
        &saved.source_id,
        &HomeSection {
            kind: HomeSectionKind::Explore,
            albums: vec![fresh_album.clone()],
            tracks: Vec::new(),
        },
    )
    .expect("replace Explore");
    let after = store
        .with_store(|store| store.load_home_sections(&saved.source_id))
        .expect("load Explore replacement");
    assert_eq!(after[0].kind, HomeSectionKind::Explore);
    assert_eq!(after[0].albums[0].id, fresh_album.id);
    assert_eq!(after[1].kind, HomeSectionKind::MostPlayed);
    assert_eq!(after[1].tracks[0].id, stale_track.id);

    cache_home_section(
        &store,
        &saved.source_id,
        &HomeSection {
            kind: HomeSectionKind::MostPlayed,
            albums: Vec::new(),
            tracks: vec![fresh_track.clone()],
        },
    )
    .expect("replace Most Played");
    let after = store
        .with_store(|store| store.load_home_sections(&saved.source_id))
        .expect("load Most Played replacement");
    assert_eq!(after[0].kind, HomeSectionKind::Explore);
    assert_eq!(after[0].albums[0].id, fresh_album.id);
    assert_eq!(after[1].kind, HomeSectionKind::MostPlayed);
    assert_eq!(after[1].tracks[0].id, fresh_track.id);
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
    store
        .with_store(|store| store.save_home_section_prefetch(&saved.source_id, &prefetched))
        .expect("stage prefetched Explore");
    let visible_before = store
        .with_store(|store| store.load_home_sections(&saved.source_id))
        .expect("load visible sections");
    assert_eq!(visible_before[0].albums[0].id, stale_album.id);
    assert_eq!(prefetched.albums[0].id, fresh_album.id);
    assert!(
        store
            .with_store(|store| {
                store.load_home_section_prefetch(&saved.source_id, HomeSectionKind::Explore)
            })
            .expect("load prefetched Explore")
            .is_some()
    );
    save_home_section_projection(&store, &saved.source_id, &visible)
        .expect("prefer persisted prefetch over the older rotation");
    let visible_after = store
        .with_store(|store| store.load_home_sections(&saved.source_id))
        .expect("load promoted sections");
    assert_eq!(visible_after[0].albums[0].id, fresh_album.id);
    assert!(
        store
            .with_store(|store| {
                store.load_home_section_prefetch(&saved.source_id, HomeSectionKind::Explore)
            })
            .expect("load cleared prefetched Explore")
            .is_none()
    );
}
#[test]
pub(in crate::controller) fn startup_emit_source_presentation() {
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
    let (owners, events) = owners_from_store_for_test(store);

    owners.source.clear_active_source_cache();

    let snapshot = wait_for_source_presentation(&events);
    assert!(!snapshot.first_run);
    assert_eq!(snapshot.source.expect("server").id, saved.source_id);
    let query = owners.library.library_query(saved.source_id);
    assert_eq!(query.albums_page(0, 1).expect("albums").total, 0);
    assert_eq!(
        query
            .tracks_page(library::TrackSort::Title, false, 0, 1)
            .expect("tracks")
            .total,
        0
    );
}
