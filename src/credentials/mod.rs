use async_trait::async_trait;
use crate::error::{HarnessError, Result};

pub mod env;
pub mod keyring;

#[async_trait]
pub trait CredentialBackend: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<String>>;
    async fn set(&self, key: &str, value: &str) -> Result<()>;
    async fn delete(&self, key: &str) -> Result<()>;
    fn list_keys(&self) -> Result<Vec<String>>;
}

pub struct CredentialManager {
    backend: Box<dyn CredentialBackend>,
}

impl CredentialManager {
    pub fn new(backend: Box<dyn CredentialBackend>) -> Self {
        Self { backend }
    }

    /// List all configured credential keys without revealing plaintext values.
    pub fn key_status(&self) -> Result<String> {
        let keys = self.backend.list_keys()?;
        if keys.is_empty() {
            Ok("No credentials configured.".to_string())
        } else {
            let mut status = String::from("Configured credentials:\n");
            for key in &keys {
                status.push_str(&format!("  {}: [configured]\n", key));
            }
            Ok(status.trim_end().to_string())
        }
    }

    /// Interactively prompt for a key name and its value, then store via the backend.
    pub async fn key_set(&self) -> Result<()> {
        use std::io::{self, Write};

        let mut key_name = String::new();
        print!("Enter key name: ");
        io::stdout()
            .flush()
            .map_err(|e| HarnessError::Credential(format!("Failed to flush stdout: {}", e)))?;
        io::stdin()
            .read_line(&mut key_name)
            .map_err(|e| HarnessError::Credential(format!("Failed to read key name: {}", e)))?;
        let key_name = key_name.trim().to_string();

        if key_name.is_empty() {
            return Err(HarnessError::Credential("Key name cannot be empty".to_string()));
        }

        let password = rpassword::prompt_password(format!("Enter value for '{}': ", key_name))
            .map_err(|e| HarnessError::Credential(format!("Failed to read password: {}", e)))?;

        if password.is_empty() {
            return Err(HarnessError::Credential("Value cannot be empty".to_string()));
        }

        self.backend.set(&key_name, &password).await?;

        eprintln!("Credential '{}' stored successfully.", key_name);
        Ok(())
    }

    /// Remove a stored credential by key name.
    pub async fn key_clear(&self, key: &str) -> Result<()> {
        self.backend.delete(key).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A mock backend for testing CredentialManager logic.
    struct MockBackend {
        store: Mutex<HashMap<String, String>>,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                store: Mutex::new(HashMap::new()),
            }
        }
    }

    #[async_trait]
    impl CredentialBackend for MockBackend {
        async fn get(&self, key: &str) -> Result<Option<String>> {
            let store = self.store.lock().unwrap();
            Ok(store.get(key).cloned())
        }

        async fn set(&self, key: &str, value: &str) -> Result<()> {
            let mut store = self.store.lock().unwrap();
            store.insert(key.to_string(), value.to_string());
            Ok(())
        }

        async fn delete(&self, key: &str) -> Result<()> {
            let mut store = self.store.lock().unwrap();
            store.remove(key);
            Ok(())
        }

        fn list_keys(&self) -> Result<Vec<String>> {
            let store = self.store.lock().unwrap();
            let mut keys: Vec<String> = store.keys().cloned().collect();
            keys.sort();
            Ok(keys)
        }
    }

    #[test]
    fn test_key_status_does_not_reveal_plaintext() {
        let backend = Box::new(MockBackend::new());
        let manager = CredentialManager::new(backend);

        // Initially no credentials
        let status = manager.key_status().unwrap();
        assert_eq!(status, "No credentials configured.");

        // Store some secrets via the backend directly (bypassing interactive prompt)
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(manager.backend.set("openai_api_key", "sk-super-secret-value"))
            .unwrap();
        rt.block_on(
            manager
                .backend
                .set("anthropic_api_key", "sk-ant-another-secret"),
        )
        .unwrap();

        // Check status: keys listed, but values NOT revealed
        let status = manager.key_status().unwrap();
        assert!(status.contains("openai_api_key"), "Status should list openai_api_key");
        assert!(
            status.contains("anthropic_api_key"),
            "Status should list anthropic_api_key"
        );
        assert!(
            status.contains("[configured]"),
            "Status should show [configured] not plaintext"
        );
        // Critical: plaintext values must not appear
        assert!(
            !status.contains("sk-super-secret-value"),
            "Status must NOT reveal the openai secret value"
        );
        assert!(
            !status.contains("sk-ant-another-secret"),
            "Status must NOT reveal the anthropic secret value"
        );
    }

    #[test]
    fn test_key_clear_removes_credential() {
        let backend = Box::new(MockBackend::new());
        let manager = CredentialManager::new(backend);

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(manager.backend.set("test_key", "test_value"))
            .unwrap();

        let keys = manager.backend.list_keys().unwrap();
        assert!(keys.contains(&"test_key".to_string()));

        rt.block_on(manager.key_clear("test_key")).unwrap();

        let keys = manager.backend.list_keys().unwrap();
        assert!(!keys.contains(&"test_key".to_string()));
    }
}