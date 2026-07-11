use super::*;
use crate::sources::{
    activate_configured_source, configured_source_needs_auth, local_configured_source_for_store,
    resolve_source_registration,
};

impl AppController {
    pub fn select_source(&self, source: LibrarySourceSelection) {
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
            let target_source_id = selection_source_id(&source);
            let transition_commit = match source_transitions.commit(transition_generation) {
                Ok(Some(commit)) => commit,
                Ok(None) => return,
                Err(error) => {
                    emit_current_source_selection_error(
                        &events,
                        &source_transitions,
                        transition_generation,
                        target_source_id.clone(),
                        error,
                    );
                    return;
                }
            };
            let _sent = events.send(ControllerEvent::SourceSelectionChanged {
                selected_source: source.clone(),
            });
            let emit_error = |error| {
                emit_runtime_snapshot(&store, &secrets, &events);
                let _sent = events.send(ControllerEvent::SourceTransitionFailed {
                    source_id: Some(target_source_id.clone()),
                    error,
                });
            };
            let saved = match configured_source_for_selection(&store, &source) {
                Ok(saved) => saved,
                Err(error) => {
                    emit_error(error);
                    return;
                }
            };
            let Some(registration) = resolve_source_registration(&saved.source.kind) else {
                emit_error("Saved source type is no longer supported.".to_string());
                return;
            };
            let needs_auth = match configured_source_needs_auth(&secrets, &saved) {
                Ok(needs_auth) => needs_auth,
                Err(error) => {
                    emit_error(error);
                    return;
                }
            };

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
            let (candidate, prepared_queue) = if needs_auth {
                (None, None)
            } else {
                let candidate = match activate_configured_source(&store, &secrets, &saved) {
                    Ok(candidate) => candidate,
                    Err(error) => {
                        emit_error(error);
                        return;
                    }
                };
                let prepared_queue =
                    match prepare_saved_queue_activation(&queue_activation_context, &saved) {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            emit_error(error);
                            return;
                        }
                    };
                (Some(candidate), prepared_queue)
            };
            let persistence = match SourcePersistenceSnapshot::capture(&store, &saved.source.id) {
                Ok(persistence) => persistence,
                Err(error) => {
                    emit_error(error);
                    return;
                }
            };
            let mut active_guard = match active_source.write() {
                Ok(active) => active,
                Err(_) => {
                    emit_error("active source lock was poisoned".to_string());
                    return;
                }
            };
            let previous_active = active_guard.clone();
            if let Err(error) =
                commit_source_selection(&store, &saved, &source, &persistence.previous_sources)
            {
                persistence.restore(&store);
                emit_error(error);
                return;
            }
            *active_guard = candidate;
            drop(active_guard);
            if needs_auth {
                clear_queue_and_stop_playback(
                    &queue,
                    &playback_request_generation,
                    &next_preload,
                    &playback,
                    &playback_snapshot,
                    &auto_dj_enabled,
                    &events,
                );
            } else if let Some(prepared_queue) = prepared_queue
                && let Err(error) =
                    apply_prepared_queue_activation(&queue_activation_context, prepared_queue)
            {
                if let Ok(mut active) = active_source.write() {
                    *active = previous_active;
                }
                persistence.restore(&store);
                emit_error(error);
                return;
            }
            if needs_auth {
                emit_runtime_snapshot(&store, &secrets, &events);
                controller.refresh_source_freshness();
                drop(transition_commit);
                return;
            }
            emit_runtime_snapshot(&store, &secrets, &events);
            if (registration.configured_for_sync)(&store, &saved) {
                controller.refresh_source_freshness();
            }
            drop(transition_commit);
        });
    }
}

fn configured_source_for_selection(
    store: &StoreHandle,
    selection: &LibrarySourceSelection,
) -> Result<SavedSource, String> {
    match selection {
        LibrarySourceSelection::Local => local_configured_source_for_store(store),
        LibrarySourceSelection::Source(source_id) => store
            .with_store(|store| store.saved_source(source_id))?
            .ok_or_else(|| "The selected source is no longer saved.".to_string()),
    }
}

fn commit_source_selection(
    store: &StoreHandle,
    saved: &SavedSource,
    selection: &LibrarySourceSelection,
    previous_sources: &domain::LibrarySourceSettings,
) -> Result<(), String> {
    let mut sources = previous_sources.clone();
    sources.selected = Some(selection.clone());
    save_source_settings(store, &sources)?;
    store.with_store(|store| {
        store.save_source(saved)?;
        store.set_active_source(&saved.source.id)
    })
}

fn emit_current_source_selection_error(
    events: &Sender<ControllerEvent>,
    source_transitions: &SourceTransitions,
    transition_generation: u64,
    source_id: SourceId,
    error: String,
) {
    if source_transitions.current(transition_generation) {
        let _sent = events.send(ControllerEvent::SourceTransitionFailed {
            source_id: Some(source_id),
            error,
        });
    }
}

fn selection_source_id(selection: &LibrarySourceSelection) -> SourceId {
    match selection {
        LibrarySourceSelection::Local => SourceId::new(LOCAL_SOURCE_IDENTITY_ID),
        LibrarySourceSelection::Source(source_id) => source_id.clone(),
    }
}
