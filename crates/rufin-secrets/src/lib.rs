use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rufin_core::ServerId;
#[cfg(unix)]
use secret_service::{EncryptionType, blocking::SecretService};
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

    fn connect(&self) -> SecretResult<SecretService<'static>> {
        SecretService::connect(EncryptionType::Dh)
            .map_err(|error| SecretError::Backend(error.to_string()))
    }
}

#[cfg(unix)]
impl SecretStore for SecretServiceStore {
    fn save_token(&self, server_id: &ServerId, token: &str) -> SecretResult<()> {
        let service = self.connect()?;
        let collection = service
            .get_default_collection()
            .or_else(|_| service.get_any_collection())
            .map_err(|error| SecretError::Backend(error.to_string()))?;
        let mut attributes = HashMap::new();
        attributes.insert("application", self.application.as_str());
        attributes.insert("kind", "jellyfin-token");
        attributes.insert("server_id", server_id.as_str());
        let label = format!("Rufin Jellyfin token {}", server_id.as_str());

        collection
            .create_item(&label, attributes, token.as_bytes(), true, "text/plain")
            .map_err(|error| SecretError::Backend(error.to_string()))?;
        Ok(())
    }

    fn load_token(&self, server_id: &ServerId) -> SecretResult<Option<String>> {
        let service = self.connect()?;
        let mut attributes = HashMap::new();
        attributes.insert("application", self.application.as_str());
        attributes.insert("kind", "jellyfin-token");
        attributes.insert("server_id", server_id.as_str());

        let results = service
            .search_items(attributes)
            .map_err(|error| SecretError::Backend(error.to_string()))?;
        let item = if let Some(item) = results.unlocked.first() {
            item
        } else if let Some(item) = results.locked.first() {
            item.unlock()
                .map_err(|error| SecretError::Backend(error.to_string()))?;
            item
        } else {
            return Ok(None);
        };

        let secret = item
            .get_secret()
            .map_err(|error| SecretError::Backend(error.to_string()))?;
        String::from_utf8(secret)
            .map(Some)
            .map_err(|_| SecretError::Utf8)
    }

    fn delete_token(&self, server_id: &ServerId) -> SecretResult<()> {
        let service = self.connect()?;
        let mut attributes = HashMap::new();
        attributes.insert("application", self.application.as_str());
        attributes.insert("kind", "jellyfin-token");
        attributes.insert("server_id", server_id.as_str());

        let results = service
            .search_items(attributes)
            .map_err(|error| SecretError::Backend(error.to_string()))?;
        for item in results.unlocked.iter().chain(results.locked.iter()) {
            if item
                .is_locked()
                .map_err(|error| SecretError::Backend(error.to_string()))?
            {
                item.unlock()
                    .map_err(|error| SecretError::Backend(error.to_string()))?;
            }
            item.delete()
                .map_err(|error| SecretError::Backend(error.to_string()))?;
        }
        Ok(())
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
