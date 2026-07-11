use super::*;
use source::RandomTrackRequest;

struct DeleteFailingSecretStore(Sender<()>);
impl SecretStore for DeleteFailingSecretStore {
    fn save_secret(&self, _key: &secrets::SecretKey, _secret: &str) -> secrets::SecretResult<()> {
        Ok(())
    }

    fn load_secret(&self, _key: &secrets::SecretKey) -> secrets::SecretResult<Option<String>> {
        Ok(Some("token".to_string()))
    }

    fn delete_secret(&self, _key: &secrets::SecretKey) -> secrets::SecretResult<()> {
        let _sent = self.0.send(());
        Err(secrets::SecretError::Backend("delete failed".to_string()))
    }
}

fn enable_auto_dj_for_test(controller: &AppController, events: &Receiver<ControllerEvent>) {
    controller.toggle_auto_dj();
    let playback = wait_for_playback_auto_dj(events, true);
    assert!(playback.auto_dj_enabled);
}

fn playback_test_track(number: u32) -> Track {
    let mut track = library_track(
        number,
        Some(ArtistId::new("test:artist:playback")),
        AlbumId::new("test:album:playback"),
        "Artist",
        &[],
    );
    track.id = TrackId::new(format!("test:track:{number}"));
    track
}

fn local_playback_test_track(number: u32, path: &Path) -> Track {
    let mut track = playback_test_track(number);
    track.id = TrackId::new(format!("local:track:playback-{number}"));
    track.album_id = AlbumId::new("local:album:playback");
    track.artist_id = Some(ArtistId::new("local:artist:playback"));
    track.local_path = Some(path.to_string_lossy().into_owned());
    track
}

fn playback_test_queue(source_id: SourceId, tracks: &[Track]) -> QueueEngine {
    let mut queue = QueueEngine::new(source_id);
    let mut tracks = tracks.iter();
    if let Some(first) = tracks.next() {
        queue.play_now(first);
    }
    for track in tracks {
        queue.append(track);
    }
    queue
}

fn set_playback_test_state(
    controller: &AppController,
    queue: QueueEngine,
    state: PlaybackState,
    position_seconds: u32,
) {
    let current = queue.current().cloned();
    let snapshot = PlaybackSnapshot {
        current_source_id: Some(queue.source_id().clone()),
        duration_seconds: current
            .as_ref()
            .map(|entry| entry.duration_seconds)
            .unwrap_or_default(),
        current,
        state,
        position_seconds,
        position_millis: u64::from(position_seconds) * 1_000,
        ..PlaybackSnapshot::default()
    };
    *controller.queue.lock().expect("queue") = Some(queue);
    *controller
        .playback_snapshot
        .lock()
        .expect("playback snapshot") = snapshot;
}

fn sync_real_local_root(controller: &AppController, root: &Path) -> SavedSource {
    let mut settings = AppSettings::default();
    settings.sources.selected = Some(LibrarySourceSelection::Local);
    settings.sources.local_folders = vec![LocalLibraryFolder {
        path: root.to_string_lossy().into_owned(),
    }];
    controller
        .store
        .save_settings(&settings)
        .expect("save local settings");
    let saved = local_source_saved();
    controller
        .store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source.id)
        })
        .expect("seed local source");
    let active =
        crate::sources::activate_configured_source(&controller.store, &controller.secrets, &saved)
            .expect("activate local source");
    *controller.active_source.write().expect("active source") = Some(Arc::clone(&active));
    let source =
        LocalSource::from_roots_with_identity(vec![root.to_path_buf()], saved.source.clone())
            .expect("load local source");
    controller
        .runtime
        .block_on(sync_local_source_outcome(
            &controller.store,
            &saved.source.id,
            &source,
        ))
        .expect("sync local source");
    saved
}

#[test]
pub(in crate::controller) fn cover_emit_fetching() {
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let path = test_cover_path("ready-cached");
    fs::write(&path, [1_u8, 2, 3]).expect("write cover");
    let image_ref = provider_cover_ref();
    seed_cover_cache(&controller, &image_ref, 256, &path);
    let key = controller.cover_key(&image_ref, 256).expect("cover key");
    controller.request_cover(image_ref, 256);
    assert_eq!(wait_for_cover_ready(&events, &key), path);
    let _cleanup = fs::remove_file(path);
}
#[test]
pub(in crate::controller) fn cover_fetch_missing() {
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let root = unique_test_dir("local-cover-request");
    let _cleanup = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("local root");
    fs::write(root.join("track.mp3"), []).expect("track file");
    let cover_bytes = [0xff_u8, 0xd8, 0xff, 0xd9];
    fs::write(root.join("cover.jpg"), cover_bytes).expect("cover file");

    let saved = sync_real_local_root(&controller, &root);
    let source_id = saved.source.id.clone();
    let image_ref = controller
        .store
        .with_store(|store| store.load_albums(&source_id, 0, 1))
        .expect("load albums")
        .items
        .into_iter()
        .next()
        .and_then(|album| album.image_ref)
        .expect("album image ref");
    let key = controller.cover_key(&image_ref, 256).expect("cover key");

    controller.request_cover_for_key(key.clone(), image_ref, 256);
    let path = wait_for_cover_ready(&events, &key);

    assert_eq!(fs::read(&path).expect("cached cover"), cover_bytes);
    let _cleanup = fs::remove_file(path);
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn external_cache_cover() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let path = test_cover_path("external-cached");
    fs::write(&path, [1_u8, 2, 3]).expect("write cover");
    let image_ref = external_cover_ref();
    seed_cover_cache(&controller, &image_ref, 256, &path);
    assert_eq!(
        controller.cached_cover_path(&image_ref, 256),
        Some(path.clone())
    );
    assert_eq!(controller.cached_cover_path(&image_ref, 512), None);
    assert_eq!(
        controller.cached_cover_path(&image_ref, 96),
        Some(path.clone())
    );
    let _cleanup = fs::remove_file(path);
}
#[test]
pub(in crate::controller) fn cover_external_size() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let image_ref = external_cover_ref();
    seed_external_cover_miss(&controller, &image_ref, 256);

    assert!(controller.external_cover_lookup_known_missing(&image_ref, 96));
    assert!(controller.external_cover_lookup_known_missing(&image_ref, 512));
    assert!(!controller.external_cover_lookup_known_missing(
        &ImageRef::new("jellyfin:album:one", Some("tag-one".to_string())),
        256
    ));
}

#[test]
pub(in crate::controller) fn cover_clear_key() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let image_ref = external_cover_ref();
    seed_external_cover_miss(&controller, &image_ref, 256);
    let key = controller.cover_key(&image_ref, 256).expect("cover key");
    let generation_before = controller
        .external_cover_retry_generation
        .load(Ordering::SeqCst);
    controller
        .cover_in_flight
        .lock()
        .expect("cover in-flight lock")
        .insert(key, generation_before);

    controller
        .retry_external_cover_lookups()
        .expect("retry external covers");

    assert!(!controller.external_cover_lookup_known_missing(&image_ref, 256));
    assert!(
        controller
            .cover_in_flight
            .lock()
            .expect("cover in-flight lock")
            .is_empty()
    );
    assert_eq!(
        controller
            .external_cover_retry_generation
            .load(Ordering::SeqCst),
        generation_before.saturating_add(1)
    );
}

