use std::fs;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use ::ui::{
    LibraryField, LibraryListKey, LibraryListSettings, LibraryListSettingsEntry,
    Settings as UiSettings,
};
use library::{HomeBlockKind, HomeSectionKind, MusicFolderId, SourceId};
use scrobbling::Settings as ScrobblingSettings;
use secrets::{
    CachedSecretStore, ConfigSecretStore, SecretKey, SecretStorageMode, SecretStore,
    SwitchableSecretStore,
};
use serde::{Deserialize, Serialize};
use sources::SourceConfiguration;
use tracing::warn;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct CredentialRef(String);

impl CredentialRef {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

pub(crate) fn fresh_credential_ref() -> Result<CredentialRef, String> {
    random_identity("source-").map(CredentialRef::new)
}

pub(crate) fn fresh_secret_scope_id() -> Result<String, String> {
    random_identity("")
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ConfiguredLocalAccess {
    pub(crate) root_path: PathBuf,
    pub(crate) server_prefix: Option<String>,
    pub(crate) local_prefix: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ConfiguredSource {
    #[serde(flatten)]
    pub(crate) configuration: SourceConfiguration,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) credential_ref: Option<CredentialRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) music_folder_id: Option<MusicFolderId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) local_access: Option<ConfiguredLocalAccess>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct SourceSettings {
    #[serde(default)]
    pub(crate) configured: Vec<ConfiguredSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) selected_source_id: Option<SourceId>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
pub(crate) enum LegacyTrackSortKey {
    TrackNumber,
    #[default]
    Title,
    Artist,
    Album,
    Year,
    Duration,
    Favorite,
}

impl LegacyTrackSortKey {
    fn library_field(self) -> LibraryField {
        match self {
            Self::TrackNumber => LibraryField::TrackNumber,
            Self::Title => LibraryField::Title,
            Self::Artist => LibraryField::Artist,
            Self::Album => LibraryField::Album,
            Self::Year => LibraryField::Year,
            Self::Duration => LibraryField::Duration,
            Self::Favorite => LibraryField::Favorite,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct LegacyTrackTableSettings {
    #[serde(default)]
    sort_key: LegacyTrackSortKey,
    #[serde(default)]
    descending: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct StoredSettings {
    #[serde(flatten)]
    pub(crate) ui: UiSettings,
    #[serde(default)]
    pub(crate) scrobbling: ScrobblingSettings,
    #[serde(default)]
    pub(crate) sources: SourceSettings,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) secret_scope_id: String,
    #[serde(default)]
    pub(crate) jellyfin_device_id: String,
    #[serde(default, rename = "home_sections", skip_serializing)]
    pub(crate) legacy_home_sections: Option<Vec<HomeSectionKind>>,
    #[serde(default, rename = "track_table", skip_serializing)]
    pub(crate) legacy_track_table: Option<LegacyTrackTableSettings>,
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            ui: UiSettings::default(),
            scrobbling: ScrobblingSettings::default(),
            sources: SourceSettings::default(),
            secret_scope_id: String::new(),
            jellyfin_device_id: String::new(),
            legacy_home_sections: None,
            legacy_track_table: None,
        }
    }
}

impl StoredSettings {
    pub(crate) fn migrate_defaults(&mut self) {
        if self.ui.lastfm_api_key.trim().is_empty() && !self.scrobbling.lastfm.api_key.is_empty() {
            self.ui.lastfm_api_key = self.scrobbling.lastfm.api_key.clone();
        }
        self.scrobbling.lastfm.api_key.clear();
        self.scrobbling.sanitize();
        self.migrate_home_blocks();
        self.migrate_legacy_track_table();
        self.ui.sanitize();
        self.ui.downloads.retain(|download| {
            self.sources
                .configured
                .iter()
                .any(|source| source.configuration.source_id == download.source_id)
        });
    }

    pub(crate) fn scrobbling_runtime_settings(&self) -> ScrobblingSettings {
        let mut settings = self.scrobbling.clone();
        settings.lastfm.api_key = self.ui.lastfm_api_key.clone();
        settings
    }

    fn migrate_home_blocks(&mut self) {
        if self.ui.home_blocks.is_empty() {
            let home_sections = self
                .legacy_home_sections
                .take()
                .filter(|sections| !sections.is_empty())
                .unwrap_or_else(default_home_sections);
            self.ui.home_blocks = Vec::with_capacity(home_sections.len() + 2);
            self.ui.home_blocks.push(HomeBlockKind::Showcase);
            for section in home_sections {
                self.ui.home_blocks.push(match section {
                    HomeSectionKind::Explore => HomeBlockKind::Explore,
                    HomeSectionKind::MostPlayed => HomeBlockKind::MostPlayed,
                    HomeSectionKind::NewlyAdded => HomeBlockKind::NewlyAdded,
                    HomeSectionKind::RecentlyPlayed => HomeBlockKind::RecentlyPlayed,
                    HomeSectionKind::RecentlyReleased => HomeBlockKind::RecentlyReleased,
                });
            }
            if !self.ui.home_blocks.contains(&HomeBlockKind::Genres) {
                self.ui.home_blocks.push(HomeBlockKind::Genres);
            }
        } else {
            self.legacy_home_sections.take();
        }
    }

