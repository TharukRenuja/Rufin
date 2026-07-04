use std::collections::HashMap;
use std::fs;
#[cfg(unix)]
use std::future::Future;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
#[cfg(unix)]
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use domain::SourceId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const CONFIG_SECRET_FORMAT: &str = "config-base64";
#[cfg(unix)]
const SECRET_SERVICE_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(unix)]
static SECRET_SERVICE_KEYRING: Mutex<Option<Arc<oo7::Keyring>>> = Mutex::new(None);
#[cfg(unix)]
static SECRET_SERVICE_KEYRING_INIT: Mutex<()> = Mutex::new(());

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("secret store lock was poisoned")]
    Locked,
    #[error("secret backend failed: {0}")]
    Backend(String),
    #[error("config secret store failed: {0}")]
    Config(String),
    #[error("secret was not valid utf-8")]
    Utf8,
}

pub type SecretResult<T> = Result<T, SecretError>;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum SecretKey {
    ProviderToken(SourceId),
    LastFmApiSecret,
    LastFmSession,
    LibreFmSession,
    ListenBrainzToken,
}

impl SecretKey {
    fn provider_token(source_id: &SourceId) -> Self {
        Self::ProviderToken(source_id.clone())
    }

    fn config_key(&self) -> String {
        match self {
            Self::ProviderToken(source_id) => {
                format!("provider-token:{}", source_id.as_str())
            }
            Self::LastFmApiSecret => "scrobbling:lastfm-api-secret".to_string(),
            Self::LastFmSession => "scrobbling:lastfm-session".to_string(),
            Self::LibreFmSession => "scrobbling:librefm-session".to_string(),
            Self::ListenBrainzToken => "scrobbling:listenbrainz-token".to_string(),
        }
    }

    fn scoped_config_key(&self, scope_id: &str) -> String {
        if scope_id.is_empty() {
            return self.config_key();
        }
        format!("scope:{scope_id}:{}", self.config_key())
    }
}

pub trait SecretStore: Send + Sync {
    fn save_secret(&self, key: &SecretKey, secret: &str) -> SecretResult<()>;
    fn load_secret(&self, key: &SecretKey) -> SecretResult<Option<String>>;
    fn delete_secret(&self, key: &SecretKey) -> SecretResult<()>;

    fn save_token(&self, source_id: &SourceId, token: &str) -> SecretResult<()> {
        self.save_secret(&SecretKey::provider_token(source_id), token)
    }

    fn load_token(&self, source_id: &SourceId) -> SecretResult<Option<String>> {
        self.load_secret(&SecretKey::provider_token(source_id))
    }

    fn delete_token(&self, source_id: &SourceId) -> SecretResult<()> {
        self.delete_secret(&SecretKey::provider_token(source_id))
    }
}

#[derive(Clone)]
pub struct CachedSecretStore {
    inner: Arc<dyn SecretStore>,
    secrets: Arc<Mutex<HashMap<SecretKey, Option<String>>>>,
}

