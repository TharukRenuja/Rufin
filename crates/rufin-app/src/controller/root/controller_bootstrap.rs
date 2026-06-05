use super::*;

impl AppController {
    #[cfg(any(test, feature = "dev-tools"))]
    pub fn bootstrap_with_fake(
        scale: FakeScale,
    ) -> (
        Self,
        Receiver<ControllerEvent>,
        LibrarySnapshot,
        Option<QueueSnapshot>,
        PlaybackSnapshot,
    ) {
        #[cfg(test)]
        let test_permit = Some(controller_test_permit());
        let (events, receiver) = channel();
        let runtime = Runtime::new()
            .map(Arc::new)
            .unwrap_or_else(|error| panic!("failed to create Tokio runtime: {error}"));
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
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let scrobbling_secrets = Arc::clone(&secrets);
        let controller = Self {
            settings: super::settings_controller::SettingsController::new(
                store.clone(),
                scrobbling_secrets,
            ),
            store,
            runtime,
            secrets,
            queue: Arc::new(Mutex::new(queue)),
            play_activation_generation: Arc::new(AtomicU64::new(0)),
            queue_persist_generation: Arc::new(AtomicU64::new(0)),
            playback_request_generation: Arc::new(AtomicU64::new(0)),
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
            playlist_refresh_in_flight: InFlightGuards::new("Playlist refresh"),
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

    pub fn bootstrap() -> (
        Self,
        Receiver<ControllerEvent>,
        LibrarySnapshot,
        Option<QueueSnapshot>,
        PlaybackSnapshot,
    ) {
        #[cfg(test)]
        let test_permit = Some(controller_test_permit());
        let (events, receiver) = channel();
        let runtime = Runtime::new()
            .map(Arc::new)
            .unwrap_or_else(|error| panic!("failed to create Tokio runtime: {error}"));
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
        let secrets = platform_token_store();
        let scrobbling_secrets = platform_secret_store();
        let controller = Self {
            settings: super::settings_controller::SettingsController::new(
                store.clone(),
                scrobbling_secrets,
            ),
            store,
            runtime,
            secrets,
            queue: Arc::new(Mutex::new(queue)),
            play_activation_generation: Arc::new(AtomicU64::new(0)),
            queue_persist_generation: Arc::new(AtomicU64::new(0)),
            playback_request_generation: Arc::new(AtomicU64::new(0)),
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
            playlist_refresh_in_flight: InFlightGuards::new("Playlist refresh"),
            explore_prefetch_in_flight: InFlightGuards::new("Explore prefetch"),
            cover_in_flight: Arc::new(Mutex::new(HashMap::new())),
            external_cover_prefetch_in_flight: Arc::new(Mutex::new(HashMap::new())),
            cover_slots: Arc::new((Mutex::new(0), Condvar::new())),
            #[cfg(test)]
            _test_permit: test_permit,
        };
        controller.warm_playback_backend();
        (
            controller,
            receiver,
            snapshot,
            queue_snapshot,
            playback_snapshot,
        )
    }
    #[cfg(test)]
    pub(in crate::controller) fn bootstrap_memory_for_test() -> (
        Self,
        Receiver<ControllerEvent>,
        LibrarySnapshot,
        Option<QueueSnapshot>,
        PlaybackSnapshot,
    ) {
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
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let scrobbling_secrets = Arc::clone(&secrets);
        let controller = Self {
            settings: super::settings_controller::SettingsController::new(
                store.clone(),
                scrobbling_secrets,
            ),
            store,
            runtime,
            secrets,
            queue: Arc::new(Mutex::new(None)),
            play_activation_generation: Arc::new(AtomicU64::new(0)),
            queue_persist_generation: Arc::new(AtomicU64::new(0)),
            playback_request_generation: Arc::new(AtomicU64::new(0)),
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
            playlist_refresh_in_flight: InFlightGuards::new("Playlist refresh"),
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
