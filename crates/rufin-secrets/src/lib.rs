use std::collections::HashMap;
#[cfg(unix)]
use std::future::Future;
use std::sync::{Arc, Mutex};

use rufin_core::ServerId;
use thiserror::Error;

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
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| SecretError::Backend(error.to_string()))?;
        runtime
            .block_on(operation)
            .map_err(|error| SecretError::Backend(error.to_string()))
    }
}

#[cfg(unix)]
impl SecretStore for SecretServiceStore {
    fn save_token(&self, server_id: &ServerId, token: &str) -> SecretResult<()> {
        let attributes = self.token_attributes(server_id);
        let label = format!("Rufin Jellyfin token {}", server_id.as_str());
        let token = token.to_string();

        self.run(async move {
            let keyring = oo7::Keyring::new().await?;
            keyring
                .create_item(&label, &attributes, oo7::Secret::text(token), true)
                .await
        })
    }

    fn load_token(&self, server_id: &ServerId) -> SecretResult<Option<String>> {
        let attributes = self.token_attributes(server_id);
        let secret = self.run(async move {
            let keyring = oo7::Keyring::new().await?;
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

        self.run(async move {
            let keyring = oo7::Keyring::new().await?;
            keyring.delete(&attributes).await
        })
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::SecretServiceStore;
    use super::{MemorySecretStore, SecretStore};
    use rufin_core::ServerId;

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
}