    fn migrate_legacy_track_table(&mut self) {
        let Some(legacy) = self.legacy_track_table.take() else {
            return;
        };
        if self
            .ui
            .library_lists
            .iter()
            .any(|entry| entry.key == LibraryListKey::Tracks)
        {
            return;
        }

        let mut settings = LibraryListSettings::for_key(LibraryListKey::Tracks);
        settings.sort_key = legacy.sort_key.library_field();
        settings.descending = legacy.descending;
        self.ui.library_lists.push(LibraryListSettingsEntry {
            key: LibraryListKey::Tracks,
            settings,
        });
    }
}

fn default_home_sections() -> Vec<HomeSectionKind> {
    vec![
        HomeSectionKind::Explore,
        HomeSectionKind::MostPlayed,
        HomeSectionKind::NewlyAdded,
        HomeSectionKind::RecentlyPlayed,
        HomeSectionKind::RecentlyReleased,
    ]
}

#[derive(Clone)]
pub(crate) struct SettingsFile {
    path: PathBuf,
    value: Arc<Mutex<StoredSettings>>,
}

impl SettingsFile {
    pub(crate) fn open(path: PathBuf) -> Result<Self, String> {
        let mut value = read_settings(&path)?;
        value.migrate_defaults();
        let mut changed = false;
        if value.jellyfin_device_id.trim().is_empty() {
            value.jellyfin_device_id = random_identity("rufin-")?;
            changed = true;
        }
        let file = Self {
            path,
            value: Arc::new(Mutex::new(value)),
        };
        if changed {
            let current = file.load();
            file.write(&current)?;
        }
        Ok(file)
    }

    pub(crate) fn load(&self) -> StoredSettings {
        self.value
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(crate) fn playback_stream_quality(&self) -> playback::StreamQuality {
        self.value
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .ui
            .playback
            .stream_quality
    }

    pub(crate) fn update<T>(
        &self,
        operation: impl FnOnce(&mut StoredSettings) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut current = self
            .value
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut next = current.clone();
        let output = operation(&mut next)?;
        next.migrate_defaults();
        write_settings(&self.path, &next)?;
        *current = next;
        Ok(output)
    }

    fn write(&self, value: &StoredSettings) -> Result<(), String> {
        write_settings(&self.path, value)?;
        *self
            .value
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = value.clone();
        Ok(())
    }
}

pub(crate) struct SettingsUiPort {
    file: SettingsFile,
    on_change: Arc<dyn Fn(&StoredSettings, &StoredSettings) + Send + Sync>,
}

impl SettingsUiPort {
    pub(crate) fn new(
        file: SettingsFile,
        on_change: impl Fn(&StoredSettings, &StoredSettings) + Send + Sync + 'static,
    ) -> Rc<Self> {
        Rc::new(Self {
            file,
            on_change: Arc::new(on_change),
        })
    }

    fn save_ui(&self, settings: &UiSettings) -> Result<UiSettings, String> {
        let previous = self.file.load();
        self.file.update(|stored| {
            stored.ui = settings.clone();
            Ok(())
        })?;
        let current = self.file.load();
        (self.on_change)(&previous, &current);
        Ok(current.ui)
    }
}

impl ::ui::SettingsPort for SettingsUiPort {
    fn load(&self) -> UiSettings {
        self.file.load().ui
    }

