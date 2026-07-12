use super::*;

pub(in crate::controller) fn promote_prefetched_home_section(
    store: &StoreHandle,
    source_id: &SourceId,
    section: &HomeSection,
) -> Result<(), String> {
    let (generation, base_cache_revision) = store.with_store(|store| {
        let state = store.sync_state(source_id)?;
        Ok((state.generation, state.cache_revision))
    })?;
    store
        .with_store(|store| {
            store.promote_home_section(source_id, generation, base_cache_revision, section)
        })
        .map(|_| ())
}
pub(in crate::controller) fn cache_home_section(
    store: &StoreHandle,
    source_id: &SourceId,
    section: &HomeSection,
    generation: i64,
    base_cache_revision: i64,
) -> Result<SyncCommit, String> {
    store.with_store(|store| {
        store.replace_home_section(source_id, generation, base_cache_revision, section)
    })
}
pub(in crate::controller) fn load_library_counts(
    store: &StoreHandle,
    source_id: &SourceId,
) -> Result<LibraryCounts, String> {
    store.with_store(|store| {
        store.read_snapshot(|store| {
            Ok(LibraryCounts {
                albums: store.load_albums(source_id, 0, 0)?.total,
                tracks: store.load_tracks(source_id, 0, 0)?.total,
                artists: store.load_artists(source_id, false, 0, 0)?.total,
                album_artists: store.load_artists(source_id, true, 0, 0)?.total,
                genres: store.load_genres(source_id, 0, 0)?.total,
                playlists: store.load_playlists(source_id, 0, 0)?.total,
            })
        })
    })
}
pub(in crate::controller) fn load_home_update(
    store: &StoreHandle,
    saved: &StoredSource,
) -> Result<LibraryHomeUpdate, String> {
    store.with_store(|store| {
        store.read_snapshot(|store| {
            let sections = store.load_home_sections(&saved.source_id)?;
            let prefetched_explore =
                store.load_home_section_prefetch(&saved.source_id, HomeSectionKind::Explore)?;
            Ok(LibraryHomeUpdate {
                sections,
                prefetched_explore,
            })
        })
    })
}
pub(in crate::controller) fn emit_snapshot(store: &StoreHandle, events: &Sender<ControllerEvent>) {
    match load_snapshot(store) {
        Ok(snapshot) => {
            let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
        }
        Err(error) => {
            let _sent = events.send(ControllerEvent::Error(error));
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
    match settings.secret_storage_mode {
        SecretStorageMode::ConfigFile => Arc::new(CachedSecretStore::new(Arc::new(
            ConfigSecretStore::with_scope(config_secrets_path(), settings.secret_scope_id.clone()),
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
pub(in crate::controller) fn emit_runtime_snapshot(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    events: &Sender<ControllerEvent>,
) {
    match load_runtime_snapshot(store, secrets) {
        Ok(snapshot) => {
            let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
        }
        Err(error) => {
            let _sent = events.send(ControllerEvent::Error(error));
        }
    }
}
