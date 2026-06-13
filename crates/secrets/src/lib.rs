use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use domain::ServerId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const CONFIG_SECRET_FORMAT: &str = "config-base64";

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
    ProviderToken(ServerId),
    LastFmApiSecret,
    LastFmSession,
    LibreFmSession,
    ListenBrainzToken,
}

impl SecretKey {
    fn provider_token(server_id: &ServerId) -> Self {
        Self::ProviderToken(server_id.clone())
    }

    fn config_key(&self) -> String {
        match self {
            Self::ProviderToken(server_id) => {
                format!("provider-token:{}", server_id.as_str())
            }
            Self::LastFmApiSecret => "scrobbling:lastfm-api-secret".to_string(),
            Self::LastFmSession => "scrobbling:lastfm-session".to_string(),
            Self::LibreFmSession => "scrobbling:librefm-session".to_string(),
            Self::ListenBrainzToken => "scrobbling:listenbrainz-token".to_string(),
        }
    }
}

pub trait SecretStore: Send + Sync {
    fn save_secret(&self, key: &SecretKey, secret: &str) -> SecretResult<()>;
    fn load_secret(&self, key: &SecretKey) -> SecretResult<Option<String>>;
    fn delete_secret(&self, key: &SecretKey) -> SecretResult<()>;

    fn save_token(&self, server_id: &ServerId, token: &str) -> SecretResult<()> {
        self.save_secret(&SecretKey::provider_token(server_id), token)
    }

    fn load_token(&self, server_id: &ServerId) -> SecretResult<Option<String>> {
        self.load_secret(&SecretKey::provider_token(server_id))
    }

    fn delete_token(&self, server_id: &ServerId) -> SecretResult<()> {
        self.delete_secret(&SecretKey::provider_token(server_id))
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

#[derive(Clone, Debug)]
pub struct ConfigSecretStore {
    path: PathBuf,
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
        Self {
            path,
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
        file.secrets.insert(key.config_key(), BASE64.encode(secret));
        self.write_file(file)
    }

    fn load_secret(&self, key: &SecretKey) -> SecretResult<Option<String>> {
        let _guard = self.lock.lock().map_err(|_| SecretError::Locked)?;
        let file = self.read_file()?;
        file.secrets
            .get(&key.config_key())
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
        if file.secrets.remove(&key.config_key()).is_some() {
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

#[cfg(test)]
mod tests {
    use super::{
        CachedSecretStore, ConfigSecretStore, MemorySecretStore, SecretError, SecretKey,
        SecretResult, SecretStore,
    };
    use domain::ServerId;
    use std::fs;
    use std::sync::{Arc, Mutex};

    #[test]
    fn memory_token_roundtrip() {
        let store = MemorySecretStore::new();
        let server_id = ServerId::fake(1);

        store.save_token(&server_id, "token").expect("save token");
        assert_eq!(
            store.load_token(&server_id).expect("load token"),
            Some("token".to_string())
        );
        store.delete_token(&server_id).expect("delete token");
        assert_eq!(store.load_token(&server_id).expect("load token"), None);
    }

    #[test]
    fn memory_secret_namespacing() {
        let store = MemorySecretStore::new();
        let server_id = ServerId::fake(1);

        store
            .save_token(&server_id, "provider-token")
            .expect("save provider token");
        store
            .save_secret(&SecretKey::LastFmSession, "lastfm-session")
            .expect("save scrobbling session");

        assert_eq!(
            store.load_token(&server_id).expect("load provider token"),
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
        let server_id = ServerId::fake(1);

        store
            .save_token(&server_id, "provider-secret-value")
            .expect("save provider token");
        store
            .save_secret(&SecretKey::LastFmSession, "scrobble-secret-value")
            .expect("save scrobbling token");

        assert_eq!(
            store.load_token(&server_id).expect("load provider token"),
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
        let server_id = ServerId::fake(1);
        inner
            .save_token(&server_id, "provider-token")
            .expect("seed provider token");
        let store = CachedSecretStore::new(inner.clone());

        assert_eq!(
            store.load_token(&server_id).expect("load provider token"),
            Some("provider-token".to_string())
        );
        assert_eq!(
            store
                .load_token(&server_id)
                .expect("load cached provider token"),
            Some("provider-token".to_string())
        );
        assert_eq!(inner.load_count(), 1);
    }

    #[test]
    fn cached_store_mutations() {
        let inner = Arc::new(CountingSecretStore::default());
        let store = CachedSecretStore::new(inner.clone());
        let server_id = ServerId::fake(1);

        store
            .save_token(&server_id, "first-token")
            .expect("save provider token");
        assert_eq!(
            store.load_token(&server_id).expect("load provider token"),
            Some("first-token".to_string())
        );
        assert_eq!(inner.load_count(), 0);

        store
            .delete_token(&server_id)
            .expect("delete provider token");
        assert_eq!(
            store.load_token(&server_id).expect("load after delete"),
            None
        );
        assert_eq!(inner.load_count(), 0);
    }
}
