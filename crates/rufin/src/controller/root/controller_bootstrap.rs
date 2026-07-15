use super::*;

pub(crate) fn bootstrap() -> Result<ProductAssembly, String> {
    let runtime = Runtime::new()
        .map(Arc::new)
        .map_err(|error| format!("failed to create Tokio runtime: {error}"))?;
    let artwork_root = artwork_cache_dir().unwrap_or_else(|| PathBuf::from("covers"));
    let (artwork, artwork_events) =
        crate::controller::artwork::open(&artwork_root, Arc::clone(&runtime))?;
    let (source_events, library_events, playback_events, lyrics_events, receivers) =
        product_event_channels(artwork_events);
    let store = StoreHandle::open_for_app()?;
    let settings = load_settings_from_store(&store);
    let secret_switch = Arc::new(SwitchableSecretStore::new(platform_secret_store(&settings)));
    let secrets: Arc<dyn SecretStore> = Arc::<SwitchableSecretStore>::clone(&secret_switch);
    let mut presentation = load_runtime_source_presentation(&store, &secrets)?;
    let active = if presentation.first_run {
        None
    } else {
        let selection = presentation
            .selected_source
            .clone()
            .ok_or_else(|| "Active source presentation has no selection.".to_string())?;
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
                presentation.first_run = true;
                None
            }
        }
    };
    let playback_source_id = active.as_ref().map(|active| active.identity.id.clone());
    let active_source = Arc::new(std::sync::RwLock::new(active));
    let settings =
        super::settings_controller::SettingsController::new(store.clone(), Arc::clone(&secrets));
    let playback_product = Arc::new(std::sync::RwLock::new(None));
    let source_transitions = Arc::new(SourceTransitions::new());
    let lyrics_request_generation = Arc::new(AtomicU64::new(0));
    let waveform_request_key = Arc::new(Mutex::new(None));
    let waveform_warm_generation = Arc::new(AtomicU64::new(0));
    let sync_coordinator = Arc::new(Mutex::new(library_sync::SyncCoordinator::new()));
    let source = SourceCommands {
        store: store.clone(),
        runtime: Arc::clone(&runtime),
        active_source: Arc::clone(&active_source),
        secrets: Arc::clone(&secrets),
        playback_product: Arc::clone(&playback_product),
        source_transitions: Arc::clone(&source_transitions),
        sync_coordinator: Arc::clone(&sync_coordinator),
        artwork: artwork.clone(),
        source_events: source_events.clone(),
        library_events: library_events.clone(),
        playback_projection: playback_events.projection.clone(),
    };
    let library = LibraryCommands {
        store: store.clone(),
        runtime: Arc::clone(&runtime),
        active_source: Arc::clone(&active_source),
        secrets: Arc::clone(&secrets),
        library_events: library_events.clone(),
        home_refresh_in_flight: InFlightGuards::new("Home refresh"),
        explore_prefetch_in_flight: InFlightGuards::new("Explore prefetch"),
    };
    let playback_commands = PlaybackCommands {
        store: store.clone(),
        runtime: Arc::clone(&runtime),
        active_source: Arc::clone(&active_source),
        settings: settings.clone(),
        playback_product: Arc::clone(&playback_product),
        waveform_request_key,
        waveform_warm_generation,
        artwork: artwork.clone(),
        library_events: library_events.clone(),
        playback_events: playback_events.clone(),
    };
    let artwork_commands = ArtworkCommands {
        active_source: Arc::clone(&active_source),
        artwork,
    };
    let lyrics = LyricsCommands {
        store: store.clone(),
        runtime: Arc::clone(&runtime),
        active_source: Arc::clone(&active_source),
        playback_product: Arc::clone(&playback_product),
        lyrics_request_generation: Arc::clone(&lyrics_request_generation),
        lyrics_events: lyrics_events.clone(),
    };
    let ui_settings = UiSettingsStore {
        store,
        active_source,
        secrets,
        secret_switch,
        settings,
        playback_product,
        source_transitions,
        lyrics_request_generation,
        sync_coordinator,
        source_presentation: source_events.presentation,
        library_sync_events: source_events.sync,
    };
    let owners = ProductOwners {
        source,
        library,
        playback: playback_commands,
        artwork: artwork_commands,
        lyrics,
        settings: ui_settings,
    };
    let playback = playback_source_id
        .as_ref()
        .map(|source_id| owners.playback.activate_playback_source(source_id))
        .transpose()?;
    Ok((owners, receivers, presentation, playback))
}