    fn save(&self, settings: &UiSettings) -> Result<UiSettings, String> {
        self.save_ui(settings)
    }
}

pub(crate) fn platform_secret_store(settings: &StoredSettings) -> Arc<dyn SecretStore> {
    match settings.ui.secret_storage_mode {
        SecretStorageMode::ConfigFile => Arc::new(CachedSecretStore::new(Arc::new(
            ConfigSecretStore::with_scope(
                crate::paths::secrets_file(),
                settings.secret_scope_id.clone(),
            ),
        ))),
        SecretStorageMode::SystemKeyring => system_keyring_secret_store(&settings.secret_scope_id),
    }
}

#[cfg(unix)]
fn system_keyring_secret_store(scope_id: &str) -> Arc<dyn SecretStore> {
    Arc::new(CachedSecretStore::new(Arc::new(
        secrets::SecretServiceStore::new(scope_id.to_string()),
    )))
}

#[cfg(not(unix))]
fn system_keyring_secret_store(_scope_id: &str) -> Arc<dyn SecretStore> {
    Arc::new(secrets::UnavailableSecretStore::new(
        "system keyring is unavailable on this platform",
    ))
}

pub(crate) fn provider_secret_key(reference: &CredentialRef) -> SecretKey {
    SecretKey::provider_token(reference.as_str())
}

pub(crate) fn all_secret_keys(settings: &StoredSettings) -> Vec<SecretKey> {
    let mut keys = settings
        .sources
        .configured
        .iter()
        .filter_map(|source| source.credential_ref.as_ref())
        .map(provider_secret_key)
        .collect::<Vec<_>>();
    keys.extend(
        scrobbling::secret_descriptors()
            .iter()
            .map(|descriptor| scrobbling_secret_key(*descriptor)),
    );
    keys
}

pub(crate) fn persist_scrobbling_settings(
    file: &SettingsFile,
    secrets: &Arc<SwitchableSecretStore>,
    input: &ScrobblingSettings,
) -> Result<ScrobblingSettings, String> {
    let mut input = input.clone();
    input.sanitize();
    let stored = file.load();
    let mut current = stored.scrobbling_runtime_settings();
    for descriptor in scrobbling::secret_descriptors() {
        if !descriptor.value(&current).trim().is_empty() {
            continue;
        }
        let key = scrobbling_secret_key(*descriptor);
        if let Some(secret) = load_secret(Arc::clone(secrets), key.clone())
            .map_err(|error| format!("failed to load scrobbling secret {key:?}: {error}"))?
        {
            *descriptor.value_mut(&mut current) = secret;
        }
    }
    current.sanitize();

    let changed_secrets = scrobbling::secret_descriptors()
        .iter()
        .copied()
        .filter_map(|descriptor| {
            let inline_secret = !descriptor.value(&stored.scrobbling).trim().is_empty();
            let changed = inline_secret || descriptor.value(&current) != descriptor.value(&input);
            changed.then(|| {
                (
                    descriptor,
                    scrobbling_secret_key(descriptor),
                    descriptor.value(&input).to_string(),
                )
            })
        })
        .collect::<Vec<_>>();

    // Removing the previous fixed-key value first makes an interrupted account
    // change disconnected rather than pairing a new username with an old session.
    for (_, key, _) in &changed_secrets {
        delete_secret(Arc::clone(secrets), key.clone())
            .map_err(|error| format!("failed to replace scrobbling secret {key:?}: {error}"))?;
    }

    let mut persisted = input.clone();
    for descriptor in scrobbling::secret_descriptors() {
        descriptor.value_mut(&mut persisted).clear();
    }
    persisted.lastfm.api_key.clear();
    file.update(|stored| {
        stored.ui.lastfm_api_key = input.lastfm.api_key.clone();
        stored.scrobbling = persisted;
        Ok(())
    })?;

    // Descriptor order keeps each session after the credentials that make it
    // usable. A partial write therefore still cannot connect the wrong account.
    for (_, key, value) in changed_secrets {
        if !value.is_empty() {
            save_secret(Arc::clone(secrets), key.clone(), value)
                .map_err(|error| format!("failed to save scrobbling secret {key:?}: {error}"))?;
        }
    }
    Ok(load_scrobbling_settings(file, secrets))
}

pub(crate) fn load_scrobbling_settings(
    file: &SettingsFile,
    secrets: &Arc<SwitchableSecretStore>,
) -> ScrobblingSettings {
    let stored = file.load();
    let mut settings = stored.scrobbling_runtime_settings();
    for descriptor in scrobbling::secret_descriptors() {
        let value = descriptor.value_mut(&mut settings);
        if !value.trim().is_empty() {
            continue;
        }
        match load_secret(Arc::clone(secrets), scrobbling_secret_key(*descriptor)) {
            Ok(Some(secret)) => *value = secret,
            Ok(None) => {}
            Err(error) => warn!(%error, "failed to load a scrobbling secret"),
        }
    }
    settings.sanitize();
    settings
}

pub(crate) fn startup_scrobbling_settings(
    file: &SettingsFile,
    secrets: &Arc<SwitchableSecretStore>,
) -> ScrobblingSettings {
    let stored = file.load();
    let settings = stored.scrobbling_runtime_settings();
    let has_inline_secrets = scrobbling::secret_descriptors()
        .iter()
        .any(|descriptor| !descriptor.value(&stored.scrobbling).trim().is_empty());
    let has_enabled_service =
        settings.lastfm.enabled || settings.librefm.enabled || settings.listenbrainz.enabled;
    if !has_inline_secrets && !has_enabled_service {
        return settings;
    }

    let settings = load_scrobbling_settings(file, secrets);
    if !has_inline_secrets {
        return settings;
    }
    match persist_scrobbling_settings(file, secrets, &settings) {
        Ok(settings) => settings,
        Err(error) => {
            warn!(%error, "could not move scrobbling credentials to secret storage");
            settings
        }
    }
}

fn scrobbling_secret_key(descriptor: scrobbling::SecretDescriptor) -> SecretKey {
    SecretKey::namespaced(
        descriptor.namespace(),
        descriptor.kind(),
        descriptor.label(),
    )
}

pub(crate) fn load_provider_secret(
    secrets: &Arc<SwitchableSecretStore>,
    reference: &CredentialRef,
) -> Result<Option<String>, String> {
    load_secret(Arc::clone(secrets), provider_secret_key(reference))
}

pub(crate) fn save_provider_secret(
    secrets: &Arc<SwitchableSecretStore>,
    reference: &CredentialRef,
    value: String,
) -> Result<(), String> {
    save_secret(Arc::clone(secrets), provider_secret_key(reference), value)
}

pub(crate) fn delete_provider_secret(
    secrets: &Arc<SwitchableSecretStore>,
    reference: &CredentialRef,
) -> Result<(), String> {
    delete_secret(Arc::clone(secrets), provider_secret_key(reference))
}

fn load_secret<S>(store: Arc<S>, key: SecretKey) -> Result<Option<String>, String>
where
    S: SecretStore + ?Sized + 'static,
{
    blocking_secret(move || store.load_secret(&key))
}

fn save_secret<S>(store: Arc<S>, key: SecretKey, value: String) -> Result<(), String>
where
    S: SecretStore + ?Sized + 'static,
{
    blocking_secret(move || store.save_secret(&key, &value))
}

fn delete_secret<S>(store: Arc<S>, key: SecretKey) -> Result<(), String>
where
    S: SecretStore + ?Sized + 'static,
{
    blocking_secret(move || store.delete_secret(&key))
}

fn blocking_secret<T: Send + 'static>(
    operation: impl FnOnce() -> secrets::SecretResult<T> + Send + 'static,
) -> Result<T, String> {
    std::thread::Builder::new()
        .name("rufin-secrets".to_string())
        .spawn(operation)
        .map_err(|error| error.to_string())?
        .join()
        .map_err(|_| "the secrets operation panicked".to_string())?
        .map_err(|error| error.to_string())
}

fn random_identity(prefix: &str) -> Result<String, String> {
    use std::fmt::Write as _;

    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    let mut value = String::with_capacity(prefix.len() + bytes.len() * 2);
    value.push_str(prefix);
    for byte in bytes {
        write!(&mut value, "{byte:02x}").map_err(|error| error.to_string())?;
    }
    Ok(value)
}

pub(crate) fn read_settings(path: &Path) -> Result<StoredSettings, String> {
    match fs::read_to_string(path) {
        Ok(json) => serde_json::from_str(&json).map_err(|error| error.to_string()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(StoredSettings::default()),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn write_settings(path: &Path, value: &StoredSettings) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    let mut file = fs::File::create(&temporary).map_err(|error| error.to_string())?;
    restrict_file(&temporary).map_err(|error| error.to_string())?;
    file.write_all(format!("{json}\n").as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string())?;
    commit_settings_file(&temporary, path)?;
    Ok(())
}

fn commit_settings_file(temporary: &Path, path: &Path) -> Result<(), String> {
    commit_settings_file_with(temporary, path, |parent| {
        fs::File::open(parent).and_then(|directory| directory.sync_all())
    })
}

fn commit_settings_file_with(
    temporary: &Path,
    path: &Path,
    sync_directory: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    fs::rename(temporary, path).map_err(|error| error.to_string())?;
    if let Some(parent) = path.parent() {
        if let Err(error) = sync_directory(parent) {
            warn!(
                %error,
                path = %path.display(),
                "could not sync the settings directory after saving"
            );
        }
    }
    Ok(())
}

fn restrict_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ::ui::{
        LeftSidebarMode, LibraryField, LibraryLayout, LibraryListKey, LibraryListSettings,
        RightSidebarMode, SettingsPort as _,
    };
    use desktop_integration::Settings as RichPresenceSettings;
    use localization::SYSTEM_LANGUAGE_PREFERENCE;
    use lyrics::{ExternalLyricsProvider, Settings as LyricsSettings};
    use playback::{DEFAULT_AUTO_DJ_REFILL_THRESHOLD, MIN_AUTO_DJ_REFILL_THRESHOLD};
    use scrobbling::{
        AudioscrobblerSettings, LASTFM_API_SECRET, LASTFM_SESSION, LIBREFM_SESSION,
        LISTENBRAINZ_TOKEN, ListenBrainzSettings, Settings as ScrobblingSettings,
    };
    use secrets::{
        MemorySecretStore, SecretError, SecretKey, SecretResult, SecretStorageMode, SecretStore,
        SwitchableSecretStore,
    };
    use sources::SourceConfiguration;

    use super::*;

    #[derive(Clone, Default)]
    struct FaultSecretStore {
        secrets: Arc<Mutex<HashMap<SecretKey, String>>>,
        fail_on_save: Arc<Mutex<Option<usize>>>,
        operations: Arc<AtomicUsize>,
    }

    impl FaultSecretStore {
        fn fail_on_save(&self, number: usize) {
            *self
                .fail_on_save
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(number);
        }

        fn operation_count(&self) -> usize {
            self.operations.load(Ordering::Acquire)
        }
    }

    impl SecretStore for FaultSecretStore {
        fn save_secret(&self, key: &SecretKey, secret: &str) -> SecretResult<()> {
            self.operations.fetch_add(1, Ordering::AcqRel);
            let mut fail_on_save = self
                .fail_on_save
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(remaining) = fail_on_save.as_mut() {
                if *remaining == 1 {
                    *fail_on_save = None;
                    return Err(SecretError::Backend("injected save failure".to_string()));
                }
                *remaining -= 1;
            }
            self.secrets
                .lock()
                .map_err(|_| SecretError::Locked)?
                .insert(key.clone(), secret.to_string());
            Ok(())
        }

        fn load_secret(&self, key: &SecretKey) -> SecretResult<Option<String>> {
            self.operations.fetch_add(1, Ordering::AcqRel);
            Ok(self
                .secrets
                .lock()
                .map_err(|_| SecretError::Locked)?
                .get(key)
                .cloned())
        }

        fn delete_secret(&self, key: &SecretKey) -> SecretResult<()> {
            self.operations.fetch_add(1, Ordering::AcqRel);
            self.secrets
                .lock()
                .map_err(|_| SecretError::Locked)?
                .remove(key);
            Ok(())
        }
    }

    #[test]
    fn settings_rename_is_the_commit_point_even_if_directory_sync_fails() {
        let directory = tempfile::tempdir().expect("temporary settings directory");
        let path = directory.path().join("settings.json");
        let temporary = directory.path().join("settings.json.tmp");
        fs::write(&path, "old").expect("write old settings");
        fs::write(&temporary, "new").expect("write new settings");

        commit_settings_file_with(&temporary, &path, |_| {
            Err(std::io::Error::other("injected directory sync failure"))
        })
        .expect("renamed settings are committed");

        assert_eq!(
            fs::read_to_string(path).expect("read committed settings"),
            "new"
        );
        assert!(!temporary.exists());
    }

    #[test]
    fn sparse_legacy_json_keeps_persisted_defaults_and_home_order() {
        let json = r#"{
            "theme_preference":"System",
            "private_mode":false,
            "notifications_enabled":false,
            "external_lyrics_enabled":false,
            "discord_presence_enabled":false,
            "home_sections":["Explore","RecentlyPlayed"]
        }"#;

        let mut settings =
            serde_json::from_str::<StoredSettings>(json).expect("deserialize legacy settings");

        assert_eq!(
            settings.ui.secret_storage_mode,
            SecretStorageMode::ConfigFile
        );
        assert_eq!(
            settings.ui.auto_dj_refill_threshold,
            DEFAULT_AUTO_DJ_REFILL_THRESHOLD
        );
        assert!(settings.ui.lyrics_panel_visible);
        assert!(settings.ui.type_to_search_enabled);
        assert!(settings.ui.control_notifications_enabled);
        assert!(settings.ui.release_notifications_enabled);
        assert_eq!(
            settings.ui.lyrics.external_lyrics_providers,
            lyrics::default_external_lyrics_providers()
        );

        settings.migrate_defaults();

        assert_eq!(
            settings.ui.home_blocks,
            vec![
                HomeBlockKind::Showcase,
                HomeBlockKind::Explore,
                HomeBlockKind::RecentlyPlayed,
                HomeBlockKind::Genres,
            ]
        );
        assert!(settings.legacy_home_sections.is_none());
        assert!(
            serde_json::to_value(&settings)
                .expect("serialize migrated settings")
                .get("home_sections")
                .is_none()
        );
        assert_eq!(settings.ui.library_lists.len(), LibraryListKey::all().len());
    }

    #[test]
    fn legacy_track_table_sort_migrates_to_the_tracks_list_owner() {
        let json = r#"{
            "theme_preference":"System",
            "private_mode":false,
            "notifications_enabled":false,
            "external_lyrics_enabled":false,
            "discord_presence_enabled":false,
            "track_table": {
                "visible_columns":["Title","Album","Year"],
                "sort_key":"Artist",
                "descending":true,
                "layout_version":4
            }
        }"#;

        let mut settings =
            serde_json::from_str::<StoredSettings>(json).expect("deserialize legacy track table");
        settings.migrate_defaults();

        let tracks = settings
            .ui
            .library_lists
            .iter()
            .find(|entry| entry.key == LibraryListKey::Tracks)
            .expect("Tracks settings should be present");
        assert_eq!(tracks.settings.sort_key, LibraryField::Artist);
        assert!(tracks.settings.descending);
        assert!(
            serde_json::to_value(settings)
                .expect("serialize migrated settings")
                .get("track_table")
                .is_none()
        );
    }

