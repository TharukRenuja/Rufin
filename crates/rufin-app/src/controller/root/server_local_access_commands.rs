use super::*;

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
        username: String,
        password: String,
        trust_invalid_cert: bool,
    ) {
        let sync_context = self.sync_context();
        let store = sync_context.store.clone();
        let runtime = Arc::clone(&sync_context.runtime);
        let secrets = Arc::clone(&sync_context.secrets);
        let events = sync_context.events.clone();
        let queue = Arc::clone(&self.queue);
        let playback_request_generation = Arc::clone(&self.playback_request_generation);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        thread::spawn(move || {
            let input = ServerSettingsInput {
                server_id: server_id.clone(),
                name,
                base_url,
                username,
                password,
                trust_invalid_cert,
            };
            let result = update_server_settings_with_login(&store, &secrets, input, |request| {
                if sync_is_running(&sync_context.sync_in_flight, &server_id) {
                    return Err(
                        "Wait for the current library sync to finish before editing server credentials."
                            .to_string(),
                    );
                }
                let provider_name = request.provider.title();
                let _sent = events.send(ControllerEvent::LoginStatus(format!(
                    "Checking {provider_name} server..."
                )));
                runtime
                    .block_on(login_provider(
                        request.provider,
                        request.base_url,
                        request.username,
                        request.password,
                        request.trust_invalid_cert,
                    ))
                    .map_err(|error| error.to_string())
            });
            match result {
                Ok(outcome) if outcome.changed && outcome.identity_changed => {
                    let Some(saved) = outcome.saved else {
                        emit_snapshot(&store, &events);
                        return;
                    };
                    if let Err(error) = clear_disk_cover_cache(&saved.server.id) {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                    if let Err(error) = reset_active_queue_after_server_identity_change(
                        &QueueActivationContext {
                            store: &store,
                            queue: &queue,
                            playback_request_generation: &playback_request_generation,
                            playback: &playback,
                            playback_snapshot: &playback_snapshot,
                            auto_dj_enabled: &auto_dj_enabled,
                            events: &events,
                        },
                        &saved,
                    ) {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                    let _sent = events.send(ControllerEvent::LoginStatus(
                        "Server settings saved. Resyncing library...".to_string(),
                    ));
                    start_sync_thread(sync_context, saved);
                }
                Ok(outcome) if outcome.changed => {
                    let _sent = events.send(ControllerEvent::LoginStatus(
                        "Server settings saved.".to_string(),
                    ));
                    emit_snapshot(&store, &events);
                }
                Ok(_) => {}
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct ServerSettingsInput {
    server_id: ServerId,
    name: String,
    base_url: String,
    username: String,
    password: String,
    trust_invalid_cert: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ServerSettingsReauthRequest {
    provider: StreamingProvider,
    base_url: String,
    username: String,
    password: String,
    trust_invalid_cert: bool,
}

#[derive(Clone, Debug)]
struct PreparedServerSettingsUpdate {
    saved: SavedServer,
    next_name: String,
    next_base_url: String,
    next_username: String,
    next_trust_invalid_cert: bool,
    reauth: Option<ServerSettingsReauthRequest>,
}

#[derive(Clone, Debug)]
struct ServerSettingsUpdateOutcome {
    changed: bool,
    identity_changed: bool,
    saved: Option<SavedServer>,
}

fn update_server_settings_with_login(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    input: ServerSettingsInput,
    login: impl FnOnce(ServerSettingsReauthRequest) -> Result<ProviderSession, String>,
) -> Result<ServerSettingsUpdateOutcome, String> {
    let saved = store.with_store(|store| {
        Ok(store
            .list_servers()?
            .into_iter()
            .find(|saved| saved.server.id == input.server_id))
    })?;
    let Some(saved) = saved else {
        return Ok(ServerSettingsUpdateOutcome {
            changed: false,
            identity_changed: false,
            saved: None,
        });
    };
    let Some(prepared) = prepare_server_settings_update(saved, input)? else {
        return Ok(ServerSettingsUpdateOutcome {
            changed: false,
            identity_changed: false,
            saved: None,
        });
    };
    let session = match prepared.reauth.clone() {
        Some(request) => Some(login(request)?),
        None => None,
    };
    persist_prepared_server_settings_update(store, secrets, &prepared, session.as_ref())
}

fn prepare_server_settings_update(
    saved: SavedServer,
    input: ServerSettingsInput,
) -> Result<Option<PreparedServerSettingsUpdate>, String> {
    let remote = saved.server.provider != LOCAL_PROVIDER_ID;
    let next_name = input.name.trim().to_string();
    let next_base_url = if remote {
        input.base_url.trim().to_string()
    } else {
        saved.server.base_url.clone()
    };
    let next_username = if remote {
        input.username.trim().to_string()
    } else {
        saved.username.clone()
    };
    let next_trust_invalid_cert = if remote {
        input.trust_invalid_cert
    } else {
        saved.trust_invalid_cert
    };

    if remote && next_base_url.is_empty() {
        return Err("Enter a server address.".to_string());
    }
    if remote && next_username.is_empty() {
        return Err("Enter a username.".to_string());
    }

    let password_entered = remote && !input.password.is_empty();
    let auth_sensitive = remote
        && (saved.server.base_url != next_base_url
            || saved.username != next_username
            || password_entered);
    if auth_sensitive && input.password.is_empty() {
        return Err("Enter the server password to save address or username changes.".to_string());
    }

    let changed = saved.server.name != next_name
        || saved.server.base_url != next_base_url
        || saved.username != next_username
        || saved.trust_invalid_cert != next_trust_invalid_cert
        || password_entered;
    if !changed {
        return Ok(None);
    }

    let reauth = if auth_sensitive {
        let provider = StreamingProvider::from_provider_id(&saved.server.provider)
            .ok_or_else(|| "Saved server provider is no longer supported.".to_string())?;
        Some(ServerSettingsReauthRequest {
            provider,
            base_url: next_base_url.clone(),
            username: next_username.clone(),
            password: input.password,
            trust_invalid_cert: next_trust_invalid_cert,
        })
    } else {
        None
    };

    Ok(Some(PreparedServerSettingsUpdate {
        saved,
        next_name,
        next_base_url,
        next_username,
        next_trust_invalid_cert,
        reauth,
    }))
}

fn persist_prepared_server_settings_update(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    prepared: &PreparedServerSettingsUpdate,
    session: Option<&ProviderSession>,
) -> Result<ServerSettingsUpdateOutcome, String> {
    if let Some(session) = session
        && session.server.provider != prepared.saved.server.provider
    {
        return Err("Authenticated provider did not match the saved server.".to_string());
    }

    let identity_changed =
        session.is_some_and(|session| authenticated_identity_changed(&prepared.saved, session));
    let next_saved = next_saved_server(prepared, session);
    let previous_token = if let Some(session) = session {
        let previous = secrets
            .load_token(&prepared.saved.server.id)
            .map_err(|error| error.to_string())?;
        secrets
            .save_token(&prepared.saved.server.id, &session.access_token)
            .map_err(|error| error.to_string())?;
        Some(previous)
    } else {
        None
    };

    let result = store.with_store(|store| {
        store.save_server_settings_update(&next_saved, identity_changed)?;
        Ok(())
    });
    if let Err(error) = result {
        if let Some(previous_token) = previous_token
            && let Err(restore_error) =
                restore_server_token(secrets, &prepared.saved.server.id, previous_token)
        {
            warn!(
                %restore_error,
                server_id = %prepared.saved.server.id,
                "failed to restore server token after settings update failed"
            );
        }
        return Err(error);
    }

    Ok(ServerSettingsUpdateOutcome {
        changed: true,
        identity_changed,
        saved: Some(next_saved),
    })
}

fn next_saved_server(
    prepared: &PreparedServerSettingsUpdate,
    session: Option<&ProviderSession>,
) -> SavedServer {
    let mut saved = prepared.saved.clone();
    saved.server.name = prepared.next_name.clone();
    saved.server.base_url = session
        .map(|session| session.server.base_url.clone())
        .unwrap_or_else(|| prepared.next_base_url.clone());
    saved.username = session
        .map(|session| session.username.clone())
        .unwrap_or_else(|| prepared.next_username.clone());
    saved.user_id = session
        .map(|session| session.user_id.clone())
        .unwrap_or_else(|| saved.user_id.clone());
    saved.trust_invalid_cert = prepared.next_trust_invalid_cert;
    saved
}

fn authenticated_identity_changed(saved: &SavedServer, session: &ProviderSession) -> bool {
    saved.server.provider != session.server.provider
        || saved.server.id != session.server.id
        || saved.user_id != session.user_id
}

fn restore_server_token(
    secrets: &Arc<dyn SecretStore>,
    server_id: &ServerId,
    previous: Option<String>,
) -> Result<(), String> {
    match previous {
        Some(token) => secrets.save_token(server_id, &token),
        None => secrets.delete_token(server_id),
    }
    .map_err(|error| error.to_string())
}

fn reset_active_queue_after_server_identity_change(
    context: &QueueActivationContext<'_>,
    saved: &SavedServer,
) -> Result<(), String> {
    let active_id = context
        .store
        .with_store(|store| Ok(store.active_server()?.map(|saved| saved.server.id)))?;
    if active_id.as_ref() != Some(&saved.server.id) {
        return Ok(());
    }

    let restored = QueueEngine::new(saved.server.id.clone());
    let queue_snapshot = restored.snapshot();
    let auto_dj_enabled = context
        .auto_dj_enabled
        .lock()
        .map(|enabled| *enabled)
        .unwrap_or_default();
    let player = playback_snapshot_from_queue(
        Some(&restored),
        auto_dj_enabled,
        &load_settings_for_saved(context.store, saved).playback,
    );
    *context
        .queue
        .lock()
        .map_err(|_| "queue lock was poisoned".to_string())? = Some(restored);
    if let Ok(mut snapshot) = context.playback_snapshot.lock() {
        *snapshot = player.clone();
    }
    invalidate_playback_requests(context.playback_request_generation);
    stop_playback_backend(context.playback, context.events);
    let _sent = context
        .events
        .send(ControllerEvent::Queue(Box::new(Some(queue_snapshot))));
    let _sent = context
        .events
        .send(ControllerEvent::Playback(Box::new(player)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved_server_for_settings() -> SavedServer {
        SavedServer {
            server: ServerIdentity {
                id: ServerId::new("jellyfin:server:settings"),
                provider: "jellyfin".to_string(),
                name: "Old Server".to_string(),
                base_url: "https://music.example.test".to_string(),
            },
            user_id: "listener-id".to_string(),
            username: "listener".to_string(),
            trust_invalid_cert: false,
        }
    }

    fn server_settings_input(
        saved: &SavedServer,
        name: &str,
        base_url: &str,
        username: &str,
        password: &str,
        trust_invalid_cert: bool,
    ) -> ServerSettingsInput {
        ServerSettingsInput {
            server_id: saved.server.id.clone(),
            name: name.to_string(),
            base_url: base_url.to_string(),
            username: username.to_string(),
            password: password.to_string(),
            trust_invalid_cert,
        }
    }

    fn provider_session(
        saved: &SavedServer,
        server_id: ServerId,
        base_url: &str,
        user_id: &str,
        username: &str,
        token: &str,
    ) -> ProviderSession {
        ProviderSession {
            server: ServerIdentity {
                id: server_id,
                provider: saved.server.provider.clone(),
                name: "Jellyfin".to_string(),
                base_url: base_url.to_string(),
            },
            user_id: user_id.to_string(),
            username: username.to_string(),
            access_token: token.to_string(),
        }
    }

    fn seed_server_cache(store: &StoreHandle, saved: &SavedServer) {
        store
            .with_store(|store| {
                store.save_server(saved)?;
                let generation = store.begin_sync(&saved.server.id)?;
                let album = super::super::lyrics_local_access_tests::library_album(
                    1,
                    "Example Artist",
                    "Example Album",
                    None,
                );
                store.upsert_albums(&saved.server.id, &[album], generation)?;
                store.complete_sync(&saved.server.id, generation)?;
                let queue = QueueEngine::new(saved.server.id.clone());
                store.save_queue_snapshot(&queue.snapshot())?;
                Ok(())
            })
            .expect("seed server cache");
    }

    fn saved_server(store: &StoreHandle, server_id: &ServerId) -> SavedServer {
        store
            .with_store(|store| {
                Ok(store
                    .list_servers()?
                    .into_iter()
                    .find(|saved| saved.server.id == *server_id))
            })
            .expect("load saved server")
            .expect("saved server")
    }

    fn cached_album_count(store: &StoreHandle, server_id: &ServerId) -> usize {
        store
            .with_store(|store| store.load_albums(server_id, 0, 1).map(|page| page.total))
            .expect("load albums")
    }

    fn queue_snapshot_saved(store: &StoreHandle, server_id: &ServerId) -> bool {
        store
            .with_store(|store| store.load_queue_snapshot(server_id))
            .expect("load queue snapshot")
            .is_some()
    }

    #[test]
    fn name_only_server_edit_preserves_token_and_cache() {
        let store = StoreHandle::open_memory().expect("memory store");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let saved = saved_server_for_settings();
        seed_server_cache(&store, &saved);
        secrets
            .save_token(&saved.server.id, "old-token")
            .expect("save token");

        let outcome = update_server_settings_with_login(
            &store,
            &secrets,
            server_settings_input(
                &saved,
                "Renamed Server",
                &saved.server.base_url,
                &saved.username,
                "",
                true,
            ),
            |_| panic!("name-only edit should not reauthenticate"),
        )
        .expect("update settings");

        assert!(outcome.changed);
        assert!(!outcome.identity_changed);
        let edited = saved_server(&store, &saved.server.id);
        assert_eq!(edited.server.name, "Renamed Server");
        assert_eq!(edited.server.base_url, saved.server.base_url);
        assert_eq!(edited.username, saved.username);
        assert!(edited.trust_invalid_cert);
        assert_eq!(
            secrets.load_token(&saved.server.id).expect("load token"),
            Some("old-token".to_string())
        );
        assert_eq!(cached_album_count(&store, &saved.server.id), 1);
        assert!(queue_snapshot_saved(&store, &saved.server.id));
    }

    #[test]
    fn auth_sensitive_server_edit_success_refreshes_token_without_clearing_same_identity() {
        let store = StoreHandle::open_memory().expect("memory store");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let saved = saved_server_for_settings();
        seed_server_cache(&store, &saved);
        secrets
            .save_token(&saved.server.id, "old-token")
            .expect("save token");

        let outcome = update_server_settings_with_login(
            &store,
            &secrets,
            server_settings_input(
                &saved,
                &saved.server.name,
                "https://music-lan.example.test",
                &saved.username,
                "updated-password",
                false,
            ),
            |request| {
                assert_eq!(request.base_url, "https://music-lan.example.test");
                assert_eq!(request.username, "listener");
                Ok(provider_session(
                    &saved,
                    saved.server.id.clone(),
                    "https://music-lan.example.test",
                    &saved.user_id,
                    &saved.username,
                    "new-token",
                ))
            },
        )
        .expect("update settings");

        assert!(outcome.changed);
        assert!(!outcome.identity_changed);
        let edited = saved_server(&store, &saved.server.id);
        assert_eq!(edited.server.base_url, "https://music-lan.example.test");
        assert_eq!(edited.user_id, saved.user_id);
        assert_eq!(
            secrets.load_token(&saved.server.id).expect("load token"),
            Some("new-token".to_string())
        );
        assert_eq!(cached_album_count(&store, &saved.server.id), 1);
        assert!(queue_snapshot_saved(&store, &saved.server.id));
    }

    #[test]
    fn auth_sensitive_server_edit_failure_preserves_settings_token_and_cache() {
        let store = StoreHandle::open_memory().expect("memory store");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let saved = saved_server_for_settings();
        seed_server_cache(&store, &saved);
        secrets
            .save_token(&saved.server.id, "old-token")
            .expect("save token");

        let error = update_server_settings_with_login(
            &store,
            &secrets,
            server_settings_input(
                &saved,
                "Renamed Server",
                &saved.server.base_url,
                "alternate",
                "updated-password",
                false,
            ),
            |_request| Err("Authentication failed".to_string()),
        )
        .expect_err("auth failure");

        assert_eq!(error, "Authentication failed");
        let current = saved_server(&store, &saved.server.id);
        assert_eq!(current, saved);
        assert_eq!(
            secrets.load_token(&saved.server.id).expect("load token"),
            Some("old-token".to_string())
        );
        assert_eq!(cached_album_count(&store, &saved.server.id), 1);
        assert!(queue_snapshot_saved(&store, &saved.server.id));
    }

    #[test]
    fn auth_sensitive_server_edit_clears_cache_when_authenticated_identity_changes() {
        let store = StoreHandle::open_memory().expect("memory store");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let saved = saved_server_for_settings();
        seed_server_cache(&store, &saved);
        secrets
            .save_token(&saved.server.id, "old-token")
            .expect("save token");

        let outcome = update_server_settings_with_login(
            &store,
            &secrets,
            server_settings_input(
                &saved,
                &saved.server.name,
                &saved.server.base_url,
                "alternate",
                "updated-password",
                false,
            ),
            |_request| {
                Ok(provider_session(
                    &saved,
                    ServerId::new("jellyfin:server:other"),
                    &saved.server.base_url,
                    "alternate-id",
                    "alternate",
                    "new-token",
                ))
            },
        )
        .expect("update settings");

        assert!(outcome.changed);
        assert!(outcome.identity_changed);
        let edited = saved_server(&store, &saved.server.id);
        assert_eq!(edited.server.id, saved.server.id);
        assert_eq!(edited.user_id, "alternate-id");
        assert_eq!(edited.username, "alternate");
        assert_eq!(
            secrets.load_token(&saved.server.id).expect("load token"),
            Some("new-token".to_string())
        );
        assert_eq!(cached_album_count(&store, &saved.server.id), 0);
        assert!(!queue_snapshot_saved(&store, &saved.server.id));
    }
}
