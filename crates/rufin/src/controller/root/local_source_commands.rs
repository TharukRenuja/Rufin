use super::*;
use crate::source_setup::{
    activate_configured_source, local_configured_source, local_configured_source_for_store,
};

impl SourceCommands {
    pub fn add_local_library_folder(&self, root_path: PathBuf) {
        self.add_library_folders(vec![root_path]);
    }
    pub(crate) fn add_library_folders(&self, root_paths: Vec<PathBuf>) {
        let controller = self.clone();
        let transition_generation = self.source_transitions.begin();
        let source_transitions = Arc::clone(&self.source_transitions);
        let store = self.store.clone();
        let source_presentation = self.source_events.presentation.clone();
        let source_transition_failure = self.source_events.transition_failure.clone();
        let playback_projection = self.playback_projection.clone();
        let secrets = Arc::clone(&self.secrets);
        let active_source = Arc::clone(&self.active_source);
        thread::spawn(move || {
            let current = || source_transitions.current(transition_generation);
            let emit_current_error = |error| {
                if current() {
                    let _sent =
                        source_transition_failure.try_send(sources::SourceTransitionFailed {
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
            let transition_commit = match source_transitions.commit(transition_generation) {
                Ok(Some(commit)) => commit,
                Ok(None) => return,
                Err(error) => {
                    emit_current_error(error);
                    return;
                }
            };
            let emit_error = |error| {
                let _sent = source_transition_failure.try_send(sources::SourceTransitionFailed {
                    source_id: Some(saved.source_id.clone()),
                    error,
                });
            };
            let persistence = match SourcePersistenceSnapshot::capture(&store, &saved.source_id) {
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
            controller.forget_source_sync(&saved.source_id);
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
                store.set_active_source(&saved.source_id)
            }) {
                persistence.restore(&store);
                emit_error(error);
                return;
            }
            *active_guard = Some(active);
            drop(active_guard);
            let projection = match activate_playback_source(
                &controller.store,
                &controller.runtime,
                &controller.active_source,
                &controller.secrets,
                &controller.artwork,
                &controller.library_events,
                &controller.playback_projection,
                &controller.playback_product,
                &saved.source_id,
            ) {
                Ok(projection) => projection,
                Err(error) => {
                    if let Ok(mut active) = active_source.write() {
                        *active = previous_active;
                    }
                    persistence.restore(&store);
                    emit_error(error);
                    return;
                }
            };
            let _sent = playback_projection.try_send(projection);
            emit_source_presentation(&store, &source_presentation);
            controller.refresh_source_freshness();
            drop(transition_commit);
        });
    }
    pub fn remove_local_library_folder(&self, path: String) {
        let controller = self.clone();
        let transition_generation = self.source_transitions.begin();
        let source_transitions = Arc::clone(&self.source_transitions);
        let store = self.store.clone();
        let source_presentation = self.source_events.presentation.clone();
        let source_transition_failure = self.source_events.transition_failure.clone();
        let playback_projection = self.playback_projection.clone();
        let secrets = Arc::clone(&self.secrets);
        let active_source = Arc::clone(&self.active_source);
        thread::spawn(move || {
            let current = || source_transitions.current(transition_generation);
            let emit_current_error = |error| {
                if current() {
                    let _sent =
                        source_transition_failure.try_send(sources::SourceTransitionFailed {
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
                let _sent = source_transition_failure.try_send(sources::SourceTransitionFailed {
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
            let persistence = match SourcePersistenceSnapshot::capture(&store, &saved.source_id) {
                Ok(persistence) => persistence,
                Err(error) => {
                    emit_error(error);
                    return;
                }
            };
            controller.forget_source_sync(&saved.source_id);
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
            let previous_active = active_guard.as_ref().map(|active| (*active).clone());
            if let Err(error) = save_source_settings(&store, &sources) {
                emit_error(error);
                return;
            }
            let result = store.with_store(|store| {
                store.save_source(&saved)?;
                if selected_local && !no_local_folders {
                    store.set_active_source(&saved.source_id)?;
                } else if selected_local {
                    store.clear_active_source()?;
                }
                if no_local_folders {
                    store.clear_library_cache(&saved.source_id)?;
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
                clear_playback_product_slot(&controller.playback_product);
            } else if selected_local {
                if let Some(mut active) = active_guard.take() {
                    *active = next_active;
                    drop(active);
                }
                let projection = match activate_playback_source(
                    &controller.store,
                    &controller.runtime,
                    &controller.active_source,
                    &controller.secrets,
                    &controller.artwork,
                    &controller.library_events,
                    &controller.playback_projection,
                    &controller.playback_product,
                    &saved.source_id,
                ) {
                    Ok(projection) => projection,
                    Err(error) => {
                        if let (Some(previous_active), Ok(mut active)) =
                            (previous_active, active_source.write())
                        {
                            *active = previous_active;
                        }
                        persistence.restore(&store);
                        emit_error(error);
                        return;
                    }
                };
                let _sent = playback_projection.try_send(projection);
            }
            if no_local_folders {
                if let Err(error) = crate::controller::artwork::invalidate_source(
                    &controller.artwork,
                    &saved.source_id,
                ) {
                    warn!(%error, source_id = %saved.source_id, "failed to invalidate Local artwork");
                }
                if let Err(error) = clear_store_disk_waveform_cache(&store, &saved.source_id) {
                    warn!(%error, source_id = %saved.source_id, "failed to clear Local waveform cache");
                }
            }
            emit_runtime_source_presentation(&store, &secrets, &source_presentation);
            if selected_local {
                controller.refresh_source_freshness();
            } else if !no_local_folders {
                controller.request_inactive_source_sync(saved.source_id);
            }
            drop(transition_commit);
        });
    }
}