    #[test]
    fn desktop_integration_owner_preserves_flat_settings_shape() {
        let mut value = serde_json::to_value(StoredSettings::default())
            .unwrap_or_else(|error| panic!("serialize settings: {error}"));
        let object = value
            .as_object_mut()
            .unwrap_or_else(|| panic!("settings should serialize as an object"));
        assert!(!object.contains_key("rich_presence"));
        assert_eq!(object["discord_presence_enabled"], false);
        object.insert(
            "discord_display_type".to_string(),
            serde_json::Value::String("app".to_string()),
        );
        object.insert(
            "discord_client_id".to_string(),
            serde_json::Value::String(String::new()),
        );
        object.remove("discord_link_type");

        let mut restored = serde_json::from_value::<StoredSettings>(value)
            .unwrap_or_else(|error| panic!("restore flat rich-presence settings: {error}"));
        restored.migrate_defaults();

        assert!(!restored.ui.rich_presence.enabled);
        assert_eq!(
            restored.ui.rich_presence.client_id,
            desktop_integration::DEFAULT_CLIENT_ID
        );
        assert_eq!(
            restored.ui.rich_presence.display_type,
            desktop_integration::DisplayType::Application
        );
        assert_eq!(
            restored.ui.rich_presence.link_type,
            desktop_integration::LinkType::MusicBrainz
        );
    }

