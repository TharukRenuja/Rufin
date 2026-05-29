use std::collections::HashMap;
#[cfg(unix)]
use std::future::Future;
use std::sync::{Arc, Mutex};
#[cfg(unix)]
use std::time::Duration;

use rufin_core::ServerId;
use thiserror::Error;

#[cfg(unix)]
const SECRET_SERVICE_TIMEOUT: Duration = Duration::from_secs(15);
// oo7's Flatpak file backend protects the keyring file per Keyring instance.
// Reuse one process-wide instance so concurrent Rufin token reads share that lock.
#[cfg(unix)]
static SECRET_SERVICE_KEYRING: Mutex<Option<Arc<oo7::Keyring>>> = Mutex::new(None);
#[cfg(unix)]
static SECRET_SERVICE_KEYRING_INIT: Mutex<()> = Mutex::new(());

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("secret store lock was poisoned")]
    Locked,
    #[error("secret service failed: {0}")]
    Backend(String),
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

#[cfg(unix)]
#[derive(Clone, Debug)]
pub struct SecretServiceStore {
    application: String,
}

#[cfg(unix)]
impl Default for SecretServiceStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(unix)]
impl SecretServiceStore {
    pub fn new() -> Self {
        Self {
            application: "Rufin".to_string(),
        }
    }

    fn secret_attributes(&self, key: &SecretKey) -> Vec<(String, String)> {
        let mut attributes = vec![("application".to_string(), self.application.clone())];
        match key {
            SecretKey::ProviderToken(server_id) => {
                attributes.push(("namespace".to_string(), "provider".to_string()));
                attributes.push(("kind".to_string(), "provider-token".to_string()));
                attributes.push(("server_id".to_string(), server_id.as_str().to_string()));
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
            SecretKey::ProviderToken(server_id) => Some(vec![
                ("application".to_string(), self.application.clone()),
                ("kind".to_string(), "jellyfin-token".to_string()),
                ("server_id".to_string(), server_id.as_str().to_string()),
            ]),
            _ => None,
        }
    }

    fn secret_label(&self, key: &SecretKey) -> String {
        match key {
            SecretKey::ProviderToken(server_id) => {
                format!("Rufin provider token {}", server_id.as_str())
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
        self.run_with_timeout(operation, SECRET_SERVICE_TIMEOUT)
    }

    fn run_with_timeout<T, Fut>(&self, operation: Fut, timeout: Duration) -> SecretResult<T>
    where
        Fut: Future<Output = oo7::Result<T>>,
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| SecretError::Backend(error.to_string()))?;
        let result = runtime
            .block_on(async { tokio::time::timeout(timeout, operation).await })
            .map_err(|_| {
                SecretError::Backend(format!(
                    "secret service timed out after {}s",
                    timeout.as_secs_f64()
                ))
            })?;
        result.map_err(|error| SecretError::Backend(error.to_string()))
    }

    fn run_with_keyring<T, Fut, Op>(&self, operation: Op) -> SecretResult<T>
    where
        Fut: Future<Output = oo7::Result<T>>,
        Op: FnOnce(Arc<oo7::Keyring>) -> Fut,
    {
        if should_cache_keyring(oo7::ashpd::is_sandboxed()) {
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

        let _init_guard = SECRET_SERVICE_KEYRING_INIT
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
fn should_cache_keyring(sandboxed: bool) -> bool {
    // Native DBus keyrings are tied to the runtime that drives their connection.
    // The sandbox file backend needs reuse to avoid per-instance file locks.
    sandboxed
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
            if let Some(legacy_attributes) = legacy_attributes
                && let Some(secret) = load_item_secret(&keyring, &legacy_attributes).await?
            {
                return Ok(Some(secret));
            }
            Ok(None)
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
    #[cfg(unix)]
    use super::SecretServiceStore;
    use super::{
        CachedSecretStore, MemorySecretStore, SecretError, SecretKey, SecretResult, SecretStore,
    };
    use rufin_core::ServerId;
    #[cfg(unix)]
    use std::future;
    use std::sync::{Arc, Mutex};
    #[cfg(unix)]
    use std::time::{Duration, Instant};

    #[test]
    fn memory_secret_store_round_trips_provider_tokens() {
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
    fn memory_secret_store_namespaces_secret_keys() {
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
    fn cached_secret_store_reuses_loaded_provider_tokens() {
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
    fn cached_secret_store_updates_cache_after_save_and_delete() {
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

    #[test]
    #[cfg(unix)]
    fn secret_service_provider_token_uses_namespaced_attributes_with_legacy_fallback() {
        let store = SecretServiceStore::new();
        let server_id = ServerId::fake(1);
        let key = SecretKey::ProviderToken(server_id.clone());

        let attributes = store.secret_attributes(&key);
        assert!(attributes.contains(&("namespace".to_string(), "provider".to_string())));
        assert!(attributes.contains(&("kind".to_string(), "provider-token".to_string())));
        assert!(attributes.contains(&("server_id".to_string(), server_id.as_str().to_string())));

        let legacy_attributes = store
            .legacy_secret_attributes(&key)
            .expect("legacy provider attributes");
        assert!(legacy_attributes.contains(&("kind".to_string(), "jellyfin-token".to_string())));
        assert!(
            legacy_attributes.contains(&("server_id".to_string(), server_id.as_str().to_string()))
        );
        assert!(
            store
                .legacy_secret_attributes(&SecretKey::LastFmSession)
                .is_none()
        );
    }

    #[test]
    #[cfg(unix)]
    fn secret_service_store_is_constructible_without_dbus_work() {
        let _store = SecretServiceStore::new();
    }

    #[test]
    #[cfg(unix)]
    fn secret_service_operations_time_out() {
        let store = SecretServiceStore::new();
        let started = Instant::now();

        let error = store
            .run_with_timeout(
                future::pending::<oo7::Result<()>>(),
                Duration::from_millis(5),
            )
            .expect_err("timeout error");

        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(matches!(error, SecretError::Backend(message) if message.contains("timed out")));
    }
}
