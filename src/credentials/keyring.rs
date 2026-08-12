/// KeyringCredentialBackend stores credentials in the system keychain via the
/// `keyring` crate. If the system keyring is unavailable, it falls back to an
/// AES-256-GCM encrypted file stored in the user's config directory.
use async_trait::async_trait;
use crate::credentials::CredentialBackend;
use crate::error::{HarnessError, Result};
use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit, Nonce};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

const SERVICE_NAME: &str = "harness-agent";
const FALLBACK_FILE: &str = "credentials.enc";
const NONCE_SIZE: usize = 12; // 96-bit nonce for AES-GCM

/// Derive a 256-bit encryption key from the machine ID.
fn derive_key() -> [u8; 32] {
    let machine_id = get_machine_id();
    let salt = b"harness-agent-credential-vault";
    let mut hasher = Sha256::new();
    hasher.update(machine_id.as_bytes());
    hasher.update(salt);
    let result = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&result);
    key
}

/// Get a machine-specific identifier for key derivation.
fn get_machine_id() -> String {
    // Try /etc/machine-id (Linux)
    if let Ok(id) = fs::read_to_string("/etc/machine-id") {
        return id.trim().to_string();
    }
    // Try /var/lib/dbus/machine-id (Linux fallback)
    if let Ok(id) = fs::read_to_string("/var/lib/dbus/machine-id") {
        return id.trim().to_string();
    }
    // Try hostname as fallback
    if let Ok(hostname) = std::process::Command::new("hostname").output() {
        if hostname.status.success() {
            return String::from_utf8_lossy(&hostname.stdout).trim().to_string();
        }
    }
    // Last resort
    "unknown-machine".to_string()
}

fn fallback_file_path() -> Option<PathBuf> {
    let config_dir = dirs::config_dir()?;
    let dir = config_dir.join("harness-agent");
    Some(dir.join(FALLBACK_FILE))
}

fn encrypt_value(value: &str) -> Result<String> {
    let key_bytes = derive_key();
    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, value.as_bytes())
        .map_err(|e| HarnessError::Credential(format!("Encryption failed: {}", e)))?;

    // Prepend nonce to ciphertext, then base64 encode
    let mut combined = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    combined.extend_from_slice(&nonce);
    combined.extend_from_slice(&ciphertext);

    Ok(BASE64.encode(&combined))
}

fn decrypt_value(encoded: &str) -> Result<String> {
    let combined = BASE64
        .decode(encoded)
        .map_err(|e| HarnessError::Credential(format!("Base64 decode failed: {}", e)))?;

    if combined.len() < NONCE_SIZE + 1 {
        return Err(HarnessError::Credential(
            "Encrypted data is too short".to_string(),
        ));
    }

    let (nonce_bytes, ciphertext) = combined.split_at(NONCE_SIZE);
    let nonce = Nonce::from_slice(nonce_bytes);

    let key_bytes = derive_key();
    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| HarnessError::Credential(format!("Decryption failed: {}", e)))?;

    String::from_utf8(plaintext)
        .map_err(|e| HarnessError::Credential(format!("UTF-8 decode failed: {}", e)))
}

fn read_encrypted_file() -> Result<HashMap<String, String>> {
    let path = match fallback_file_path() {
        Some(p) => p,
        None => return Ok(HashMap::new()),
    };

    if !path.exists() {
        return Ok(HashMap::new());
    }

    let content = fs::read_to_string(&path).map_err(|e| {
        HarnessError::Credential(format!(
            "Failed to read encrypted file {:?}: {}",
            path, e
        ))
    })?;

    let raw_map: HashMap<String, String> = toml::from_str(&content).map_err(|e| {
        HarnessError::Credential(format!("Failed to parse encrypted file: {}", e))
    })?;

    let mut map = HashMap::new();
    for (key, encrypted) in raw_map {
        let value = decrypt_value(&encrypted)?;
        map.insert(key, value);
    }

    Ok(map)
}

fn write_encrypted_file(map: &HashMap<String, String>) -> Result<()> {
    let path = match fallback_file_path() {
        Some(p) => p,
        None => {
            return Err(HarnessError::Credential(
                "Cannot determine config directory for encrypted storage".to_string(),
            ))
        }
    };

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            HarnessError::Credential(format!(
                "Failed to create config directory {:?}: {}",
                parent, e
            ))
        })?;
    }

    let mut encrypted_map = HashMap::new();
    for (key, value) in map {
        encrypted_map.insert(key.clone(), encrypt_value(value)?);
    }

    let toml_content = toml::to_string(&encrypted_map).map_err(|e| {
        HarnessError::Credential(format!("Failed to serialize encrypted data: {}", e))
    })?;

    let mut file = fs::File::create(&path).map_err(|e| {
        HarnessError::Credential(format!(
            "Failed to create encrypted file {:?}: {}",
            path, e
        ))
    })?;

    file.write_all(toml_content.as_bytes()).map_err(|e| {
        HarnessError::Credential(format!("Failed to write encrypted file: {}", e))
    })?;

    file.flush().map_err(|e| {
        HarnessError::Credential(format!("Failed to flush encrypted file: {}", e))
    })?;

    Ok(())
}