    #[test]
    fn unknown_layout_modes_do_not_discard_other_stored_fields() {
        let json = r#"{
            "layout": {
                "default_profile": {
                    "left_sidebar": "Future",
                    "right_sidebar": "Future",
                    "last_visible_right_sidebar": "Future"
                },
                "narrow_profile": {
                    "left_sidebar": "Hidden",
                    "right_sidebar": "Comfortable"
                }
            },
            "theme_preference": "System",
            "private_mode": false,
            "notifications_enabled": false,
            "secret_storage_mode": "system-keyring",
            "secret_scope_id": "test-scope",
            "external_lyrics_enabled": true,
            "discord_presence_enabled": false
        }"#;

        let settings =
            serde_json::from_str::<StoredSettings>(json).expect("deserialize stored settings");

        assert_eq!(
            settings.ui.secret_storage_mode,
            SecretStorageMode::SystemKeyring
        );
        assert_eq!(settings.secret_scope_id, "test-scope");
        assert_eq!(
            settings.ui.layout.default_profile.left_sidebar,
            LeftSidebarMode::Full
        );
        assert_eq!(
            settings.ui.layout.default_profile.right_sidebar,
            RightSidebarMode::Visible
        );
        assert_eq!(
            settings.ui.layout.narrow_profile.left_sidebar,
            LeftSidebarMode::Hidden
        );
        assert_eq!(
            settings.ui.layout.narrow_profile.right_sidebar,
            RightSidebarMode::Visible
        );
        assert_eq!(settings.ui.layout.preferred_right_sidebar_width, 300);
    }