#[test]
pub(in crate::controller) fn cover_emit_unavailable() {
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let image_ref = external_cover_ref();
    seed_external_cover_miss(&controller, &image_ref, 256);
    let key = controller.cover_key(&image_ref, 256).expect("cover key");

    controller.request_cover_for_key(key.clone(), image_ref, 256);

    loop {
        match events
            .recv_timeout(Duration::from_secs(5))
            .expect("controller event")
        {
            ControllerEvent::CoverUnavailable {
                key: event_key,
                external_retry_generation,
            } if event_key == key => {
                assert_eq!(external_retry_generation, Some(0));
                return;
            }
            ControllerEvent::CoverReady { key: event_key, .. } if event_key == key => {
                panic!("known missing cover unexpectedly became ready");
            }
            ControllerEvent::Snapshot(_)
            | ControllerEvent::SourceSelectionChanged { .. }
            | ControllerEvent::SourceSyncChanged(_)
            | ControllerEvent::LibraryCommitted(_)
            | ControllerEvent::LibraryDelta(_)
            | ControllerEvent::HomeSectionsUpdated { .. }
            | ControllerEvent::PlaylistChanged { .. }
            | ControllerEvent::SmartPlaylistChanged { .. }
            | ControllerEvent::FavoriteChanged { .. }
            | ControllerEvent::Queue(_)
            | ControllerEvent::Playback(_)
            | ControllerEvent::Visualizer(_)
            | ControllerEvent::Lyrics { .. }
            | ControllerEvent::LyricsSearchResults { .. }
            | ControllerEvent::LyricsSearchFailed { .. }
            | ControllerEvent::SearchLoaded { .. }
            | ControllerEvent::SearchFailed { .. }
            | ControllerEvent::LyricsSaved { .. }
            | ControllerEvent::FolderLoaded { .. }
            | ControllerEvent::FolderLoadFailed { .. }
            | ControllerEvent::HomeSectionPrefetched { .. }
            | ControllerEvent::ServerDiscovery { .. }
            | ControllerEvent::CoverReady { .. }
            | ControllerEvent::CoverUnavailable { .. }
            | ControllerEvent::CoverDeferred { .. }
            | ControllerEvent::SourceNotice(_)
            | ControllerEvent::SourceTransitionFailed { .. } => {}
            ControllerEvent::FavoriteChangeFailed { error, .. } => {
                panic!("favorite change failed: {error}");
            }
            ControllerEvent::Error(error) => panic!("controller error: {error}"),
        }
    }
}

#[test]
pub(in crate::controller) fn cover_known_external_miss_bypasses_fetch_slots() {
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let image_ref = external_cover_ref();
    seed_external_cover_miss(&controller, &image_ref, 256);
    let key = controller.cover_key(&image_ref, 256).expect("cover key");

    {
        let (lock, _ready) = &*controller.cover_slots;
        *lock.lock().expect("cover slots") = 2;
    }

    assert!(controller.request_cover_for_key(key.clone(), image_ref, 256));

    let event = events.recv_timeout(Duration::from_secs(1));
    {
        let (lock, ready) = &*controller.cover_slots;
        *lock.lock().expect("cover slots") = 0;
        ready.notify_all();
    }

    assert!(matches!(
        event.expect("controller event"),
        ControllerEvent::CoverUnavailable {
            key: event_key,
            external_retry_generation: Some(0),
        } if event_key == key
    ));
}

#[test]
pub(in crate::controller) fn cache_cover_reuse() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let path = test_cover_path("provider-cached");
    fs::write(&path, [1_u8, 2, 3]).expect("write cover");
    let image_ref = provider_cover_ref();
    seed_cover_cache(&controller, &image_ref, 256, &path);
    assert_eq!(
        controller.cached_cover_path(&image_ref, 256),
        Some(path.clone())
    );
    assert_eq!(controller.cached_cover_path(&image_ref, 512), None);
    assert_eq!(
        controller.cached_cover_path(&image_ref, 96),
        Some(path.clone())
    );
    let _cleanup = fs::remove_file(path);
}
#[test]
pub(in crate::controller) fn cover_thumbnail_request() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let path = test_cover_path("thumbnail");
    fs::write(&path, [1_u8, 2, 3]).expect("write cover");
    let image_ref = provider_cover_ref();
    seed_cover_cache(&controller, &image_ref, 96, &path);
    assert_eq!(controller.cached_cover_path(&image_ref, 256), None);
    assert_eq!(
        controller.cached_cover_path(&image_ref, 96),
        Some(path.clone())
    );
    let _cleanup = fs::remove_file(path);
}
#[test]
pub(in crate::controller) fn cover_read_lookup() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let path = test_cover_path("missing-cached");
    let _cleanup = fs::remove_file(&path);
    let image_ref = provider_cover_ref();
    let source_id = seed_cover_cache(&controller, &image_ref, 256, &path);
    assert_eq!(controller.cached_cover_path(&image_ref, 256), None);
    assert_eq!(
        controller
            .store
            .with_store(|store| store.load_cover_cache_entry(
                &source_id,
                &image_ref.item_id,
                "tag-one",
                256
            ))
            .expect("load cover cache"),
        Some(CoverCacheEntry {
            source_id,
            item_id: image_ref.item_id,
            image_tag: "tag-one".to_string(),
            size: 256,
            path: path.to_string_lossy().to_string(),
        })
    );
}

#[test]
pub(in crate::controller) fn cover_reuses_external_content_for_local_source() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let path = test_cover_path("external-content-local");
    fs::write(&path, [1_u8, 2, 3]).expect("write cover");
    let remote = saved_source();
    let local = local_source_saved();
    let image_ref = ImageRef::new(
        "external:mb-release-group:group-one",
        Some("external-v2-test".to_string()),
    );
    controller
        .store
        .with_store(|store| {
            store.save_source(&remote)?;
            store.save_source(&local)?;
            store.save_cover_cache_entry(&CoverCacheEntry {
                source_id: remote.source.id.clone(),
                item_id: image_ref.item_id.clone(),
                image_tag: image_ref.tag.clone().expect("external tag"),
                size: 256,
                path: path.to_string_lossy().to_string(),
            })?;
            store.set_active_source(&local.source.id)
        })
        .expect("seed external cache");

    assert_eq!(
        controller.cached_cover_path(&image_ref, 256),
        Some(path.clone())
    );
    let _cleanup = fs::remove_file(path);
}

