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
        let mut settings = self.settings.load_settings();
        if settings.secret_storage_mode == mode {
            return Ok(settings);
        }

        if settings.secret_storage_mode == SecretStorageMode::ConfigFile {
            let saved_servers = self.store.with_store(|store| store.list_servers())?;
            delete_current_secrets(&self.secrets, &saved_servers)?;
        }
        settings.secret_storage_mode = mode;
        settings.secret_scope_id = new_secret_scope_id();
        clear_scrobbling_secret_fields(&mut settings);
        settings.migrate_defaults();
        self.store.save_settings(&settings)?;
        self.secret_switch
            .replace(platform_secret_store(&settings))
            .map_err(|error| error.to_string())?;
        self.reload_snapshot();
        Ok(settings)
    }
}

fn clear_scrobbling_secret_fields(settings: &mut AppSettings) {
    settings.scrobbling.lastfm.api_secret.clear();
    settings.scrobbling.lastfm.session_key.clear();
    settings.scrobbling.librefm.session_key.clear();
    settings.scrobbling.listenbrainz.user_token.clear();
}

fn delete_current_secrets(
    secrets: &Arc<dyn SecretStore>,
    saved_servers: &[SavedServer],
) -> Result<(), String> {
    for saved in saved_servers {
        secrets
            .delete_token(&saved.server.id)
            .map_err(|error| error.to_string())?;
    }

    for key in [
        SecretKey::LastFmApiSecret,
        SecretKey::LastFmSession,
        SecretKey::LibreFmSession,
        SecretKey::ListenBrainzToken,
    ] {
        secrets
            .delete_secret(&key)
            .map_err(|error| error.to_string())?;
    }

    Ok(())
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
        let first = saved_server_with_id("server:first");
        let second = saved_server_with_id("server:second");
        let secrets = Arc::new(MemorySecretStore::new());
        secrets
            .save_token(&first.server.id, "first-token")
            .expect("save first token");
        secrets
            .save_token(&second.server.id, "second-token")
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
        delete_current_secrets(&backend, &[first.clone(), second.clone()])
            .expect("delete current secrets");

        assert_eq!(
            secrets.load_token(&first.server.id).expect("load first"),
            None
        );
        assert_eq!(
            secrets.load_token(&second.server.id).expect("load second"),
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
    fn legacy_backend_change_keeps_mode_when_delete_fails() {
        let (controller, _events, _snapshot, _queue, _player) =
            AppController::bootstrap_memory_for_test();
        let mut settings = controller.load_settings();
        settings.secret_storage_mode = SecretStorageMode::ConfigFile;
        controller
            .save_settings(&settings)
            .expect("save legacy settings");
        let failing: Arc<dyn SecretStore> = Arc::new(DeleteFailingSecretStore);
        controller
            .secret_switch
            .replace(failing)
            .expect("replace backend");

        let error = controller
            .set_secret_storage_mode(SecretStorageMode::SystemKeyring)
            .expect_err("backend switch should fail");

        assert!(error.contains("delete failed"));
        assert_eq!(
            controller.load_settings().secret_storage_mode,
            SecretStorageMode::ConfigFile
        );
    }

    #[test]
    fn secure_backend_change_skips_keyring_delete_failure() {
        let (controller, _events, _snapshot, _queue, _player) =
            AppController::bootstrap_memory_for_test();
        let mut settings = controller.load_settings();
        settings.secret_storage_mode = SecretStorageMode::SystemKeyring;
        controller
            .save_settings(&settings)
            .expect("save secure settings");
        let failing: Arc<dyn SecretStore> = Arc::new(DeleteFailingSecretStore);
        controller
            .secret_switch
            .replace(failing)
            .expect("replace backend");

        controller
            .set_secret_storage_mode(SecretStorageMode::ConfigFile)
            .expect("switch from secure storage");

        assert_eq!(
            controller.load_settings().secret_storage_mode,
            SecretStorageMode::ConfigFile
        );
    }

    fn saved_server_with_id(id: &str) -> SavedServer {
        SavedServer {
            server: ServerIdentity {
                id: ServerId::new(id),
                provider: "test".to_string(),
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