impl CachedSecretStore {
    pub fn new(inner: Arc<dyn SecretStore>) -> Self {
        Self {
            inner,
            secrets: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl SecretStore for CachedSecretStore {
    fn save_secret(&self, key: &SecretKey, secret: &str) -> SecretResult<()> {
        self.inner.save_secret(key, secret)?;
        let mut secrets = self.secrets.lock().map_err(|_| SecretError::Locked)?;
        secrets.insert(key.clone(), Some(secret.to_string()));
        Ok(())
    }

    fn load_secret(&self, key: &SecretKey) -> SecretResult<Option<String>> {
        if let Some(secret) = self
            .secrets
            .lock()
            .map_err(|_| SecretError::Locked)?
            .get(key)
            .cloned()
        {
            return Ok(secret);
        }

        let secret = self.inner.load_secret(key)?;
        let mut secrets = self.secrets.lock().map_err(|_| SecretError::Locked)?;
        secrets.insert(key.clone(), secret.clone());
        Ok(secret)
    }

    fn delete_secret(&self, key: &SecretKey) -> SecretResult<()> {
        self.inner.delete_secret(key)?;
        let mut secrets = self.secrets.lock().map_err(|_| SecretError::Locked)?;
        secrets.insert(key.clone(), None);
        Ok(())
    }
}

#[derive(Clone)]
pub struct SwitchableSecretStore {
    inner: Arc<Mutex<Arc<dyn SecretStore>>>,
}

impl SwitchableSecretStore {
    pub fn new(inner: Arc<dyn SecretStore>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    pub fn replace(&self, inner: Arc<dyn SecretStore>) -> SecretResult<()> {
        *self.inner.lock().map_err(|_| SecretError::Locked)? = inner;
        Ok(())
    }

    fn current(&self) -> SecretResult<Arc<dyn SecretStore>> {
        Ok(self.inner.lock().map_err(|_| SecretError::Locked)?.clone())
    }
}

impl SecretStore for SwitchableSecretStore {
    fn save_secret(&self, key: &SecretKey, secret: &str) -> SecretResult<()> {
        self.current()?.save_secret(key, secret)
    }

    fn load_secret(&self, key: &SecretKey) -> SecretResult<Option<String>> {
        self.current()?.load_secret(key)
    }

    fn delete_secret(&self, key: &SecretKey) -> SecretResult<()> {
        self.current()?.delete_secret(key)
    }
}

#[derive(Clone, Debug)]
pub struct ConfigSecretStore {
    path: PathBuf,
    scope_id: String,
    lock: Arc<Mutex<()>>,
}

#[derive(Deserialize, Serialize)]
struct ConfigSecretFile {
    #[serde(default = "config_secret_format")]
    format: String,
    #[serde(default)]
    secrets: HashMap<String, String>,
}

impl Default for ConfigSecretFile {
    fn default() -> Self {
        Self {
            format: config_secret_format(),
            secrets: HashMap::new(),
        }
    }
}

impl ConfigSecretStore {
    pub fn new(path: PathBuf) -> Self {
        Self::with_scope(path, String::new())
    }

    pub fn with_scope(path: PathBuf, scope_id: impl Into<String>) -> Self {
        Self {
            path,
            scope_id: scope_id.into(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    fn read_file(&self) -> SecretResult<ConfigSecretFile> {
        match fs::read_to_string(&self.path) {
            Ok(value) => serde_json::from_str(&value).map_err(config_error),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(ConfigSecretFile::default()),
            Err(error) => Err(config_error(error)),
        }
    }

    fn write_file(&self, mut file: ConfigSecretFile) -> SecretResult<()> {
        file.format = config_secret_format();
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(config_error)?;
        }
        let value = serde_json::to_string_pretty(&file).map_err(config_error)?;
        let temp_path = self.path.with_extension("json.tmp");
        fs::write(&temp_path, format!("{value}\n")).map_err(config_error)?;
        restrict_config_secret_file(&temp_path).map_err(config_error)?;
        fs::rename(&temp_path, &self.path).map_err(config_error)?;
        Ok(())
    }
}

impl SecretStore for ConfigSecretStore {
    fn save_secret(&self, key: &SecretKey, secret: &str) -> SecretResult<()> {
        let _guard = self.lock.lock().map_err(|_| SecretError::Locked)?;
        let mut file = self.read_file()?;
        file.secrets
            .insert(key.scoped_config_key(&self.scope_id), BASE64.encode(secret));
        self.write_file(file)
    }

    fn load_secret(&self, key: &SecretKey) -> SecretResult<Option<String>> {
        let _guard = self.lock.lock().map_err(|_| SecretError::Locked)?;
        let file = self.read_file()?;
        file.secrets
            .get(&key.scoped_config_key(&self.scope_id))
            .map(|secret| {
                BASE64
                    .decode(secret)
                    .map_err(config_error)
                    .and_then(|bytes| String::from_utf8(bytes).map_err(|_| SecretError::Utf8))
            })
            .transpose()
    }

    fn delete_secret(&self, key: &SecretKey) -> SecretResult<()> {
        let _guard = self.lock.lock().map_err(|_| SecretError::Locked)?;
        match fs::metadata(&self.path) {
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(config_error(error)),
        }
        let mut file = self.read_file()?;
        if file
            .secrets
            .remove(&key.scoped_config_key(&self.scope_id))
            .is_some()
        {
            return self.write_file(file);
        }
        Ok(())
    }
}

fn config_secret_format() -> String {
    CONFIG_SECRET_FORMAT.to_string()
}

fn config_error(error: impl std::fmt::Display) -> SecretError {
    SecretError::Config(error.to_string())
}

fn restrict_config_secret_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[derive(Clone, Default)]
pub struct MemorySecretStore {
    secrets: Arc<Mutex<HashMap<SecretKey, String>>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemorySecretStore {
    fn save_secret(&self, key: &SecretKey, secret: &str) -> SecretResult<()> {
        let mut secrets = self.secrets.lock().map_err(|_| SecretError::Locked)?;
        secrets.insert(key.clone(), secret.to_string());
        Ok(())
    }

    fn load_secret(&self, key: &SecretKey) -> SecretResult<Option<String>> {
        let secrets = self.secrets.lock().map_err(|_| SecretError::Locked)?;
        Ok(secrets.get(key).cloned())
    }

    fn delete_secret(&self, key: &SecretKey) -> SecretResult<()> {
        let mut secrets = self.secrets.lock().map_err(|_| SecretError::Locked)?;
        secrets.remove(key);
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct UnavailableSecretStore {
    message: String,
}

impl UnavailableSecretStore {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl SecretStore for UnavailableSecretStore {
    fn save_secret(&self, _key: &SecretKey, _secret: &str) -> SecretResult<()> {
        Err(SecretError::Backend(self.message.clone()))
    }

    fn load_secret(&self, _key: &SecretKey) -> SecretResult<Option<String>> {
        Err(SecretError::Backend(self.message.clone()))
    }

    fn delete_secret(&self, _key: &SecretKey) -> SecretResult<()> {
        Err(SecretError::Backend(self.message.clone()))
    }
}

#[cfg(unix)]
#[derive(Clone, Debug)]
pub struct SecretServiceStore {
    application: String,
    scope_id: String,
}

#[cfg(unix)]
impl SecretServiceStore {
    pub fn new(scope_id: impl Into<String>) -> Self {
        Self {
            application: "Rufin".to_string(),
            scope_id: scope_id.into(),
        }
    }

    fn secret_attributes(&self, key: &SecretKey) -> Vec<(String, String)> {
        let mut attributes = vec![
            ("application".to_string(), self.application.clone()),
            ("scope".to_string(), self.scope_id.clone()),
        ];
        match key {
            SecretKey::ProviderToken(source_id) => {
                attributes.push(("namespace".to_string(), "provider".to_string()));
                attributes.push(("kind".to_string(), "provider-token".to_string()));
                attributes.push(("source_id".to_string(), source_id.as_str().to_string()));
            }
            SecretKey::LastFmApiSecret => {
                attributes.push(("namespace".to_string(), "scrobbling".to_string()));
                attributes.push(("kind".to_string(), "lastfm-api-secret".to_string()));
            }
            SecretKey::LastFmSession => {
                attributes.push(("namespace".to_string(), "scrobbling".to_string()));
                attributes.push(("kind".to_string(), "lastfm-session".to_string()));
            }
            SecretKey::LibreFmSession => {
                attributes.push(("namespace".to_string(), "scrobbling".to_string()));
                attributes.push(("kind".to_string(), "librefm-session".to_string()));
            }
            SecretKey::ListenBrainzToken => {
                attributes.push(("namespace".to_string(), "scrobbling".to_string()));
                attributes.push(("kind".to_string(), "listenbrainz-token".to_string()));
            }
        }
        attributes
    }

    fn legacy_secret_attributes(&self, key: &SecretKey) -> Option<Vec<(String, String)>> {
        match key {
            SecretKey::ProviderToken(source_id) => {
                let mut attributes = vec![
                    ("application".to_string(), self.application.clone()),
                    ("scope".to_string(), self.scope_id.clone()),
                    ("namespace".to_string(), "provider".to_string()),
                    ("kind".to_string(), "provider-token".to_string()),
                ];
                attributes.push(("server_id".to_string(), source_id.as_str().to_string()));
                Some(attributes)
            }
            _ => None,
        }
    }

    fn secret_label(&self, key: &SecretKey) -> String {
        match key {
            SecretKey::ProviderToken(source_id) => {
                format!("Rufin provider token {}", source_id.as_str())
            }
            SecretKey::LastFmApiSecret => "Rufin Last.fm API secret".to_string(),
            SecretKey::LastFmSession => "Rufin Last.fm session".to_string(),
            SecretKey::LibreFmSession => "Rufin Libre.fm session".to_string(),
            SecretKey::ListenBrainzToken => "Rufin ListenBrainz token".to_string(),
        }
    }

    fn run<T, Fut>(&self, operation: Fut) -> SecretResult<T>
    where
        Fut: Future<Output = oo7::Result<T>>,
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| SecretError::Backend(error.to_string()))?;
        let result = runtime
            .block_on(async { tokio::time::timeout(SECRET_SERVICE_TIMEOUT, operation).await })
            .map_err(|_| {
                SecretError::Backend(format!(
                    "secret service timed out after {}s",
                    SECRET_SERVICE_TIMEOUT.as_secs_f64()
                ))
            })?;
        result.map_err(|error| SecretError::Backend(error.to_string()))
    }

    fn run_with_keyring<T, Fut, Op>(&self, operation: Op) -> SecretResult<T>
    where
        Fut: Future<Output = oo7::Result<T>>,
        Op: FnOnce(Arc<oo7::Keyring>) -> Fut,
    {
        if oo7::ashpd::is_sandboxed() {
            let keyring = self.cached_keyring()?;
            return self.run(operation(keyring));
        }

        self.run(async move {
            let keyring = Arc::new(oo7::Keyring::new().await?);
            operation(keyring).await
        })
    }

    fn cached_keyring(&self) -> SecretResult<Arc<oo7::Keyring>> {
        if let Some(keyring) = SECRET_SERVICE_KEYRING
            .lock()
            .map_err(|_| SecretError::Locked)?
            .clone()
        {
            return Ok(keyring);
        }

        let _guard = SECRET_SERVICE_KEYRING_INIT
            .lock()
            .map_err(|_| SecretError::Locked)?;
        if let Some(keyring) = SECRET_SERVICE_KEYRING
            .lock()
            .map_err(|_| SecretError::Locked)?
            .clone()
        {
            return Ok(keyring);
        }

        let keyring = Arc::new(self.run(oo7::Keyring::new())?);
        *SECRET_SERVICE_KEYRING
            .lock()
            .map_err(|_| SecretError::Locked)? = Some(Arc::clone(&keyring));
        Ok(keyring)
    }
}

#[cfg(unix)]
async fn load_item_secret(
    keyring: &oo7::Keyring,
    attributes: &Vec<(String, String)>,
) -> oo7::Result<Option<oo7::Secret>> {
    let Some(item) = keyring.search_items(attributes).await?.into_iter().next() else {
        return Ok(None);
    };
    if item.is_locked().await? {
        item.unlock().await?;
    }
    Ok(Some(item.secret().await?))
}

#[cfg(unix)]
impl SecretStore for SecretServiceStore {
    fn save_secret(&self, key: &SecretKey, secret: &str) -> SecretResult<()> {
        let attributes = self.secret_attributes(key);
        let label = self.secret_label(key);
        let secret = secret.to_string();

        self.run_with_keyring(move |keyring| async move {
            keyring
                .create_item(&label, &attributes, oo7::Secret::text(secret), true)
                .await
        })
    }

    fn load_secret(&self, key: &SecretKey) -> SecretResult<Option<String>> {
        let attributes = self.secret_attributes(key);
        let legacy_attributes = self.legacy_secret_attributes(key);
        let secret = self.run_with_keyring(move |keyring| async move {
            if let Some(secret) = load_item_secret(&keyring, &attributes).await? {
                return Ok(Some(secret));
            }
            let Some(legacy_attributes) = legacy_attributes else {
                return Ok(None);
            };
            load_item_secret(&keyring, &legacy_attributes).await
        })?;

        secret
            .map(|secret| String::from_utf8(secret.as_bytes().to_vec()))
            .transpose()
            .map_err(|_| SecretError::Utf8)
    }

    fn delete_secret(&self, key: &SecretKey) -> SecretResult<()> {
        let attributes = self.secret_attributes(key);
        let legacy_attributes = self.legacy_secret_attributes(key);

        self.run_with_keyring(move |keyring| async move {
            keyring.delete(&attributes).await?;
            if let Some(legacy_attributes) = legacy_attributes {
                keyring.delete(&legacy_attributes).await?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CachedSecretStore, ConfigSecretStore, MemorySecretStore, SecretError, SecretKey,
        SecretResult, SecretStore, SwitchableSecretStore,
    };
    use domain::SourceId;
    use std::fs;
    use std::sync::{Arc, Mutex};

    #[test]
    fn memory_token_roundtrip() {
        let store = MemorySecretStore::new();
        let source_id = SourceId::fake(1);

        store.save_token(&source_id, "token").expect("save token");
        assert_eq!(
            store.load_token(&source_id).expect("load token"),
            Some("token".to_string())
        );
        store.delete_token(&source_id).expect("delete token");
        assert_eq!(store.load_token(&source_id).expect("load token"), None);
    }

    #[test]
    fn memory_secret_namespacing() {
        let store = MemorySecretStore::new();
        let source_id = SourceId::fake(1);

        store
            .save_token(&source_id, "provider-token")
            .expect("save provider token");
        store
            .save_secret(&SecretKey::LastFmSession, "lastfm-session")
            .expect("save scrobbling session");

        assert_eq!(
            store.load_token(&source_id).expect("load provider token"),
            Some("provider-token".to_string())
        );
        assert_eq!(
            store
                .load_secret(&SecretKey::LastFmSession)
                .expect("load scrobbling session"),
            Some("lastfm-session".to_string())
        );
        assert_eq!(
            store
                .load_secret(&SecretKey::ListenBrainzToken)
                .expect("load listenbrainz token"),
            None
        );
    }

    #[test]
    fn config_store_redaction() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("secrets.json");
        let store = ConfigSecretStore::new(path.clone());
        let source_id = SourceId::fake(1);

        store
            .save_token(&source_id, "provider-secret-value")
            .expect("save provider token");
        store
            .save_secret(&SecretKey::LastFmSession, "scrobble-secret-value")
            .expect("save scrobbling token");

        assert_eq!(
            store.load_token(&source_id).expect("load provider token"),
            Some("provider-secret-value".to_string())
        );
        assert_eq!(
            store
                .load_secret(&SecretKey::LastFmSession)
                .expect("load scrobbling token"),
            Some("scrobble-secret-value".to_string())
        );
        let raw = fs::read_to_string(&path).expect("read config secrets");
        assert!(raw.contains("config-base64"));
        assert!(!raw.contains("provider-secret-value"));
        assert!(!raw.contains("scrobble-secret-value"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = fs::metadata(&path)
                .expect("config secret metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn config_empty_scope_preserves_legacy_keys() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("secrets.json");
        let source_id = SourceId::fake(1);

        ConfigSecretStore::new(path.clone())
            .save_token(&source_id, "legacy-token")
            .expect("save legacy token");

        assert_eq!(
            ConfigSecretStore::with_scope(path, "")
                .load_token(&source_id)
                .expect("load legacy token"),
            Some("legacy-token".to_string())
        );
    }

    #[test]
    fn config_scopes_hide_old_tokens() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("secrets.json");
        let source_id = SourceId::fake(1);

        ConfigSecretStore::new(path.clone())
            .save_token(&source_id, "legacy-token")
            .expect("save legacy token");
        ConfigSecretStore::with_scope(path.clone(), "new-scope")
            .save_token(&source_id, "scoped-token")
            .expect("save scoped token");

        assert_eq!(
            ConfigSecretStore::new(path.clone())
                .load_token(&source_id)
                .expect("load legacy token"),
            Some("legacy-token".to_string())
        );
        assert_eq!(
            ConfigSecretStore::with_scope(path, "other-scope")
                .load_token(&source_id)
                .expect("load other scope token"),
            None
        );
    }

    #[derive(Default)]
    struct CountingSecretStore {
        inner: MemorySecretStore,
        loads: Mutex<usize>,
    }

    impl CountingSecretStore {
        fn load_count(&self) -> usize {
            *self.loads.lock().expect("load count")
        }
    }

    impl SecretStore for CountingSecretStore {
        fn save_secret(&self, key: &SecretKey, secret: &str) -> SecretResult<()> {
            self.inner.save_secret(key, secret)
        }

        fn load_secret(&self, key: &SecretKey) -> SecretResult<Option<String>> {
            *self.loads.lock().map_err(|_| SecretError::Locked)? += 1;
            self.inner.load_secret(key)
        }

        fn delete_secret(&self, key: &SecretKey) -> SecretResult<()> {
            self.inner.delete_secret(key)
        }
    }

    #[test]
    fn cached_token_reuse() {
        let inner = Arc::new(CountingSecretStore::default());
        let source_id = SourceId::fake(1);
        inner
            .save_token(&source_id, "provider-token")
            .expect("seed provider token");
        let store = CachedSecretStore::new(Arc::<CountingSecretStore>::clone(&inner));

        assert_eq!(
            store.load_token(&source_id).expect("load provider token"),
            Some("provider-token".to_string())
        );
        assert_eq!(
            store
                .load_token(&source_id)
                .expect("load cached provider token"),
            Some("provider-token".to_string())
        );
        assert_eq!(inner.load_count(), 1);
    }

    #[test]
    fn cached_store_mutations() {
        let inner = Arc::new(CountingSecretStore::default());
        let store = CachedSecretStore::new(Arc::<CountingSecretStore>::clone(&inner));
        let source_id = SourceId::fake(1);

        store
            .save_token(&source_id, "first-token")
            .expect("save provider token");
        assert_eq!(
            store.load_token(&source_id).expect("load provider token"),
            Some("first-token".to_string())
        );
        assert_eq!(inner.load_count(), 0);

        store
            .delete_token(&source_id)
            .expect("delete provider token");
        assert_eq!(
            store.load_token(&source_id).expect("load after delete"),
            None
        );
        assert_eq!(inner.load_count(), 0);
    }

    #[test]
    fn switchable_store_replaces_cached_backend() {
        let source_id = SourceId::fake(1);
        let first_inner = Arc::new(CountingSecretStore::default());
        first_inner
            .save_token(&source_id, "first-token")
            .expect("seed first token");
        let second_inner = Arc::new(CountingSecretStore::default());
        second_inner
            .save_token(&source_id, "second-token")
            .expect("seed second token");
        let store = SwitchableSecretStore::new(Arc::new(CachedSecretStore::new(first_inner)));

        assert_eq!(
            store.load_token(&source_id).expect("load first token"),
            Some("first-token".to_string())
        );
        store
            .replace(Arc::new(CachedSecretStore::new(second_inner)))
            .expect("replace backend");

        assert_eq!(
            store.load_token(&source_id).expect("load second token"),
            Some("second-token".to_string())
        );
    }
}
