use super::*;
use crate::sources::{AuthenticatedSource, source_identity_changed};

impl AppController {
    pub fn forget_source(&self, source_id: SourceId) {
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
                    let _sent = events.send(ControllerEvent::Error(error));
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
                let _sent = events.send(ControllerEvent::Error(error));
            };
            let saved = match store.with_store(|store| {
                let active_id = store.active_source()?.map(|saved| saved.source.id);
                let saved = store.saved_source(&source_id)?;
                Ok((saved, active_id))
            }) {
                Ok((Some(saved), active_id)) => (saved, active_id),
                Ok((None, _)) => {
                    emit_error("The selected server is no longer saved.".to_string());
                    return;
                }
                Err(error) => {
                    emit_error(error);
                    return;
                }
            };
            let (saved, active_id) = saved;
            let persistence = match SourcePersistenceSnapshot::capture(&store, &saved.source.id) {
                Ok(persistence) => persistence,
                Err(error) => {
                    emit_error(error);
                    return;
                }
            };
            controller.forget_source_sync(&saved.source.id);
            let mut active_guard = if active_id.as_ref() == Some(&saved.source.id) {
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
            let mut sources = persistence.previous_sources.clone();
            if sources.selected == Some(LibrarySourceSelection::Source(saved.source.id.clone())) {
                sources.selected = None;
                if let Err(error) = save_source_settings(&store, &sources) {
                    emit_error(error);
                    return;
                }
            }
            if let Err(error) = store.with_store(|store| store.forget_source(&saved.source.id)) {
                persistence.restore(&store);
                emit_error(error);
                return;
            }
            if let Some(mut active) = active_guard.take() {
                *active = None;
                drop(active);
                clear_queue_and_stop_playback(
                    &queue,
                    &playback_request_generation,
                    &next_preload,
                    &playback,
                    &playback_snapshot,
                    &auto_dj_enabled,
                    &events,
                );
            }
            if let Err(error) = secrets.delete_token(&saved.source.id) {
                warn!(%error, source_id = %saved.source.id, "failed to delete forgotten server token");
            }
            if let Err(error) = clear_store_disk_cover_cache(&store, &saved.source.id) {
                warn!(%error, source_id = %saved.source.id, "failed to clear forgotten source cover cache");
            }
            emit_runtime_snapshot(&store, &secrets, &events);
            controller.refresh_source_freshness();
            drop(transition_commit);
        });
    }

    pub(crate) fn configure_authenticated_source<Authenticate>(
        &self,
        source_name: &'static str,
        authenticate: Authenticate,
    ) where
        Authenticate:
            FnOnce(&Runtime, &StoreHandle) -> Result<AuthenticatedSource, String> + Send + 'static,
    {
        let controller = self.clone();
        let transition_generation = self.source_transitions.begin();
        let source_transitions = Arc::clone(&self.source_transitions);
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
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
                        source_id: None,
                        error,
                    });
                }
            };
            let _sent = events.send(ControllerEvent::SourceNotice(SourceNotice::Checking {
                source_name: source_name.to_string(),
            }));
            let authenticated = match authenticate(&runtime, &store) {
                Ok(authenticated) => authenticated,
                Err(error) => {
                    emit_current_error(error);
                    return;
                }
            };
            let AuthenticatedSource {
                saved,
                credential,
                active,
                authenticated_source_id,
            } = authenticated;
            let context = QueueActivationContext {
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
            controller.forget_source_sync(&saved.source.id);
            let persistence = match SourcePersistenceSnapshot::capture(&store, &saved.source.id) {
                Ok(persistence) => persistence,
                Err(error) => {
                    emit_error(error);
                    return;
                }
            };
            let identity_changed = persistence.previous_saved.as_ref().is_some_and(|previous| {
                source_identity_changed(previous, &saved, &authenticated_source_id)
            });
            let prepared_queue = if identity_changed {
                None
            } else {
                match prepare_saved_queue_activation(&context, &saved) {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        emit_error(error);
                        return;
                    }
                }
            };
            let prepared_queue_reset =
                identity_changed.then(|| prepare_active_source_queue_reset(&context, &saved));
            let mut active_guard = match active_source.write() {
                Ok(active) => active,
                Err(_) => {
                    emit_error("active source lock was poisoned".to_string());
                    return;
                }
            };
            let queue_reset = if let Some(reset) = prepared_queue_reset {
                let queue = match queue.lock() {
                    Ok(queue) => queue,
                    Err(_) => {
                        emit_error("queue lock was poisoned".to_string());
                        return;
                    }
                };
                Some((queue, reset))
            } else {
                None
            };
            let previous_active = active_guard.clone();
            let committed = commit_authenticated_source(
                &store,
                &secrets,
                persistence,
                &saved,
                &credential,
                identity_changed,
            );
            let rollback = match committed {
                Ok(rollback) => rollback,
                Err(error) => {
                    emit_error(error);
                    return;
                }
            };
            *active_guard = Some(active);
            drop(active_guard);
            let queue_result = if let Some((queue, reset)) = queue_reset {
                apply_active_source_queue_reset(&context, queue, reset);
                Ok(())
            } else if let Some(prepared_queue) = prepared_queue {
                apply_prepared_queue_activation(&context, prepared_queue)
            } else {
                Ok(())
            };
            if let Err(error) = queue_result {
                if let Ok(mut active) = active_source.write() {
                    *active = previous_active;
                }
                rollback_authenticated_source(&store, &secrets, rollback);
                emit_error(error);
                return;
            }
            if identity_changed
                && let Err(error) = clear_store_disk_cover_cache(&store, &saved.source.id)
            {
                warn!(%error, source_id = %saved.source.id, "failed to clear replaced source cover cache");
            }
            let _sent = events.send(ControllerEvent::SourceNotice(SourceNotice::Connected));
            emit_runtime_snapshot(&store, &secrets, &events);
            controller.refresh_source_freshness();
            drop(transition_commit);
        });
    }
}