    #[test]
    fn layout_migrates_legacy_right_size_to_one_global_preference() {
        let mut value = serde_json::to_value(StoredSettings::default())
            .expect("serialize current settings fixture");
        value["layout"] = serde_json::json!({
            "default_profile": {
                "left_sidebar": "Full",
                "right_sidebar": "Hidden",
                "last_visible_right_sidebar": "Comfortable"
            },
            "narrow_profile": {
                "left_sidebar": "Compact",
                "right_sidebar": "Spacious"
            }
        });

        let settings =
            serde_json::from_value::<StoredSettings>(value).expect("deserialize legacy layout");

        assert_eq!(
            settings.ui.layout.default_profile.right_sidebar,
            RightSidebarMode::Hidden
        );
        assert_eq!(
            settings.ui.layout.narrow_profile.right_sidebar,
            RightSidebarMode::Visible
        );
        assert_eq!(settings.ui.layout.preferred_right_sidebar_width, 400);
        assert_eq!(
            settings.ui.layout.preferred_left_sidebar_width,
            ::ui::DEFAULT_LEFT_SIDEBAR_WIDTH
        );

        let mut value = serde_json::to_value(settings).expect("serialize migrated layout");
        assert_eq!(
            value["layout"]["default_profile"]["right_sidebar"],
            "Hidden"
        );
        assert_eq!(
            value["layout"]["narrow_profile"]["right_sidebar"],
            "Visible"
        );
        assert!(
            value["layout"]["default_profile"]
                .get("last_visible_right_sidebar")
                .is_none()
        );

        value["layout"]["narrow_profile"]["right_sidebar"] =
            serde_json::Value::String("Shown".to_string());
        let previous_name = serde_json::from_value::<StoredSettings>(value)
            .expect("deserialize previous visible-state name");
        assert_eq!(
            previous_name.ui.layout.narrow_profile.right_sidebar,
            RightSidebarMode::Visible
        );
    }

    #[test]
    fn aggregate_migration_preserves_cross_setting_compatibility() {
        let mut settings = StoredSettings {
            ui: UiSettings {
                language: "de_DE\0".to_string(),
                release_notification_seen_version: Some("  ".to_string()),
                rich_presence: RichPresenceSettings {
                    enabled: false,
                    client_id: String::new(),
                    ..RichPresenceSettings::default()
                },
                lyrics: LyricsSettings {
                    external_lyrics_providers: vec![
                        ExternalLyricsProvider::Genius,
                        ExternalLyricsProvider::Netease,
                        ExternalLyricsProvider::Genius,
                    ],
                    lyrics_provider_settings_version: 0,
                    suppressed_auto_lyrics_track_ids: vec!["track-one".to_string()],
                    ..LyricsSettings::default()
                },
                auto_dj_refill_threshold: 0,
                tray_enabled: false,
                exit_to_tray: true,
                start_minimized: true,
                lastfm_api_key: String::new(),
                library_lists: vec![LibraryListSettingsEntry {
                    key: LibraryListKey::Playlists,
                    settings: LibraryListSettings {
                        layout: LibraryLayout::Grid,
                        row_fields: vec![
                            LibraryField::Image,
                            LibraryField::Title,
                            LibraryField::SongCount,
                            LibraryField::Duration,
                        ],
                        grid_fields: vec![LibraryField::SongCount, LibraryField::Duration],
                        detail_track_fields: Vec::new(),
                        sort_key: LibraryField::Title,
                        descending: false,
                        row_column_widths: Vec::new(),
                        layout_version: 2,
                    },
                }],
                ..UiSettings::default()
            },
            scrobbling: ScrobblingSettings {
                lastfm: AudioscrobblerSettings {
                    api_key: " scrobble-key ".to_string(),
                    ..AudioscrobblerSettings::default()
                },
                ..ScrobblingSettings::default()
            },
            ..StoredSettings::default()
        };

        settings.migrate_defaults();

        assert_eq!(settings.ui.language, SYSTEM_LANGUAGE_PREFERENCE);
        assert_eq!(settings.ui.release_notification_seen_version, None);
        assert_eq!(
            settings.ui.rich_presence.client_id,
            desktop_integration::DEFAULT_CLIENT_ID
        );
        assert!(!settings.ui.rich_presence.enabled);
        assert_eq!(
            settings.ui.lyrics.external_lyrics_providers,
            vec![
                ExternalLyricsProvider::Genius,
                ExternalLyricsProvider::Netease
            ]
        );
        assert_eq!(
            settings.ui.lyrics.lyrics_provider_settings_version,
            lyrics::LYRICS_PROVIDER_SETTINGS_VERSION
        );
        assert!(
            settings
                .ui
                .lyrics
                .suppressed_auto_lyrics_track_ids
                .is_empty()
        );
        assert_eq!(
            settings.ui.auto_dj_refill_threshold,
            MIN_AUTO_DJ_REFILL_THRESHOLD
        );
        assert!(!settings.ui.exit_to_tray);
        assert!(!settings.ui.start_minimized);
        assert_eq!(settings.ui.lastfm_api_key, "scrobble-key");
        assert!(settings.scrobbling.lastfm.api_key.is_empty());
        assert_eq!(
            settings.scrobbling_runtime_settings().lastfm.api_key,
            "scrobble-key"
        );
        assert_eq!(
            settings
                .ui
                .library_list(LibraryListKey::Playlists)
                .row_fields,
            vec![
                LibraryField::Image,
                LibraryField::Title,
                LibraryField::SongCount
            ]
        );
    }

