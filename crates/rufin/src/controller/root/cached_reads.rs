use super::*;

pub(in crate::controller) fn save_home_section_projection(
    store: &StoreHandle,
    source_id: &SourceId,
    section: &HomeSection,
) -> Result<library::HomeSectionCommit, String> {
    store.with_store(|store| store.promote_home_section(source_id, section))
}
pub(in crate::controller) fn cache_home_section(
    store: &StoreHandle,
    source_id: &SourceId,
    section: &HomeSection,
) -> Result<library::HomeSectionCommit, String> {
    store.with_store(|store| store.replace_home_section(source_id, section))
}
pub(in crate::controller) fn emit_source_presentation(
    store: &StoreHandle,
    source_presentation: &Sender<SourcePresentationState>,
) {
    match load_source_presentation(store) {
        Ok(presentation) => {
            let _sent = source_presentation.try_send(presentation);
        }
        Err(error) => {
            warn!(%error, "failed to load source presentation");
        }
    }
}
pub(in crate::controller) fn trimmed_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}
pub(crate) fn load_settings_from_store(store: &StoreHandle) -> StoredSettings {
    let mut settings = store.load_settings();
    settings.migrate_defaults();
    settings
}
pub(in crate::controller) fn shuffle_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(1)
}
pub(in crate::controller) fn platform_secret_store(
    settings: &StoredSettings,
) -> Arc<dyn SecretStore> {
    match settings.ui.secret_storage_mode {
        SecretStorageMode::ConfigFile => Arc::new(CachedSecretStore::new(Arc::new(
            ConfigSecretStore::with_scope(
                super::app_paths::config_secrets_path(),
                settings.secret_scope_id.clone(),
            ),
        ))),
        SecretStorageMode::SystemKeyring => system_keyring_secret_store(&settings.secret_scope_id),
    }
}

#[cfg(unix)]
fn system_keyring_secret_store(scope_id: &str) -> Arc<dyn SecretStore> {
    Arc::new(CachedSecretStore::new(Arc::new(SecretServiceStore::new(
        scope_id.to_string(),
    ))))
}

#[cfg(not(unix))]
fn system_keyring_secret_store(_scope_id: &str) -> Arc<dyn SecretStore> {
    Arc::new(UnavailableSecretStore::new(
        "system keyring is unavailable on this platform",
    ))
}
pub(in crate::controller) fn saved_server_needs_auth(
    secrets: &Arc<dyn SecretStore>,
    saved: &StoredSource,
) -> bool {
    match crate::source_setup::configured_source_needs_auth(secrets, saved) {
        Ok(needs_auth) => needs_auth,
        Err(error) => {
            warn!(
                %error,
                source_id = %saved.source_id,
                source_kind = %saved.kind,
                "failed to resolve source authentication state"
            );
            true
        }
    }
}
pub(in crate::controller) fn emit_runtime_source_presentation(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    source_presentation: &Sender<SourcePresentationState>,
) {
    match load_runtime_source_presentation(store, secrets) {
        Ok(presentation) => {
            let _sent = source_presentation.try_send(presentation);
        }
        Err(error) => {
            warn!(%error, "failed to load runtime source presentation");
        }
    }
}