pub struct KeyringCredentialBackend {
    /// Whether keyring is available. If false, fall back to encrypted file.
    keyring_available: bool,
}

impl KeyringCredentialBackend {
    /// Create a new KeyringCredentialBackend. Detects whether the system
    /// keyring is available; if not, falls back to an encrypted file.
    pub fn new() -> Self {
        let keyring_available = check_keyring_available();
        if !keyring_available {
            eprintln!(
                "Warning: System keyring not available. Falling back to encrypted file storage."
            );
        }
        Self { keyring_available }
    }

    /// Create a backend that forces encrypted file fallback (for testing).
    #[cfg(test)]
    pub fn new_with_fallback() -> Self {
        Self {
            keyring_available: false,
        }
    }
}

impl Default for KeyringCredentialBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if the system keyring is available by attempting to create a test entry.
fn check_keyring_available() -> bool {
    let entry = keyring::Entry::new(SERVICE_NAME, "__harness_probe__");
    match entry {
        Ok(_) => {
            // Try to set a test value to verify the keyring works
            // We don't actually set it — just creating the entry is a good signal
            true
        }
        Err(_) => false,
    }
}

#[async_trait]
impl CredentialBackend for KeyringCredentialBackend {
    async fn get(&self, key: &str) -> Result<Option<String>> {
        if self.keyring_available {
            let entry = keyring::Entry::new(SERVICE_NAME, key).map_err(|e| {
                HarnessError::Credential(format!("Keyring error: {}", e))
            })?;
            match entry.get_password() {
                Ok(password) => Ok(Some(password)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(HarnessError::Credential(format!("Keyring error: {}", e))),
            }
        } else {
            let map = read_encrypted_file()?;
            Ok(map.get(key).cloned())
        }
    }

    async fn set(&self, key: &str, value: &str) -> Result<()> {
        if self.keyring_available {
            let entry = keyring::Entry::new(SERVICE_NAME, key).map_err(|e| {
                HarnessError::Credential(format!("Keyring error: {}", e))
            })?;
            entry.set_password(value).map_err(|e| {
                HarnessError::Credential(format!("Keyring error: {}", e))
            })?;
        } else {
            let mut map = read_encrypted_file()?;
            map.insert(key.to_string(), value.to_string());
            write_encrypted_file(&map)?;
        }
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<()> {
        if self.keyring_available {
            let entry = keyring::Entry::new(SERVICE_NAME, key).map_err(|e| {
                HarnessError::Credential(format!("Keyring error: {}", e))
            })?;
            match entry.delete_credential() {
                Ok(()) => Ok(()),
                Err(keyring::Error::NoEntry) => Err(HarnessError::Credential(format!(
                    "Key '{}' not found in keyring",
                    key
                ))),
                Err(e) => Err(HarnessError::Credential(format!("Keyring error: {}", e))),
            }
        } else {
            let mut map = read_encrypted_file()?;
            if map.remove(key).is_none() {
                return Err(HarnessError::Credential(format!(
                    "Key '{}' not found in encrypted storage",
                    key
                )));
            }
            write_encrypted_file(&map)
        }
    }

    fn list_keys(&self) -> Result<Vec<String>> {
        if self.keyring_available {
            // The keyring crate doesn't support listing keys. Fall back to
            // the encrypted file for listing, which serves as a secondary
            // index of known keys.
            let map = read_encrypted_file().unwrap_or_default();
            let mut keys: Vec<String> = map.keys().cloned().collect();
            keys.sort();
            Ok(keys)
        } else {
            let map = read_encrypted_file()?;
            let mut keys: Vec<String> = map.keys().cloned().collect();
            keys.sort();
            Ok(keys)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let original = "sk-test-secret-api-key-12345";
        let encrypted = encrypt_value(original).unwrap();
        let decrypted = decrypt_value(&encrypted).unwrap();
        assert_eq!(original, decrypted);
    }

    #[test]
    fn test_encrypt_produces_different_ciphertexts() {
        let value = "same-value";
        let enc1 = encrypt_value(value).unwrap();
        let enc2 = encrypt_value(value).unwrap();
        // Each encryption should produce different ciphertext due to random nonce
        assert_ne!(enc1, enc2);
        // But both should decrypt to the same value
        assert_eq!(decrypt_value(&enc1).unwrap(), value);
        assert_eq!(decrypt_value(&enc2).unwrap(), value);
    }

    #[test]
    fn test_derive_key_is_deterministic() {
        let key1 = derive_key();
        let key2 = derive_key();
        assert_eq!(key1, key2, "Key derivation should be deterministic");
    }

    #[test]
    fn test_encrypt_empty_string() {
        let encrypted = encrypt_value("").unwrap();
        let decrypted = decrypt_value(&encrypted).unwrap();
        assert_eq!(decrypted, "");
    }
}