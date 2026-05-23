impl AppController {
    pub fn set_selected_music_folder(&self, server_id: ServerId, folder_id: Option<MusicFolderId>) {
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            if let Err(error) = store.with_store(|store| {
                store.set_selected_music_folder_id(&server_id, folder_id.as_ref())
            }) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            match load_snapshot(&store) {
                Ok(snapshot) => {
                    let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }
        });
    }
    pub fn load_folder_for_active(&self, request_id: u64, path: Vec<FolderPathItem>) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        thread::spawn(move || {
            let result = load_folder_detail(&store, &runtime, &secrets, &path);
            match result {
                Ok(detail) => {
                    let _sent = events.send(ControllerEvent::FolderLoaded {
                        request_id,
                        path,
                        detail,
                    });
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::FolderLoadFailed {
                        request_id,
                        path,
                        error,
                    });
                }
            }
        });
    }
    pub fn search(&self, query: String) {
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            let settings = load_settings_for_active_server(&store);
            let mut snapshot = match load_snapshot(&store) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            if let Some(server) = &snapshot.server {
                match store.with_store(|store| store.search_library(&server.id, &query, 50)) {
                    Ok(mut results) => {
                        external_metadata::normalize_search_results(&mut results, &settings);
                        snapshot.search = results;
                    }
                    Err(error) => {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                }
            }
            let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
        });
    }
}
