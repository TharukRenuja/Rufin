impl AppController {
    pub fn select_source(&self, source: LibrarySourceSelection) {
        let sync_context = self.sync_context();
        let store = sync_context.store.clone();
        let events = sync_context.events.clone();
        let queue = Arc::clone(&self.queue);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        thread::spawn(move || {
            let mut settings = load_settings_from_store(&store);
            settings.sources.selected = Some(source.clone());
            settings.migrate_defaults();
            if let Err(error) = store.save_settings(&settings) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }

            let sync_saved = match source {
                LibrarySourceSelection::Local => {
                    let saved = match ensure_local_source_server(&store) {
                        Ok(saved) => saved,
                        Err(error) => {
                            let _sent = events.send(ControllerEvent::Error(error));
                            return;
                        }
                    };
                    if let Err(error) =
                        store.with_store(|store| store.set_active_server(&saved.server.id))
                    {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                    if let Err(error) = activate_queue_for_saved_and_emit(
                        &store,
                        &queue,
                        &playback,
                        &playback_snapshot,
                        &auto_dj_enabled,
                        &events,
                        &saved,
                    ) {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                    (!settings.sources.local_folders.is_empty()).then_some(saved)
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
                    if let Err(error) = activate_queue_for_saved_and_emit(
                        &store,
                        &queue,
                        &playback,
                        &playback_snapshot,
                        &auto_dj_enabled,
                        &events,
                        &saved,
                    ) {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                    active_server_needs_sync(&store, &saved.server.id).then_some(saved)
                }
            };

            emit_snapshot(&store, &events);
            if let Some(saved) = sync_saved {
                start_sync_thread(sync_context, saved);
            }
        });
    }
}
