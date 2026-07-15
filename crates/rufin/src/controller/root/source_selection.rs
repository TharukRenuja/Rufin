use super::*;
use crate::source_setup::{
    activate_configured_source, configured_source_needs_auth, configured_source_ready_for_sync,
    configured_source_supported, local_configured_source_for_store,
};

impl SourceCommands {
    pub fn select_source(&self, source: LibrarySourceSelection) {
        let controller = self.clone();
        let transition_generation = self.source_transitions.begin();
        let source_transitions = Arc::clone(&self.source_transitions);
        let store = self.store.clone();
        let source_presentation = self.source_events.presentation.clone();
        let source_selection = self.source_events.selection.clone();
        let source_transition_failure = self.source_events.transition_failure.clone();
        let playback_projection = self.playback_projection.clone();
        let secrets = Arc::clone(&self.secrets);
        let active_source = Arc::clone(&self.active_source);
        thread::spawn(move || {
            let target_source_id = selection_source_id(&source);
            let transition_commit = match source_transitions.commit(transition_generation) {
                Ok(Some(commit)) => commit,
                Ok(None) => return,
                Err(error) => {
                    emit_current_source_selection_error(
                        &source_transition_failure,
                        &source_transitions,
                        transition_generation,
                        target_source_id.clone(),
                        error,
                    );
                    return;
                }
            };
            let _sent = source_selection.try_send(sources::SourceSelectionChanged {
                selected_source: source.clone(),
            });
            let emit_error = |error| {
                emit_runtime_source_presentation(&store, &secrets, &source_presentation);
                let _sent = source_transition_failure.try_send(sources::SourceTransitionFailed {
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
            if !configured_source_supported(&saved.kind) {
                emit_error("Saved source type is no longer supported.".to_string());
                return;
            }
            let needs_auth = match configured_source_needs_auth(&secrets, &saved) {
                Ok(needs_auth) => needs_auth,
                Err(error) => {
                    emit_error(error);
                    return;
                }
            };

            let candidate = if needs_auth {
                None
            } else {
                let candidate = match activate_configured_source(&store, &secrets, &saved) {
                    Ok(candidate) => candidate,
                    Err(error) => {
                        emit_error(error);
                        return;
                    }
                };
                Some(candidate)
            };
            let persistence = match SourcePersistenceSnapshot::capture(&store, &saved.source_id) {
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
                clear_playback_product_slot(&controller.playback_product);
            } else {
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
            }
            if needs_auth {
                emit_runtime_source_presentation(&store, &secrets, &source_presentation);
                controller.refresh_source_freshness();
                drop(transition_commit);
                return;
            }
            emit_runtime_source_presentation(&store, &secrets, &source_presentation);
            if configured_source_ready_for_sync(&store, &saved) {
                controller.refresh_source_freshness();
            }
            drop(transition_commit);
        });
    }
}

fn configured_source_for_selection(
    store: &StoreHandle,
    selection: &LibrarySourceSelection,
) -> Result<StoredSource, String> {
    match selection {
        LibrarySourceSelection::Local => local_configured_source_for_store(store),
        LibrarySourceSelection::Source(source_id) => store
            .with_store(|store| store.stored_source(source_id))?
            .ok_or_else(|| "The selected source is no longer saved.".to_string()),
    }
}

fn commit_source_selection(
    store: &StoreHandle,
    saved: &StoredSource,
    selection: &LibrarySourceSelection,
    previous_sources: &LibrarySourceSettings,
) -> Result<(), String> {
    let mut sources = previous_sources.clone();
    sources.selected = Some(selection.clone());
    save_source_settings(store, &sources)?;
    store.with_store(|store| {
        store.save_source(saved)?;
        store.set_active_source(&saved.source_id)
    })
}

fn emit_current_source_selection_error(
    source_transition_failure: &Sender<sources::SourceTransitionFailed>,
    source_transitions: &SourceTransitions,
    transition_generation: u64,
    source_id: SourceId,
    error: String,
) {
    if source_transitions.current(transition_generation) {
        let _sent = source_transition_failure.try_send(sources::SourceTransitionFailed {
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