/// Keep only the source settings needed to undo a failed source change
pub(in crate::controller) struct SourcePersistenceSnapshot {
    source_id: SourceId,
    previous_saved: Option<SavedSource>,
    pub(in crate::controller) previous_active_id: Option<SourceId>,
    pub(in crate::controller) previous_sources: domain::LibrarySourceSettings,
}

impl SourcePersistenceSnapshot {
    pub(in crate::controller) fn capture(
        store: &StoreHandle,
        source_id: &SourceId,
    ) -> Result<Self, String> {
        Ok(Self {
            source_id: source_id.clone(),
            previous_saved: store.with_store(|store| store.saved_source(source_id))?,
            previous_active_id: store
                .with_store(|store| Ok(store.active_source()?.map(|saved| saved.source.id)))?,
            previous_sources: store.load_settings().sources,
        })
    }

    fn restore_source(&self, store: &StoreHandle) {
        let result = match &self.previous_saved {
            Some(saved) => store.with_store(|store| store.save_source(saved)),
            None => store.with_store(|store| store.forget_source(&self.source_id)),
        };
        if let Err(error) = result {
            warn!(%error, source_id = %self.source_id, "failed to restore configured source");
        }
    }

    pub(in crate::controller) fn restore(&self, store: &StoreHandle) {
        if let Err(error) = save_source_settings(store, &self.previous_sources) {
            warn!(%error, source_id = %self.source_id, "failed to restore source settings");
        }
        self.restore_source(store);
        let result = match &self.previous_active_id {
            Some(source_id) => store.with_store(|store| store.set_active_source(source_id)),
            None => store.with_store(library::Store::clear_active_source),
        };
        if let Err(error) = result {
            warn!(%error, source_id = %self.source_id, "failed to restore active source");
        }
    }
}

pub(in crate::controller) fn save_source_settings(
    store: &StoreHandle,
    sources: &domain::LibrarySourceSettings,
) -> Result<(), String> {
    store.update_settings(|settings| {
        settings.sources = sources.clone();
        settings.migrate_defaults();
        Ok(())
    })
}

struct AuthenticatedSourceRollback {
    persistence: SourcePersistenceSnapshot,
    previous_token: Option<String>,
}

fn commit_authenticated_source(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    persistence: SourcePersistenceSnapshot,
    saved: &SavedSource,
    credential: &str,
    identity_changed: bool,
) -> Result<AuthenticatedSourceRollback, String> {
    let previous_token = secrets
        .load_token(&saved.source.id)
        .map_err(|error| error.to_string())?;
    secrets
        .save_token(&saved.source.id, credential)
        .map_err(|error| error.to_string())?;

    let rollback = AuthenticatedSourceRollback {
        persistence,
        previous_token,
    };
    let mut sources = rollback.persistence.previous_sources.clone();
    sources.selected = Some(LibrarySourceSelection::Source(saved.source.id.clone()));
    if let Err(error) = save_source_settings(store, &sources) {
        rollback.persistence.restore_source(store);
        restore_authenticated_source_token(secrets, &rollback);
        return Err(error);
    }
    if let Err(error) =
        store.with_store(|store| store.save_source_activation(saved, identity_changed))
    {
        rollback_authenticated_source(store, secrets, rollback);
        return Err(error);
    }
    Ok(rollback)
}

fn rollback_authenticated_source(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    rollback: AuthenticatedSourceRollback,
) {
    rollback.persistence.restore(store);
    restore_authenticated_source_token(secrets, &rollback);
}

