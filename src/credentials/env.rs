/// EnvCredentialBackend reads and writes credentials to a `.env` file.
///
/// # Security Warning
///
/// This backend stores credentials in **plaintext** on disk. It is intended for
/// **development use only**. For production deployments, use the
/// `KeyringCredentialBackend` which stores credentials in the system keychain
/// or an encrypted file.
use async_trait::async_trait;
use crate::credentials::CredentialBackend;
use crate::error::{HarnessError, Result};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

pub struct EnvCredentialBackend {
    file_path: PathBuf,
}

impl EnvCredentialBackend {
    /// Create a new EnvCredentialBackend that stores credentials in the given file.
    pub fn new(file_path: impl Into<PathBuf>) -> Self {
        Self {
            file_path: file_path.into(),
        }
    }

    /// Create a new EnvCredentialBackend with the default `.env` file path.
    pub fn default_env() -> Self {
        Self {
            file_path: PathBuf::from(".env"),
        }
    }

    fn read_env(&self) -> Result<HashMap<String, String>> {
        let mut map = HashMap::new();
        if !self.file_path.exists() {
            return Ok(map);
        }
        let file = fs::File::open(&self.file_path).map_err(|e| {
            HarnessError::Credential(format!(
                "Failed to open env file {:?}: {}",
                self.file_path, e
            ))
        })?;
        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line = line.map_err(|e| {
                HarnessError::Credential(format!("Failed to read env file: {}", e))
            })?;
            let trimmed = line.trim();
            // Skip empty lines and comments
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some(eq_pos) = trimmed.find('=') {
                let key = trimmed[..eq_pos].trim().to_string();
                let value = trimmed[eq_pos + 1..].trim().to_string();
                // Strip surrounding quotes if present
                let value = if (value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\''))
                {
                    value[1..value.len() - 1].to_string()
                } else {
                    value
                };
                if !key.is_empty() {
                    map.insert(key, value);
                }
            }
        }
        Ok(map)
    }

    fn write_env(&self, map: &HashMap<String, String>) -> Result<()> {
        // Ensure the parent directory exists
        if let Some(parent) = self.file_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| {
                    HarnessError::Credential(format!(
                        "Failed to create directory {:?}: {}",
                        parent, e
                    ))
                })?;
            }
        }

        let mut file = fs::File::create(&self.file_path).map_err(|e| {
            HarnessError::Credential(format!(
                "Failed to create env file {:?}: {}",
                self.file_path, e
            ))
        })?;

        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();

        for key in keys {
            let value = &map[key];
            writeln!(file, "{}={}", key, value).map_err(|e| {
                HarnessError::Credential(format!("Failed to write to env file: {}", e))
            })?;
        }

        file.flush().map_err(|e| {
            HarnessError::Credential(format!("Failed to flush env file: {}", e))
        })?;

        Ok(())
    }
}

#[async_trait]
impl CredentialBackend for EnvCredentialBackend {
    async fn get(&self, key: &str) -> Result<Option<String>> {
        let map = self.read_env()?;
        Ok(map.get(key).cloned())
    }

    async fn set(&self, key: &str, value: &str) -> Result<()> {
        let mut map = self.read_env()?;
        map.insert(key.to_string(), value.to_string());
        self.write_env(&map)
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let mut map = self.read_env()?;
        if map.remove(key).is_none() {
            return Err(HarnessError::Credential(format!(
                "Key '{}' not found in env file",
                key
            )));
        }
        self.write_env(&map)
    }

    fn list_keys(&self) -> Result<Vec<String>> {
        let map = self.read_env()?;
        let mut keys: Vec<String> = map.keys().cloned().collect();
        keys.sort();
        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn create_backend() -> (EnvCredentialBackend, NamedTempFile) {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_path_buf();
        // tempfile creates the file; we need to close it so we can write to it
        drop(temp_file);
        // Create a fresh empty file for the backend
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"").unwrap();
        f.flush().unwrap();
        // Return a new NamedTempFile for cleanup (the old one was dropped)
        let temp_file = NamedTempFile::new().unwrap();
        let _ = std::fs::remove_file(temp_file.path());
        std::fs::copy(&path, temp_file.path()).unwrap();
        let backend = EnvCredentialBackend::new(temp_file.path().to_path_buf());
        (backend, temp_file)
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Runtime::new().unwrap()
    }

