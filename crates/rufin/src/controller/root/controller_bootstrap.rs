use super::*;

impl AppController {
    #[cfg(any(test, feature = "dev-tools"))]
    pub(crate) fn bootstrap_with_fake(scale: FakeScale) -> ControllerBootstrap {
        #[cfg(test)]
        let test_permit = Some(controller_test_permit());
        let (events, receiver) = channel();
        let runtime = Runtime::new()
            .map(Arc::new)
            .unwrap_or_else(|error| panic!("failed to create Tokio runtime: {error}"));
        let store = StoreHandle::open_memory()
            .unwrap_or_else(|error| panic!("failed to open fake memory store: {error}"));
        let settings = load_settings_from_store(&store);
        let (snapshot, queue) = if scale == FakeScale::ThirtyK {
            let snapshot = fake_snapshot(&store, runtime.as_ref(), scale)
                .unwrap_or_else(|error| panic!("failed to build fake snapshot: {error}"));
            let queue = snapshot
                .server
                .as_ref()
                .map(|server| QueueEngine::new(server.id.clone()));
            (snapshot, queue)
        } else {
            seed_fake_cache(&store, scale)
                .unwrap_or_else(|error| panic!("failed to seed fake cache: {error}"));
            let snapshot = load_snapshot(&store).unwrap_or_else(|error| {
                warn!(%error, "failed to load fake snapshot");
                LibrarySnapshot::first_run()
            });
            let queue = restore_queue(&store, snapshot.server.as_ref());
            (snapshot, queue)
        };
        let queue_snapshot = queue.as_ref().map(QueueEngine::snapshot);
        let playback_snapshot = playback_snapshot_from_queue(
            queue.as_ref(),
            settings.auto_dj_enabled,
            &settings.playback,
        );
        let secret_switch = Arc::new(SwitchableSecretStore::new(Arc::new(
            MemorySecretStore::new(),
        )));
        let secrets: Arc<dyn SecretStore> = Arc::<SwitchableSecretStore>::clone(&secret_switch);
        let scrobbling_secrets = Arc::clone(&secrets);
        let controller = Self {
            settings: super::settings_controller::SettingsController::new(
                store.clone(),
                scrobbling_secrets,
            ),
            store,
            runtime,
            secrets,
            secret_switch,
            queue: Arc::new(Mutex::new(queue)),
            play_activation_generation: Arc::new(AtomicU64::new(0)),
            queue_persist_generation: Arc::new(AtomicU64::new(0)),
            playback_request_generation: Arc::new(AtomicU64::new(0)),
            next_preload: Arc::new(Mutex::new(NextPreloadState::default())),
            waveform_warm_generation: Arc::new(AtomicU64::new(0)),
            playback: Arc::new(Mutex::new(Box::new(FakePlaybackBackend::new()))),
            playback_snapshot: Arc::new(Mutex::new(playback_snapshot.clone())),
            playback_activity: Arc::new(Mutex::new(PlaybackActivityState::default())),
            auto_dj_enabled: Arc::new(Mutex::new(settings.auto_dj_enabled)),
            last_progress_snapshot: Arc::new(Mutex::new(None)),
            last_report_snapshot: Arc::new(Mutex::new(None)),
            external_scrobble_state: Arc::new(Mutex::new(ExternalScrobbleState::default())),
            external_cover_retry_generation: Arc::new(AtomicU64::new(0)),
            events,
            sync_in_flight: InFlightGuards::new("Sync"),
            home_refresh_in_flight: InFlightGuards::new("Home refresh"),
            explore_prefetch_in_flight: InFlightGuards::new("Explore prefetch"),
            cover_in_flight: Arc::new(Mutex::new(HashMap::new())),
            external_cover_prefetch_in_flight: Arc::new(Mutex::new(HashMap::new())),
            cover_slots: Arc::new((Mutex::new(0), Condvar::new())),
            #[cfg(test)]
            _test_permit: test_permit,
        };
        (
            controller,
            receiver,
            snapshot,
            queue_snapshot,
            playback_snapshot,
        )
    }