fn restore_authenticated_source_token(
    secrets: &Arc<dyn SecretStore>,
    rollback: &AuthenticatedSourceRollback,
) {
    let result = match &rollback.previous_token {
        Some(token) => secrets.save_token(&rollback.persistence.source_id, token),
        None => secrets.delete_token(&rollback.persistence.source_id),
    };
    if let Err(error) = result {
        warn!(%error, source_id = %rollback.persistence.source_id, "failed to restore source credential");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_previous_user_state(store: &StoreHandle, previous: &SavedSource) {
        let album = library_album(1, "Previous User", "Private Album", None);
        let track = library_track(
            1,
            album.artist_id.clone(),
            album.id.clone(),
            &album.artist,
            &[],
        );
        store
            .with_store(|store| {
                store.save_source(previous)?;
                store.set_active_source(&previous.source.id)?;
                let generation = store.begin_sync(&previous.source.id)?;
                commit_cached_library(
                    store,
                    &previous.source.id,
                    generation,
                    CachedLibraryObservation {
                        albums: vec![album.clone()],
                        ..CachedLibraryObservation::default()
                    },
                )?;
                let mut queue = QueueEngine::new(previous.source.id.clone());
                queue.play_now(&track);
                store.save_queue_snapshot(&queue.snapshot())
            })
            .expect("seed previous user state");
        let mut sources = store.load_settings().sources;
        sources.selected = Some(LibrarySourceSelection::Source(previous.source.id.clone()));
        save_source_settings(store, &sources).expect("select previous source");
    }

    fn other_user(previous: &SavedSource) -> SavedSource {
        let mut next = previous.clone();
        next.user_id = "other-user-id".to_string();
        next.username = "other-user".to_string();
        next
    }

    #[test]
    fn reconnecting_as_another_user_clears_source_state() {
        let store = StoreHandle::open_memory().expect("memory store");
        let previous = saved_source();
        seed_previous_user_state(&store, &previous);
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        secrets
            .save_token(&previous.source.id, "previous-token")
            .expect("save previous token");

        let next = other_user(&previous);
        let persistence =
            SourcePersistenceSnapshot::capture(&store, &next.source.id).expect("capture source");
        let identity_changed = source_identity_changed(&previous, &next, &next.source.id);
        assert!(identity_changed);

        let _rollback = commit_authenticated_source(
            &store,
            &secrets,
            persistence,
            &next,
            "other-token",
            identity_changed,
        )
        .expect("commit reconnect");

        assert_eq!(
            store
                .with_store(|store| store.saved_source(&next.source.id))
                .expect("load source"),
            Some(next.clone())
        );
        assert_eq!(
            store
                .with_store(|store| store.load_albums(&next.source.id, 0, 1))
                .expect("load albums")
                .total,
            0
        );
        assert!(
            store
                .with_store(|store| store.load_queue_snapshot(&next.source.id))
                .expect("load queue")
                .is_none()
        );
        assert_eq!(
            secrets.load_token(&next.source.id).expect("load token"),
            Some("other-token".to_string())
        );
    }

    #[test]
    fn failed_identity_reconnect_keeps_previous_source_state() {
        let (store, root) = disk_store_for_test("failed-identity-reconnect");
        let previous = saved_source();
        seed_previous_user_state(&store, &previous);
        rusqlite::Connection::open(disk_store_database_path(&store))
            .expect("open store database")
            .execute_batch(
                "
                CREATE TRIGGER reject_queue_clear
                BEFORE DELETE ON queue_snapshots
                BEGIN
                    SELECT RAISE(ABORT, 'reject queue clear');
                END;
                ",
            )
            .expect("install failing cache trigger");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        secrets
            .save_token(&previous.source.id, "previous-token")
            .expect("save previous token");
        let next = other_user(&previous);
        let persistence =
            SourcePersistenceSnapshot::capture(&store, &next.source.id).expect("capture source");

        let error = match commit_authenticated_source(
            &store,
            &secrets,
            persistence,
            &next,
            "other-token",
            true,
        ) {
            Ok(_) => panic!("reconnect commit should fail"),
            Err(error) => error,
        };

        assert!(error.contains("reject queue clear"));
        assert_eq!(
            store
                .with_store(|store| store.saved_source(&previous.source.id))
                .expect("load source"),
            Some(previous.clone())
        );
        assert_eq!(
            store
                .with_store(|store| store.load_albums(&previous.source.id, 0, 1))
                .expect("load albums")
                .total,
            1
        );
        assert!(
            store
                .with_store(|store| store.load_queue_snapshot(&previous.source.id))
                .expect("load queue")
                .is_some()
        );
        assert_eq!(
            store
                .with_store(|store| store.active_source())
                .expect("load active source"),
            Some(previous.clone())
        );
        assert_eq!(
            store.load_settings().sources.selected,
            Some(LibrarySourceSelection::Source(previous.source.id.clone()))
        );
        assert_eq!(
            secrets.load_token(&previous.source.id).expect("load token"),
            Some("previous-token".to_string())
        );
        let _cleanup = fs::remove_dir_all(root);
    }
}