    #[test]
    fn test_env_backend_set_and_get() {
        let (backend, _temp) = create_backend();
        let rt = runtime();

        // Initially, key should not exist
        let val = rt.block_on(backend.get("OPENAI_API_KEY")).unwrap();
        assert!(val.is_none(), "Key should not exist initially");

        // Set a key
        rt.block_on(backend.set("OPENAI_API_KEY", "sk-test-123")).unwrap();

        // Get the key back
        let val = rt.block_on(backend.get("OPENAI_API_KEY")).unwrap();
        assert_eq!(val, Some("sk-test-123".to_string()));

        // Set another key
        rt.block_on(backend.set("ANTHROPIC_API_KEY", "sk-ant-test-456"))
            .unwrap();

        // Both keys should be retrievable
        let val1 = rt.block_on(backend.get("OPENAI_API_KEY")).unwrap();
        let val2 = rt.block_on(backend.get("ANTHROPIC_API_KEY")).unwrap();
        assert_eq!(val1, Some("sk-test-123".to_string()));
        assert_eq!(val2, Some("sk-ant-test-456".to_string()));
    }

    #[test]
    fn test_env_backend_delete() {
        let (backend, _temp) = create_backend();
        let rt = runtime();

        // Set a key
        rt.block_on(backend.set("OPENAI_API_KEY", "sk-test-123")).unwrap();

        // Verify it exists
        let val = rt.block_on(backend.get("OPENAI_API_KEY")).unwrap();
        assert_eq!(val, Some("sk-test-123".to_string()));

        // Delete the key
        rt.block_on(backend.delete("OPENAI_API_KEY")).unwrap();

        // Verify it's gone
        let val = rt.block_on(backend.get("OPENAI_API_KEY")).unwrap();
        assert!(val.is_none(), "Key should be deleted");

        // Deleting a non-existent key should error
        let result = rt.block_on(backend.delete("NONEXISTENT_KEY"));
        assert!(result.is_err(), "Deleting non-existent key should error");
    }

    #[test]
    fn test_env_backend_list_keys() {
        let (backend, _temp) = create_backend();
        let rt = runtime();

        // Initially empty
        let keys = backend.list_keys().unwrap();
        assert!(keys.is_empty());

        // Add keys
        rt.block_on(backend.set("KEY_A", "val_a")).unwrap();
        rt.block_on(backend.set("KEY_B", "val_b")).unwrap();
        rt.block_on(backend.set("KEY_C", "val_c")).unwrap();

        // List should return sorted keys
        let keys = backend.list_keys().unwrap();
        assert_eq!(keys, vec!["KEY_A", "KEY_B", "KEY_C"]);
    }

    #[test]
    fn test_env_backend_overwrite() {
        let (backend, _temp) = create_backend();
        let rt = runtime();

        rt.block_on(backend.set("MY_KEY", "original")).unwrap();
        let val = rt.block_on(backend.get("MY_KEY")).unwrap();
        assert_eq!(val, Some("original".to_string()));

        // Overwrite
        rt.block_on(backend.set("MY_KEY", "updated")).unwrap();
        let val = rt.block_on(backend.get("MY_KEY")).unwrap();
        assert_eq!(val, Some("updated".to_string()));
    }

    #[test]
    fn test_env_backend_persistence() {
        let (backend, temp) = create_backend();
        let rt = runtime();
        let path = temp.path().to_path_buf();

        rt.block_on(backend.set("PERSIST_KEY", "persist_value"))
            .unwrap();

        // Create a new backend pointing to the same file
        let backend2 = EnvCredentialBackend::new(path);
        let val = rt.block_on(backend2.get("PERSIST_KEY")).unwrap();
        assert_eq!(val, Some("persist_value".to_string()));
    }
}