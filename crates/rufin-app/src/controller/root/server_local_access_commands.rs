impl AppController {
    pub fn save_server_local_access(
        &self,
        server_id: ServerId,
        root_path: PathBuf,
        path_replace_from: Option<String>,
        path_replace_to: Option<String>,
    ) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let queue = Arc::clone(&self.queue);
        let playback = Arc::clone(&self.playback);
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(root_path) = root_path.to_str().map(ToString::to_string) else {
                let _sent = events.send(ControllerEvent::Error(
                    "Could not use the selected local folder path.".to_string(),
                ));
                return;
            };
            let path_replace_to =
                trimmed_optional(path_replace_to.as_deref()).unwrap_or_else(|| root_path.clone());
            let matched_server_id = server_id.clone();
            let result = store.with_store(|store| {
                store.save_server_local_access(&ServerLocalAccess {
                    server_id,
                    root_path: root_path.clone(),
                    path_replace_from: trimmed_optional(path_replace_from.as_deref()),
                    path_replace_to: Some(path_replace_to),
                })
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Err(error) =
                runtime.block_on(refresh_local_track_matches(&store, &matched_server_id))
            {
                warn!(%error, "failed to refresh local track matches");
            }
            prepare_next_stream_from_handles(
                store.clone(),
                Arc::clone(&runtime),
                Arc::clone(&secrets),
                Arc::clone(&playback),
                Arc::clone(&queue),
                events.clone(),
            );
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
    pub fn update_server_settings(
        &self,
        server_id: ServerId,
        name: String,
        base_url: String,
        trust_invalid_cert: bool,
    ) {
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            let result = store.with_store(|store| {
                let Some(mut saved) = store
                    .list_servers()?
                    .into_iter()
                    .find(|saved| saved.server.id == server_id)
                else {
                    return Ok(false);
                };
                if saved.server.provider != LOCAL_PROVIDER_ID && base_url.trim().is_empty() {
                    return Ok(false);
                }
                let next_name = name.trim().to_string();
                let next_base_url = if saved.server.provider == LOCAL_PROVIDER_ID {
                    saved.server.base_url.clone()
                } else {
                    base_url.trim().to_string()
                };
                let changed = saved.server.name != next_name
                    || saved.server.base_url != next_base_url
                    || saved.trust_invalid_cert != trust_invalid_cert;
                if !changed {
                    return Ok(false);
                }
                saved.server.name = next_name;
                saved.server.base_url = next_base_url;
                saved.trust_invalid_cert = trust_invalid_cert;
                store.save_server(&saved)?;
                Ok(true)
            });
            match result {
                Ok(true) => {
                    let _sent = events.send(ControllerEvent::LoginStatus(
                        "Server settings saved.".to_string(),
                    ));
                    emit_snapshot(&store, &events);
                }
                Ok(false) => {}
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }
        });
    }
    pub fn clear_server_local_access(&self, server_id: ServerId) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let queue = Arc::clone(&self.queue);
        let playback = Arc::clone(&self.playback);
        let events = self.events.clone();
        thread::spawn(move || {
            if let Err(error) = store.with_store(|store| {
                store.delete_server_local_access(&server_id)?;
                store.delete_track_local_matches(&server_id)
            }) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            prepare_next_stream_from_handles(
                store.clone(),
                Arc::clone(&runtime),
                Arc::clone(&secrets),
                Arc::clone(&playback),
                Arc::clone(&queue),
                events.clone(),
            );
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
}
