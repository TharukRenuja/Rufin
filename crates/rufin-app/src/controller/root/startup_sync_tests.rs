    use std::collections::HashSet;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::mpsc::{Receiver, channel};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;
    use super::{
        AppController, CACHE_DATABASE_FILE_NAME, ControllerEvent, LOCAL_SOURCE_SERVER_ID,
        LibrarySnapshot, RandomPlayAction, RandomPlayRequest, SETTINGS_FILE_NAME,
        SNAPSHOT_GRID_LIMIT, SNAPSHOT_TRACK_LIMIT, StoreHandle, activate_logged_in_server,
        app_cache_database_path_for_cache_dir, app_settings_path_for_config_dir,
        auto_dj_candidates, cover_cache_dir_for_cache_dir, ensure_app_cache_dirs,
        home_refresh_completed_event, load_settings_from_store, load_snapshot,
        lyrics_cache_dir_for_cache_dir, playback_cache_dir_for_cache_dir,
        playback_snapshot_from_queue, prefetch_home_section, promote_prefetched_home_section,
        refresh_home_section, refresh_home_sections, refresh_home_sections_without_explore,
        refresh_playlist_pages, restore_queue, save_token_and_activate_logged_in_server,
        sync_page_finished, sync_provider, tmp_cache_dir_for_cache_dir,
    };
    use crate::external_scrobbling::ExternalScrobbleState;
    use rufin_core::{
        Album, AlbumId, AppSettings, ArtistCredit, ArtistId, HomeSection, HomeSectionKind,
        ImageRef, LibrarySourceSelection, LocalLibraryFolder, PlaybackSettings, Playlist,
        PlaylistId, QueueEngine, RepeatMode, ServerId, ServerIdentity, ThemePreference, Track,
        TrackId,
    };
    use rufin_playback::{
        PlaybackBackend, PlaybackCommand, PlaybackError, PlaybackEvent, PlaybackState,
        PlaybackTrack,
    };
    use rufin_provider::{
        FavoriteItemId, LyricLine, Lyrics, LyricsSource, MusicProvider, PagedRequest, PlayedFilter,
        PlaylistEntry, ProviderSession,
    };
    use rufin_secrets::{MemorySecretStore, SecretStore};
    use rufin_store::{CachedArtistDetail, CoverCacheEntry, SavedServer, ServerLocalAccess};
    use rufin_test_support::{FakeProvider, FakeScale};
    use tokio::runtime::Runtime;
    use crate::providers::JellyfinLyricsSearch;
    struct SaveFailingSecretStore;
    impl SecretStore for SaveFailingSecretStore {
        fn save_token(
            &self,
            _server_id: &ServerId,
            _token: &str,
        ) -> rufin_secrets::SecretResult<()> {
            Err(rufin_secrets::SecretError::Backend(
                "save failed".to_string(),
            ))
        }

        fn load_token(&self, _server_id: &ServerId) -> rufin_secrets::SecretResult<Option<String>> {
            Ok(None)
        }

        fn delete_token(&self, _server_id: &ServerId) -> rufin_secrets::SecretResult<()> {
            Ok(())
        }
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
    fn source_selection_activates_queue_for_selected_source() {
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
    fn first_run_local_server_initializes_active_queue() {
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
    fn first_run_local_server_accepts_multiple_folders() {
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
    fn activate_logged_in_server_selects_server_without_saving_token() {
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
        };
        activate_logged_in_server(
            &controller.store,
            &controller.queue,
            &controller.playback,
            &controller.playback_snapshot,
            &controller.auto_dj_enabled,
            &controller.events,
            &session,
            false,
            None,
            None,
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
    fn token_save_failure_does_not_persist_empty_server() {
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
        };
        let error = save_token_and_activate_logged_in_server(
            &controller.store,
            &controller.queue,
            &controller.playback,
            &controller.playback_snapshot,
            &controller.auto_dj_enabled,
            &controller.events,
            &secrets,
            &session,
            false,
            None,
            None,
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
