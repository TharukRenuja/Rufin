impl AppController {
    pub fn add_local_server(&self, root_path: PathBuf) {
        self.add_local_server_folders(vec![root_path]);
    }
    pub fn add_local_server_folders(&self, root_paths: Vec<PathBuf>) {
        self.add_local_library_folders_with_selection(root_paths, true);
    }
    pub fn add_local_library_folder(&self, root_path: PathBuf) {
        self.add_local_library_folders_with_selection(vec![root_path], false);
    }
    fn add_local_library_folders_with_selection(
        &self,
        root_paths: Vec<PathBuf>,
        select_local: bool,
    ) {
        let sync_context = self.sync_context();
        let store = sync_context.store.clone();
        let events = sync_context.events.clone();
        let queue = Arc::clone(&self.queue);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        thread::spawn(move || {
            if root_paths.is_empty() {
                let _sent = events.send(ControllerEvent::Error(
                    "Choose at least one local music folder.".to_string(),
                ));
                return;
            }
            let mut local_paths = Vec::new();
            for root_path in root_paths {
                match LocalProvider::identity_for_root(&root_path) {
                    Ok(identity) => {
                        if !local_paths.iter().any(|path| path == &identity.base_url) {
                            local_paths.push(identity.base_url);
                        }
                    }
                    Err(error) => {
                        let _sent = events.send(ControllerEvent::Error(error.to_string()));
                        return;
                    }
                }
            }
            let mut settings = load_settings_from_store(&store);
            for path in local_paths {
                if !settings
                    .sources
                    .local_folders
                    .iter()
                    .any(|folder| folder.path == path)
                {
                    settings
                        .sources
                        .local_folders
                        .push(LocalLibraryFolder { path });
                }
            }
            if select_local {
                settings.sources.selected = Some(LibrarySourceSelection::Local);
            }
            settings.migrate_defaults();
            if let Err(error) = store.save_settings(&settings) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            let saved = match ensure_local_source_server(&store) {
                Ok(saved) => saved,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            if select_local
                && let Err(error) =
                    store.with_store(|store| store.set_active_server(&saved.server.id))
            {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if select_local
                && let Err(error) = activate_queue_for_saved_and_emit(
                    &store,
                    &queue,
                    &playback,
                    &playback_snapshot,
                    &auto_dj_enabled,
                    &events,
                    &saved,
                )
            {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            emit_snapshot(&store, &events);
            if select_local {
                start_sync_thread(sync_context, saved);
            }
        });
    }
    pub fn remove_local_library_folder(&self, path: String) {
        let sync_context = self.sync_context();
        let store = sync_context.store.clone();
        let events = sync_context.events.clone();
        thread::spawn(move || {
            let mut settings = load_settings_from_store(&store);
            let before = settings.sources.local_folders.len();
            settings
                .sources
                .local_folders
                .retain(|folder| folder.path != path);
            if settings.sources.local_folders.len() == before {
                return;
            }
            let selected_local = matches!(
                settings.sources.selected,
                Some(LibrarySourceSelection::Local)
            );
            if selected_local && settings.sources.local_folders.is_empty() {
                settings.sources.selected = None;
            }
            settings.migrate_defaults();
            if let Err(error) = store.save_settings(&settings) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            let saved = match ensure_local_source_server(&store) {
                Ok(saved) => saved,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            let result = store.with_store(|store| {
                if selected_local && !settings.sources.local_folders.is_empty() {
                    store.set_active_server(&saved.server.id)?;
                }
                store.clear_library_cache(&saved.server.id)
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            emit_snapshot(&store, &events);
            if selected_local && !settings.sources.local_folders.is_empty() {
                start_sync_thread(sync_context, saved);
            }
        });
    }
}
