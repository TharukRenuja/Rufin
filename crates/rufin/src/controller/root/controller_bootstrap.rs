use super::*;

impl AppController {
    pub(crate) fn bootstrap() -> Result<ControllerBootstrap, String> {
        let (events, receiver) = channel();
        let runtime = Runtime::new()
            .map(Arc::new)
            .map_err(|error| format!("failed to create Tokio runtime: {error}"))?;
        let artwork_root = artwork_cache_dir().unwrap_or_else(|| PathBuf::from("covers"));
        let (artwork, artwork_events) =
            crate::controller::artwork::open(&artwork_root, Arc::clone(&runtime))?;
        crate::controller::artwork::forward_events(artwork_events, events.clone())?;
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
                    crate::source_setup::ensure_local_configured_source(&store)?
                }
                LibrarySourceSelection::Source(source_id) => store
                    .with_store(|store| store.stored_source(&source_id))?
                    .ok_or_else(|| "The selected source is no longer saved.".to_string())?,
            };
            match crate::source_setup::activate_configured_source(&store, &secrets, &saved) {
                Ok(active) => {
                    store.with_store(|store| store.set_active_source(&saved.source_id))?;
                    Some(active)
                }
                Err(error) => {
                    warn!(%error, source_id = %saved.source_id, "failed to activate selected source");
                    snapshot.first_run = true;
                    None
                }
            }
        };
        let playback_source_id = active.as_ref().map(|active| active.identity.id.clone());
        let active_source = Arc::new(std::sync::RwLock::new(active));
        let scrobbling_secrets = Arc::clone(&secrets);
        let controller = Self {
            settings: super::settings_controller::SettingsController::new(
                store.clone(),
                scrobbling_secrets,
            ),
            store,
            runtime,
            active_source,
            secrets,
            secret_switch,
            playback_product: Arc::new(std::sync::RwLock::new(None)),
            source_transitions: Arc::new(SourceTransitions::new()),
            lyrics_request_generation: Arc::new(AtomicU64::new(0)),
            waveform_request_key: Arc::new(Mutex::new(None)),
            waveform_warm_generation: Arc::new(AtomicU64::new(0)),
            sync_coordinator: Arc::new(Mutex::new(library_sync::SyncCoordinator::new())),
            artwork,
            events,
            home_refresh_in_flight: InFlightGuards::new("Home refresh"),
            explore_prefetch_in_flight: InFlightGuards::new("Explore prefetch"),
        };
        let playback = playback_source_id
            .as_ref()
            .map(|source_id| controller.activate_playback_source(source_id))
            .transpose()?;
        Ok((controller, receiver, snapshot, playback))
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
        let secret_switch = Arc::new(SwitchableSecretStore::new(Arc::new(
            MemorySecretStore::new(),
        )));
        let secrets: Arc<dyn SecretStore> = Arc::<SwitchableSecretStore>::clone(&secret_switch);
        let scrobbling_secrets = Arc::clone(&secrets);
        let artwork =
            crate::controller::artwork::open_for_test(Arc::clone(&runtime), events.clone());
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
            playback_product: Arc::new(std::sync::RwLock::new(None)),
            source_transitions: Arc::new(SourceTransitions::new()),
            lyrics_request_generation: Arc::new(AtomicU64::new(0)),
            waveform_request_key: Arc::new(Mutex::new(None)),
            waveform_warm_generation: Arc::new(AtomicU64::new(0)),
            sync_coordinator: Arc::new(Mutex::new(library_sync::SyncCoordinator::new())),
            artwork,
            events,
            home_refresh_in_flight: InFlightGuards::new("Home refresh"),
            explore_prefetch_in_flight: InFlightGuards::new("Explore prefetch"),
        };
        (controller, receiver, snapshot, None)
    }
}
