use super::*;

impl AppController {
    pub fn load_settings(&self) -> AppSettings {
        self.settings.load_settings()
    }
    pub fn load_settings_with_scrobbling_secrets(&self) -> AppSettings {
        self.settings.load_settings_with_scrobbling_secrets()
    }
    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), String> {
        self.settings.save_settings(settings)
    }
    pub fn save_settings_with_scrobbling_deletes(
        &self,
        settings: &AppSettings,
    ) -> Result<(), String> {
        self.settings
            .save_settings_with_scrobbling_deletes(settings)
    }
    pub fn reload_snapshot(&self) {
        let store = self.store.clone();
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        thread::spawn(move || emit_runtime_snapshot(&store, &secrets, &events));
    }

    pub fn set_secret_storage_mode(&self, mode: SecretStorageMode) -> Result<AppSettings, String> {
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
            self.forget_source_sync(&saved.source.id);
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
        clear_queue_and_stop_playback(
            &self.queue,
            &self.playback_request_generation,
            &self.next_preload,
            &self.playback,
            &self.playback_snapshot,
            &self.auto_dj_enabled,
            &self.events,
        );
        delete_current_secrets(&previous_secrets, &saved_sources);
        emit_runtime_snapshot(&self.store, &self.secrets, &self.events);
        self.refresh_source_freshness();
        drop(transition_commit);
        Ok(settings)
    }
}

fn clear_scrobbling_secret_fields(settings: &mut AppSettings) {
    settings.scrobbling.lastfm.api_secret.clear();
    settings.scrobbling.lastfm.session_key.clear();
    settings.scrobbling.librefm.session_key.clear();
    settings.scrobbling.listenbrainz.user_token.clear();
}

fn delete_current_secrets(secrets: &Arc<dyn SecretStore>, saved_sources: &[SavedSource]) {
    for saved in saved_sources {
        if let Err(error) = secrets.delete_token(&saved.source.id) {
            warn!(%error, source_id = %saved.source.id, "failed to remove token from previous secret backend");
        }
    }

    for key in [
        SecretKey::LastFmApiSecret,
        SecretKey::LastFmSession,
        SecretKey::LibreFmSession,
        SecretKey::ListenBrainzToken,
    ] {
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
    use secrets::{SecretError, SecretResult};

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
            .save_token(&first.source.id, "first-token")
            .expect("save first token");
        secrets
            .save_token(&second.source.id, "second-token")
            .expect("save second token");
        secrets
            .save_secret(&SecretKey::LastFmApiSecret, "lastfm-api-secret")
            .expect("save lastfm api secret");
        secrets
            .save_secret(&SecretKey::LastFmSession, "lastfm-session")
            .expect("save lastfm session");
        secrets
            .save_secret(&SecretKey::LibreFmSession, "librefm-session")
            .expect("save librefm session");
        secrets
            .save_secret(&SecretKey::ListenBrainzToken, "listenbrainz-token")
            .expect("save listenbrainz token");

        let backend: Arc<dyn SecretStore> = Arc::<MemorySecretStore>::clone(&secrets);
        delete_current_secrets(&backend, &[first.clone(), second.clone()]);

        assert_eq!(
            secrets.load_token(&first.source.id).expect("load first"),
            None
        );
        assert_eq!(
            secrets.load_token(&second.source.id).expect("load second"),
            None
        );
        assert_eq!(
            secrets
                .load_secret(&SecretKey::LastFmApiSecret)
                .expect("load lastfm api secret"),
            None
        );
        assert_eq!(
            secrets
                .load_secret(&SecretKey::LastFmSession)
                .expect("load lastfm session"),
            None
        );
        assert_eq!(
            secrets
                .load_secret(&SecretKey::LibreFmSession)
                .expect("load librefm session"),
            None
        );
        assert_eq!(
            secrets
                .load_secret(&SecretKey::ListenBrainzToken)
                .expect("load listenbrainz token"),
            None
        );
    }

    #[test]
    fn backend_change_completes_when_previous_cleanup_fails() {
        let (controller, _events, _snapshot, _queue, _player) =
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

    fn saved_source_with_id(id: &str) -> SavedSource {
        SavedSource {
            source: SourceIdentity {
                id: SourceId::new(id),
                kind: "test".to_string(),
                name: "Test".to_string(),
                base_url: "https://example.invalid".to_string(),
            },
            user_id: "user".to_string(),
            username: "user".to_string(),
            trust_invalid_cert: false,
            use_jellyfin_instant_mix: false,
        }
    }
}
