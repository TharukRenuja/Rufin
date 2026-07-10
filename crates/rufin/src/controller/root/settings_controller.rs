use super::*;

#[derive(Clone)]
pub(in crate::controller) struct SettingsController {
    store: StoreHandle,
    secrets: Arc<dyn SecretStore>,
}

impl SettingsController {
    pub(in crate::controller) fn new(store: StoreHandle, secrets: Arc<dyn SecretStore>) -> Self {
        Self { store, secrets }
    }

    pub(in crate::controller) fn load_settings(&self) -> AppSettings {
        load_settings_from_store(&self.store)
    }

    pub(in crate::controller) fn load_settings_with_scrobbling_secrets(&self) -> AppSettings {
        load_settings_with_secrets(&self.store, &self.secrets)
    }

    pub(in crate::controller) fn save_settings(
        &self,
        settings: &AppSettings,
    ) -> Result<(), String> {
        save_settings_preserving_scrobbling_secrets(&self.store, &self.secrets, settings)
    }

    pub(in crate::controller) fn save_settings_with_scrobbling_deletes(
        &self,
        settings: &AppSettings,
    ) -> Result<(), String> {
        save_settings_with_scrobbling_deletes(&self.store, &self.secrets, settings)
    }
}

pub(in crate::controller) fn load_settings_with_secrets(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
) -> AppSettings {
    let mut settings = load_settings_from_store(store);
    let mut persisted = settings.clone();
    let migrated = persist_scrobbling_secret_values(
        secrets,
        &settings,
        &mut persisted,
        MissingSecretAction::Keep,
    )
    .unwrap_or(false);
    hydrate_scrobbling_secret_values(secrets, &mut settings);
    if migrated {
        persisted.migrate_defaults();
        if let Err(error) = save_non_source_settings(store, persisted) {
            warn!(%error, "failed to clear migrated scrobbling secrets from settings");
        }
    }
    settings
}

pub(in crate::controller) fn save_settings_preserving_scrobbling_secrets(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    settings: &AppSettings,
) -> Result<(), String> {
    save_settings_with_missing_secret_action(store, secrets, settings, MissingSecretAction::Keep)
}

pub(in crate::controller) fn save_settings_with_scrobbling_deletes(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    settings: &AppSettings,
) -> Result<(), String> {
    save_settings_with_missing_secret_action(store, secrets, settings, MissingSecretAction::Delete)
}

fn save_settings_with_missing_secret_action(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    settings: &AppSettings,
    missing_action: MissingSecretAction,
) -> Result<(), String> {
    let mut persisted = settings.clone();
    persist_scrobbling_secret_values(secrets, settings, &mut persisted, missing_action)?;
    persisted.migrate_defaults();
    save_non_source_settings(store, persisted)
}

