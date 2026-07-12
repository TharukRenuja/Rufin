use super::*;
use crate::source_setup::PreparedSourceSettingsUpdate;
#[cfg(test)]
use crate::source_setup::{
    AuthenticatedSource, CredentialHostInput, CredentialSettingsInput, JellyfinSettingsInput,
};
use playback::SessionCommand;

impl AppController {
    pub fn save_source_local_access(
        &self,
        source_id: SourceId,
        root_path: PathBuf,
        path_replace_from: Option<String>,
        path_replace_to: Option<String>,
    ) {
        let controller = self.clone();
        let store = self.store.clone();
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
            let changed = match store.with_store(|store| {
                store.save_source_local_access(&SourceLocalAccess {
                    source_id: source_id.clone(),
                    root_path: root_path.clone(),
                    path_replace_from: trimmed_optional(path_replace_from.as_deref()),
                    path_replace_to: Some(path_replace_to),
                })
            }) {
                Ok(changed) => changed,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            let current = sync_target_is_current(&store, &matched_source_id);
            if current {
                controller.send_session_command(SessionCommand::StreamInputsChanged);
            }
            emit_snapshot(&store, &events);
            if changed && current {
                controller.refresh_source_freshness();
            }
        });
    }

    pub(crate) fn configured_source(&self, source_id: &SourceId) -> Option<StoredSource> {
        self.store
            .with_store(|store| store.stored_source(source_id))
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
                StoredSource,
                &dyn Fn(),
            ) -> Result<Option<PreparedSourceSettingsUpdate>, String>
            + Send
            + 'static,
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
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            };
            let saved = match store.with_store(|store| store.stored_source(&source_id)) {
                Ok(Some(saved)) => saved,
                Ok(None) => {
                    if current() {
                        let _sent =
                            events.send(ControllerEvent::SourceNotice(SourceNotice::NoChanges));
                    }
                    return;
                }
                Err(error) => {
                    emit_current_error(error);
                    return;
                }
            };
            let authentication_started = || {
                let _sent = events.send(ControllerEvent::SourceNotice(SourceNotice::Checking {
                    source_name: source_name.to_string(),
                }));
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
                    let _sent = events.send(ControllerEvent::SourceNotice(SourceNotice::NoChanges));
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
            let current_saved = match store.with_store(|store| store.stored_source(&source_id)) {
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
            let selected = source_is_selected(&store, &saved.source_id);
            if reauthenticated || identity_changed {
                controller.forget_source_sync(&source_id);
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
                if let Err(error) = controller.invalidate_artwork_source(&source_id) {
                    warn!(%error, %source_id, "failed to invalidate replaced source artwork");
                }
                if selected {
                    let projection = match controller.activate_playback_source(&saved.source_id) {
                        Ok(projection) => projection,
                        Err(error) => {
                            emit_error(error);
                            return;
                        }
                    };
                    let _sent = events.send(ControllerEvent::PlaybackProduct(Box::new(projection)));
                }
            }
            if selected {
                controller.refresh_source_freshness();
            }
            let _sent = events.send(ControllerEvent::SourceNotice(SourceNotice::SettingsSaved));
            if !selected {
                emit_snapshot(&store, &events);
            }
            drop(transition_commit);
        });
    }

    pub fn clear_source_local_access(&self, source_id: SourceId) {
        let controller = self.clone();
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            if let Err(error) =
                store.with_store(|store| store.clear_source_local_access(&source_id))
            {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if sync_target_is_current(&store, &source_id) {
                controller.send_session_command(SessionCommand::StreamInputsChanged);
            }
            emit_snapshot(&store, &events);
        });
    }
}

