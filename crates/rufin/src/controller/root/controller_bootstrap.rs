use super::*;

impl AppController {
    pub(crate) fn bootstrap() -> Result<ControllerBootstrap, String> {
        let (events, receiver) = channel();
        let runtime = Runtime::new()
            .map(Arc::new)
            .map_err(|error| format!("failed to create Tokio runtime: {error}"))?;
        let store = StoreHandle::open_for_app()?;
        let settings = load_settings_from_store(&store);
        let secret_switch = Arc::new(SwitchableSecretStore::new(platform_secret_store(&settings)));
        let secrets: Arc<dyn SecretStore> = Arc::<SwitchableSecretStore>::clone(&secret_switch);
        let mut snapshot = load_runtime_snapshot(&store, &secrets)?;
        let active = if snapshot.first_run {
            None
        } else {
            let selection = snapshot
                .selected_source
                .clone()
                .ok_or_else(|| "Active library snapshot has no selected source.".to_string())?;
            let saved = match selection {
                LibrarySourceSelection::Local => {
                    crate::sources::ensure_local_configured_source(&store)?
                }
                LibrarySourceSelection::Source(source_id) => store
                    .with_store(|store| store.saved_source(&source_id))?
                    .ok_or_else(|| "The selected source is no longer saved.".to_string())?,
            };
            match crate::sources::activate_configured_source(&store, &secrets, &saved) {
                Ok(active) => {
                    store.with_store(|store| store.set_active_source(&saved.source.id))?;
                    Some(active)
                }
                Err(error) => {
                    warn!(%error, source_id = %saved.source.id, "failed to activate selected source");
                    snapshot.first_run = true;
                    snapshot.sync_status =
                        "Connect once more to continue using this server.".to_string();
                    snapshot.last_error = None;
                    None
                }
            }
        };
        let queue = if snapshot.first_run && snapshot.source.is_some() {
            None
        } else {
            restore_queue(&store, snapshot.source.as_ref())
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
            active_source: Arc::new(std::sync::RwLock::new(active)),
            secrets,
            secret_switch,
            queue: Arc::new(Mutex::new(queue)),
            source_transitions: Arc::new(SourceTransitions::new()),
            play_activation_generation: Arc::new(AtomicU64::new(0)),
            queue_persist_generation: Arc::new(AtomicU64::new(0)),
            playback_request_generation: Arc::new(AtomicU64::new(0)),
            next_preload: Arc::new(Mutex::new(NextPreloadState::default())),
            waveform_warm_generation: Arc::new(AtomicU64::new(0)),
            playback: Arc::new(Mutex::new(
                Box::new(LazyGStreamerPlaybackBackend::new()) as Box<dyn PlaybackBackend>
            )),
            playback_snapshot: Arc::new(Mutex::new(playback_snapshot.clone())),
            playback_activity: Arc::new(Mutex::new(PlaybackActivityState::default())),
            auto_dj_enabled: Arc::new(Mutex::new(settings.auto_dj_enabled)),
            last_progress_snapshot: Arc::new(Mutex::new(None)),
            last_report_snapshot: Arc::new(Mutex::new(None)),
            external_scrobble_state: Arc::new(Mutex::new(ExternalScrobbleState::default())),
            source_freshness_watcher: Arc::new(Mutex::new(None)),
            external_cover_retry_generation: Arc::new(AtomicU64::new(0)),
            events,
            sync_in_flight: InFlightGuards::new("Sync"),
            home_refresh_in_flight: InFlightGuards::new("Home refresh"),
            explore_prefetch_in_flight: InFlightGuards::new("Explore prefetch"),
            cover_in_flight: Arc::new(Mutex::new(HashMap::new())),
            external_cover_prefetch_in_flight: Arc::new(Mutex::new(HashMap::new())),
            cover_slots: Arc::new((Mutex::new(0), Condvar::new())),
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
            active_source: Arc::new(std::sync::RwLock::new(None)),
            secrets,
            secret_switch,
            queue: Arc::new(Mutex::new(None)),
            source_transitions: Arc::new(SourceTransitions::new()),
            play_activation_generation: Arc::new(AtomicU64::new(0)),
            queue_persist_generation: Arc::new(AtomicU64::new(0)),
            playback_request_generation: Arc::new(AtomicU64::new(0)),
            next_preload: Arc::new(Mutex::new(NextPreloadState::default())),
            waveform_warm_generation: Arc::new(AtomicU64::new(0)),
            playback: Arc::new(Mutex::new(Box::new(playback::FakePlaybackBackend::new()))),
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
            source_freshness_watcher: Arc::new(Mutex::new(None)),
            external_cover_retry_generation: Arc::new(AtomicU64::new(0)),
            events,
            sync_in_flight: InFlightGuards::new("Sync"),
            home_refresh_in_flight: InFlightGuards::new("Home refresh"),
            explore_prefetch_in_flight: InFlightGuards::new("Explore prefetch"),
            cover_in_flight: Arc::new(Mutex::new(HashMap::new())),
            external_cover_prefetch_in_flight: Arc::new(Mutex::new(HashMap::new())),
            cover_slots: Arc::new((Mutex::new(0), Condvar::new())),
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
