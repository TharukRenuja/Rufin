use super::*;
use crate::sources::{
    activate_configured_source, local_configured_source, local_configured_source_for_store,
};

impl AppController {
    pub fn add_local_library_folder(&self, root_path: PathBuf) {
        self.add_library_folders(vec![root_path]);
    }
    pub(crate) fn add_library_folders(&self, root_paths: Vec<PathBuf>) {
        let controller = self.clone();
        let transition_generation = self.source_transitions.begin();
        let source_transitions = Arc::clone(&self.source_transitions);
        let store = self.store.clone();
        let events = self.events.clone();
        let secrets = Arc::clone(&self.secrets);
        let active_source = Arc::clone(&self.active_source);
        let queue = Arc::clone(&self.queue);
        let playback_request_generation = Arc::clone(&self.playback_request_generation);
        let next_preload = Arc::clone(&self.next_preload);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        thread::spawn(move || {
            let current = || source_transitions.current(transition_generation);
            let emit_current_error = |error| {
                if current() {
                    let _sent = events.send(ControllerEvent::SourceTransitionFailed {
                        source_id: Some(SourceId::new(LOCAL_SOURCE_IDENTITY_ID)),
                        error,
                    });
                }
            };
            if root_paths.is_empty() {
                emit_current_error("Choose at least one local music folder.".to_string());
                return;
            }
            let mut local_paths = Vec::new();
            for root_path in root_paths {
                match LocalSource::identity_for_root(&root_path) {
                    Ok(identity) => {
                        if !local_paths.iter().any(|path| path == &identity.base_url) {
                            local_paths.push(identity.base_url);
                        }
                    }
                    Err(error) => {
                        emit_current_error(error.to_string());
                        return;
                    }
                }
            }
            let saved = local_configured_source();
            let active = match activate_configured_source(&store, &secrets, &saved) {
                Ok(active) => active,
                Err(error) => {
                    emit_current_error(error);
                    return;
                }
            };
            let queue_context = QueueActivationContext {
                store: &store,
                queue: &queue,
                playback_request_generation: &playback_request_generation,
                next_preload: &next_preload,
                playback: &playback,
                playback_snapshot: &playback_snapshot,
                auto_dj_enabled: &auto_dj_enabled,
                events: &events,
            };
            let transition_commit = match source_transitions.commit(transition_generation) {
                Ok(Some(commit)) => commit,
                Ok(None) => return,
                Err(error) => {
                    emit_current_error(error);
                    return;
                }
            };
            let emit_error = |error| {
                let _sent = events.send(ControllerEvent::SourceTransitionFailed {
                    source_id: Some(saved.source.id.clone()),
                    error,
                });
            };
            let persistence = match SourcePersistenceSnapshot::capture(&store, &saved.source.id) {
                Ok(persistence) => persistence,
                Err(error) => {
                    emit_error(error);
                    return;
                }
            };
            let mut sources = persistence.previous_sources.clone();
            for path in local_paths {
                if !sources
                    .local_folders
                    .iter()
                    .any(|folder| folder.path == path)
                {
                    sources.local_folders.push(LocalLibraryFolder { path });
                }
            }
            sources.selected = Some(LibrarySourceSelection::Local);
            let prepared_queue = match prepare_saved_queue_activation(&queue_context, &saved) {
                Ok(prepared) => prepared,
                Err(error) => {
                    emit_error(error);
                    return;
                }
            };
            controller.forget_source_sync(&saved.source.id);
            let mut active_guard = match active_source.write() {
                Ok(active) => active,
                Err(_) => {
                    emit_error("active source lock was poisoned".to_string());
                    return;
                }
            };
            let previous_active = active_guard.clone();
            if let Err(error) = save_source_settings(&store, &sources) {
                emit_error(error);
                return;
            }
            if let Err(error) = store.with_store(|store| {
                store.save_source(&saved)?;
                store.set_active_source(&saved.source.id)
            }) {
                persistence.restore(&store);
                emit_error(error);
                return;
            }
            *active_guard = Some(active);
            drop(active_guard);
            if let Some(prepared_queue) = prepared_queue
                && let Err(error) = apply_prepared_queue_activation(&queue_context, prepared_queue)
            {
                if let Ok(mut active) = active_source.write() {
                    *active = previous_active;
                }
                persistence.restore(&store);
                emit_error(error);
                return;
            }
            emit_snapshot(&store, &events);
            controller.refresh_source_freshness();
            drop(transition_commit);
        });
    }
    pub fn remove_local_library_folder(&self, path: String) {
        let controller = self.clone();
        let transition_generation = self.source_transitions.begin();
        let source_transitions = Arc::clone(&self.source_transitions);
        let store = self.store.clone();
        let events = self.events.clone();
        let secrets = Arc::clone(&self.secrets);
        let active_source = Arc::clone(&self.active_source);
        let queue = Arc::clone(&self.queue);
        let playback_request_generation = Arc::clone(&self.playback_request_generation);
        let next_preload = Arc::clone(&self.next_preload);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        thread::spawn(move || {
            let current = || source_transitions.current(transition_generation);
            let emit_current_error = |error| {
                if current() {
                    let _sent = events.send(ControllerEvent::SourceTransitionFailed {
                        source_id: Some(SourceId::new(LOCAL_SOURCE_IDENTITY_ID)),
                        error,
                    });
                }
            };
            let transition_commit = match source_transitions.commit(transition_generation) {
                Ok(Some(commit)) => commit,
                Ok(None) => return,
                Err(error) => {
                    emit_current_error(error);
                    return;
                }
            };
            let emit_error = |error| {
                let _sent = events.send(ControllerEvent::SourceTransitionFailed {
                    source_id: Some(SourceId::new(LOCAL_SOURCE_IDENTITY_ID)),
                    error,
                });
            };
            let mut sources = store.load_settings().sources;
            let before = sources.local_folders.len();
            sources.local_folders.retain(|folder| folder.path != path);
            if sources.local_folders.len() == before {
                return;
            }
            let saved = match local_configured_source_for_store(&store) {
                Ok(saved) => saved,
                Err(error) => {
                    emit_error(error);
                    return;
                }
            };
            let persistence = match SourcePersistenceSnapshot::capture(&store, &saved.source.id) {
                Ok(persistence) => persistence,
                Err(error) => {
                    emit_error(error);
                    return;
                }
            };
            controller.forget_source_sync(&saved.source.id);
            let selected_local = matches!(sources.selected, Some(LibrarySourceSelection::Local));
            if selected_local && sources.local_folders.is_empty() {
                sources.selected = None;
            }
            let no_local_folders = sources.local_folders.is_empty();
            let next_active = if selected_local && !no_local_folders {
                match activate_configured_source(&store, &secrets, &saved) {
                    Ok(active) => Some(active),
                    Err(error) => {
                        emit_error(error);
                        return;
                    }
                }
            } else {
                None
            };
            let mut active_guard = if selected_local {
                match active_source.write() {
                    Ok(active) => Some(active),
                    Err(_) => {
                        emit_error("active source lock was poisoned".to_string());
                        return;
                    }
                }
            } else {
                None
            };
            if let Err(error) = save_source_settings(&store, &sources) {
                emit_error(error);
                return;
            }
            let result = store.with_store(|store| {
                store.save_source(&saved)?;
                if selected_local && !no_local_folders {
                    store.set_active_source(&saved.source.id)?;
                } else if selected_local {
                    store.clear_active_source()?;
                }
                if no_local_folders {
                    store.clear_library_cache(&saved.source.id)?;
                }
                Ok(())
            });
            if let Err(error) = result {
                persistence.restore(&store);
                emit_error(error);
                return;
            }
            if selected_local && no_local_folders {
                if let Some(mut active) = active_guard.take() {
                    *active = None;
                    drop(active);
                }
                clear_queue_and_stop_playback(
                    &queue,
                    &playback_request_generation,
                    &next_preload,
                    &playback,
                    &playback_snapshot,
                    &auto_dj_enabled,
                    &events,
                );
            } else if selected_local {
                if let Some(mut active) = active_guard.take() {
                    *active = next_active;
                    drop(active);
                }
                let restored = QueueEngine::new(saved.source.id.clone());
                let queue_snapshot = restored.snapshot();
                let auto_dj = auto_dj_enabled
                    .lock()
                    .map(|enabled| *enabled)
                    .unwrap_or_default();
                let player = playback_snapshot_from_queue(
                    Some(&restored),
                    auto_dj,
                    &load_settings_from_store(&store).playback,
                );
                invalidate_playback_requests(&playback_request_generation);
                if let Ok(mut queue) = queue.lock() {
                    *queue = Some(restored);
                }
                stop_playback_backend(&playback, &next_preload, &events);
                if let Ok(mut snapshot) = playback_snapshot.lock() {
                    *snapshot = player.clone();
                }
                let _sent = events.send(ControllerEvent::Queue(Box::new(Some(queue_snapshot))));
                let _sent = events.send(ControllerEvent::Playback(Box::new(player)));
            }
            if no_local_folders {
                if let Err(error) = clear_store_disk_cover_cache(&store, &saved.source.id) {
                    warn!(%error, source_id = %saved.source.id, "failed to clear Local cover cache");
                }
                if let Err(error) = clear_store_disk_waveform_cache(&store, &saved.source.id) {
                    warn!(%error, source_id = %saved.source.id, "failed to clear Local waveform cache");
                }
            }
            emit_runtime_snapshot(&store, &secrets, &events);
            if selected_local {
                controller.refresh_source_freshness();
            } else if !no_local_folders {
                controller.request_inactive_source_sync(saved.source.id);
            }
            drop(transition_commit);
        });
    }
}
