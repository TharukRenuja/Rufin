use super::*;
use crate::source_setup::{AuthenticatedSource, source_identity_changed};

impl AppController {
    pub fn forget_source(&self, source_id: SourceId) {
        let controller = self.clone();
        let transition_generation = self.source_transitions.begin();
        let source_transitions = Arc::clone(&self.source_transitions);
        let store = self.store.clone();
        let events = self.events.clone();
        let secrets = Arc::clone(&self.secrets);
        let active_source = Arc::clone(&self.active_source);
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
                let active_id = store.active_source()?.map(|saved| saved.source_id);
                let saved = store.stored_source(&source_id)?;
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
            let persistence = match SourcePersistenceSnapshot::capture(&store, &saved.source_id) {
                Ok(persistence) => persistence,
                Err(error) => {
                    emit_error(error);
                    return;
                }
            };
            controller.forget_source_sync(&saved.source_id);
            let mut active_guard = if active_id.as_ref() == Some(&saved.source_id) {
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
            if sources.selected == Some(LibrarySourceSelection::Source(saved.source_id.clone())) {
                sources.selected = None;
                if let Err(error) = save_source_settings(&store, &sources) {
                    emit_error(error);
                    return;
                }
            }
            if let Err(error) = store.with_store(|store| store.forget_source(&saved.source_id)) {
                persistence.restore(&store);
                emit_error(error);
                return;
            }
            if let Some(mut active) = active_guard.take() {
                *active = None;
                drop(active);
                controller.clear_playback_product();
            }
            if let Err(error) = secrets.delete_token(saved.source_id.as_str()) {
                warn!(%error, source_id = %saved.source_id, "failed to delete forgotten server token");
            }
            if let Err(error) = controller.invalidate_artwork_source(&saved.source_id) {
                warn!(%error, source_id = %saved.source_id, "failed to invalidate forgotten source artwork");
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
                    source_id: Some(saved.source_id.clone()),
                    error,
                });
            };
            controller.forget_source_sync(&saved.source_id);
            let persistence = match SourcePersistenceSnapshot::capture(&store, &saved.source_id) {
                Ok(persistence) => persistence,
                Err(error) => {
                    emit_error(error);
                    return;
                }
            };
            let identity_changed = persistence.previous_saved.as_ref().is_some_and(|previous| {
                source_identity_changed(previous, &saved, &authenticated_source_id)
            });
            let mut active_guard = match active_source.write() {
                Ok(active) => active,
                Err(_) => {
                    emit_error("active source lock was poisoned".to_string());
                    return;
                }
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
            let projection = match controller.activate_playback_source(&saved.source_id) {
                Ok(projection) => projection,
                Err(error) => {
                    if let Ok(mut active) = active_source.write() {
                        *active = previous_active;
                    }
                    rollback_authenticated_source(&store, &secrets, rollback);
                    emit_error(error);
                    return;
                }
            };
            let _sent = events.send(ControllerEvent::PlaybackProduct(Box::new(projection)));
            if identity_changed
                && let Err(error) = controller.invalidate_artwork_source(&saved.source_id)
            {
                warn!(%error, source_id = %saved.source_id, "failed to invalidate replaced source artwork");
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
    previous_saved: Option<StoredSource>,
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
            previous_saved: store.with_store(|store| store.stored_source(source_id))?,
            previous_active_id: store
                .with_store(|store| Ok(store.active_source()?.map(|saved| saved.source_id)))?,
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
    saved: &StoredSource,
    credential: &str,
    identity_changed: bool,
) -> Result<AuthenticatedSourceRollback, String> {
    let previous_token = secrets
        .load_token(saved.source_id.as_str())
        .map_err(|error| error.to_string())?;
    secrets
        .save_token(saved.source_id.as_str(), credential)
        .map_err(|error| error.to_string())?;

    let rollback = AuthenticatedSourceRollback {
        persistence,
        previous_token,
    };
    let mut sources = rollback.persistence.previous_sources.clone();
    sources.selected = Some(LibrarySourceSelection::Source(saved.source_id.clone()));
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
        Some(token) => secrets.save_token(rollback.persistence.source_id.as_str(), token),
        None => secrets.delete_token(rollback.persistence.source_id.as_str()),
    };
    if let Err(error) = result {
        warn!(%error, source_id = %rollback.persistence.source_id, "failed to restore source credential");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_previous_user_state(store: &StoreHandle, previous: &StoredSource) {
        let album = library_album(1, "Previous User", "Private Album", None);
        let track = library_track(
            1,
            album.artist_id.clone(),
            album.id.clone(),
            &album.artist,
            &[],
        );
        let mut sequence = playback::Sequence::new(previous.source_id.clone());
        sequence
            .apply_batch(
                playback::Batch::new(vec![playback::BatchItem::new(
                    track,
                    playback::Provenance::Manual,
                )]),
                playback::Placement::Replace { anchor_index: 0 },
            )
            .expect("seed playback sequence");
        let checkpoint = playback::encode_checkpoint(&sequence).expect("encode checkpoint");
        let checkpoint = library::PlaybackCheckpointRecord {
            source_id: checkpoint.header.source_id,
            revision: checkpoint.header.revision,
            selected_occurrence_id: checkpoint
                .header
                .selected_occurrence
                .map(|occurrence| occurrence.to_string()),
            progress_millis: checkpoint.header.progress_millis,
            repeat_mode: "Off".to_string(),
            shuffle_enabled: checkpoint.header.shuffle_enabled,
            payload: checkpoint.payload,
        };
        store
            .with_store(|store| {
                store.save_source(previous)?;
                store.set_active_source(&previous.source_id)?;
                let generation = store.begin_sync(&previous.source_id)?;
                commit_cached_library(
                    store,
                    &previous.source_id,
                    generation,
                    CachedLibraryObservation {
                        albums: vec![album.clone()],
                        ..CachedLibraryObservation::default()
                    },
                )?;
                store.save_playback_checkpoint(&checkpoint)
            })
            .expect("seed previous user state");
        let mut sources = store.load_settings().sources;
        sources.selected = Some(LibrarySourceSelection::Source(previous.source_id.clone()));
        save_source_settings(store, &sources).expect("select previous source");
    }

    fn other_user(previous: &StoredSource) -> StoredSource {
        let mut config = sources::jellyfin::JellyfinSourceConfig::from_stored(previous)
            .expect("Jellyfin source config");
        config.credentials.user_id = "other-user-id".to_string();
        config.credentials.username = "other-user".to_string();
        config.into_stored()
    }

    #[test]
    fn reconnecting_as_another_user_clears_source_state() {
        let store = StoreHandle::open_memory().expect("memory store");
        let previous = saved_source();
        seed_previous_user_state(&store, &previous);
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        secrets
            .save_token(previous.source_id.as_str(), "previous-token")
            .expect("save previous token");

        let next = other_user(&previous);
        let persistence =
            SourcePersistenceSnapshot::capture(&store, &next.source_id).expect("capture source");
        let identity_changed = source_identity_changed(&previous, &next, &next.source_id);
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
                .with_store(|store| store.stored_source(&next.source_id))
                .expect("load source"),
            Some(next.clone())
        );
        assert_eq!(
            store
                .with_store(|store| store.load_albums(&next.source_id, 0, 1))
                .expect("load albums")
                .total,
            0
        );
        assert!(
            store
                .with_store(|store| store.load_playback_checkpoint(&next.source_id))
                .expect("load playback checkpoint")
                .is_none()
        );
        assert_eq!(
            secrets
                .load_token(next.source_id.as_str())
                .expect("load token"),
            Some("other-token".to_string())
        );
    }
}