#[cfg(test)]
pub(in crate::controller) fn bootstrap_memory_for_test() -> ProductAssembly {
    let runtime = Runtime::new()
        .map(Arc::new)
        .unwrap_or_else(|error| panic!("failed to create Tokio runtime: {error}"));
    let store = StoreHandle::open_memory()
        .unwrap_or_else(|error| panic!("failed to open memory store: {error}"));
    let presentation = load_source_presentation(&store).unwrap_or_else(|error| {
        panic!("failed to load memory source presentation: {error}");
    });
    let secret_switch = Arc::new(SwitchableSecretStore::new(Arc::new(
        MemorySecretStore::new(),
    )));
    let secrets: Arc<dyn SecretStore> = Arc::<SwitchableSecretStore>::clone(&secret_switch);
    let (artwork, artwork_events) = crate::controller::artwork::open_for_test(Arc::clone(&runtime));
    let (source_events, library_events, playback_events, lyrics_events, receivers) =
        product_event_channels(artwork_events);
    let active_source = Arc::new(std::sync::RwLock::new(None));
    let settings =
        super::settings_controller::SettingsController::new(store.clone(), Arc::clone(&secrets));
    let playback_product = Arc::new(std::sync::RwLock::new(None));
    let source_transitions = Arc::new(SourceTransitions::new());
    let lyrics_request_generation = Arc::new(AtomicU64::new(0));
    let sync_coordinator = Arc::new(Mutex::new(library_sync::SyncCoordinator::new()));
    let owners = ProductOwners {
        source: SourceCommands {
            store: store.clone(),
            runtime: Arc::clone(&runtime),
            active_source: Arc::clone(&active_source),
            secrets: Arc::clone(&secrets),
            playback_product: Arc::clone(&playback_product),
            source_transitions: Arc::clone(&source_transitions),
            sync_coordinator: Arc::clone(&sync_coordinator),
            artwork: artwork.clone(),
            source_events: source_events.clone(),
            library_events: library_events.clone(),
            playback_projection: playback_events.projection.clone(),
        },
        library: LibraryCommands {
            store: store.clone(),
            runtime: Arc::clone(&runtime),
            active_source: Arc::clone(&active_source),
            secrets: Arc::clone(&secrets),
            library_events: library_events.clone(),
            home_refresh_in_flight: InFlightGuards::new("Home refresh"),
            explore_prefetch_in_flight: InFlightGuards::new("Explore prefetch"),
        },
        playback: PlaybackCommands {
            store: store.clone(),
            runtime: Arc::clone(&runtime),
            active_source: Arc::clone(&active_source),
            settings: settings.clone(),
            playback_product: Arc::clone(&playback_product),
            waveform_request_key: Arc::new(Mutex::new(None)),
            waveform_warm_generation: Arc::new(AtomicU64::new(0)),
            artwork: artwork.clone(),
            library_events: library_events.clone(),
            playback_events: playback_events.clone(),
        },
        artwork: ArtworkCommands {
            active_source: Arc::clone(&active_source),
            artwork,
        },
        lyrics: LyricsCommands {
            store: store.clone(),
            runtime: Arc::clone(&runtime),
            active_source: Arc::clone(&active_source),
            playback_product: Arc::clone(&playback_product),
            lyrics_request_generation: Arc::clone(&lyrics_request_generation),
            lyrics_events: lyrics_events.clone(),
        },
        settings: UiSettingsStore {
            store,
            active_source,
            secrets,
            secret_switch,
            settings,
            playback_product,
            source_transitions,
            lyrics_request_generation,
            sync_coordinator,
            source_presentation: source_events.presentation,
            library_sync_events: source_events.sync,
        },
    };
    (owners, receivers, presentation, None)
}
