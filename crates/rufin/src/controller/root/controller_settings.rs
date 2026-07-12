use super::*;

impl AppController {
    pub fn load_settings(&self) -> StoredSettings {
        self.settings.load_settings()
    }
    pub fn load_settings_with_scrobbling_secrets(&self) -> StoredSettings {
        self.settings.load_settings_with_scrobbling_secrets()
    }
    pub fn save_settings(&self, settings: &StoredSettings) -> Result<(), String> {
        let previous = self.settings.load_settings();
        let committed = self.settings.save_settings(settings)?;
        self.invalidate_changed_lyrics_requests(&previous, &committed);
        if let Some(product) = self.playback_product_if_present() {
            product.update_runtime_settings(&committed)?;
        }
        Ok(())
    }
    pub fn save_settings_with_scrobbling_deletes(
        &self,
        settings: &StoredSettings,
    ) -> Result<(), String> {
        let previous = self.settings.load_settings();
        let committed = self
            .settings
            .save_settings_with_scrobbling_deletes(settings)?;
        self.invalidate_changed_lyrics_requests(&previous, &committed);
        if let Some(product) = self.playback_product_if_present() {
            product.update_runtime_settings(&committed)?;
        }
        Ok(())
    }

    fn invalidate_changed_lyrics_requests(
        &self,
        previous: &StoredSettings,
        current: &StoredSettings,
    ) {
        let previous_metadata = &previous.metadata;
        let current_metadata = &current.metadata;
        if previous_metadata.external_lyrics_enabled != current_metadata.external_lyrics_enabled
            || previous_metadata.external_lyrics_providers
                != current_metadata.external_lyrics_providers
            || previous_metadata.prefer_server_lyrics != current_metadata.prefer_server_lyrics
            || previous_metadata.lyrics_provider_settings_version
                != current_metadata.lyrics_provider_settings_version
            || previous_metadata.suppressed_auto_lyrics_track_ids
                != current_metadata.suppressed_auto_lyrics_track_ids
            || previous.private_mode != current.private_mode
        {
            self.lyrics_request_generation
                .fetch_add(1, Ordering::AcqRel);
        }
    }
    pub fn reload_snapshot(&self) {
        let store = self.store.clone();
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        thread::spawn(move || emit_runtime_snapshot(&store, &secrets, &events));
    }

    pub fn set_secret_storage_mode(
        &self,
        mode: SecretStorageMode,
    ) -> Result<StoredSettings, String> {
        let transition_generation = self.source_transitions.begin();
        let transition_commit = self
            .source_transitions
            .commit(transition_generation)?
            .ok_or_else(|| "A newer source transition replaced this request.".to_string())?;
        let previous = self.settings.load_settings();
        if previous.secret_storage_mode == mode {
            return Ok(previous);
        }

        let saved_sources = self.store.with_store(|store| store.list_sources())?;
        for saved in &saved_sources {
            self.forget_source_sync(&saved.source_id);
        }
        let mut active = self
            .active_source
            .write()
            .map_err(|_| "active source lock was poisoned".to_string())?;
        let settings = self.store.update_settings(|settings| {
            settings.secret_storage_mode = mode;
            settings.secret_scope_id = new_secret_scope_id();
            clear_scrobbling_secret_fields(settings);
            settings.migrate_defaults();
            Ok(settings.clone())
        })?;
        let previous_secrets = self.secret_switch.replace(platform_secret_store(&settings));
        *active = None;
        drop(active);
        self.clear_playback_product();
        delete_current_secrets(&previous_secrets, &saved_sources);
        emit_runtime_snapshot(&self.store, &self.secrets, &self.events);
        self.refresh_source_freshness();
        drop(transition_commit);
        Ok(settings)
    }
}

fn clear_scrobbling_secret_fields(settings: &mut StoredSettings) {
    for descriptor in scrobbling::secret_descriptors() {
        descriptor.value_mut(&mut settings.scrobbling).clear();
    }
}

fn delete_current_secrets(secrets: &Arc<dyn SecretStore>, saved_sources: &[StoredSource]) {
    for saved in saved_sources {
        if let Err(error) = secrets.delete_token(saved.source_id.as_str()) {
            warn!(%error, source_id = %saved.source_id, "failed to remove token from previous secret backend");
        }
    }

    for descriptor in scrobbling::secret_descriptors() {
        let key = settings_controller::scrobbling_secret_key(*descriptor);
        if let Err(error) = secrets.delete_secret(&key) {
            warn!(%error, ?key, "failed to remove API secret from previous secret backend");
        }
    }
}

