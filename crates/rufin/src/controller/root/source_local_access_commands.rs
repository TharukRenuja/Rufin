use super::*;

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
        let secrets = Arc::clone(&self.secrets);
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
            let result = store.with_store(|store| {
                store.save_source_local_access(&SourceLocalAccess {
                    source_id,
                    root_path: root_path.clone(),
                    path_replace_from: trimmed_optional(path_replace_from.as_deref()),
                    path_replace_to: Some(path_replace_to),
                })
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Err(error) = runtime.block_on(refresh_local_track_matches(
                &store,
                &matched_source_id,
                None,
                None,
            )) {
                warn!(%error, "failed to refresh local track matches");
            }
            clear_next_preload(&next_preload);
            prepare_next_stream_from_handles(
                store.clone(),
                Arc::clone(&runtime),
                Arc::clone(&secrets),
                Arc::clone(&playback),
                Arc::clone(&queue),
                Arc::clone(&next_preload),
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
    pub fn update_source_settings(&self, input: SourceSettingsInput) {
        let source_id = input.source_id.clone();
        let sync_context = self.sync_context();
        let store = sync_context.store.clone();
        let runtime = Arc::clone(&sync_context.runtime);
        let secrets = Arc::clone(&sync_context.secrets);
        let events = sync_context.events.clone();
        let queue = Arc::clone(&self.queue);
        let playback_request_generation = Arc::clone(&self.playback_request_generation);
        let next_preload = Arc::clone(&self.next_preload);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        thread::spawn(move || {
            let result = update_source_settings_with_login(&store, &secrets, input, |request| {
                if sync_is_running(&sync_context.sync_in_flight, &source_id) {
                    return Err(
                        "Wait for the current library sync to finish before editing server credentials."
                            .to_string(),
                    );
                }
                let source_name = request.source.title();
                let _sent = events.send(ControllerEvent::LoginStatus(format!(
                    "Checking {source_name} server..."
                )));
                let device_id = if request.source == StreamingSource::Jellyfin {
                    Some(ensure_jellyfin_device_id(&store)?)
                } else {
                    None
                };
                runtime
                    .block_on(login_source(
                        request.source,
                        request.base_url,
                        request.username,
                        request.password,
                        request.trust_invalid_cert,
                        device_id,
                    ))
                    .map_err(|error| error.to_string())
            });
            match result {
                Ok(outcome) if outcome.changed && outcome.identity_changed => {
                    let status = source_settings_status_for_outcome(&outcome);
                    let Some(saved) = outcome.saved else {
                        emit_snapshot(&store, &events);
                        return;
                    };
                    if let Err(error) = clear_store_disk_cover_cache(&store, &saved.source.id) {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                    if let Err(error) = reset_identity_queue(
                        &QueueActivationContext {
                            store: &store,
                            queue: &queue,
                            playback_request_generation: &playback_request_generation,
                            next_preload: &next_preload,
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
                        source_settings_status_message(status).to_string(),
                    ));
                    start_sync_thread_with_snapshots(sync_context, saved, SyncPresentation::Silent);
                }
                Ok(outcome) if outcome.changed => {
                    if source_settings_status_for_outcome(&outcome)
                        == SourceSettingsStatus::Resyncing
                    {
                        let Some(saved) = outcome.saved else {
                            emit_snapshot(&store, &events);
                            return;
                        };
                        let _sent = events.send(ControllerEvent::LoginStatus(
                            source_settings_status_message(SourceSettingsStatus::Resyncing)
                                .to_string(),
                        ));
                        start_sync_thread_with_snapshots(
                            sync_context,
                            saved,
                            SyncPresentation::Silent,
                        );
                        return;
                    }
                    let _sent = events.send(ControllerEvent::LoginStatus(
                        source_settings_status_message(SourceSettingsStatus::Saved).to_string(),
                    ));
                    emit_snapshot(&store, &events);
                }
                Ok(_) => {
                    let _sent = events.send(ControllerEvent::LoginStatus(
                        source_settings_status_message(SourceSettingsStatus::Unchanged).to_string(),
                    ));
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }
        });
    }
    pub fn clear_source_local_access(&self, source_id: SourceId) {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
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
                Arc::clone(&secrets),
                Arc::clone(&playback),
                Arc::clone(&queue),
                Arc::clone(&next_preload),
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
pub(crate) struct SourceSettingsInput {
    pub(crate) source_id: SourceId,
    pub(crate) name: String,
    pub(crate) base_url: String,
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) trust_invalid_cert: bool,
    pub(crate) use_jellyfin_instant_mix: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceSettingsReauthRequest {
    source: StreamingSource,
    base_url: String,
    username: String,
    password: String,
    trust_invalid_cert: bool,
}

#[derive(Clone, Debug)]
struct PreparedSourceSettingsUpdate {
    saved: SavedSource,
    next_name: String,
    next_base_url: String,
    next_username: String,
    next_trust_invalid_cert: bool,
    next_use_jellyfin_instant_mix: bool,
    reauth: Option<SourceSettingsReauthRequest>,
}

#[derive(Clone, Debug)]
struct SourceSettingsUpdateOutcome {
    changed: bool,
    identity_changed: bool,
    reauthenticated: bool,
    saved: Option<SavedSource>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourceSettingsStatus {
    Saved,
    Resyncing,
    Unchanged,
}

fn source_settings_status_for_outcome(
    outcome: &SourceSettingsUpdateOutcome,
) -> SourceSettingsStatus {
    if !outcome.changed {
        SourceSettingsStatus::Unchanged
    } else if outcome.identity_changed || outcome.reauthenticated {
        SourceSettingsStatus::Resyncing
    } else {
        SourceSettingsStatus::Saved
    }
}

fn source_settings_status_message(status: SourceSettingsStatus) -> &'static str {
    match status {
        SourceSettingsStatus::Saved => "Source settings saved.",
        SourceSettingsStatus::Resyncing => "Source settings saved.",
        SourceSettingsStatus::Unchanged => "No changes to save.",
    }
}

fn update_source_settings_with_login(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    input: SourceSettingsInput,
    login: impl FnOnce(SourceSettingsReauthRequest) -> Result<SourceSession, String>,
) -> Result<SourceSettingsUpdateOutcome, String> {
    let saved = store.with_store(|store| {
        Ok(store
            .list_sources()?
            .into_iter()
            .find(|saved| saved.source.id == input.source_id))
    })?;
    let Some(saved) = saved else {
        return Ok(SourceSettingsUpdateOutcome {
            changed: false,
            identity_changed: false,
            reauthenticated: false,
            saved: None,
        });
    };
    let Some(prepared) = prepare_source_settings_update(saved, input)? else {
        return Ok(SourceSettingsUpdateOutcome {
            changed: false,
            identity_changed: false,
            reauthenticated: false,
            saved: None,
        });
    };
    let session = match prepared.reauth.clone() {
        Some(request) => Some(login(request)?),
        None => None,
    };
    persist_prepared_source_settings_update(store, secrets, &prepared, session.as_ref())
}

fn prepare_source_settings_update(
    saved: SavedSource,
    input: SourceSettingsInput,
) -> Result<Option<PreparedSourceSettingsUpdate>, String> {
    let remote = saved.source.kind != LOCAL_SOURCE_ID;
    let next_name = input.name.trim().to_string();
    let next_base_url = if remote {
        input.base_url.trim().to_string()
    } else {
        saved.source.base_url.clone()
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
    let next_use_jellyfin_instant_mix = if saved.source.kind == "jellyfin" {
        input.use_jellyfin_instant_mix
    } else {
        false
    };

    if remote && next_base_url.is_empty() {
        return Err("Enter a server address.".to_string());
    }
    if remote && next_username.is_empty() {
        return Err("Enter a username.".to_string());
    }

    let password_entered = remote && !input.password.is_empty();
    let auth_sensitive = remote
        && (saved.source.base_url != next_base_url
            || saved.username != next_username
            || password_entered);
    if auth_sensitive && input.password.is_empty() {
        return Err("Enter the server password to save address or username changes.".to_string());
    }

    let changed = saved.source.name != next_name
        || saved.source.base_url != next_base_url
        || saved.username != next_username
        || saved.trust_invalid_cert != next_trust_invalid_cert
        || saved.use_jellyfin_instant_mix != next_use_jellyfin_instant_mix
        || password_entered;
    if !changed {
        return Ok(None);
    }

    let reauth = if auth_sensitive {
        let source = StreamingSource::from_source_id(&saved.source.kind)
            .ok_or_else(|| "Saved server source is no longer supported.".to_string())?;
        Some(SourceSettingsReauthRequest {
            source,
            base_url: next_base_url.clone(),
            username: next_username.clone(),
            password: input.password,
            trust_invalid_cert: next_trust_invalid_cert,
        })
    } else {
        None
    };

    Ok(Some(PreparedSourceSettingsUpdate {
        saved,
        next_name,
        next_base_url,
        next_username,
        next_trust_invalid_cert,
        next_use_jellyfin_instant_mix,
        reauth,
    }))
}

fn persist_prepared_source_settings_update(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    prepared: &PreparedSourceSettingsUpdate,
    session: Option<&SourceSession>,
) -> Result<SourceSettingsUpdateOutcome, String> {
    if let Some(session) = session
        && session.source.kind != prepared.saved.source.kind
    {
        return Err("Authenticated source did not match the saved server.".to_string());
    }

    let identity_changed =
        session.is_some_and(|session| authenticated_identity_changed(&prepared.saved, session));
    let next_saved = next_saved_server(prepared, session);
    let previous_token = if let Some(session) = session {
        let previous = secrets
            .load_token(&prepared.saved.source.id)
            .map_err(|error| error.to_string())?;
        secrets
            .save_token(&prepared.saved.source.id, &session.access_token)
            .map_err(|error| error.to_string())?;
        Some(previous)
    } else {
        None
    };

    let result = store.with_store(|store| {
        store.save_source_settings_update(&next_saved, identity_changed)?;
        Ok(())
    });
    if let Err(error) = result {
        if let Some(previous_token) = previous_token
            && let Err(restore_error) =
                restore_server_token(secrets, &prepared.saved.source.id, previous_token)
        {
            warn!(
                %restore_error,
                source_id = %prepared.saved.source.id,
                "failed to restore server token after settings update failed"
            );
        }
        return Err(error);
    }

    Ok(SourceSettingsUpdateOutcome {
        changed: true,
        identity_changed,
        reauthenticated: session.is_some(),
        saved: Some(next_saved),
    })
}

fn next_saved_server(
    prepared: &PreparedSourceSettingsUpdate,
    session: Option<&SourceSession>,
) -> SavedSource {
    let mut saved = prepared.saved.clone();
    saved.source.name = prepared.next_name.clone();
    saved.source.base_url = session
        .map(|session| session.source.base_url.clone())
        .unwrap_or_else(|| prepared.next_base_url.clone());
    saved.username = session
        .map(|session| session.username.clone())
        .unwrap_or_else(|| prepared.next_username.clone());
    saved.user_id = session
        .map(|session| session.user_id.clone())
        .unwrap_or_else(|| saved.user_id.clone());
    saved.trust_invalid_cert = prepared.next_trust_invalid_cert;
    saved.use_jellyfin_instant_mix = prepared.next_use_jellyfin_instant_mix;
    saved
}

fn authenticated_identity_changed(saved: &SavedSource, session: &SourceSession) -> bool {
    saved.source.kind != session.source.kind
        || saved.source.id != session.source.id
        || saved.user_id != session.user_id
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

fn reset_identity_queue(
    context: &QueueActivationContext<'_>,
    saved: &SavedSource,
) -> Result<(), String> {
    let active_id = context
        .store
        .with_store(|store| Ok(store.active_source()?.map(|saved| saved.source.id)))?;
    if active_id.as_ref() != Some(&saved.source.id) {
        return Ok(());
    }

    let restored = QueueEngine::new(saved.source.id.clone());
    let queue_snapshot = restored.snapshot();
    let auto_dj_enabled = context
        .auto_dj_enabled
        .lock()
        .map(|enabled| *enabled)
        .unwrap_or_default();
    let player = playback_snapshot_from_queue(
        Some(&restored),
        auto_dj_enabled,
        &load_settings_from_store(context.store).playback,
    );
    *context
        .queue
        .lock()
        .map_err(|_| "queue lock was poisoned".to_string())? = Some(restored);
    if let Ok(mut snapshot) = context.playback_snapshot.lock() {
        *snapshot = player.clone();
    }
    invalidate_playback_requests(context.playback_request_generation);
    stop_playback_backend(context.playback, context.next_preload, context.events);
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
    ) -> SourceSettingsInput {
        SourceSettingsInput {
            source_id: saved.source.id.clone(),
            name: name.to_string(),
            base_url: base_url.to_string(),
            username: username.to_string(),
            password: password.to_string(),
            trust_invalid_cert,
            use_jellyfin_instant_mix,
        }
    }

    fn provider_session(
        saved: &SavedSource,
        source_id: SourceId,
        base_url: &str,
        user_id: &str,
        username: &str,
        token: &str,
    ) -> SourceSession {
        SourceSession {
            source: SourceIdentity {
                id: source_id,
                kind: saved.source.kind.clone(),
                name: "Jellyfin".to_string(),
                base_url: base_url.to_string(),
            },
            user_id: user_id.to_string(),
            username: username.to_string(),
            access_token: token.to_string(),
            device_id: Some("rufin-install-one".to_string()),
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

        let outcome = update_source_settings_with_login(
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
            |_| panic!("name-only edit should not reauthenticate"),
        )
        .expect("update settings");

        assert!(outcome.changed);
        assert!(!outcome.identity_changed);
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

        let outcome = update_source_settings_with_login(
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
            |request| {
                assert_eq!(request.base_url, "https://music-lan.example.test");
                assert_eq!(request.username, "listener");
                Ok(provider_session(
                    &saved,
                    saved.source.id.clone(),
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
        assert!(outcome.reauthenticated);
        assert_eq!(
            source_settings_status_for_outcome(&outcome),
            SourceSettingsStatus::Resyncing
        );
        assert_eq!(
            source_settings_status_message(source_settings_status_for_outcome(&outcome)),
            "Source settings saved."
        );
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

        let error = update_source_settings_with_login(
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
            |_request| Err("Authentication failed".to_string()),
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

        let outcome = update_source_settings_with_login(
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
            |_request| {
                Ok(provider_session(
                    &saved,
                    SourceId::new("jellyfin:server:other"),
                    &saved.source.base_url,
                    "alternate-id",
                    "alternate",
                    "new-token",
                ))
            },
        )
        .expect("update settings");

        assert!(outcome.changed);
        assert!(outcome.identity_changed);
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
