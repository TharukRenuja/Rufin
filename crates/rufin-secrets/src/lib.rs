use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rufin_core::ServerId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("secret store lock was poisoned")]
    Locked,
}

pub type SecretResult<T> = Result<T, SecretError>;

pub trait SecretStore {
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

#[cfg(test)]
mod tests {
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
}
