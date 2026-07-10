use super::*;
use crate::sources::{
    activate_configured_source, configured_source_needs_auth, local_configured_source_for_store,
    resolve_source_registration,
};

impl AppController {
    pub fn select_source(&self, source: LibrarySourceSelection) {
        let transition_generation = self.source_transitions.begin();
        let source_transitions = Arc::clone(&self.source_transitions);
        let sync_context = self.sync_context();
        let store = sync_context.store.clone();
        let events = sync_context.events.clone();
        let active_source = Arc::clone(&self.active_source);
        let source_freshness_watcher = Arc::clone(&self.source_freshness_watcher);
        let queue = Arc::clone(&self.queue);
        let playback_request_generation = Arc::clone(&self.playback_request_generation);
        let next_preload = Arc::clone(&self.next_preload);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        thread::spawn(move || {
            let transition_commit = match source_transitions.commit(transition_generation) {
                Ok(Some(commit)) => commit,
                Ok(None) => return,
                Err(error) => {
                    emit_current_source_selection_error(
                        &events,
                        &source_transitions,
                        transition_generation,
                        error,
                    );
                    return;
                }
            };
            let _sent = events.send(ControllerEvent::SourceSelectionChanged {
                selected_source: source.clone(),
            });
            let emit_error = |error| {
                emit_runtime_snapshot(&store, &sync_context.secrets, &events);
                let _sent = events.send(ControllerEvent::Error(error));
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
            let needs_auth = match configured_source_needs_auth(&sync_context.secrets, &saved) {
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
                let candidate =
                    match activate_configured_source(&store, &sync_context.secrets, &saved) {
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
            if let Err(error) = cancel_previous_source_sync(
                &sync_context,
                persistence.previous_active_id.as_ref(),
                &saved,
            ) {
                emit_error(error);
                return;
            }
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
                emit_runtime_snapshot(&store, &sync_context.secrets, &events);
                refresh_source_freshness_watcher(sync_context, source_freshness_watcher);
                drop(transition_commit);
                return;
            }

            let configured_for_sync = (registration.configured_for_sync)(&store, &saved);
            let needs_sync = configured_for_sync
                && selected_active_source(&active_source, &saved.source.id)
                    .is_ok_and(|active| active_source_needs_sync(&store, &active));
            if needs_sync {
                if cached_library_exists(&store, &saved.source.id) {
                    emit_runtime_snapshot(&store, &sync_context.secrets, &events);
                    start_silent_sync_thread(sync_context.clone(), saved.clone());
                } else {
                    start_silent_sync_thread_with_completion_snapshot(
                        sync_context.clone(),
                        saved.clone(),
                    );
                }
            } else {
                emit_runtime_snapshot(&store, &sync_context.secrets, &events);
                if configured_for_sync {
                    start_background_sync_thread(sync_context.clone(), saved);
                }
            }
            refresh_source_freshness_watcher(sync_context, source_freshness_watcher);
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
    error: String,
) {
    if source_transitions.current(transition_generation) {
        let _sent = events.send(ControllerEvent::Error(error));
    }
}

fn cancel_previous_source_sync(
    sync_context: &SyncContext,
    previous_active: Option<&SourceId>,
    selected: &SavedSource,
) -> Result<(), String> {
    let Some(previous_id) = previous_active else {
        return Ok(());
    };
    if previous_id == &selected.source.id {
        return Ok(());
    }
    cancel_sync_if_running(&sync_context.sync_in_flight, previous_id).map(|_| ())
}
