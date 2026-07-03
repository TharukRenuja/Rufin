use super::*;

impl AppController {
    pub fn select_source(&self, source: LibrarySourceSelection) {
        let selection_generation = self
            .source_selection_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let _sent = self.events.send(ControllerEvent::SourceSelectionChanged {
            selected_source: source.clone(),
        });
        let source_selection_generation = Arc::clone(&self.source_selection_generation);
        let sync_context = self.sync_context();
        let store = sync_context.store.clone();
        let events = sync_context.events.clone();
        let local_library_watcher = Arc::clone(&self.local_library_watcher);
        let remote_library_watcher = Arc::clone(&self.remote_library_watcher);
        let queue = Arc::clone(&self.queue);
        let playback_request_generation = Arc::clone(&self.playback_request_generation);
        let next_preload = Arc::clone(&self.next_preload);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        thread::spawn(move || {
            let current =
                || source_selection_is_current(&source_selection_generation, selection_generation);
            let emit_error = |error| {
                emit_current_source_selection_error(
                    &events,
                    &source_selection_generation,
                    selection_generation,
                    error,
                );
            };
            let previous_active = match store
                .with_store(|store| Ok(store.active_server()?.map(|saved| saved.server.id)))
            {
                Ok(previous_active) => previous_active,
                Err(error) => {
                    emit_error(error);
                    return;
                }
            };
            if !current() {
                return;
            }
            let mut settings = load_settings_from_store(&store);
            settings.sources.selected = Some(source.clone());
            settings.migrate_defaults();
            if !current() {
                return;
            }
            if let Err(error) = store.save_settings(&settings) {
                emit_error(error);
                return;
            }
            if !current() {
                return;
            }

            let queue_activation_context = QueueActivationContext {
                store: &store,
                queue: &queue,
                playback_request_generation: &playback_request_generation,
                next_preload: &next_preload,
                playback: &playback,
                playback_snapshot: &playback_snapshot,
                auto_dj_enabled: &auto_dj_enabled,
                events: &events,
            };

            let (selected_saved_needing_sync, selected_saved_for_reconciliation) = match source {
                LibrarySourceSelection::Local => {
                    let saved = match ensure_local_source_server(&store) {
                        Ok(saved) => saved,
                        Err(error) => {
                            emit_error(error);
                            return;
                        }
                    };
                    if !current() {
                        return;
                    }
                    if let Err(error) =
                        cancel_previous_source_sync(&sync_context, previous_active.as_ref(), &saved)
                    {
                        emit_error(error);
                        return;
                    }
                    if !current() {
                        return;
                    }
                    if let Err(error) =
                        store.with_store(|store| store.set_active_server(&saved.server.id))
                    {
                        emit_error(error);
                        return;
                    }
                    let activation =
                        match prepare_saved_queue_activation(&queue_activation_context, &saved) {
                            Ok(activation) => activation,
                            Err(error) => {
                                emit_error(error);
                                return;
                            }
                        };
                    if !current() {
                        return;
                    }
                    if let Some(activation) = activation
                        && let Err(error) =
                            apply_prepared_queue_activation(&queue_activation_context, activation)
                    {
                        emit_error(error);
                        return;
                    }
                    if !current() {
                        return;
                    }
                    let local_configured = !settings.sources.local_folders.is_empty();
                    let needs_sync =
                        local_configured && active_server_needs_sync(&store, &saved.server.id);
                    (
                        needs_sync.then_some(saved.clone()),
                        (local_configured && !needs_sync).then_some(saved),
                    )
                }
                LibrarySourceSelection::Server(server_id) => {
                    let saved = match store.with_store(|store| {
                        let saved = store
                            .list_servers()?
                            .into_iter()
                            .find(|saved| saved.server.id == server_id);
                        Ok(saved)
                    }) {
                        Ok(Some(saved)) => saved,
                        Ok(None) => {
                            emit_error("The selected source is no longer saved.".to_string());
                            return;
                        }
                        Err(error) => {
                            emit_error(error);
                            return;
                        }
                    };
                    if !current() {
                        return;
                    }
                    if let Err(error) =
                        cancel_previous_source_sync(&sync_context, previous_active.as_ref(), &saved)
                    {
                        emit_error(error);
                        return;
                    }
                    if !current() {
                        return;
                    }
                    if let Err(error) =
                        store.with_store(|store| store.set_active_server(&server_id))
                    {
                        emit_error(error);
                        return;
                    }
                    if !current() {
                        return;
                    }
                    if saved_server_needs_auth(&sync_context.secrets, &saved) {
                        clear_queue_and_stop_playback(
                            &queue,
                            &playback_request_generation,
                            &next_preload,
                            &playback,
                            &playback_snapshot,
                            &auto_dj_enabled,
                            &events,
                        );
                        if !current() {
                            return;
                        }
                        if !emit_current_runtime_snapshot(
                            &store,
                            &sync_context.secrets,
                            &events,
                            &source_selection_generation,
                            selection_generation,
                        ) {
                            return;
                        }
                        if !current() {
                            return;
                        }
                        refresh_local_library_watcher(
                            sync_context.clone(),
                            Arc::clone(&local_library_watcher),
                        );
                        refresh_remote_library_watcher(
                            sync_context,
                            Arc::clone(&remote_library_watcher),
                        );
                        return;
                    }
                    let activation =
                        match prepare_saved_queue_activation(&queue_activation_context, &saved) {
                            Ok(activation) => activation,
                            Err(error) => {
                                emit_error(error);
                                return;
                            }
                        };
                    if !current() {
                        return;
                    }
                    if let Some(activation) = activation
                        && let Err(error) =
                            apply_prepared_queue_activation(&queue_activation_context, activation)
                    {
                        emit_error(error);
                        return;
                    }
                    if !current() {
                        return;
                    }
                    let needs_sync = active_server_needs_sync(&store, &saved.server.id);
                    (
                        needs_sync.then_some(saved.clone()),
                        (!needs_sync).then_some(saved),
                    )
                }
            };

            if !current() {
                return;
            }
            if let Some(saved) = selected_saved_needing_sync {
                if cached_library_exists(&store, &saved.server.id) {
                    if !emit_current_runtime_snapshot(
                        &store,
                        &sync_context.secrets,
                        &events,
                        &source_selection_generation,
                        selection_generation,
                    ) {
                        return;
                    }
                    if !current() {
                        return;
                    }
                    start_silent_sync_thread(sync_context.clone(), saved);
                } else {
                    start_silent_sync_thread_with_completion_snapshot(sync_context.clone(), saved);
                }
            } else {
                if !emit_current_runtime_snapshot(
                    &store,
                    &sync_context.secrets,
                    &events,
                    &source_selection_generation,
                    selection_generation,
                ) {
                    return;
                }
                if let Some(saved) = selected_saved_for_reconciliation {
                    start_background_sync_thread(sync_context.clone(), saved);
                }
            }
            if !current() {
                return;
            }
            refresh_local_library_watcher(sync_context.clone(), local_library_watcher);
            refresh_remote_library_watcher(sync_context, remote_library_watcher);
        });
    }
}

