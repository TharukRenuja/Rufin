use super::*;

impl AppController {
    pub fn set_selected_music_folder(&self, source_id: SourceId, folder_id: Option<MusicFolderId>) {
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            if let Err(error) = store.with_store(|store| {
                store.set_selected_music_folder_id(&source_id, folder_id.as_ref())
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
    pub fn load_search_for_active(&self, expected: SearchRequestKey) {
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            let settings = load_settings_for_active_source(&store);
            let query = expected.query.clone();
            let kind = expected.kind.clone();
            let (key, mut results) = match store.with_store(|store| {
                let Some(saved) = store.active_source()? else {
                    return Ok((expected.clone(), SearchResults::default()));
                };
                let source_id = saved.source.id.clone();
                let selected_music_folder_id = store.selected_music_folder_id(&source_id)?;
                let key = SearchRequestKey {
                    request_id: expected.request_id,
                    query: query.clone(),
                    kind: kind.clone(),
                    source_id: Some(source_id.clone()),
                    selected_music_folder_id,
                };
                let results = store.search_library(&source_id, &query, 50)?;
                Ok((key, results))
            }) {
                Ok(result) => result,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::SearchFailed {
                        key: expected,
                        error,
                    });
                    return;
                }
            };
            cover_art_policy::bind_search_results(&mut results, &settings);
            let _sent = events.send(ControllerEvent::SearchLoaded { key, results });
        });
    }
}