fn save_non_source_settings(store: &StoreHandle, mut persisted: AppSettings) -> Result<(), String> {
    store.update_settings(move |current| {
        persisted.sources = current.sources.clone();
        persisted.jellyfin_device_id = current.jellyfin_device_id.clone();
        persisted.secret_storage_mode = current.secret_storage_mode;
        persisted.secret_scope_id = current.secret_scope_id.clone();
        *current = persisted;
        Ok(())
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MissingSecretAction {
    Keep,
    Delete,
}

fn persist_scrobbling_secret_values(
    secrets: &Arc<dyn SecretStore>,
    settings: &AppSettings,
    persisted: &mut AppSettings,
    missing_action: MissingSecretAction,
) -> Result<bool, String> {
    let mut migrated = false;
    persist_secret_value(
        secrets,
        &SecretKey::LastFmApiSecret,
        &settings.scrobbling.lastfm.api_secret,
        &mut persisted.scrobbling.lastfm.api_secret,
        missing_action,
        &mut migrated,
    )?;
    persist_secret_value(
        secrets,
        &SecretKey::LastFmSession,
        &settings.scrobbling.lastfm.session_key,
        &mut persisted.scrobbling.lastfm.session_key,
        missing_action,
        &mut migrated,
    )?;
    persist_secret_value(
        secrets,
        &SecretKey::LibreFmSession,
        &settings.scrobbling.librefm.session_key,
        &mut persisted.scrobbling.librefm.session_key,
        missing_action,
        &mut migrated,
    )?;
    persist_secret_value(
        secrets,
        &SecretKey::ListenBrainzToken,
        &settings.scrobbling.listenbrainz.user_token,
        &mut persisted.scrobbling.listenbrainz.user_token,
        missing_action,
        &mut migrated,
    )?;
    Ok(migrated)
}

fn persist_secret_value(
    secrets: &Arc<dyn SecretStore>,
    key: &SecretKey,
    value: &str,
    persisted_value: &mut String,
    missing_action: MissingSecretAction,
    migrated: &mut bool,
) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        if missing_action == MissingSecretAction::Delete
            && let Err(error) = secrets.delete_secret(key)
        {
            warn!(%error, ?key, "failed to delete scrobbling secret");
        }
        persisted_value.clear();
        return Ok(());
    }

    match secrets.save_secret(key, value) {
        Ok(()) => {
            persisted_value.clear();
            *migrated = true;
            Ok(())
        }
        Err(error) => {
            warn!(%error, ?key, "failed to save scrobbling secret");
            if missing_action == MissingSecretAction::Delete {
                return Err(format!("failed to save scrobbling secret: {error}"));
            }
            Ok(())
        }
    }
}

fn hydrate_scrobbling_secret_values(secrets: &Arc<dyn SecretStore>, settings: &mut AppSettings) {
    hydrate_secret_value(
        secrets,
        &SecretKey::LastFmApiSecret,
        &mut settings.scrobbling.lastfm.api_secret,
    );
    hydrate_secret_value(
        secrets,
        &SecretKey::LastFmSession,
        &mut settings.scrobbling.lastfm.session_key,
    );
    hydrate_secret_value(
        secrets,
        &SecretKey::LibreFmSession,
        &mut settings.scrobbling.librefm.session_key,
    );
    hydrate_secret_value(
        secrets,
        &SecretKey::ListenBrainzToken,
        &mut settings.scrobbling.listenbrainz.user_token,
    );
    settings.migrate_defaults();
}

fn hydrate_secret_value(secrets: &Arc<dyn SecretStore>, key: &SecretKey, value: &mut String) {
    if !value.trim().is_empty() {
        return;
    }
    match secrets.load_secret(key) {
        Ok(Some(secret)) => *value = secret,
        Ok(None) => {}
        Err(error) => warn!(%error, ?key, "failed to load scrobbling secret"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        AudioscrobblerScrobbleSettings, ListenBrainzScrobbleSettings, ScrobblingSettings,
    };
    use secrets::{SecretError, SecretResult};
    use std::sync::atomic::{AtomicBool, Ordering};

    struct UnexpectedSecretStore {
        touched: Arc<AtomicBool>,
    }

    impl SecretStore for UnexpectedSecretStore {
        fn save_secret(&self, _key: &SecretKey, _secret: &str) -> SecretResult<()> {
            self.touched.store(true, Ordering::Relaxed);
            Err(SecretError::Backend("unexpected secret save".to_string()))
        }

        fn load_secret(&self, _key: &SecretKey) -> SecretResult<Option<String>> {
            self.touched.store(true, Ordering::Relaxed);
            Err(SecretError::Backend("unexpected secret load".to_string()))
        }

        fn delete_secret(&self, _key: &SecretKey) -> SecretResult<()> {
            self.touched.store(true, Ordering::Relaxed);
            Err(SecretError::Backend("unexpected secret delete".to_string()))
        }
    }

    struct FailingSecretStore;

    impl SecretStore for FailingSecretStore {
        fn save_secret(&self, _key: &SecretKey, _secret: &str) -> SecretResult<()> {
            Err(SecretError::Backend("unavailable".to_string()))
        }

        fn load_secret(&self, _key: &SecretKey) -> SecretResult<Option<String>> {
            Ok(None)
        }

        fn delete_secret(&self, _key: &SecretKey) -> SecretResult<()> {
            Ok(())
        }
    }

    #[test]
    fn settings_load_skips_scrobbling_secret_store() {
        let store = StoreHandle::open_memory().expect("memory store");
        let touched = Arc::new(AtomicBool::new(false));
        let secrets: Arc<dyn SecretStore> = Arc::new(UnexpectedSecretStore {
            touched: Arc::clone(&touched),
        });
        let controller =
            SettingsController::new(store.clone(), Arc::<dyn SecretStore>::clone(&secrets));
        let settings = AppSettings {
            scrobbling: ScrobblingSettings {
                lastfm: AudioscrobblerScrobbleSettings {
                    api_key: "lastfm-key".to_string(),
                    api_secret: "lastfm-secret".to_string(),
                    session_key: "lastfm-session".to_string(),
                    ..AudioscrobblerScrobbleSettings::default()
                },
                listenbrainz: ListenBrainzScrobbleSettings {
                    user_token: "listenbrainz-token".to_string(),
                    ..ListenBrainzScrobbleSettings::default()
                },
                ..ScrobblingSettings::default()
            },
            ..AppSettings::default()
        };
        store.save_settings(&settings).expect("save settings");

        let loaded = controller.load_settings();

        assert_eq!(loaded.scrobbling.lastfm.api_secret, "lastfm-secret");
        assert_eq!(loaded.scrobbling.lastfm.session_key, "lastfm-session");
        assert_eq!(
            loaded.scrobbling.listenbrainz.user_token,
            "listenbrainz-token"
        );
        let persisted = store.load_settings();
        assert_eq!(persisted.scrobbling.lastfm.api_secret, "lastfm-secret");
        assert_eq!(persisted.scrobbling.lastfm.session_key, "lastfm-session");
        assert_eq!(
            persisted.scrobbling.listenbrainz.user_token,
            "listenbrainz-token"
        );
        assert!(!touched.load(Ordering::Relaxed));
    }

    #[test]
    fn settings_load_scrobbling_secrets_migrates_legacy_values() {
        let store = StoreHandle::open_memory().expect("memory store");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let controller =
            SettingsController::new(store.clone(), Arc::<dyn SecretStore>::clone(&secrets));
        let settings = AppSettings {
            scrobbling: ScrobblingSettings {
                lastfm: AudioscrobblerScrobbleSettings {
                    api_key: "lastfm-key".to_string(),
                    api_secret: "lastfm-secret".to_string(),
                    session_key: "lastfm-session".to_string(),
                    ..AudioscrobblerScrobbleSettings::default()
                },
                listenbrainz: ListenBrainzScrobbleSettings {
                    user_token: "listenbrainz-token".to_string(),
                    ..ListenBrainzScrobbleSettings::default()
                },
                ..ScrobblingSettings::default()
            },
            ..AppSettings::default()
        };
        store.save_settings(&settings).expect("save settings");

        let loaded = controller.load_settings_with_scrobbling_secrets();

        assert_eq!(loaded.scrobbling.lastfm.api_secret, "lastfm-secret");
        assert_eq!(loaded.scrobbling.lastfm.session_key, "lastfm-session");
        assert_eq!(
            loaded.scrobbling.listenbrainz.user_token,
            "listenbrainz-token"
        );
        assert_eq!(
            secrets
                .load_secret(&SecretKey::LastFmSession)
                .expect("load lastfm session"),
            Some("lastfm-session".to_string())
        );
        let persisted = store.load_settings();
        assert_eq!(persisted.scrobbling.lastfm.api_secret, "");
        assert_eq!(persisted.scrobbling.lastfm.session_key, "");
        assert_eq!(persisted.scrobbling.listenbrainz.user_token, "");
    }

    #[test]
    fn settings_persist_secret() {
        let store = StoreHandle::open_memory().expect("memory store");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let controller =
            SettingsController::new(store.clone(), Arc::<dyn SecretStore>::clone(&secrets));
        let settings = AppSettings {
            lastfm_api_key: "cover-key".to_string(),
            scrobbling: ScrobblingSettings {
                lastfm: AudioscrobblerScrobbleSettings {
                    api_key: "scrobble-key".to_string(),
                    api_secret: "lastfm-secret".to_string(),
                    session_key: "lastfm-session".to_string(),
                    ..AudioscrobblerScrobbleSettings::default()
                },
                librefm: AudioscrobblerScrobbleSettings {
                    session_key: "librefm-session".to_string(),
                    ..AudioscrobblerScrobbleSettings::default()
                },
                listenbrainz: ListenBrainzScrobbleSettings {
                    user_token: "listenbrainz-token".to_string(),
                    ..ListenBrainzScrobbleSettings::default()
                },
            },
            ..AppSettings::default()
        };

        controller.save_settings(&settings).expect("save settings");

        let persisted = store.load_settings();
        assert_eq!(persisted.lastfm_api_key, "cover-key");
        assert_eq!(persisted.scrobbling.lastfm.api_key, "cover-key");
        assert_eq!(persisted.scrobbling.lastfm.api_secret, "");
        assert_eq!(persisted.scrobbling.lastfm.session_key, "");
        assert_eq!(persisted.scrobbling.librefm.session_key, "");
        assert_eq!(persisted.scrobbling.listenbrainz.user_token, "");

        let loaded = controller.load_settings_with_scrobbling_secrets();
        assert_eq!(loaded.scrobbling.lastfm.api_secret, "lastfm-secret");
        assert_eq!(loaded.scrobbling.lastfm.session_key, "lastfm-session");
        assert_eq!(loaded.scrobbling.librefm.session_key, "librefm-session");
        assert_eq!(
            loaded.scrobbling.listenbrainz.user_token,
            "listenbrainz-token"
        );
    }

    #[test]
    fn settings_store_fails() {
        let store = StoreHandle::open_memory().expect("memory store");
        let secrets: Arc<dyn SecretStore> = Arc::new(FailingSecretStore);
        let controller = SettingsController::new(store.clone(), secrets);
        let settings = AppSettings {
            scrobbling: ScrobblingSettings {
                lastfm: AudioscrobblerScrobbleSettings {
                    session_key: "lastfm-session".to_string(),
                    ..AudioscrobblerScrobbleSettings::default()
                },
                ..ScrobblingSettings::default()
            },
            ..AppSettings::default()
        };

        let error = controller
            .save_settings_with_scrobbling_deletes(&settings)
            .expect_err("secret save failure");

        assert!(error.contains("failed to save scrobbling secret"));
        let persisted = store.load_settings();
        assert_eq!(persisted.scrobbling.lastfm.session_key, "");
    }

    #[test]
    fn settings_save_preserves_missing_scrobbling_secret() {
        let store = StoreHandle::open_memory().expect("memory store");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let controller =
            SettingsController::new(store.clone(), Arc::<dyn SecretStore>::clone(&secrets));
        secrets
            .save_secret(&SecretKey::LastFmSession, "lastfm-session")
            .expect("seed secret");
        let settings = AppSettings {
            language: "en".to_string(),
            ..AppSettings::default()
        };

        controller.save_settings(&settings).expect("save settings");

        assert_eq!(
            secrets
                .load_secret(&SecretKey::LastFmSession)
                .expect("load secret"),
            Some("lastfm-session".to_string())
        );
    }

    #[test]
    fn general_settings_save_preserves_source_and_credential_owners() {
        let store = StoreHandle::open_memory().expect("memory store");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let controller = SettingsController::new(store.clone(), secrets);
        let mut current = AppSettings::default();
        current.sources.selected = Some(LibrarySourceSelection::Local);
        current.sources.local_folders = vec![LocalLibraryFolder {
            path: "/music".to_string(),
        }];
        current.jellyfin_device_id = "rufin-current".to_string();
        current.secret_storage_mode = SecretStorageMode::ConfigFile;
        current.secret_scope_id = "current-scope".to_string();
        store.save_settings(&current).expect("seed settings");

        let stale = AppSettings {
            language: "tr".to_string(),
            jellyfin_device_id: "rufin-stale".to_string(),
            secret_storage_mode: SecretStorageMode::SystemKeyring,
            secret_scope_id: "stale-scope".to_string(),
            ..AppSettings::default()
        };
        controller.save_settings(&stale).expect("save settings");

        let persisted = store.load_settings();
        assert_eq!(persisted.language, "tr");
        assert_eq!(persisted.sources, current.sources);
        assert_eq!(persisted.jellyfin_device_id, "rufin-current");
        assert_eq!(persisted.secret_storage_mode, SecretStorageMode::ConfigFile);
        assert_eq!(persisted.secret_scope_id, "current-scope");
    }

    #[test]
    fn settings_scrobbling_save_deletes_missing_secret() {
        let store = StoreHandle::open_memory().expect("memory store");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let controller =
            SettingsController::new(store.clone(), Arc::<dyn SecretStore>::clone(&secrets));
        secrets
            .save_secret(&SecretKey::LastFmSession, "lastfm-session")
            .expect("seed secret");
        let settings = AppSettings::default();

        controller
            .save_settings_with_scrobbling_deletes(&settings)
            .expect("save settings");

        assert_eq!(
            secrets
                .load_secret(&SecretKey::LastFmSession)
                .expect("load secret"),
            None
        );
    }
}
