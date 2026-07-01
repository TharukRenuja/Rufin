use super::*;

impl AppController {
    pub fn select_source(&self, source: LibrarySourceSelection) {
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
            let previous_active = match store
                .with_store(|store| Ok(store.active_server()?.map(|saved| saved.server.id)))
            {
                Ok(previous_active) => previous_active,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            let mut settings = load_settings_from_store(&store);
            settings.sources.selected = Some(source.clone());
            settings.migrate_defaults();
            if let Err(error) = store.save_settings(&settings) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }

            let (selected_saved_needing_sync, selected_saved_for_reconciliation) = match source {
                LibrarySourceSelection::Local => {
                    let saved = match ensure_local_source_server(&store) {
                        Ok(saved) => saved,
                        Err(error) => {
                            let _sent = events.send(ControllerEvent::Error(error));
                            return;
                        }
                    };
                    if let Err(error) =
                        cancel_previous_source_sync(&sync_context, previous_active.as_ref(), &saved)
                    {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                    if let Err(error) =
                        store.with_store(|store| store.set_active_server(&saved.server.id))
                    {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                    if let Err(error) = activate_saved_queue(
                        &QueueActivationContext {
                            store: &store,
                            queue: &queue,
                            playback_request_generation: &playback_request_generation,
                            next_preload: &next_preload,
                            playback: &playback,
                            playback_snapshot: &playback_snapshot,
                            auto_dj_enabled: &auto_dj_enabled,
                            events: &events,
                        },
                        &saved,
                    ) {
                        let _sent = events.send(ControllerEvent::Error(error));
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
                        if saved.is_some() {
                            store.set_active_server(&server_id)?;
                        }
                        Ok(saved)
                    }) {
                        Ok(Some(saved)) => saved,
                        Ok(None) => {
                            let _sent = events.send(ControllerEvent::Error(
                                "The selected source is no longer saved.".to_string(),
                            ));
                            return;
                        }
                        Err(error) => {
                            let _sent = events.send(ControllerEvent::Error(error));
                            return;
                        }
                    };
                    if let Err(error) =
                        cancel_previous_source_sync(&sync_context, previous_active.as_ref(), &saved)
                    {
                        let _sent = events.send(ControllerEvent::Error(error));
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
                        emit_runtime_snapshot(&store, &sync_context.secrets, &events);
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
                    if let Err(error) = activate_saved_queue(
                        &QueueActivationContext {
                            store: &store,
                            queue: &queue,
                            playback_request_generation: &playback_request_generation,
                            next_preload: &next_preload,
                            playback: &playback,
                            playback_snapshot: &playback_snapshot,
                            auto_dj_enabled: &auto_dj_enabled,
                            events: &events,
                        },
                        &saved,
                    ) {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                    let needs_sync = active_server_needs_sync(&store, &saved.server.id);
                    (
                        needs_sync.then_some(saved.clone()),
                        (!needs_sync).then_some(saved),
                    )
                }
            };

            if let Some(saved) = selected_saved_needing_sync {
                start_sync_thread_with_snapshots(sync_context.clone(), saved);
            } else {
                emit_runtime_snapshot(&store, &sync_context.secrets, &events);
                if let Some(saved) = selected_saved_for_reconciliation {
                    start_background_sync_thread(sync_context.clone(), saved);
                }
            }
            refresh_local_library_watcher(sync_context.clone(), local_library_watcher);
            refresh_remote_library_watcher(sync_context, remote_library_watcher);
        });
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
