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

pub trait SecretStore: Send + Sync {
    fn save_token(&self, server_id: &ServerId, token: &str) -> SecretResult<()>;
    fn load_token(&self, server_id: &ServerId) -> SecretResult<Option<String>>;
    fn delete_token(&self, server_id: &ServerId) -> SecretResult<()>;
}

#[derive(Clone, Default)]
pub struct MemorySecretStore {
    tokens: Arc<Mutex<HashMap<ServerId, String>>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemorySecretStore {
    fn save_token(&self, server_id: &ServerId, token: &str) -> SecretResult<()> {
        let mut tokens = self.tokens.lock().map_err(|_| SecretError::Locked)?;
        tokens.insert(server_id.clone(), token.to_string());
        Ok(())
    }

    fn load_token(&self, server_id: &ServerId) -> SecretResult<Option<String>> {
        let tokens = self.tokens.lock().map_err(|_| SecretError::Locked)?;
        Ok(tokens.get(server_id).cloned())
    }

    fn delete_token(&self, server_id: &ServerId) -> SecretResult<()> {
        let mut tokens = self.tokens.lock().map_err(|_| SecretError::Locked)?;
        tokens.remove(server_id);
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

    fn token_attributes(&self, server_id: &ServerId) -> Vec<(String, String)> {
        vec![
            ("application".to_string(), self.application.clone()),
            ("kind".to_string(), "jellyfin-token".to_string()),
            ("server_id".to_string(), server_id.as_str().to_string()),
        ]
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
fn should_cache_keyring(sandboxed: bool) -> bool {
    // Native DBus keyrings are tied to the runtime that drives their connection.
    // The sandbox file backend needs reuse to avoid per-instance file locks.
    sandboxed
}

#[cfg(unix)]
impl SecretStore for SecretServiceStore {
    fn save_token(&self, server_id: &ServerId, token: &str) -> SecretResult<()> {
        let attributes = self.token_attributes(server_id);
        let label = format!("Rufin Jellyfin token {}", server_id.as_str());
        let token = token.to_string();

        self.run_with_keyring(move |keyring| async move {
            keyring
                .create_item(&label, &attributes, oo7::Secret::text(token), true)
                .await
        })
    }

    fn load_token(&self, server_id: &ServerId) -> SecretResult<Option<String>> {
        let attributes = self.token_attributes(server_id);
        let secret = self.run_with_keyring(move |keyring| async move {
            let Some(item) = keyring.search_items(&attributes).await?.into_iter().next() else {
                return Ok(None);
            };
            if item.is_locked().await? {
                item.unlock().await?;
            }
            Ok(Some(item.secret().await?))
        })?;

        secret
            .map(|secret| String::from_utf8(secret.as_bytes().to_vec()))
            .transpose()
            .map_err(|_| SecretError::Utf8)
    }

    fn delete_token(&self, server_id: &ServerId) -> SecretResult<()> {
        let attributes = self.token_attributes(server_id);

        self.run_with_keyring(move |keyring| async move { keyring.delete(&attributes).await })
    }
}

#[cfg(test)]
mod tests {
    use super::{MemorySecretStore, SecretStore};
    #[cfg(unix)]
    use super::{SecretError, SecretServiceStore};
    use rufin_core::ServerId;
    #[cfg(unix)]
    use std::future;
    #[cfg(unix)]
    use std::time::{Duration, Instant};

    #[test]
    fn memory_secret_store_round_trips_tokens() {
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