fn new_secret_scope_id() -> String {
    use std::fmt::Write as _;

    let mut bytes = [0_u8; 16];
    if getrandom::fill(&mut bytes).is_ok() {
        let mut value = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            let _ = write!(&mut value, "{byte:02x}");
        }
        return value;
    }

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use scrobbling::{LASTFM_API_SECRET, LASTFM_SESSION, LIBREFM_SESSION, LISTENBRAINZ_TOKEN};
    use secrets::{SecretError, SecretResult};
    use sources::{CredentialSourceConfig, jellyfin::JellyfinSourceConfig};

    struct DeleteFailingSecretStore;

    impl SecretStore for DeleteFailingSecretStore {
        fn save_secret(&self, _key: &SecretKey, _secret: &str) -> SecretResult<()> {
            Ok(())
        }

        fn load_secret(&self, _key: &SecretKey) -> SecretResult<Option<String>> {
            Ok(None)
        }

        fn delete_secret(&self, _key: &SecretKey) -> SecretResult<()> {
            Err(SecretError::Backend("delete failed".to_string()))
        }
    }

    #[test]
    fn secret_backend_change_deletes_current_secrets() {
        let first = saved_source_with_id("server:first");
        let second = saved_source_with_id("server:second");
        let secrets = Arc::new(MemorySecretStore::new());
        secrets
            .save_token(first.source_id.as_str(), "first-token")
            .expect("save first token");
        secrets
            .save_token(second.source_id.as_str(), "second-token")
            .expect("save second token");
        secrets
            .save_secret(
                &settings_controller::scrobbling_secret_key(LASTFM_API_SECRET),
                "lastfm-api-secret",
            )
            .expect("save lastfm api secret");
        secrets
            .save_secret(
                &settings_controller::scrobbling_secret_key(LASTFM_SESSION),
                "lastfm-session",
            )
            .expect("save lastfm session");
        secrets
            .save_secret(
                &settings_controller::scrobbling_secret_key(LIBREFM_SESSION),
                "librefm-session",
            )
            .expect("save librefm session");
        secrets
            .save_secret(
                &settings_controller::scrobbling_secret_key(LISTENBRAINZ_TOKEN),
                "listenbrainz-token",
            )
            .expect("save listenbrainz token");

        let backend: Arc<dyn SecretStore> = Arc::<MemorySecretStore>::clone(&secrets);
        delete_current_secrets(&backend, &[first.clone(), second.clone()]);

        assert_eq!(
            secrets
                .load_token(first.source_id.as_str())
                .expect("load first"),
            None
        );
        assert_eq!(
            secrets
                .load_token(second.source_id.as_str())
                .expect("load second"),
            None
        );
        assert_eq!(
            secrets
                .load_secret(&settings_controller::scrobbling_secret_key(
                    LASTFM_API_SECRET,
                ))
                .expect("load lastfm api secret"),
            None
        );
        assert_eq!(
            secrets
                .load_secret(&settings_controller::scrobbling_secret_key(LASTFM_SESSION))
                .expect("load lastfm session"),
            None
        );
        assert_eq!(
            secrets
                .load_secret(&settings_controller::scrobbling_secret_key(LIBREFM_SESSION))
                .expect("load librefm session"),
            None
        );
        assert_eq!(
            secrets
                .load_secret(&settings_controller::scrobbling_secret_key(
                    LISTENBRAINZ_TOKEN,
                ))
                .expect("load listenbrainz token"),
            None
        );
    }

    #[test]
    fn backend_change_completes_when_previous_cleanup_fails() {
        let (controller, _events, _snapshot, _playback) =
            AppController::bootstrap_memory_for_test();
        controller
            .store
            .update_settings(|settings| {
                settings.secret_storage_mode = SecretStorageMode::ConfigFile;
                Ok(())
            })
            .expect("save legacy settings");
        let failing: Arc<dyn SecretStore> = Arc::new(DeleteFailingSecretStore);
        let _previous = controller.secret_switch.replace(failing);

        controller
            .set_secret_storage_mode(SecretStorageMode::SystemKeyring)
            .expect("switch backend");

        assert_eq!(
            controller.load_settings().secret_storage_mode,
            SecretStorageMode::SystemKeyring
        );
    }

    fn saved_source_with_id(id: &str) -> StoredSource {
        JellyfinSourceConfig {
            credentials: CredentialSourceConfig {
                source: SourceIdentity {
                    id: SourceId::new(id),
                    kind: sources::jellyfin::JELLYFIN_SOURCE_ID.to_string(),
                    name: "Test".to_string(),
                    base_url: "https://example.invalid".to_string(),
                },
                user_id: "user".to_string(),
                username: "user".to_string(),
                trust_invalid_cert: false,
            },
            use_instant_mix: false,
        }
        .into_stored()
    }
}