    #[test]
    fn disabled_startup_does_not_read_secret_storage() {
        let directory = tempfile::tempdir().expect("temporary settings directory");
        let file =
            SettingsFile::open(directory.path().join("settings.json")).expect("open settings");
        let backend = FaultSecretStore::default();
        let secrets = Arc::new(SwitchableSecretStore::new(Arc::new(backend.clone())));

        let defaults = startup_scrobbling_settings(&file, &secrets);
        assert!(!defaults.lastfm.enabled);
        assert_eq!(backend.operation_count(), 0);

        file.update(|stored| {
            stored.ui.lastfm_api_key = "retained-api-key".to_string();
            stored.scrobbling.lastfm.username = "retained-listener".to_string();
            stored.scrobbling.lastfm.enabled = false;
            Ok(())
        })
        .expect("save disabled account metadata");
        let retained = startup_scrobbling_settings(&file, &secrets);
        assert_eq!(retained.lastfm.username, "retained-listener");
        assert_eq!(backend.operation_count(), 0);
    }

    #[test]
    fn enabled_startup_hydrates_stored_scrobbling_account() {
        let directory = tempfile::tempdir().expect("temporary settings directory");
        let file =
            SettingsFile::open(directory.path().join("settings.json")).expect("open settings");
        file.update(|stored| {
            stored.ui.lastfm_api_key = "lastfm-key".to_string();
            stored.scrobbling.lastfm.enabled = true;
            stored.scrobbling.lastfm.username = "listener".to_string();
            Ok(())
        })
        .expect("save enabled account");
        let secrets = Arc::new(SwitchableSecretStore::new(Arc::new(
            MemorySecretStore::new(),
        )));
        secrets
            .save_secret(&scrobbling_secret_key(LASTFM_API_SECRET), "lastfm-secret")
            .expect("save API secret");
        secrets
            .save_secret(&scrobbling_secret_key(LASTFM_SESSION), "lastfm-session")
            .expect("save session");

        let settings = startup_scrobbling_settings(&file, &secrets);

        assert!(settings.lastfm.enabled);
        assert_eq!(settings.lastfm.api_secret, "lastfm-secret");
        assert_eq!(settings.lastfm.session_key, "lastfm-session");
    }

    #[test]
    fn inline_legacy_scrobbling_secrets_migrate_while_disabled() {
        let directory = tempfile::tempdir().expect("temporary settings directory");
        let file =
            SettingsFile::open(directory.path().join("settings.json")).expect("open settings");
        file.update(|stored| {
            stored.ui.lastfm_api_key = "lastfm-key".to_string();
            stored.scrobbling = ScrobblingSettings {
                lastfm: AudioscrobblerSettings {
                    api_secret: "lastfm-secret".to_string(),
                    session_key: "lastfm-session".to_string(),
                    ..AudioscrobblerSettings::default()
                },
                librefm: AudioscrobblerSettings {
                    session_key: "librefm-session".to_string(),
                    ..AudioscrobblerSettings::default()
                },
                listenbrainz: ListenBrainzSettings {
                    user_token: "listenbrainz-token".to_string(),
                    ..ListenBrainzSettings::default()
                },
            };
            Ok(())
        })
        .expect("save inline credentials");
        let secrets = Arc::new(SwitchableSecretStore::new(Arc::new(
            MemorySecretStore::new(),
        )));

        let migrated = startup_scrobbling_settings(&file, &secrets);

        assert_eq!(migrated.lastfm.api_key, "lastfm-key");
        assert_eq!(migrated.lastfm.api_secret, "lastfm-secret");
        assert_eq!(migrated.lastfm.session_key, "lastfm-session");
        assert_eq!(migrated.librefm.session_key, "librefm-session");
        assert_eq!(migrated.listenbrainz.user_token, "listenbrainz-token");
        let persisted = file.load();
        for descriptor in scrobbling::secret_descriptors() {
            assert!(descriptor.value(&persisted.scrobbling).is_empty());
        }
        for (descriptor, expected) in [
            (LASTFM_API_SECRET, "lastfm-secret"),
            (LASTFM_SESSION, "lastfm-session"),
            (LIBREFM_SESSION, "librefm-session"),
            (LISTENBRAINZ_TOKEN, "listenbrainz-token"),
        ] {
            assert_eq!(
                secrets
                    .load_secret(&scrobbling_secret_key(descriptor))
                    .expect("load migrated credential")
                    .as_deref(),
                Some(expected)
            );
        }

        let ui = SettingsUiPort::new(file.clone(), |_, _| {});
        let mut ordinary = ui.load();
        ordinary.language = "tr".to_string();
        ui.save(&ordinary).expect("save ordinary UI setting");

        assert_eq!(load_scrobbling_settings(&file, &secrets), migrated);
    }