    pub(crate) fn bootstrap() -> Result<ControllerBootstrap, String> {
        #[cfg(test)]
        let test_permit = Some(controller_test_permit());
        let (events, receiver) = channel();
        let runtime = Runtime::new()
            .map(Arc::new)
            .map_err(|error| format!("failed to create Tokio runtime: {error}"))?;
        let store = match StoreHandle::open_for_app() {
            Ok(store) => store,
            Err(error) => {
                warn!(%error, "failed to open app store, falling back to memory");
                StoreHandle::open_memory()
                    .map_err(|error| format!("failed to open memory store: {error}"))?
            }
        };
        let settings = load_settings_from_store(&store);
        let secret_switch = Arc::new(SwitchableSecretStore::new(platform_secret_store(&settings)));
        let secrets: Arc<dyn SecretStore> = Arc::<SwitchableSecretStore>::clone(&secret_switch);
        let snapshot = load_runtime_snapshot(&store, &secrets).unwrap_or_else(|error| {
            warn!(%error, "failed to load app snapshot");
            LibrarySnapshot::first_run()
        });
        let queue = if snapshot.first_run && snapshot.server.is_some() {
            None
        } else {
            restore_queue(&store, snapshot.server.as_ref())
        };
        let queue_snapshot = queue.as_ref().map(QueueEngine::snapshot);
        let playback_snapshot = playback_snapshot_from_queue(
            queue.as_ref(),
            settings.auto_dj_enabled,
            &settings.playback,
        );
        let scrobbling_secrets = Arc::clone(&secrets);
        let controller = Self {
            settings: super::settings_controller::SettingsController::new(
                store.clone(),
                scrobbling_secrets,
            ),
            store,
            runtime,
            secrets,
            secret_switch,
            queue: Arc::new(Mutex::new(queue)),
            play_activation_generation: Arc::new(AtomicU64::new(0)),
            queue_persist_generation: Arc::new(AtomicU64::new(0)),
            playback_request_generation: Arc::new(AtomicU64::new(0)),
            next_preload: Arc::new(Mutex::new(NextPreloadState::default())),
            waveform_warm_generation: Arc::new(AtomicU64::new(0)),
            playback: Arc::new(Mutex::new(playback_backend(false))),
            playback_snapshot: Arc::new(Mutex::new(playback_snapshot.clone())),
            playback_activity: Arc::new(Mutex::new(PlaybackActivityState::default())),
            auto_dj_enabled: Arc::new(Mutex::new(settings.auto_dj_enabled)),
            last_progress_snapshot: Arc::new(Mutex::new(None)),
            last_report_snapshot: Arc::new(Mutex::new(None)),
            external_scrobble_state: Arc::new(Mutex::new(ExternalScrobbleState::default())),
            external_cover_retry_generation: Arc::new(AtomicU64::new(0)),
            events,
            sync_in_flight: InFlightGuards::new("Sync"),
            home_refresh_in_flight: InFlightGuards::new("Home refresh"),
            explore_prefetch_in_flight: InFlightGuards::new("Explore prefetch"),
            cover_in_flight: Arc::new(Mutex::new(HashMap::new())),
            external_cover_prefetch_in_flight: Arc::new(Mutex::new(HashMap::new())),
            cover_slots: Arc::new((Mutex::new(0), Condvar::new())),
            #[cfg(test)]
            _test_permit: test_permit,
        };
        controller.warm_playback_backend();
        Ok((
            controller,
            receiver,
            snapshot,
            queue_snapshot,
            playback_snapshot,
        ))
    }
    #[cfg(test)]
    pub(in crate::controller) fn bootstrap_memory_for_test() -> ControllerBootstrap {
        let test_permit = Some(controller_test_permit());
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
        let secret_switch = Arc::new(SwitchableSecretStore::new(Arc::new(
            MemorySecretStore::new(),
        )));
        let secrets: Arc<dyn SecretStore> = Arc::<SwitchableSecretStore>::clone(&secret_switch);
        let scrobbling_secrets = Arc::clone(&secrets);
        let controller = Self {
            settings: super::settings_controller::SettingsController::new(
                store.clone(),
                scrobbling_secrets,
            ),
            store,
            runtime,
            secrets,
            secret_switch,
            queue: Arc::new(Mutex::new(None)),
            play_activation_generation: Arc::new(AtomicU64::new(0)),
            queue_persist_generation: Arc::new(AtomicU64::new(0)),
            playback_request_generation: Arc::new(AtomicU64::new(0)),
            next_preload: Arc::new(Mutex::new(NextPreloadState::default())),
            waveform_warm_generation: Arc::new(AtomicU64::new(0)),
            playback: Arc::new(Mutex::new(Box::new(FakePlaybackBackend::new()))),
            playback_snapshot: Arc::new(Mutex::new(PlaybackSnapshot {
                auto_dj_enabled: settings.auto_dj_enabled,
                volume: settings.playback.volume,
                muted: settings.playback.muted,
                ..PlaybackSnapshot::default()
            })),
            playback_activity: Arc::new(Mutex::new(PlaybackActivityState::default())),
            auto_dj_enabled: Arc::new(Mutex::new(settings.auto_dj_enabled)),
            last_progress_snapshot: Arc::new(Mutex::new(None)),
            last_report_snapshot: Arc::new(Mutex::new(None)),
            external_scrobble_state: Arc::new(Mutex::new(ExternalScrobbleState::default())),
            external_cover_retry_generation: Arc::new(AtomicU64::new(0)),
            events,
            sync_in_flight: InFlightGuards::new("Sync"),
            home_refresh_in_flight: InFlightGuards::new("Home refresh"),
            explore_prefetch_in_flight: InFlightGuards::new("Explore prefetch"),
            cover_in_flight: Arc::new(Mutex::new(HashMap::new())),
            external_cover_prefetch_in_flight: Arc::new(Mutex::new(HashMap::new())),
            cover_slots: Arc::new((Mutex::new(0), Condvar::new())),
            _test_permit: test_permit,
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
}

#[cfg(any(test, feature = "dev-tools"))]
fn fake_snapshot(
    store: &StoreHandle,
    runtime: &Runtime,
    scale: FakeScale,
) -> Result<LibrarySnapshot, String> {
    let started = std::time::Instant::now();
    let provider = FakeProvider::new(scale);
    let server = provider.identity().server.clone();
    let saved = SavedServer {
        server: server.clone(),
        user_id: "fake-user".to_string(),
        username: "fake".to_string(),
        trust_invalid_cert: false,
        use_jellyfin_instant_mix: false,
    };
    store.with_store(|store| {
        store.save_server(&saved)?;
        store.set_active_server(&server.id)?;
        Ok(())
    })?;
    let (
        home_sections,
        album_page,
        track_page,
        artist_page,
        album_artist_page,
        genre_page,
        playlist_page,
    ) = runtime.block_on(async {
        let home_sections = provider
            .home_sections()
            .await
            .map_err(|error| error.to_string())?;
        let album_page = provider
            .albums(PagedRequest::new(0, SNAPSHOT_GRID_LIMIT))
            .await
            .map_err(|error| error.to_string())?;
        let track_page = provider
            .tracks(PagedRequest::new(0, SNAPSHOT_TRACK_LIMIT))
            .await
            .map_err(|error| error.to_string())?;
        let artist_page = provider
            .artists(PagedRequest::new(0, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        let album_artist_page = provider
            .album_artists(PagedRequest::new(0, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        let genre_page = provider
            .genres(PagedRequest::new(0, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        let playlist_page = provider
            .playlists(PagedRequest::new(0, PAGE_SIZE))
            .await
            .map_err(|error| error.to_string())?;
        Ok::<_, String>((
            home_sections,
            album_page,
            track_page,
            artist_page,
            album_artist_page,
            genre_page,
            playlist_page,
        ))
    })?;
    info!(
        ?scale,
        cached_albums = album_page.total,
        cached_tracks = track_page.total,
        preloaded_albums = album_page.items.len(),
        preloaded_tracks = track_page.items.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "built direct fake snapshot"
    );
    Ok(LibrarySnapshot {
        server: Some(server.clone()),
        servers: vec![server.clone()],
        selected_source: Some(LibrarySourceSelection::Server(server.id)),
        local_folders: Vec::new(),
        server_local_access: Vec::new(),
        local_access: None,
        local_access_status: LocalAccessStatus::default(),
        music_folders: Vec::new(),
        selected_music_folder_id: None,
        username: Some(saved.username),
        first_run: false,
        sync_status: "Fake library ready".to_string(),
        last_error: None,
        cached_album_count: album_page.total,
        cached_track_count: track_page.total,
        cached_artist_count: artist_page.total,
        cached_album_artist_count: album_artist_page.total,
        cached_genre_count: genre_page.total,
        cached_playlist_count: playlist_page.total,
        home_sections,
        prefetched_explore: None,
        albums: album_page.items,
        tracks: track_page.items,
        artists: artist_page.items,
        album_artists: album_artist_page.items,
        genres: genre_page.items,
        playlists: playlist_page.items,
        playlist_entry_keys: HashMap::new(),
        favorites: Vec::new(),
        search: SearchResults::default(),
    })
}
