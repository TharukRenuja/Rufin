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
    fn thumbnail_cached_cover_does_not_satisfy_grid_request() {
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
            "thumbnail"
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
                    size: 96,
                    path: path.to_string_lossy().to_string(),
                })
            })
            .expect("seed cover cache");
        assert_eq!(controller.cached_cover_path(&image_ref, 256), None);
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
        let cache_database_path = dir.join(CACHE_DATABASE_FILE_NAME);
        let store = StoreHandle::Path {
            cache_database_path: cache_database_path.clone(),
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
    fn app_paths_separate_config_data_and_cache_roots() {
        let root = PathBuf::from("/tmp/rufin-path-layout");
        assert_eq!(
            app_cache_database_path_for_cache_dir(&root.join("cache")),
            root.join("cache")
                .join("store")
                .join(CACHE_DATABASE_FILE_NAME)
        );
        assert_eq!(
            app_settings_path_for_config_dir(&root.join("config")),
            root.join("config").join(SETTINGS_FILE_NAME)
        );
        assert_eq!(
            cover_cache_dir_for_cache_dir(&root.join("cache")),
            root.join("cache").join("covers")
        );
        assert_eq!(
            lyrics_cache_dir_for_cache_dir(&root.join("cache")),
            root.join("cache").join("lyrics")
        );
        assert_eq!(
            playback_cache_dir_for_cache_dir(&root.join("cache")),
            root.join("cache").join("playback")
        );
        assert_eq!(
            tmp_cache_dir_for_cache_dir(&root.join("cache")),
            root.join("cache").join("tmp")
        );
    }
    #[test]
    fn app_cache_layout_creates_expected_subdirs_without_playlist_folder() {
        let root = unique_test_dir("cache-layout");
        ensure_app_cache_dirs(&root).expect("ensure cache layout");
        assert!(root.join("store").is_dir());
        assert!(root.join("covers").is_dir());
        assert!(root.join("lyrics").is_dir());
        assert!(root.join("playback").is_dir());
        assert!(root.join("tmp").is_dir());
        assert!(!root.join("playlists").exists());
        let _cleanup = fs::remove_dir_all(root);
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
    fn play_last_appends_tracks_without_replacing_current() {
        let (controller, events, snapshot, _queue, _player) =
            AppController::bootstrap(Some(FakeScale::Small));
        let first = snapshot.tracks[0].clone();
        let second = snapshot.tracks[1].clone();
        let third = snapshot.tracks[2].clone();
        let fourth = snapshot.tracks[3].clone();
        controller.play_tracks_now(vec![first.clone(), second.clone()]);
        let _queue = wait_for_queue(&events).expect("initial queue");
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