#[test]
pub(in crate::controller) fn cover_forget_source_removes_snapshot_and_token() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = saved_source();
    let source_id = saved.source.id.clone();
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&source_id)
        })
        .expect("seed source");
    let (controller, events) = controller_from_store_for_test(store);
    controller
        .secrets
        .save_token(&source_id, "token")
        .expect("save token");
    controller.forget_source(source_id.clone());

    let snapshot = wait_for_snapshot(&events);
    assert!(snapshot.first_run);
    assert!(snapshot.source.is_none());
    assert!(snapshot.sources.is_empty());
    assert_eq!(
        controller
            .store
            .with_store(|store| store.list_sources())
            .expect("servers"),
        Vec::new()
    );
    wait_for_token_deleted(&controller.secrets, &source_id);
}
#[test]
pub(in crate::controller) fn cover_delete_fails() {
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = saved_source();
    let source_id = saved.source.id.clone();
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&source_id)
        })
        .expect("seed source");
    let (mut controller, events) = controller_from_store_for_test(store);
    let (delete_entered, delete_observed) = channel();
    controller.secrets = Arc::new(DeleteFailingSecretStore(delete_entered));

    controller.forget_source(source_id);

    let snapshot = wait_for_snapshot(&events);
    assert!(snapshot.first_run);
    assert!(snapshot.source.is_none());
    assert!(snapshot.sources.is_empty());
    assert_eq!(
        controller
            .store
            .with_store(|store| store.list_sources())
            .expect("servers"),
        Vec::new()
    );
    delete_observed
        .recv_timeout(Duration::from_secs(1))
        .expect("secret deletion attempted");
}
#[test]
pub(in crate::controller) fn cover_persist_queue() {
    let root = unique_test_dir("persisted-local-queue");
    let _cleanup = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create local playback root");
    let path = root.join("track.flac");
    fs::write(&path, []).expect("write local track");
    let store = StoreHandle::open_memory().expect("memory store");
    let saved = local_source_saved();
    let track = local_playback_test_track(1, &path);
    seed_cached_library(&store, &saved, &[], std::slice::from_ref(&track), &[]);
    let (controller, events) = controller_from_store_for_test(store);
    controller.play_now(track.clone());
    let queue = wait_for_queue(&events).expect("queue");
    assert_eq!(queue.entries.len(), 1);
    assert_eq!(queue.entries[0].track_id, track.id.clone());
    assert_eq!(
        controller
            .store
            .with_store(|store| store.load_queue_snapshot(&queue.source_id))
            .expect("store")
            .expect("snapshot")
            .entries
            .len(),
        1
    );
    let playback = wait_for_playback_state(&controller, &events, PlaybackState::Playing);
    assert_eq!(playback.current.expect("current").track_id, track.id);
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn cover_start_stream() {
    let root = unique_test_dir("local-playback-prepare");
    let _cleanup = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create local playback root");
    let first_path = root.join("first.flac");
    let second_path = root.join("second.flac");
    fs::write(&first_path, []).expect("write first local track");
    fs::write(&second_path, []).expect("write second local track");
    let first = local_playback_test_track(1, &first_path);
    let second = local_playback_test_track(2, &second_path);
    let store = StoreHandle::open_memory().expect("memory store");
    seed_cached_library(
        &store,
        &local_source_saved(),
        &[],
        &[first.clone(), second.clone()],
        &[],
    );
    let (controller, _events) = controller_from_store_for_test(store);
    let commands = Arc::new(Mutex::new(Vec::new()));
    *controller.playback.lock().expect("playback") =
        Box::new(RecordingPlaybackBackend::new(Arc::clone(&commands)));
    controller.play_tracks_now(vec![first.clone(), second.clone()]);
    let command = wait_for_recorded_command(&commands, |command| {
        matches!(command, PlaybackCommand::PlayPrepared { .. })
    });
    let PlaybackCommand::PlayPrepared { item, next, .. } = command else {
        panic!("expected prepared play command");
    };
    assert_eq!(item.track.id, first.id);
    let first_uri = reqwest::Url::from_file_path(&first_path)
        .expect("first file URI")
        .to_string();
    assert_eq!(item.stream.uri(), first_uri);
    assert!(next.is_none());
    let command = wait_for_recorded_command(&commands, |command| {
        matches!(command, PlaybackCommand::PrepareNext(Some(_)))
    });
    let PlaybackCommand::PrepareNext(Some(item)) = command else {
        panic!("expected prepared next command");
    };
    assert_eq!(item.track.id, second.id);
    let second_uri = reqwest::Url::from_file_path(&second_path)
        .expect("second file URI")
        .to_string();
    assert_eq!(item.stream.uri(), second_uri);
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn stopped_next_restarts_current() {
    let root = unique_test_dir("stopped-next-local");
    let _cleanup = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create local playback root");
    let path = root.join("current.flac");
    fs::write(&path, []).expect("write local track");
    let track = local_playback_test_track(1, &path);
    let store = StoreHandle::open_memory().expect("memory store");
    seed_cached_library(
        &store,
        &local_source_saved(),
        &[],
        std::slice::from_ref(&track),
        &[],
    );
    let (controller, _events) = controller_from_store_for_test(store);
    let commands = Arc::new(Mutex::new(Vec::new()));
    *controller.playback.lock().expect("playback") =
        Box::new(RecordingPlaybackBackend::new(Arc::clone(&commands)));
    controller.play_now(track);
    let _play = wait_for_recorded_command(&commands, |command| {
        matches!(command, PlaybackCommand::PlayPrepared { .. })
    });
    controller.cycle_repeat();
    controller.cycle_repeat();
    controller.stop();
    commands.lock().expect("commands").clear();

    controller.next_track();

    let command = wait_for_recorded_command(&commands, |command| {
        matches!(
            command,
            PlaybackCommand::PlayPrepared { .. } | PlaybackCommand::SeekMillis(0)
        )
    });
    assert!(matches!(command, PlaybackCommand::PlayPrepared { .. }));
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn manual_next_silences_current_audio_before_start() {
    let root = unique_test_dir("manual-next-local");
    let _cleanup = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create local playback root");
    let first_path = root.join("first.flac");
    let second_path = root.join("second.flac");
    fs::write(&first_path, []).expect("write first local track");
    fs::write(&second_path, []).expect("write second local track");
    let saved = local_source_saved();
    let first = local_playback_test_track(1, &first_path);
    let second = local_playback_test_track(2, &second_path);
    let store = StoreHandle::open_memory().expect("memory store");
    seed_cached_library(&store, &saved, &[], &[first.clone(), second.clone()], &[]);
    let (controller, _events) = controller_from_store_for_test(store);
    let queue = playback_test_queue(saved.source.id, &[first, second.clone()]);
    set_playback_test_state(&controller, queue, PlaybackState::Playing, 0);
    let commands = Arc::new(Mutex::new(Vec::new()));
    *controller.playback.lock().expect("playback") =
        Box::new(RecordingPlaybackBackend::new(Arc::clone(&commands)));

    controller.next_track();

    let command = wait_for_recorded_command(&commands, |command| {
        matches!(command, PlaybackCommand::PlayPrepared { .. })
    });
    let PlaybackCommand::PlayPrepared { item, .. } = command else {
        panic!("expected prepared play command");
    };
    assert_eq!(item.track.id, second.id);
    let commands = commands.lock().expect("commands");
    let silence_index = commands
        .iter()
        .position(|command| matches!(command, PlaybackCommand::Silence))
        .expect("silence command");
    let play_index = commands
        .iter()
        .position(|command| matches!(command, PlaybackCommand::PlayPrepared { .. }))
        .expect("prepared play command");
    assert!(silence_index < play_index);
    assert!(
        commands
            .iter()
            .all(|command| !matches!(command, PlaybackCommand::Stop))
    );
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn playback_duplicate_current_start_ignored() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let source_id = SourceId::new("test:source:duplicate-start");
    let first = playback_test_track(1);
    let second = playback_test_track(2);
    let queue = playback_test_queue(source_id, &[first, second]);
    set_playback_test_state(&controller, queue, PlaybackState::Playing, 0);
    let commands = Arc::new(Mutex::new(Vec::new()));
    *controller.playback.lock().expect("playback") =
        Box::new(RecordingPlaybackBackend::new(Arc::clone(&commands)));

    controller.start_current_track();
    std::thread::sleep(std::time::Duration::from_millis(150));

    assert!(commands.lock().expect("commands").is_empty());
}

#[test]
pub(in crate::controller) fn playback_current_commits_before_backend_accepts_start() {
    let root = unique_test_dir("blocking-start-local");
    let _cleanup = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create local playback root");
    let first_path = root.join("first.flac");
    let second_path = root.join("second.flac");
    fs::write(&first_path, []).expect("write first local track");
    fs::write(&second_path, []).expect("write second local track");
    let first = local_playback_test_track(1, &first_path);
    let second = local_playback_test_track(2, &second_path);
    let store = StoreHandle::open_memory().expect("memory store");
    seed_cached_library(
        &store,
        &local_source_saved(),
        &[],
        &[first.clone(), second.clone()],
        &[],
    );
    let (controller, _events) = controller_from_store_for_test(store);
    let mut queue = QueueEngine::new(local_source_saved().source.id);
    queue.play_now(&first);
    let second_entry = queue.append(&second);
    *controller.queue.lock().expect("queue") = Some(queue);

    let blocked_commands = Arc::new(Mutex::new(Vec::new()));
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *controller.playback.lock().expect("playback") = Box::new(BlockingPlaybackBackend::new(
        Arc::clone(&blocked_commands),
        entered_tx,
        release_rx,
    ));

    controller.activate_queue_entry(second_entry);
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("backend start entered");

    let playback = controller
        .playback_snapshot
        .lock()
        .expect("playback snapshot")
        .clone();
    assert_eq!(playback.state, PlaybackState::Buffering);
    assert_eq!(playback.current.expect("current").track_id, second.id);
    assert!(
        !blocked_commands
            .lock()
            .expect("commands")
            .iter()
            .any(|command| {
                matches!(
                    command,
                    PlaybackCommand::Play { .. } | PlaybackCommand::PlayPrepared { .. }
                )
            })
    );

    release_tx.send(()).expect("release backend");
    let _command = wait_for_recorded_command(&blocked_commands, |command| {
        matches!(command, PlaybackCommand::PlayPrepared { .. })
    });
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn rejected_start_commits_attempted_playback_current() {
    let root = unique_test_dir("rejected-start-local");
    let _cleanup = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create local playback root");
    let first_path = root.join("first.flac");
    let second_path = root.join("second.flac");
    fs::write(&first_path, []).expect("write first local track");
    fs::write(&second_path, []).expect("write second local track");
    let first = local_playback_test_track(1, &first_path);
    let second = local_playback_test_track(2, &second_path);
    let store = StoreHandle::open_memory().expect("memory store");
    seed_cached_library(
        &store,
        &local_source_saved(),
        &[],
        &[first.clone(), second.clone()],
        &[],
    );
    let (controller, events) = controller_from_store_for_test(store);
    let commands = Arc::new(Mutex::new(Vec::new()));
    *controller.playback.lock().expect("playback") =
        Box::new(RejectingPlaybackBackend::new(Arc::clone(&commands)));
    let mut queue = QueueEngine::new(local_source_saved().source.id);
    queue.play_now(&first);
    let second_entry = queue.append(&second);
    *controller.queue.lock().expect("queue") = Some(queue);

    controller.activate_queue_entry(second_entry);
    let _queue = wait_for_queue(&events).expect("queue");
    let _rejected = wait_for_recorded_command(&commands, |command| {
        matches!(command, PlaybackCommand::PlayPrepared { .. })
    });
    let playback = wait_for_playback_matching(&controller, &events, |playback| {
        playback.state == PlaybackState::Stopped && playback.last_error.is_some()
    });

    assert_eq!(playback.current.expect("current").track_id, second.id);
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn stale_desired_queue_current_does_not_receive_playback_progress() {
    let root = unique_test_dir("stale-progress-local");
    let _cleanup = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create local playback root");
    let first_path = root.join("first.flac");
    let second_path = root.join("second.flac");
    fs::write(&first_path, []).expect("write first local track");
    fs::write(&second_path, []).expect("write second local track");
    let first = local_playback_test_track(1, &first_path);
    let second = local_playback_test_track(2, &second_path);
    let store = StoreHandle::open_memory().expect("memory store");
    seed_cached_library(
        &store,
        &local_source_saved(),
        &[],
        &[first.clone(), second.clone()],
        &[],
    );
    let (controller, events) = controller_from_store_for_test(store);
    let commands = Arc::new(Mutex::new(Vec::new()));
    *controller.playback.lock().expect("playback") =
        Box::new(RejectingPlaybackBackend::new(Arc::clone(&commands)));
    let mut queue = QueueEngine::new(local_source_saved().source.id);
    queue.play_now(&first);
    let second_entry = queue.append(&second);
    *controller.queue.lock().expect("queue") = Some(queue);

    controller.activate_queue_entry(second_entry);
    let queue = wait_for_queue(&events).expect("queue");
    assert_eq!(queue.current_index, Some(1));
    let _rejected = wait_for_recorded_command(&commands, |command| {
        matches!(command, PlaybackCommand::PlayPrepared { .. })
    });

    *controller.playback.lock().expect("playback") = Box::new(QueuedPlaybackEvents::new(vec![
        PlaybackEvent::PositionChanged {
            track_id: Some(first.id.clone()),
            seconds: 42,
            millis: 42_000,
        },
    ]));
    controller.poll_playback_events();

    let playback = controller
        .playback_snapshot
        .lock()
        .expect("playback snapshot")
        .clone();
    assert_eq!(playback.current.expect("current").track_id, second.id);
    assert_eq!(playback.position_seconds, 0);
    let queue = controller.queue_snapshot().expect("queue");
    assert_eq!(queue.current_index, Some(1));
    assert_eq!(
        queue.entries[queue.current_index.expect("current")].track_id,
        second.id
    );
    assert_eq!(queue.progress_seconds, 0);
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn playback_current_queue_activation_restarts() {
    let root = unique_test_dir("current-activation-local");
    let _cleanup = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create local playback root");
    let path = root.join("current.flac");
    fs::write(&path, []).expect("write local track");
    let first = local_playback_test_track(1, &path);
    let store = StoreHandle::open_memory().expect("memory store");
    seed_cached_library(
        &store,
        &local_source_saved(),
        &[],
        std::slice::from_ref(&first),
        &[],
    );
    let (controller, _events) = controller_from_store_for_test(store);
    let mut queue = QueueEngine::new(local_source_saved().source.id);
    let current_entry_id = queue.play_now(&first);
    *controller.queue.lock().expect("queue") = Some(queue);
    let commands = Arc::new(Mutex::new(Vec::new()));
    *controller.playback.lock().expect("playback") =
        Box::new(RecordingPlaybackBackend::new(Arc::clone(&commands)));

    controller.activate_queue_entry(current_entry_id);

    let command = wait_for_recorded_command(&commands, |command| {
        matches!(command, PlaybackCommand::PlayPrepared { .. })
    });
    let PlaybackCommand::PlayPrepared { item, .. } = command else {
        panic!("expected prepared play command");
    };
    assert_eq!(item.track.id, first.id);
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn cover_change_backend() {
    let root = unique_test_dir("reprepare-local-access");
    let _cleanup = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create root");
    let second_path = root.join("second.flac");
    fs::write(&second_path, []).expect("write second local match");
    let saved = saved_source();
    let source_id = saved.source.id.clone();
    let mut first = playback_test_track(1);
    first.id = TrackId::new("jellyfin:track:local-access-first");
    first.local_path = Some("/server/music/first.flac".to_string());
    let mut second = playback_test_track(2);
    second.id = TrackId::new("jellyfin:track:local-access-second");
    second.local_path = Some("/server/music/second.flac".to_string());
    let store = StoreHandle::open_memory().expect("memory store");
    store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&source_id)?;
            store.save_source_local_access(&SourceLocalAccess {
                source_id: source_id.clone(),
                root_path: root.to_string_lossy().into_owned(),
                path_replace_from: Some("/server/music".to_string()),
                path_replace_to: Some(root.to_string_lossy().into_owned()),
            })?;
            let generation = store.begin_sync(&source_id)?;
            commit_cached_library(
                store,
                &source_id,
                generation,
                CachedLibraryObservation {
                    tracks: vec![first.clone(), second.clone()],
                    local_matches: vec![(
                        second.id.clone(),
                        second_path.to_string_lossy().into_owned(),
                        "metadata".to_string(),
                    )],
                    ..CachedLibraryObservation::default()
                },
            )
            .map(|_| ())
        })
        .expect("seed source with local match");
    let (controller, _events) = controller_from_store_for_test(store);
    controller
        .secrets
        .save_token(&source_id, "token")
        .expect("save token");
    let mut queue = QueueEngine::new(source_id.clone());
    queue.play_now(&first);
    queue.append(&second);
    *controller.queue.lock().expect("queue") = Some(queue);
    let commands = Arc::new(Mutex::new(Vec::new()));
    *controller.playback.lock().expect("playback") =
        Box::new(RecordingPlaybackBackend::new(Arc::clone(&commands)));
    controller.prepare_next_stream();
    let command = wait_for_recorded_command(&commands, |command| {
        matches!(command, PlaybackCommand::PrepareNext(Some(_)))
    });
    let PlaybackCommand::PrepareNext(Some(item)) = command else {
        panic!("expected prepared next command");
    };
    assert_eq!(item.track.id, second.id);
    let local_uri = reqwest::Url::from_file_path(&second_path)
        .expect("local file URI")
        .to_string();
    assert_eq!(item.stream.uri(), local_uri);
    commands.lock().expect("commands").clear();
    controller.clear_source_local_access(source_id.clone());
    let command = wait_for_recorded_command(&commands, |command| {
        matches!(command, PlaybackCommand::PrepareNext(Some(_)))
    });
    let PlaybackCommand::PrepareNext(Some(item)) = command else {
        panic!("expected prepared next command");
    };
    assert_eq!(item.track.id, second.id);
    assert!(item.stream.uri().starts_with("https://music.example/"));
    commands.lock().expect("commands").clear();
    controller.save_source_local_access(
        source_id,
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
    assert!(item.stream.uri().starts_with("https://music.example/"));
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn prepared_send_reject() {
    let source_id = SourceId::new("test:source:prepared-reject");
    let first = playback_test_track(1);
    let repeated = playback_test_track(2);
    let mut engine = QueueEngine::new(source_id);
    engine.play_now(&first);
    let initial_next_entry_id = engine.append(&repeated);
    let replacement_next_entry_id = engine.append(&repeated);
    let queue = Arc::new(Mutex::new(Some(engine)));
    let request =
        next_preload_request_from_queue(&queue, &PlaybackSettings::default()).expect("request");
    assert_eq!(request.next_entry_id, initial_next_entry_id);
    {
        let mut queue = queue.lock().expect("queue");
        let queue = queue.as_mut().expect("queue");
        assert!(queue.move_after_current(&replacement_next_entry_id));
    }
    let (current_entry_id, next_entry_id, next_track_id) = {
        let queue = queue.lock().expect("queue");
        let queue = queue.as_ref().expect("queue");
        let current = queue.current().expect("current");
        let next = next_queue_entry_after_current(queue).expect("next");
        (current.id.clone(), next.id, next.track_id)
    };
    assert_eq!(current_entry_id, request.current_entry_id);
    assert_eq!(next_track_id, request.next_entry.track_id);
    assert_ne!(next_entry_id, request.next_entry_id);
    let commands = Arc::new(Mutex::new(Vec::new()));
    let playback = Arc::new(Mutex::new(
        Box::new(RecordingPlaybackBackend::new(Arc::clone(&commands))) as Box<dyn PlaybackBackend>,
    ));
    let (events, _receiver) = channel();
    let prepared = prepared_item_from_entry(
        &request.next_entry,
        StreamDescriptor::new("test://stream/duplicate"),
    );
    assert!(!send_prepared_next(
        &playback, &queue, &events, &request, prepared
    ));
    assert!(commands.lock().expect("commands").is_empty());
}
#[test]
pub(in crate::controller) fn prepared_skip_current_repeat() {
    let source_id = SourceId::new("test:source:no-next");
    let track = playback_test_track(1);
    let mut engine = QueueEngine::new(source_id);
    engine.play_now(&track);
    let queue = Arc::new(Mutex::new(Some(engine)));

    assert!(next_preload_request_from_queue(&queue, &PlaybackSettings::default()).is_none());
}
#[test]
pub(in crate::controller) fn prepared_uses_shuffled_next() {
    let source_id = SourceId::new("test:source:shuffled-next");
    let first = playback_test_track(1);
    let second = playback_test_track(2);
    let third = playback_test_track(3);
    let mut engine = QueueEngine::new(source_id);
    engine.play_now(&first);
    engine.append(&second);
    engine.append(&third);
    engine.set_shuffle(true, 19);
    let snapshot = engine.snapshot();
    let current_index = snapshot.current_index.expect("current queue index");
    let current_shuffle_position = snapshot
        .shuffle_order
        .iter()
        .position(|index| *index == current_index)
        .expect("current shuffle position");
    let next_shuffle_position = (current_shuffle_position + 1) % snapshot.shuffle_order.len();
    let next_index = snapshot.shuffle_order[next_shuffle_position];
    let expected = snapshot.entries[next_index].id.clone();
    let queue = Arc::new(Mutex::new(Some(engine)));

    let request =
        next_preload_request_from_queue(&queue, &PlaybackSettings::default()).expect("request");

    assert_eq!(request.next_entry_id, expected);
}
#[test]
pub(in crate::controller) fn prepared_uses_appended_next() {
    let source_id = SourceId::new("test:source:appended-next");
    let first = playback_test_track(1);
    let second = playback_test_track(2);
    let mut engine = QueueEngine::new(source_id);
    engine.play_now(&first);
    let appended = engine.append(&second);
    let queue = Arc::new(Mutex::new(Some(engine)));

    let request =
        next_preload_request_from_queue(&queue, &PlaybackSettings::default()).expect("request");

    assert_eq!(request.next_entry_id, appended);
}
#[test]
pub(in crate::controller) fn prepared_next_dedupes_until_cleared() {
    let root = unique_test_dir("prepared-dedupe-local");
    let _cleanup = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create local playback root");
    let first_path = root.join("first.flac");
    let second_path = root.join("second.flac");
    fs::write(&first_path, []).expect("write first local track");
    fs::write(&second_path, []).expect("write second local track");
    let first = local_playback_test_track(1, &first_path);
    let second = local_playback_test_track(2, &second_path);
    let source_id = local_source_saved().source.id;
    let store = StoreHandle::open_memory().expect("memory store");
    seed_cached_library(
        &store,
        &local_source_saved(),
        &[],
        &[first.clone(), second.clone()],
        &[],
    );
    let (controller, _events) = controller_from_store_for_test(store);
    let mut engine = QueueEngine::new(source_id);
    engine.play_now(&first);
    engine.append(&second);
    let queue = Arc::new(Mutex::new(Some(engine)));
    let commands = Arc::new(Mutex::new(Vec::new()));
    let playback = Arc::new(Mutex::new(
        Box::new(RecordingPlaybackBackend::new(Arc::clone(&commands))) as Box<dyn PlaybackBackend>,
    ));
    let next_preload = Arc::new(Mutex::new(NextPreloadState::default()));
    let events = controller.events.clone();

    prepare_next_stream_from_handles(
        controller.store.clone(),
        Arc::clone(&controller.runtime),
        Arc::clone(&controller.active_source),
        Arc::clone(&playback),
        Arc::clone(&queue),
        Arc::clone(&next_preload),
        events.clone(),
    );
    prepare_next_stream_from_handles(
        controller.store.clone(),
        Arc::clone(&controller.runtime),
        Arc::clone(&controller.active_source),
        Arc::clone(&playback),
        Arc::clone(&queue),
        Arc::clone(&next_preload),
        events.clone(),
    );
    let command = wait_for_recorded_command(&commands, |command| {
        matches!(command, PlaybackCommand::PrepareNext(Some(_)))
    });
    let PlaybackCommand::PrepareNext(Some(item)) = command else {
        panic!("expected prepared next command");
    };
    assert_eq!(item.track.id, second.id);
    std::thread::sleep(std::time::Duration::from_millis(150));
    let prepare_count = commands
        .lock()
        .expect("commands")
        .iter()
        .filter(|command| matches!(command, PlaybackCommand::PrepareNext(Some(_))))
        .count();
    assert_eq!(prepare_count, 1);

    clear_next_preload(&next_preload);
    commands.lock().expect("commands").clear();
    prepare_next_stream_from_handles(
        controller.store.clone(),
        Arc::clone(&controller.runtime),
        Arc::clone(&controller.active_source),
        Arc::clone(&playback),
        Arc::clone(&queue),
        next_preload,
        events,
    );
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
pub(in crate::controller) fn cover_reject_switch() {
    let source_id = SourceId::new("test:source:request-switch");
    let track = playback_test_track(1);
    let mut engine = QueueEngine::new(source_id.clone());
    engine.play_now(&track);
    let entry = engine.current().expect("current").clone();
    let queue = Arc::new(Mutex::new(Some(engine)));
    let playback_request_generation = Arc::new(AtomicU64::new(1));

    assert!(request_generation_match(
        &playback_request_generation,
        1,
        &queue,
        &source_id,
        &entry
    ));

    *queue.lock().expect("queue") = Some(QueueEngine::new(SourceId::new("server:other")));
    assert!(!request_generation_match(
        &playback_request_generation,
        1,
        &queue,
        &source_id,
        &entry
    ));
}
#[test]
pub(in crate::controller) fn playback_request_reject() {
    let source_id = SourceId::new("test:source:request-generation");
    let track = playback_test_track(1);
    let mut engine = QueueEngine::new(source_id.clone());
    engine.play_now(&track);
    let stale_entry = engine.current().expect("current").clone();
    let mut replacement = QueueEngine::new(source_id.clone());
    replacement.play_now(&track);
    let queue = Arc::new(Mutex::new(Some(replacement)));
    let playback_request_generation = Arc::new(AtomicU64::new(1));
    invalidate_playback_requests(&playback_request_generation);

    assert!(!request_generation_match(
        &playback_request_generation,
        1,
        &queue,
        &source_id,
        &stale_entry
    ));
}
#[test]
pub(in crate::controller) fn cover_track_selected() {
    let root = unique_test_dir("selected-local-queue-track");
    let _cleanup = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create local playback root");
    let first_path = root.join("first.flac");
    let second_path = root.join("second.flac");
    fs::write(&first_path, []).expect("write first local track");
    fs::write(&second_path, []).expect("write second local track");
    let saved = local_source_saved();
    let first = local_playback_test_track(1, &first_path);
    let second = local_playback_test_track(2, &second_path);
    let store = StoreHandle::open_memory().expect("memory store");
    seed_cached_library(&store, &saved, &[], &[first.clone(), second.clone()], &[]);
    let (controller, events) = controller_from_store_for_test(store);
    let mut engine = QueueEngine::new(saved.source.id);
    engine.play_now(&first);
    let second_entry = engine.append(&second);
    *controller.queue.lock().expect("queue") = Some(engine);
    controller.activate_queue_entry(second_entry);
    let queue = wait_for_queue(&events).expect("activated queue");
    assert_eq!(queue.current_index, Some(1));
    let playback = wait_for_playback_state(&controller, &events, PlaybackState::Playing);
    assert_eq!(playback.current.expect("current").track_id, second.id);
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn cover_emit_position() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let source_id = SourceId::new("test:source:seek");
    let track = playback_test_track(1);
    let queue = playback_test_queue(source_id, std::slice::from_ref(&track));
    set_playback_test_state(&controller, queue, PlaybackState::Playing, 0);
    controller.seek_millis(12_345);
    let playback = controller
        .playback_snapshot
        .lock()
        .expect("playback snapshot")
        .clone();
    assert_eq!(playback.position_millis, 12_345);
    assert_eq!(playback.position_seconds, 12);
    assert_eq!(playback.current.expect("current").track_id, track.id);
    assert_eq!(
        controller
            .queue_snapshot()
            .expect("queue snapshot")
            .progress_seconds,
        12
    );
}
#[test]
pub(in crate::controller) fn cover_ignore_positions() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let source_id = SourceId::new("test:source:error-position");
    let track = playback_test_track(1);
    let queue = playback_test_queue(source_id, &[track]);
    set_playback_test_state(&controller, queue, PlaybackState::Playing, 7);
    let initial_position = 7_000;

    *controller.playback.lock().expect("playback") = Box::new(QueuedPlaybackEvents::new(vec![
        PlaybackEvent::Error("stream failed".to_string()),
        PlaybackEvent::PositionChanged {
            track_id: None,
            seconds: 42,
            millis: 42_000,
        },
    ]));

    controller.poll_playback_events();

    let playback = controller
        .playback_snapshot
        .lock()
        .expect("playback snapshot")
        .clone();
    assert_eq!(playback.state, PlaybackState::Stopped);
    assert_eq!(playback.last_error.as_deref(), Some("stream failed"));
    assert_eq!(playback.position_millis, initial_position);
    assert_ne!(
        controller
            .queue_snapshot()
            .expect("queue snapshot")
            .progress_seconds,
        42
    );
}
#[test]
pub(in crate::controller) fn cover_keep_sync() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let first = playback_test_track(1);
    let second = playback_test_track(2);
    let queue = playback_test_queue(
        SourceId::new("test:source:queue-sync"),
        &[first.clone(), second.clone()],
    );
    *controller.queue.lock().expect("queue") = Some(queue);
    controller.next_track();
    let queue = controller.queue_snapshot().expect("next queue");
    assert_eq!(
        queue.entries[queue.current_index.expect("current")].track_id,
        second.id
    );
    controller.previous_track();
    let queue = controller.queue_snapshot().expect("previous queue");
    assert_eq!(
        queue.entries[queue.current_index.expect("current")].track_id,
        first.id
    );
    controller.clear_queue();
    let queue = controller.queue_snapshot().expect("clear queue");
    assert_eq!(queue.entries.len(), 1);
    assert_eq!(queue.current_index, Some(0));
    assert_eq!(queue.entries[0].track_id, first.id);
}
#[test]
pub(in crate::controller) fn cover_track_first() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let source_id = SourceId::new("test:source:repeat-all");
    let first = playback_test_track(1);
    let second = playback_test_track(2);
    let mut queue = playback_test_queue(source_id, &[first.clone(), second]);
    queue.next_track();
    queue.set_progress_seconds(12);
    set_playback_test_state(&controller, queue, PlaybackState::Playing, 12);
    controller.next_track();
    let playback = controller
        .playback_snapshot
        .lock()
        .expect("playback snapshot")
        .clone();
    assert_eq!(playback.current.expect("current").track_id, first.id);
    assert_eq!(playback.position_millis, 0);
    assert_ne!(playback.state, PlaybackState::Stopped);
}
#[test]
pub(in crate::controller) fn manual_ten_seconds() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let source_id = SourceId::new("test:source:previous-threshold");
    let first = playback_test_track(1);
    let second = playback_test_track(2);
    let mut queue = playback_test_queue(source_id, &[first, second.clone()]);
    queue.next_track();
    queue.set_progress_seconds(11);
    set_playback_test_state(&controller, queue, PlaybackState::Playing, 11);
    controller.previous_track();
    let playback = controller
        .playback_snapshot
        .lock()
        .expect("playback snapshot")
        .clone();
    assert_eq!(playback.current.expect("current").track_id, second.id);
    assert_eq!(playback.position_millis, 0);
}
#[test]
pub(in crate::controller) fn cover_use_order() {
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let track = playback_test_track(1);
    let queue = playback_test_queue(SourceId::new("test:source:repeat-order"), &[track]);
    *controller.queue.lock().expect("queue") = Some(queue);
    controller.cycle_repeat();
    let playback = wait_for_playback_repeat(&events, RepeatMode::One);
    assert_eq!(playback.repeat_mode, RepeatMode::One);
    controller.cycle_repeat();
    let playback = wait_for_playback_repeat(&events, RepeatMode::Off);
    assert_eq!(playback.repeat_mode, RepeatMode::Off);
    controller.cycle_repeat();
    let playback = wait_for_playback_repeat(&events, RepeatMode::All);
    assert_eq!(playback.repeat_mode, RepeatMode::All);
}

#[test]
pub(in crate::controller) fn playback_modes_do_not_emit_queue() {
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let first = playback_test_track(1);
    let second = playback_test_track(2);
    let queue = playback_test_queue(SourceId::new("test:source:mode-events"), &[first, second]);
    *controller.queue.lock().expect("queue") = Some(queue);

    controller.cycle_repeat();
    let playback = wait_for_repeat_without_queue(&events, RepeatMode::One);
    assert_eq!(playback.repeat_mode, RepeatMode::One);

    controller.toggle_shuffle();
    let playback = wait_for_shuffle_without_queue(&events, true);
    assert!(playback.shuffle_enabled);
}

#[test]
pub(in crate::controller) fn repeat_one_clears_prepared_next() {
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let commands = Arc::new(Mutex::new(Vec::new()));
    *controller.playback.lock().expect("playback") =
        Box::new(RecordingPlaybackBackend::new(Arc::clone(&commands)));
    let first = playback_test_track(1);
    let second = playback_test_track(2);
    let queue = playback_test_queue(
        SourceId::new("test:source:repeat-preload"),
        &[first, second],
    );
    *controller.queue.lock().expect("queue") = Some(queue);

    controller.cycle_repeat();
    let playback = wait_for_repeat_without_queue(&events, RepeatMode::One);
    assert_eq!(playback.repeat_mode, RepeatMode::One);
    let command = wait_for_recorded_command(&commands, |command| {
        matches!(command, PlaybackCommand::PrepareNext(None))
    });
    assert_eq!(command, PlaybackCommand::PrepareNext(None));
}

#[test]
pub(in crate::controller) fn cover_use_sqlite() {
    let dir = unique_test_dir("settings-round-trip");
    let settings_path = dir.join("config").join(SETTINGS_FILE_NAME);
    let cache_database_path = dir.join(CACHE_DATABASE_FILE_NAME);
    let store = StoreHandle::Path {
        cache_database_path: cache_database_path.clone(),
        settings_path: settings_path.clone(),
        settings: Arc::new(Mutex::new(AppSettings::default())),
    };
    let settings = AppSettings {
        theme_preference: ThemePreference::Dark,
        auto_dj_enabled: true,
        ..AppSettings::default()
    };
    store.save_settings(&settings).expect("save settings");
    assert_eq!(load_settings_from_store(&store), settings);
    assert!(settings_path.exists());
    assert!(!cache_database_path.exists());
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
pub(in crate::controller) fn cover_app_root() {
    let root = PathBuf::from("/tmp/rufin-path-layout");
    assert_eq!(
        cache_db_path(&root.join("cache")),
        root.join("cache")
            .join("store")
            .join(CACHE_DATABASE_FILE_NAME)
    );
    assert_eq!(
        settings_file_path(&root.join("config")),
        root.join("config").join(SETTINGS_FILE_NAME)
    );
    assert_eq!(
        cover_cache_path(&root.join("cache")),
        root.join("cache").join("covers")
    );
    assert_eq!(
        lyrics_cache_dir(&root.join("cache")),
        root.join("cache").join("lyrics")
    );
    assert_eq!(
        playback_cache_dir(&root.join("cache")),
        root.join("cache").join("playback")
    );
}
#[test]
pub(in crate::controller) fn cover_create_folder() {
    let root = unique_test_dir("cache-layout");
    ensure_app_cache_dirs(&root).expect("ensure cache layout");
    assert!(root.join("store").is_dir());
    assert!(root.join("covers").is_dir());
    assert!(root.join("lyrics").is_dir());
    assert!(root.join("playback").is_dir());
    assert!(!root.join("tmp").exists());
    assert!(!root.join("playlists").exists());
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn cover_remove_waveform_tmp() {
    let root = unique_test_dir("waveform-tmp");
    let waveform_tmp = root.join("tmp").join("waveforms");
    fs::create_dir_all(&waveform_tmp).expect("create waveform tmp");
    fs::write(waveform_tmp.join("track.audio"), b"audio").expect("write waveform tmp");

    remove_waveform_tmp(&root).expect("remove waveform tmp");

    assert!(!root.join("tmp").exists());
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn cover_emit_state() {
    let (controller, events, _snapshot, _queue, player) =
        AppController::bootstrap_memory_for_test();
    assert!(!player.auto_dj_enabled);
    controller.toggle_auto_dj();
    let playback = wait_for_playback_auto_dj(&events, true);
    assert!(playback.auto_dj_enabled);
    assert!(controller.load_settings().auto_dj_enabled);
}
#[test]
pub(in crate::controller) fn random_play_now() {
    let root = unique_test_dir("local-random-play-now");
    let _cleanup = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create local root");
    for number in 1..=3 {
        fs::write(root.join(format!("track-{number}.mp3")), []).expect("write local track");
    }
    fs::write(root.join("cover.jpg"), [0xff_u8, 0xd8, 0xff, 0xd9]).expect("write local cover");
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let saved = sync_real_local_root(&controller, &root);
    let active =
        crate::sources::activate_configured_source(&controller.store, &controller.secrets, &saved)
            .expect("activate local source");
    let expected = controller
        .runtime
        .block_on(
            active
                .random_tracks
                .executor
                .random_tracks(RandomTrackRequest {
                    limit: 3,
                    min_year: None,
                    max_year: None,
                    genre_id: None,
                    genre_name: None,
                    played_filter: PlayedFilter::All,
                }),
        )
        .expect("local random tracks")
        .into_iter()
        .map(|track| track.id)
        .collect::<Vec<_>>();
    *controller.queue.lock().expect("queue") = Some(QueueEngine::new(saved.source.id.clone()));
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
    assert_eq!(queue.entries.len(), 3);
    assert!(queue.entries[0].image_ref.is_some());
    let first_track_id = queue.entries[0].track_id.clone();
    let playback = wait_for_playback_matching(&controller, &events, |playback| {
        playback.state == PlaybackState::Playing
            && playback
                .current
                .as_ref()
                .is_some_and(|current| current.track_id == first_track_id)
    });
    assert_eq!(playback.current.expect("current").track_id, first_track_id);
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn play_append_track() {
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let first = playback_test_track(1);
    let second = playback_test_track(2);
    let third = playback_test_track(3);
    let fourth = playback_test_track(4);
    let queue = playback_test_queue(
        SourceId::new("test:source:append"),
        &[first.clone(), second.clone()],
    );
    *controller.queue.lock().expect("queue") = Some(queue);
    controller.play_last(vec![third.clone(), fourth.clone()]);
    let queue = wait_for_queue(&events).expect("append queue");
    let ids = queue
        .entries
        .iter()
        .map(|entry| entry.track_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(queue.current_index, Some(0));
    assert_eq!(ids, vec![first.id, second.id, third.id, fourth.id]);
}
#[test]
pub(in crate::controller) fn cover_auto_library() {
    let root = unique_test_dir("local-auto-dj-library");
    let _cleanup = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create local root");
    for number in 0..=super::AUTO_DJ_ITEM_COUNT + 1 {
        fs::write(root.join(format!("track-{number}.mp3")), []).expect("write local track");
    }
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let saved = sync_real_local_root(&controller, &root);
    let snapshot = load_snapshot(&controller.store).expect("load local snapshot");
    *controller.queue.lock().expect("queue") = Some(QueueEngine::new(saved.source.id.clone()));
    enable_auto_dj_for_test(&controller, &events);
    let first = snapshot.tracks[0].clone();
    controller.play_now(first.clone());
    let queue = wait_for_queue(&events).expect("initial queue");
    assert_eq!(queue.entries.len(), 1);
    let queue = wait_for_queue_matching(&events, |queue| {
        queue.entries.len() == 1 + super::AUTO_DJ_ITEM_COUNT
    });
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
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn cover_auto_timing() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let saved = local_source_saved();
    controller
        .store
        .with_store(|store| {
            store.save_source(&saved)?;
            store.set_active_source(&saved.source.id)
        })
        .expect("seed local source");
    let queue = playback_test_queue(
        saved.source.id,
        &[playback_test_track(1), playback_test_track(2)],
    );
    *controller.queue.lock().expect("queue") = Some(queue);
    *controller.auto_dj_enabled.lock().expect("Auto DJ") = true;
    let mut settings = controller.load_settings();
    settings.auto_dj_enabled = true;
    settings.auto_dj_refill_threshold = 1;
    controller
        .save_settings(&settings)
        .expect("save Auto DJ settings");
    assert!(!controller.refill_auto_dj_queue());

    let mut settings = controller.load_settings();
    settings.auto_dj_refill_threshold = 2;
    controller
        .save_settings(&settings)
        .expect("save Auto DJ settings");
    assert!(controller.refill_auto_dj_queue());
}
#[test]
pub(in crate::controller) fn manual_skip_does_not_refill_auto_dj() {
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let first = playback_test_track(1);
    let second = playback_test_track(2);
    let queue = playback_test_queue(
        SourceId::new("test:source:manual-auto-dj"),
        &[first.clone(), second.clone()],
    );
    *controller.queue.lock().expect("queue") = Some(queue);
    let mut settings = controller.load_settings();
    settings.auto_dj_refill_threshold = 0;
    controller
        .save_settings(&settings)
        .expect("save Auto DJ settings");
    enable_auto_dj_for_test(&controller, &events);
    controller.next_track();
    let queue = controller.queue_snapshot().expect("next queue");
    assert_eq!(
        queue.entries[queue.current_index.expect("current")].track_id,
        second.id
    );
    let mut settings = controller.load_settings();
    settings.auto_dj_refill_threshold = 2;
    controller
        .save_settings(&settings)
        .expect("save Auto DJ settings");
    controller.next_track();
    let queue = controller.queue_snapshot().expect("wrapped queue");
    assert_eq!(queue.entries.len(), 2);
    assert_ne!(
        queue.entries[queue.current_index.expect("current")].track_id,
        second.id
    );
}

#[test]
pub(in crate::controller) fn cover_auto_next() {
    let root = unique_test_dir("local-auto-dj-end-of-stream");
    let _cleanup = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create local root");
    for number in 1..=6 {
        fs::write(root.join(format!("track-{number}.mp3")), []).expect("write local track");
    }
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let saved = sync_real_local_root(&controller, &root);
    let tracks = load_snapshot(&controller.store)
        .expect("load local snapshot")
        .tracks;
    let first = tracks[0].clone();
    let second = tracks[1].clone();
    let queue = playback_test_queue(saved.source.id, &[first, second.clone()]);
    set_playback_test_state(&controller, queue, PlaybackState::Playing, 0);
    *controller.auto_dj_enabled.lock().expect("Auto DJ") = true;
    let mut settings = controller.load_settings();
    settings.auto_dj_enabled = true;
    settings.auto_dj_refill_threshold = 2;
    controller
        .save_settings(&settings)
        .expect("save Auto DJ settings");
    controller.advance_after_end_of_stream();
    let queue = wait_for_queue_matching(&events, |queue| queue.entries.len() > 2);
    assert!(queue.entries.len() > 2);
    assert_eq!(
        queue.entries[queue.current_index.expect("current")].track_id,
        second.id
    );
    let _cleanup = fs::remove_dir_all(root);
}

#[test]
pub(in crate::controller) fn cover_auto_repeat_one_skips_refill() {
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let first = playback_test_track(1);
    let second = playback_test_track(2);
    let queue = playback_test_queue(
        SourceId::new("test:source:repeat-auto-dj"),
        &[first, second],
    );
    *controller.queue.lock().expect("queue") = Some(queue);
    let mut settings = controller.load_settings();
    settings.auto_dj_refill_threshold = 0;
    controller
        .save_settings(&settings)
        .expect("save Auto DJ settings");
    enable_auto_dj_for_test(&controller, &events);
    let queue = controller.queue_snapshot().expect("queue");
    assert_eq!(queue.entries.len(), 2);
    controller.cycle_repeat();
    let _playback = wait_for_playback_repeat(&events, RepeatMode::One);

    let mut settings = controller.load_settings();
    settings.auto_dj_refill_threshold = 2;
    controller
        .save_settings(&settings)
        .expect("save Auto DJ settings");
    assert!(!controller.refill_auto_dj_queue());
    let queue = controller.queue_snapshot().expect("queue snapshot");
    assert_eq!(queue.entries.len(), 2);
}
#[test]
pub(in crate::controller) fn end_stream_repeat() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let first = playback_test_track(1);
    let second = playback_test_track(2);
    let mut queue = playback_test_queue(
        SourceId::new("test:source:end-repeat"),
        &[first.clone(), second],
    );
    queue.set_repeat_mode(RepeatMode::One);
    *controller.queue.lock().expect("queue") = Some(queue);
    controller.advance_after_end_of_stream();
    let queue = controller.queue_snapshot().expect("repeated queue");
    assert_eq!(
        queue.entries[queue.current_index.expect("current")].track_id,
        first.id
    );
}
#[test]
pub(in crate::controller) fn end_of_stream_advances_queue() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let first = playback_test_track(1);
    let second = playback_test_track(2);
    let queue = playback_test_queue(
        SourceId::new("test:source:end-advance"),
        &[first, second.clone()],
    );
    *controller.queue.lock().expect("queue") = Some(queue);
    controller.advance_after_end_of_stream();
    let queue = controller.queue_snapshot().expect("next queue");
    assert_eq!(
        queue.entries[queue.current_index.expect("current")].track_id,
        second.id
    );
}
#[test]
pub(in crate::controller) fn cover_track_event() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let source_id = SourceId::new("test:source:track-boundary");
    let first = playback_test_track(1);
    let second = playback_test_track(2);
    let queue = playback_test_queue(source_id, &[first.clone(), second.clone()]);
    set_playback_test_state(&controller, queue, PlaybackState::Playing, 0);
    *controller.playback.lock().expect("playback") = Box::new(QueuedPlaybackEvents::new(vec![
        PlaybackEvent::EndOfStream,
        PlaybackEvent::StateChanged(PlaybackState::Stopped),
        PlaybackEvent::PositionChanged {
            track_id: Some(first.id.clone()),
            seconds: first.duration_seconds,
            millis: u64::from(first.duration_seconds) * 1_000,
        },
        PlaybackEvent::DurationChanged {
            track_id: Some(first.id.clone()),
            seconds: first.duration_seconds,
        },
    ]));

    controller.poll_playback_events();

    let playback = controller
        .playback_snapshot
        .lock()
        .expect("playback snapshot")
        .clone();
    assert_eq!(playback.current.expect("current").track_id, second.id);
    assert_eq!(playback.state, PlaybackState::Buffering);
    assert_eq!(playback.position_seconds, 0);
    assert_eq!(playback.position_millis, 0);
    assert_eq!(playback.duration_seconds, second.duration_seconds);
    let queue = controller.queue_snapshot().expect("queue snapshot");
    assert_eq!(queue.progress_seconds, 0);
}
#[test]
pub(in crate::controller) fn cover_track_queue() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let source_id = SourceId::new("test:source:stale-timing");
    let first = playback_test_track(1);
    let second = playback_test_track(2);
    let mut queue = playback_test_queue(source_id, &[first.clone(), second.clone()]);
    queue.next_track();
    set_playback_test_state(&controller, queue, PlaybackState::Playing, 0);
    *controller.playback.lock().expect("playback") = Box::new(QueuedPlaybackEvents::new(vec![
        PlaybackEvent::PositionChanged {
            track_id: Some(first.id.clone()),
            seconds: first.duration_seconds,
            millis: u64::from(first.duration_seconds) * 1_000,
        },
        PlaybackEvent::DurationChanged {
            track_id: Some(first.id.clone()),
            seconds: first.duration_seconds,
        },
    ]));

    controller.poll_playback_events();

    let playback = controller
        .playback_snapshot
        .lock()
        .expect("playback snapshot")
        .clone();
    assert_eq!(playback.current.expect("current").track_id, second.id);
    assert_eq!(playback.position_seconds, 0);
    assert_eq!(playback.position_millis, 0);
    assert_eq!(playback.duration_seconds, second.duration_seconds);
}
#[test]
pub(in crate::controller) fn cover_track_ignored() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let source_id = SourceId::new("test:source:future-timing");
    let first = playback_test_track(1);
    let second = playback_test_track(2);
    let queue = playback_test_queue(source_id, &[first.clone(), second.clone()]);
    set_playback_test_state(&controller, queue, PlaybackState::Playing, 0);
    *controller.playback.lock().expect("playback") = Box::new(QueuedPlaybackEvents::new(vec![
        PlaybackEvent::DurationChanged {
            track_id: Some(second.id.clone()),
            seconds: second.duration_seconds,
        },
    ]));

    controller.poll_playback_events();

    let playback = controller
        .playback_snapshot
        .lock()
        .expect("playback snapshot")
        .clone();
    assert_eq!(playback.current.expect("current").track_id, first.id);
    assert_eq!(playback.duration_seconds, first.duration_seconds);
}
#[test]
pub(in crate::controller) fn cover_ignores_implausible_backend_duration() {
    let (controller, _events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let source_id = SourceId::new("test:source:duration");
    let first = playback_test_track(1);
    let queue = playback_test_queue(source_id, std::slice::from_ref(&first));
    set_playback_test_state(&controller, queue, PlaybackState::Playing, 0);
    *controller.playback.lock().expect("playback") = Box::new(QueuedPlaybackEvents::new(vec![
        PlaybackEvent::DurationChanged {
            track_id: Some(first.id.clone()),
            seconds: 99 * 60 * 60 + 99 * 60 + 99,
        },
    ]));

    controller.poll_playback_events();

    let playback = controller
        .playback_snapshot
        .lock()
        .expect("playback snapshot")
        .clone();
    assert_eq!(playback.current.expect("current").track_id, first.id);
    assert_eq!(playback.duration_seconds, first.duration_seconds);
}
#[test]
pub(in crate::controller) fn cover_advance_playback() {
    let root = unique_test_dir("prepared-local-advance");
    let _cleanup = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create local playback root");
    let first_path = root.join("first.flac");
    let second_path = root.join("second.flac");
    let third_path = root.join("third.flac");
    fs::write(&first_path, []).expect("write first local track");
    fs::write(&second_path, []).expect("write second local track");
    fs::write(&third_path, []).expect("write third local track");
    let saved = local_source_saved();
    let first = local_playback_test_track(1, &first_path);
    let second = local_playback_test_track(2, &second_path);
    let third = local_playback_test_track(3, &third_path);
    let store = StoreHandle::open_memory().expect("memory store");
    seed_cached_library(
        &store,
        &saved,
        &[],
        &[first.clone(), second.clone(), third.clone()],
        &[],
    );
    let (controller, events) = controller_from_store_for_test(store);
    let commands = Arc::new(Mutex::new(Vec::new()));
    *controller.playback.lock().expect("playback") =
        Box::new(RecordingPlaybackBackend::new(Arc::clone(&commands)));
    let queue = playback_test_queue(saved.source.id, &[first, second.clone(), third.clone()]);
    set_playback_test_state(&controller, queue, PlaybackState::Playing, 0);
    controller.advance_after_prepared_track_started(PlaybackTrack {
        id: second.id.clone(),
        album_id: Some(second.album_id.clone()),
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
    let playback = controller
        .playback_snapshot
        .lock()
        .expect("playback snapshot")
        .clone();
    assert_eq!(playback.state, PlaybackState::Playing);
    assert_eq!(playback.current.expect("current").track_id, second.id);
    assert_eq!(playback.position_seconds, 0);
    assert_eq!(playback.position_millis, 0);
    assert_eq!(playback.duration_seconds, second.duration_seconds);
    let command = wait_for_recorded_command(&commands, |command| {
        matches!(command, PlaybackCommand::PrepareNext(Some(_)))
    });
    let PlaybackCommand::PrepareNext(Some(item)) = command else {
        panic!("expected prepared next command");
    };
    assert_eq!(item.track.id, third.id);
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
    let _cleanup = fs::remove_dir_all(root);
}
#[test]
pub(in crate::controller) fn cover_update_snapshot() {
    let root = unique_test_dir("local-favorite-projection");
    let _cleanup = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create local root");
    fs::write(root.join("track.mp3"), []).expect("write local track");
    let (controller, events, _snapshot, _queue, _player) =
        AppController::bootstrap_memory_for_test();
    let saved = sync_real_local_root(&controller, &root);
    let snapshot = load_snapshot(&controller.store).expect("load local snapshot");
    let track = snapshot
        .tracks
        .iter()
        .find(|track| !track.favorite)
        .expect("non-favorite track")
        .clone();
    let queue = playback_test_queue(saved.source.id, std::slice::from_ref(&track));
    set_playback_test_state(&controller, queue, PlaybackState::Playing, 0);
    controller.set_track_favorite(track.id.clone(), true);
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
    let _cleanup = fs::remove_dir_all(root);
}