    #[test]
    fn interrupted_scrobbling_account_change_cannot_pair_old_identity_and_session() {
        let directory = tempfile::tempdir().expect("temporary settings directory");
        let file =
            SettingsFile::open(directory.path().join("settings.json")).expect("open settings");
        let backend = FaultSecretStore::default();
        let secrets = Arc::new(SwitchableSecretStore::new(Arc::new(backend.clone())));
        let account = |username: &str, api_secret: &str, session_key: &str| ScrobblingSettings {
            lastfm: AudioscrobblerSettings {
                enabled: true,
                username: username.to_string(),
                api_key: "lastfm-key".to_string(),
                api_secret: api_secret.to_string(),
                session_key: session_key.to_string(),
                ..AudioscrobblerSettings::default()
            },
            ..ScrobblingSettings::default()
        };

        persist_scrobbling_settings(
            &file,
            &secrets,
            &account("first-listener", "first-secret", "first-session"),
        )
        .expect("save first account");
        backend.fail_on_save(2);
        let error = persist_scrobbling_settings(
            &file,
            &secrets,
            &account("second-listener", "second-secret", "second-session"),
        )
        .expect_err("interrupt second account");
        assert!(error.contains("injected save failure"));

        let interrupted = load_scrobbling_settings(&file, &secrets);
        assert_eq!(interrupted.lastfm.username, "second-listener");
        assert_eq!(interrupted.lastfm.api_secret, "second-secret");
        assert!(interrupted.lastfm.session_key.is_empty());
        assert_ne!(interrupted.lastfm.session_key, "first-session");
    }

    #[test]
    fn current_home_blocks_replace_the_read_only_legacy_input() {
        let local_id = SourceId::new("local:server:library");
        let sources = SourceSettings {
            selected_source_id: Some(local_id.clone()),
            configured: vec![ConfiguredSource {
                configuration: SourceConfiguration {
                    source_id: local_id,
                    kind: "local".to_string(),
                    name: "Local".to_string(),
                    provider_payload: serde_json::json!({
                        "version": 1,
                        "roots": ["/music"],
                    })
                    .to_string(),
                },
                credential_ref: None,
                music_folder_id: None,
                local_access: None,
            }],
        };
        let mut stored = StoredSettings {
            sources: sources.clone(),
            jellyfin_device_id: "device-id".to_string(),
            secret_scope_id: "scope-id".to_string(),
            legacy_home_sections: Some(vec![HomeSectionKind::MostPlayed]),
            ..StoredSettings::default()
        };
        let mut settings = stored.ui.clone();
        settings.private_mode = true;
        settings.home_blocks = vec![HomeBlockKind::Showcase, HomeBlockKind::RecentlyPlayed];

        stored.ui = settings;

        assert_eq!(stored.sources, sources);
        assert_eq!(stored.jellyfin_device_id, "device-id");
        assert_eq!(stored.secret_scope_id, "scope-id");
        assert_eq!(
            stored.legacy_home_sections,
            Some(vec![HomeSectionKind::MostPlayed])
        );
        assert!(stored.ui.private_mode);

        stored.migrate_defaults();

        assert_eq!(stored.sources, sources);
        assert_eq!(stored.jellyfin_device_id, "device-id");
        assert_eq!(stored.secret_scope_id, "scope-id");
        assert!(stored.legacy_home_sections.is_none());
        let serialized = serde_json::to_value(&stored).expect("serialize current settings");
        assert!(serialized.get("home_sections").is_none());
        assert_eq!(
            serialized.get("home_blocks"),
            Some(&serde_json::json!(["Showcase", "RecentlyPlayed"]))
        );
    }

    #[test]
    fn download_rules_follow_their_configured_source() {
        let source_id = SourceId::new("jellyfin:configured");
        let removed_id = SourceId::new("jellyfin:removed");
        let mut stored = StoredSettings {
            sources: SourceSettings {
                selected_source_id: Some(source_id.clone()),
                configured: vec![ConfiguredSource {
                    configuration: SourceConfiguration {
                        source_id: source_id.clone(),
                        kind: "jellyfin".to_string(),
                        name: "Server".to_string(),
                        provider_payload: "{}".to_string(),
                    },
                    credential_ref: None,
                    music_folder_id: None,
                    local_access: None,
                }],
            },
            ..StoredSettings::default()
        };
        stored.ui.set_download_rules(
            source_id.clone(),
            ::ui::DownloadRules {
                entire_library: true,
                ..::ui::DownloadRules::default()
            },
        );
        stored.ui.set_download_rules(
            removed_id.clone(),
            ::ui::DownloadRules {
                favorites: true,
                ..::ui::DownloadRules::default()
            },
        );

        stored.migrate_defaults();

        assert!(stored.ui.download_rules(&source_id).entire_library);
        assert!(stored.ui.download_rules(&removed_id).is_empty());
    }
}
