use super::*;
use crate::sources::PreparedSourceSettingsUpdate;
#[cfg(test)]
use crate::sources::{
    AuthenticatedSource, CredentialHostInput, CredentialSettingsInput, JellyfinSettingsInput,
};

impl AppController {
    pub fn save_source_local_access(
        &self,
        source_id: SourceId,
        root_path: PathBuf,
        path_replace_from: Option<String>,
        path_replace_to: Option<String>,
    ) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let active_source = Arc::clone(&self.active_source);
        let queue = Arc::clone(&self.queue);
        let playback = Arc::clone(&self.playback);
        let next_preload = Arc::clone(&self.next_preload);
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
            let matched_source_id = source_id.clone();
            let generation = match store.with_store(|store| {
                store.save_source_local_access(&SourceLocalAccess {
                    source_id,
                    root_path: root_path.clone(),
                    path_replace_from: trimmed_optional(path_replace_from.as_deref()),
                    path_replace_to: Some(path_replace_to),
                })?;
                Ok(store.sync_state(&matched_source_id)?.generation)
            }) {
                Ok(generation) => generation,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            if let Err(error) = runtime.block_on(refresh_local_track_matches(
                &store,
                &matched_source_id,
                generation,
                None,
            )) {
                warn!(%error, "failed to refresh local track matches");
            }
            clear_next_preload(&next_preload);
            prepare_next_stream_from_handles(
                store.clone(),
                Arc::clone(&runtime),
                Arc::clone(&active_source),
                Arc::clone(&playback),
                Arc::clone(&queue),
                Arc::clone(&next_preload),
                events.clone(),
            );
            emit_snapshot(&store, &events);
        });
    }

    pub(crate) fn configured_source(&self, source_id: &SourceId) -> Option<SavedSource> {
        self.store
            .with_store(|store| store.saved_source(source_id))
            .ok()
            .flatten()
    }

    pub(crate) fn update_source_settings<Prepare>(
        &self,
        source_id: SourceId,
        source_name: &'static str,
        prepare: Prepare,
    ) where
        Prepare: FnOnce(
                &Runtime,
                &StoreHandle,
                &Arc<dyn SecretStore>,
                SavedSource,
                &dyn Fn(),
            ) -> Result<Option<PreparedSourceSettingsUpdate>, String>
            + Send
            + 'static,
    {
        let transition_generation = self.source_transitions.begin();
        let source_transitions = Arc::clone(&self.source_transitions);
        let sync_context = self.sync_context();
        let store = sync_context.store.clone();
        let runtime = Arc::clone(&sync_context.runtime);
        let secrets = Arc::clone(&sync_context.secrets);
        let events = sync_context.events.clone();
        let active_source = Arc::clone(&self.active_source);
        let queue = Arc::clone(&self.queue);
        let playback_request_generation = Arc::clone(&self.playback_request_generation);
        let next_preload = Arc::clone(&self.next_preload);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        let source_freshness_watcher = Arc::clone(&self.source_freshness_watcher);
        thread::spawn(move || {
            let current = || source_transitions.current(transition_generation);
            let emit_current_error = |error| {
                if current() {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            };
            let saved = match store.with_store(|store| store.saved_source(&source_id)) {
                Ok(Some(saved)) => saved,
                Ok(None) => {
                    if current() {
                        let _sent = events.send(ControllerEvent::LoginStatus(
                            "No changes to save.".to_string(),
                        ));
                    }
                    return;
                }
                Err(error) => {
                    emit_current_error(error);
                    return;
                }
            };
            let authentication_started = || {
                let _sent = events.send(ControllerEvent::LoginStatus(format!(
                    "Checking {source_name} server..."
                )));
            };
            let prepared = match prepare(&runtime, &store, &secrets, saved, &authentication_started)
            {
                Ok(prepared) => prepared,
                Err(error) => {
                    emit_current_error(error);
                    return;
                }
            };
            let Some(PreparedSourceSettingsUpdate {
                previous,
                saved,
                active,
                identity_changed,
                credential,
            }) = prepared
            else {
                if current() {
                    let _sent = events.send(ControllerEvent::LoginStatus(
                        "No changes to save.".to_string(),
                    ));
                }
                return;
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
            let current_saved = match store.with_store(|store| store.saved_source(&source_id)) {
                Ok(current) => current,
                Err(error) => {
                    emit_error(error);
                    return;
                }
            };
            if current_saved.as_ref() != Some(&previous) {
                emit_error("Source settings changed before this update completed.".to_string());
                return;
            }
            let reauthenticated = credential.is_some();
            let selected = source_is_selected(&store, &saved.source.id);
            if (reauthenticated || selected)
                && let Err(error) = cancel_sync_if_running(&sync_context.sync_in_flight, &source_id)
            {
                emit_error(error);
                return;
            }
            let mut active_guard = if selected {
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
            let queue_reset = if identity_changed && selected {
                let reset = prepare_active_source_queue_reset(&queue_context, &saved);
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
            if let Err(error) = persist_source_settings_update(
                &store,
                &secrets,
                &source_id,
                &saved,
                identity_changed,
                credential.as_deref(),
            ) {
                emit_error(error);
                return;
            }
            if selected && let Some(mut current) = active_guard.take() {
                *current = Some(Arc::clone(&active));
                drop(current);
            }
            if identity_changed {
                if let Err(error) = clear_store_disk_cover_cache(&store, &saved.source.id) {
                    warn!(%error, source_id = %saved.source.id, "failed to clear replaced source cover cache");
                }
                if let Some((queue, reset)) = queue_reset {
                    apply_active_source_queue_reset(&queue_context, queue, reset);
                }
            }
            if selected {
                refresh_source_freshness_watcher(sync_context.clone(), source_freshness_watcher);
            }
            let _sent = events.send(ControllerEvent::LoginStatus(
                "Source settings saved.".to_string(),
            ));
            if selected {
                start_background_sync_thread(sync_context, saved);
            } else if identity_changed || reauthenticated {
                start_sync_thread_with_snapshots(sync_context, saved, SyncPresentation::Silent);
            } else {
                emit_snapshot(&store, &events);
            }
            drop(transition_commit);
        });
    }

    pub fn clear_source_local_access(&self, source_id: SourceId) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let active_source = Arc::clone(&self.active_source);
        let queue = Arc::clone(&self.queue);
        let playback = Arc::clone(&self.playback);
        let next_preload = Arc::clone(&self.next_preload);
        let events = self.events.clone();
        thread::spawn(move || {
            if let Err(error) = store.with_store(|store| {
                store.delete_source_local_access(&source_id)?;
                store.delete_track_local_matches(&source_id)
            }) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            clear_next_preload(&next_preload);
            prepare_next_stream_from_handles(
                store.clone(),
                Arc::clone(&runtime),
                Arc::clone(&active_source),
                Arc::clone(&playback),
                Arc::clone(&queue),
                Arc::clone(&next_preload),
                events.clone(),
            );
            emit_snapshot(&store, &events);
        });
    }
}

fn persist_source_settings_update(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    source_id: &SourceId,
    saved: &SavedSource,
    identity_changed: bool,
    credential: Option<&str>,
) -> Result<(), String> {
    let previous_token = credential
        .map(|credential| {
            let previous = secrets
                .load_token(source_id)
                .map_err(|error| error.to_string())?;
            secrets
                .save_token(source_id, credential)
                .map_err(|error| error.to_string())?;
            Ok::<_, String>(previous)
        })
        .transpose()?;
    if let Err(error) =
        store.with_store(|store| store.save_source_settings_update(saved, identity_changed))
    {
        if let Some(previous_token) = previous_token
            && let Err(restore_error) = restore_server_token(secrets, source_id, previous_token)
        {
            warn!(
                %restore_error,
                %source_id,
                "failed to restore server token after settings update failed"
            );
        }
        return Err(error);
    }
    Ok(())
}

fn restore_server_token(
    secrets: &Arc<dyn SecretStore>,
    source_id: &SourceId,
    previous: Option<String>,
) -> Result<(), String> {
    match previous {
        Some(token) => secrets.save_token(source_id, &token),
        None => secrets.delete_token(source_id),
    }
    .map_err(|error| error.to_string())
}

fn source_is_selected(store: &StoreHandle, source_id: &SourceId) -> bool {
    store
        .with_store(|store| Ok(store.active_source()?.map(|saved| saved.source.id)))
        .ok()
        .flatten()
        .as_ref()
        == Some(source_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved_server_for_settings() -> SavedSource {
        SavedSource {
            source: SourceIdentity {
                id: SourceId::new("jellyfin:server:settings"),
                kind: "jellyfin".to_string(),
                name: "Old Server".to_string(),
                base_url: "https://music.example.test".to_string(),
            },
            user_id: "listener-id".to_string(),
            username: "listener".to_string(),
            trust_invalid_cert: false,
            use_jellyfin_instant_mix: false,
        }
    }

    fn server_settings_input(
        saved: &SavedSource,
        name: &str,
        base_url: &str,
        username: &str,
        password: &str,
        trust_invalid_cert: bool,
        use_jellyfin_instant_mix: bool,
    ) -> JellyfinSettingsInput {
        JellyfinSettingsInput {
            credentials: CredentialSettingsInput {
                source_id: saved.source.id.clone(),
                name: name.to_string(),
                base_url: base_url.to_string(),
                username: username.to_string(),
                password: password.to_string(),
                trust_invalid_cert,
            },
            use_instant_mix: use_jellyfin_instant_mix,
        }
    }

    fn update_jellyfin_settings_with_login(
        store: &StoreHandle,
        secrets: &Arc<dyn SecretStore>,
        input: JellyfinSettingsInput,
        login: impl FnOnce(SavedSource, CredentialHostInput) -> Result<AuthenticatedSource, String>,
    ) -> Result<(bool, bool), String> {
        let saved = store
            .with_store(|store| store.saved_source(&input.credentials.source_id))?
            .ok_or_else(|| "saved source missing".to_string())?;
        let prepared = crate::sources::prepare_jellyfin_settings_update_with_login(
            store, secrets, saved, input, login,
        )?;
        let Some(PreparedSourceSettingsUpdate {
            saved,
            identity_changed,
            credential,
            ..
        }) = prepared
        else {
            return Ok((false, false));
        };
        let reauthenticated = credential.is_some();
        persist_source_settings_update(
            store,
            secrets,
            &saved.source.id,
            &saved,
            identity_changed,
            credential.as_deref(),
        )?;
        Ok((identity_changed, reauthenticated))
    }

    fn provider_session(
        store: &StoreHandle,
        mut saved: SavedSource,
        source_id: SourceId,
        base_url: &str,
        user_id: &str,
        username: &str,
        token: &str,
    ) -> AuthenticatedSource {
        saved.source.base_url = base_url.to_string();
        saved.user_id = user_id.to_string();
        saved.username = username.to_string();
        let credential = token.to_string();
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        secrets
            .save_token(&saved.source.id, &credential)
            .expect("save authenticated source token");
        let active = crate::sources::activate_configured_source(store, &secrets, &saved)
            .expect("activate authenticated source");
        AuthenticatedSource {
            saved,
            credential,
            active,
            authenticated_source_id: source_id,
        }
    }

    fn seed_source_cache(store: &StoreHandle, saved: &SavedSource) {
        store
            .with_store(|store| {
                store.save_source(saved)?;
                let generation = store.begin_sync(&saved.source.id)?;
                let album = library_album(1, "Example Artist", "Example Album", None);
                store.upsert_albums(&saved.source.id, &[album], generation)?;
                store.complete_sync(&saved.source.id, generation)?;
                let queue = QueueEngine::new(saved.source.id.clone());
                store.save_queue_snapshot(&queue.snapshot())?;
                Ok(())
            })
            .expect("seed source cache");
    }

    fn saved_source(store: &StoreHandle, source_id: &SourceId) -> SavedSource {
        store
            .with_store(|store| {
                Ok(store
                    .list_sources()?
                    .into_iter()
                    .find(|saved| saved.source.id == *source_id))
            })
            .expect("load saved server")
            .expect("saved server")
    }

    fn cached_album_count(store: &StoreHandle, source_id: &SourceId) -> usize {
        store
            .with_store(|store| store.load_albums(source_id, 0, 1).map(|page| page.total))
            .expect("load albums")
    }

    fn queue_snapshot_saved(store: &StoreHandle, source_id: &SourceId) -> bool {
        store
            .with_store(|store| store.load_queue_snapshot(source_id))
            .expect("load queue snapshot")
            .is_some()
    }

    #[test]
    fn name_server_edit() {
        let store = StoreHandle::open_memory().expect("memory store");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let saved = saved_server_for_settings();
        seed_source_cache(&store, &saved);
        secrets
            .save_token(&saved.source.id, "old-token")
            .expect("save token");

        let (identity_changed, reauthenticated) = update_jellyfin_settings_with_login(
            &store,
            &secrets,
            server_settings_input(
                &saved,
                "Renamed Server",
                &saved.source.base_url,
                &saved.username,
                "",
                true,
                false,
            ),
            |_, _| panic!("name-only edit should not reauthenticate"),
        )
        .expect("update settings");

        assert!(!identity_changed);
        assert!(!reauthenticated);
        let edited = saved_source(&store, &saved.source.id);
        assert_eq!(edited.source.name, "Renamed Server");
        assert_eq!(edited.source.base_url, saved.source.base_url);
        assert_eq!(edited.username, saved.username);
        assert!(edited.trust_invalid_cert);
        assert_eq!(
            secrets.load_token(&saved.source.id).expect("load token"),
            Some("old-token".to_string())
        );
        assert_eq!(cached_album_count(&store, &saved.source.id), 1);
        assert!(queue_snapshot_saved(&store, &saved.source.id));
    }

    #[test]
    fn server_auth_identity() {
        let store = StoreHandle::open_memory().expect("memory store");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let saved = saved_server_for_settings();
        seed_source_cache(&store, &saved);
        secrets
            .save_token(&saved.source.id, "old-token")
            .expect("save token");

        let (identity_changed, reauthenticated) = update_jellyfin_settings_with_login(
            &store,
            &secrets,
            server_settings_input(
                &saved,
                &saved.source.name,
                "https://music-lan.example.test",
                &saved.username,
                "updated-password",
                false,
                false,
            ),
            |target, request| {
                assert_eq!(request.server_url, "https://music-lan.example.test");
                assert_eq!(request.username, "listener");
                Ok(provider_session(
                    &store,
                    target,
                    saved.source.id.clone(),
                    "https://music-lan.example.test",
                    &saved.user_id,
                    &saved.username,
                    "new-token",
                ))
            },
        )
        .expect("update settings");

        assert!(!identity_changed);
        assert!(reauthenticated);
        let edited = saved_source(&store, &saved.source.id);
        assert_eq!(edited.source.base_url, "https://music-lan.example.test");
        assert_eq!(edited.user_id, saved.user_id);
        assert_eq!(
            secrets.load_token(&saved.source.id).expect("load token"),
            Some("new-token".to_string())
        );
        assert_eq!(cached_album_count(&store, &saved.source.id), 1);
        assert!(queue_snapshot_saved(&store, &saved.source.id));
    }

    #[test]
    fn auth_sensitive_server() {
        let store = StoreHandle::open_memory().expect("memory store");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let saved = saved_server_for_settings();
        seed_source_cache(&store, &saved);
        secrets
            .save_token(&saved.source.id, "old-token")
            .expect("save token");

        let error = update_jellyfin_settings_with_login(
            &store,
            &secrets,
            server_settings_input(
                &saved,
                "Renamed Server",
                &saved.source.base_url,
                "alternate",
                "updated-password",
                false,
                false,
            ),
            |_, _| Err("Authentication failed".to_string()),
        )
        .expect_err("auth failure");

        assert_eq!(error, "Authentication failed");
        let current = saved_source(&store, &saved.source.id);
        assert_eq!(current, saved);
        assert_eq!(
            secrets.load_token(&saved.source.id).expect("load token"),
            Some("old-token".to_string())
        );
        assert_eq!(cached_album_count(&store, &saved.source.id), 1);
        assert!(queue_snapshot_saved(&store, &saved.source.id));
    }

    #[test]
    fn server_change_identity() {
        let store = StoreHandle::open_memory().expect("memory store");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let saved = saved_server_for_settings();
        seed_source_cache(&store, &saved);
        secrets
            .save_token(&saved.source.id, "old-token")
            .expect("save token");

        let (identity_changed, reauthenticated) = update_jellyfin_settings_with_login(
            &store,
            &secrets,
            server_settings_input(
                &saved,
                &saved.source.name,
                &saved.source.base_url,
                "alternate",
                "updated-password",
                false,
                false,
            ),
            |target, _request| {
                Ok(provider_session(
                    &store,
                    target,
                    SourceId::new("jellyfin:server:other"),
                    &saved.source.base_url,
                    "alternate-id",
                    "alternate",
                    "new-token",
                ))
            },
        )
        .expect("update settings");

        assert!(identity_changed);
        assert!(reauthenticated);
        let edited = saved_source(&store, &saved.source.id);
        assert_eq!(edited.source.id, saved.source.id);
        assert_eq!(edited.user_id, "alternate-id");
        assert_eq!(edited.username, "alternate");
        assert_eq!(
            secrets.load_token(&saved.source.id).expect("load token"),
            Some("new-token".to_string())
        );
        assert_eq!(cached_album_count(&store, &saved.source.id), 0);
        assert!(!queue_snapshot_saved(&store, &saved.source.id));
    }
}