fn source_selection_is_current(
    source_selection_generation: &Arc<AtomicU64>,
    selection_generation: u64,
) -> bool {
    source_selection_generation.load(Ordering::Acquire) == selection_generation
}

fn emit_current_source_selection_error(
    events: &Sender<ControllerEvent>,
    source_selection_generation: &Arc<AtomicU64>,
    selection_generation: u64,
    error: String,
) {
    if source_selection_is_current(source_selection_generation, selection_generation) {
        let _sent = events.send(ControllerEvent::Error(error));
    }
}

fn emit_current_runtime_snapshot(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    events: &Sender<ControllerEvent>,
    source_selection_generation: &Arc<AtomicU64>,
    selection_generation: u64,
) -> bool {
    match load_runtime_snapshot(store, secrets) {
        Ok(snapshot) => {
            if !source_selection_is_current(source_selection_generation, selection_generation) {
                return false;
            }
            let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
            true
        }
        Err(error) => {
            emit_current_source_selection_error(
                events,
                source_selection_generation,
                selection_generation,
                error,
            );
            false
        }
    }
}

fn cancel_previous_source_sync(
    sync_context: &SyncContext,
    previous_active: Option<&ServerId>,
    selected: &SavedServer,
) -> Result<(), String> {
    let Some(previous_id) = previous_active else {
        return Ok(());
    };
    if previous_id == &selected.server.id {
        return Ok(());
    }
    cancel_sync_if_running(&sync_context.sync_in_flight, previous_id).map(|_| ())
}