fn persist_source_settings_update(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    source_id: &SourceId,
    saved: &StoredSource,
    identity_changed: bool,
    credential: Option<&str>,
) -> Result<(), String> {
    let previous_token = credential
        .map(|credential| {
            let previous = secrets
                .load_token(source_id.as_str())
                .map_err(|error| error.to_string())?;
            secrets
                .save_token(source_id.as_str(), credential)
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
        Some(token) => secrets.save_token(source_id.as_str(), &token),
        None => secrets.delete_token(source_id.as_str()),
    }
    .map_err(|error| error.to_string())
}

fn source_is_selected(store: &StoreHandle, source_id: &SourceId) -> bool {
    store
        .with_store(|store| Ok(store.active_source()?.map(|saved| saved.source_id)))
        .ok()
        .flatten()
        .as_ref()
        == Some(source_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sources::{CredentialSourceConfig, jellyfin::JellyfinSourceConfig};

    fn saved_server_for_settings() -> StoredSource {
        JellyfinSourceConfig {
            credentials: CredentialSourceConfig {
                source: SourceIdentity {
                    id: SourceId::new("jellyfin:server:settings"),
                    kind: sources::jellyfin::JELLYFIN_SOURCE_ID.to_string(),
                    name: "Old Server".to_string(),
                    base_url: "https://music.example.test".to_string(),
                },
                user_id: "listener-id".to_string(),
                username: "listener".to_string(),
                trust_invalid_cert: false,
            },
            use_instant_mix: false,
        }
        .into_stored()
    }

    fn jellyfin_config(saved: &StoredSource) -> JellyfinSourceConfig {
        JellyfinSourceConfig::from_stored(saved).expect("decode Jellyfin source")
    }

    fn server_settings_input(
        saved: &StoredSource,
        name: &str,
        base_url: &str,
        username: &str,
        password: &str,
        trust_invalid_cert: bool,
        use_jellyfin_instant_mix: bool,
    ) -> JellyfinSettingsInput {
        JellyfinSettingsInput {
            credentials: CredentialSettingsInput {
                source_id: saved.source_id.clone(),
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
        login: impl FnOnce(StoredSource, CredentialHostInput) -> Result<AuthenticatedSource, String>,
    ) -> Result<(bool, bool), String> {
        let saved = store
            .with_store(|store| store.stored_source(&input.credentials.source_id))?
            .ok_or_else(|| "saved source missing".to_string())?;
        let prepared = crate::source_setup::prepare_jellyfin_settings_update_with_login(
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
            &saved.source_id,
            &saved,
            identity_changed,
            credential.as_deref(),
        )?;
        Ok((identity_changed, reauthenticated))
    }

    fn provider_session(
        store: &StoreHandle,
        saved: StoredSource,
        source_id: SourceId,
        base_url: &str,
        user_id: &str,
        username: &str,
        token: &str,
    ) -> AuthenticatedSource {
        let mut config = jellyfin_config(&saved);
        config.credentials.source.base_url = base_url.to_string();
        config.credentials.user_id = user_id.to_string();
        config.credentials.username = username.to_string();
        let saved = config.into_stored();
        let credential = token.to_string();
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        secrets
            .save_token(saved.source_id.as_str(), &credential)
            .expect("save authenticated source token");
        let active = crate::source_setup::activate_configured_source(store, &secrets, &saved)
            .expect("activate authenticated source");
        AuthenticatedSource {
            saved,
            credential,
            active,
            authenticated_source_id: source_id,
        }
    }

    fn seed_source_cache(store: &StoreHandle, saved: &StoredSource) {
        let album = library_album(1, "Example Artist", "Example Album", None);
        let track = library_track(
            1,
            album.artist_id.clone(),
            album.id.clone(),
            &album.artist,
            &[],
        );
        let mut sequence = playback::Sequence::new(saved.source_id.clone());
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
                store.save_source(saved)?;
                let generation = store.begin_sync(&saved.source_id)?;
                commit_cached_library(
                    store,
                    &saved.source_id,
                    generation,
                    CachedLibraryObservation {
                        albums: vec![album],
                        ..CachedLibraryObservation::default()
                    },
                )?;
                store.save_playback_checkpoint(&checkpoint)?;
                Ok(())
            })
            .expect("seed source cache");
    }

    fn saved_source(store: &StoreHandle, source_id: &SourceId) -> StoredSource {
        store
            .with_store(|store| {
                Ok(store
                    .list_sources()?
                    .into_iter()
                    .find(|saved| saved.source_id == *source_id))
            })
            .expect("load saved server")
            .expect("saved server")
    }

    fn cached_album_count(store: &StoreHandle, source_id: &SourceId) -> usize {
        store
            .with_store(|store| store.load_albums(source_id, 0, 1).map(|page| page.total))
            .expect("load albums")
    }

    fn playback_checkpoint_saved(store: &StoreHandle, source_id: &SourceId) -> bool {
        store
            .with_store(|store| store.load_playback_checkpoint(source_id))
            .expect("load playback checkpoint")
            .is_some()
    }

    #[test]
    fn name_server_edit() {
        let store = StoreHandle::open_memory().expect("memory store");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let saved = saved_server_for_settings();
        let saved_config = jellyfin_config(&saved);
        seed_source_cache(&store, &saved);
        secrets
            .save_token(saved.source_id.as_str(), "old-token")
            .expect("save token");

        let (identity_changed, reauthenticated) = update_jellyfin_settings_with_login(
            &store,
            &secrets,
            server_settings_input(
                &saved,
                "Renamed Server",
                &saved_config.credentials.source.base_url,
                &saved_config.credentials.username,
                "",
                true,
                false,
            ),
            |_, _| panic!("name-only edit should not reauthenticate"),
        )
        .expect("update settings");

        assert!(!identity_changed);
        assert!(!reauthenticated);
        let edited = saved_source(&store, &saved.source_id);
        let edited_config = jellyfin_config(&edited);
        assert_eq!(edited.name, "Renamed Server");
        assert_eq!(
            edited_config.credentials.source.base_url,
            saved_config.credentials.source.base_url
        );
        assert_eq!(
            edited_config.credentials.username,
            saved_config.credentials.username
        );
        assert!(edited_config.credentials.trust_invalid_cert);
        assert_eq!(
            secrets
                .load_token(saved.source_id.as_str())
                .expect("load token"),
            Some("old-token".to_string())
        );
        assert_eq!(cached_album_count(&store, &saved.source_id), 1);
        assert!(playback_checkpoint_saved(&store, &saved.source_id));
    }

    #[test]
    fn server_auth_identity() {
        let store = StoreHandle::open_memory().expect("memory store");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let saved = saved_server_for_settings();
        let saved_config = jellyfin_config(&saved);
        seed_source_cache(&store, &saved);
        secrets
            .save_token(saved.source_id.as_str(), "old-token")
            .expect("save token");

        let (identity_changed, reauthenticated) = update_jellyfin_settings_with_login(
            &store,
            &secrets,
            server_settings_input(
                &saved,
                &saved.name,
                "https://music-lan.example.test",
                &saved_config.credentials.username,
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
                    saved.source_id.clone(),
                    "https://music-lan.example.test",
                    &saved_config.credentials.user_id,
                    &saved_config.credentials.username,
                    "new-token",
                ))
            },
        )
        .expect("update settings");

        assert!(!identity_changed);
        assert!(reauthenticated);
        let edited = saved_source(&store, &saved.source_id);
        let edited_config = jellyfin_config(&edited);
        assert_eq!(
            edited_config.credentials.source.base_url,
            "https://music-lan.example.test"
        );
        assert_eq!(
            edited_config.credentials.user_id,
            saved_config.credentials.user_id
        );
        assert_eq!(
            secrets
                .load_token(saved.source_id.as_str())
                .expect("load token"),
            Some("new-token".to_string())
        );
        assert_eq!(cached_album_count(&store, &saved.source_id), 1);
        assert!(playback_checkpoint_saved(&store, &saved.source_id));
    }

    #[test]
    fn auth_sensitive_server() {
        let store = StoreHandle::open_memory().expect("memory store");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let saved = saved_server_for_settings();
        let saved_config = jellyfin_config(&saved);
        seed_source_cache(&store, &saved);
        secrets
            .save_token(saved.source_id.as_str(), "old-token")
            .expect("save token");

        let error = update_jellyfin_settings_with_login(
            &store,
            &secrets,
            server_settings_input(
                &saved,
                "Renamed Server",
                &saved_config.credentials.source.base_url,
                "alternate",
                "updated-password",
                false,
                false,
            ),
            |_, _| Err("Authentication failed".to_string()),
        )
        .expect_err("auth failure");

        assert_eq!(error, "Authentication failed");
        let current = saved_source(&store, &saved.source_id);
        assert_eq!(current, saved);
        assert_eq!(
            secrets
                .load_token(saved.source_id.as_str())
                .expect("load token"),
            Some("old-token".to_string())
        );
        assert_eq!(cached_album_count(&store, &saved.source_id), 1);
        assert!(playback_checkpoint_saved(&store, &saved.source_id));
    }

    #[test]
    fn server_change_identity() {
        let store = StoreHandle::open_memory().expect("memory store");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let saved = saved_server_for_settings();
        let saved_config = jellyfin_config(&saved);
        seed_source_cache(&store, &saved);
        secrets
            .save_token(saved.source_id.as_str(), "old-token")
            .expect("save token");

        let (identity_changed, reauthenticated) = update_jellyfin_settings_with_login(
            &store,
            &secrets,
            server_settings_input(
                &saved,
                &saved.name,
                &saved_config.credentials.source.base_url,
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
                    &saved_config.credentials.source.base_url,
                    "alternate-id",
                    "alternate",
                    "new-token",
                ))
            },
        )
        .expect("update settings");

        assert!(identity_changed);
        assert!(reauthenticated);
        let edited = saved_source(&store, &saved.source_id);
        let edited_config = jellyfin_config(&edited);
        assert_eq!(edited.source_id, saved.source_id);
        assert_eq!(edited_config.credentials.user_id, "alternate-id");
        assert_eq!(edited_config.credentials.username, "alternate");
        assert_eq!(
            secrets
                .load_token(saved.source_id.as_str())
                .expect("load token"),
            Some("new-token".to_string())
        );
        assert_eq!(cached_album_count(&store, &saved.source_id), 0);
        assert!(!playback_checkpoint_saved(&store, &saved.source_id));
    }
}
