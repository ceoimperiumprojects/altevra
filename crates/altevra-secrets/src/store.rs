//! Secret storage backends — OS keyring (primary) with an encrypted local file
//! fallback for headless / CI environments.

use anyhow::{anyhow, Context, Result};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Default sidecar path for keyring manifest (list of key names).
const KEYRING_MANIFEST_NAME: &str = "secrets-keys.json";

/// Selects how secrets are persisted.
#[derive(Debug, Clone)]
pub enum SecretBackend {
    /// Use the OS keyring (Linux Secret Service, macOS Keychain, Windows
    /// Credential Manager). A small sidecar JSON file tracks the set of known
    /// key names so `list_keys()` can work.
    Keyring,
    /// Encrypted JSON map on disk. The encryption key is derived from the
    /// value of the environment variable named by `key_env` (via SHA-256 to a
    /// 32-byte key). Useful for headless servers or tests.
    EncryptedFile {
        /// Where the encrypted blob lives (e.g. `~/.altevra/secrets.enc`).
        path: PathBuf,
        /// Name of the env var holding the master passphrase.
        key_env: String,
    },
}

/// Top-level secret store. Cheap to clone-by-value — clone the `SecretStore`
/// if you need to pass it across threads.
#[derive(Debug, Clone)]
pub struct SecretStore {
    backend: SecretBackend,
    service: String,
}

impl SecretStore {
    /// Build a store backed by the OS keyring. `service` is the service name
    /// passed to `keyring::Entry::new(service, key)`.
    pub fn new_keyring(service: &str) -> Self {
        Self {
            backend: SecretBackend::Keyring,
            service: service.to_string(),
        }
    }

    /// Build an encrypted-file backed store. `path` is the on-disk location of
    /// the ciphertext blob; `key_env` is the env var that holds the
    /// passphrase.
    pub fn new_encrypted_file(service: &str, path: PathBuf, key_env: &str) -> Self {
        Self {
            backend: SecretBackend::EncryptedFile {
                path,
                key_env: key_env.to_string(),
            },
            service: service.to_string(),
        }
    }

    /// Insert (or overwrite) the value for `key`.
    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        match &self.backend {
            SecretBackend::Keyring => {
                let entry = keyring::Entry::new(&self.service, key)
                    .with_context(|| format!("keyring entry for {key}"))?;
                entry
                    .set_password(value)
                    .with_context(|| format!("keyring set_password for {key}"))?;
                self.manifest_add(key)?;
                Ok(())
            }
            SecretBackend::EncryptedFile { path, key_env } => {
                let mut map = read_encrypted_map(path, key_env).unwrap_or_default();
                map.insert(key.to_string(), value.to_string());
                write_encrypted_map(path, key_env, &map)
            }
        }
    }

    /// Look up `key`. Returns `Ok(None)` when the key does not exist.
    pub fn get(&self, key: &str) -> Result<Option<String>> {
        match &self.backend {
            SecretBackend::Keyring => {
                let entry = keyring::Entry::new(&self.service, key)
                    .with_context(|| format!("keyring entry for {key}"))?;
                match entry.get_password() {
                    Ok(v) => Ok(Some(v)),
                    Err(keyring::Error::NoEntry) => Ok(None),
                    Err(e) => Err(anyhow!("keyring get_password for {key}: {e}")),
                }
            }
            SecretBackend::EncryptedFile { path, key_env } => {
                if !path.exists() {
                    return Ok(None);
                }
                let map = read_encrypted_map(path, key_env)?;
                Ok(map.get(key).cloned())
            }
        }
    }

    /// Delete `key`. No-op when the key does not exist.
    pub fn delete(&self, key: &str) -> Result<()> {
        match &self.backend {
            SecretBackend::Keyring => {
                let entry = keyring::Entry::new(&self.service, key)
                    .with_context(|| format!("keyring entry for {key}"))?;
                match entry.delete_credential() {
                    Ok(_) => {}
                    Err(keyring::Error::NoEntry) => {}
                    Err(e) => return Err(anyhow!("keyring delete_credential for {key}: {e}")),
                }
                self.manifest_remove(key)?;
                Ok(())
            }
            SecretBackend::EncryptedFile { path, key_env } => {
                if !path.exists() {
                    return Ok(());
                }
                let mut map = read_encrypted_map(path, key_env)?;
                map.remove(key);
                write_encrypted_map(path, key_env, &map)
            }
        }
    }

    /// Enumerate known key names.
    ///
    /// For the keyring backend this reads the sidecar manifest written by
    /// [`set`] / [`delete`]. For the encrypted-file backend this returns the
    /// keys of the decrypted map.
    pub fn list_keys(&self) -> Result<Vec<String>> {
        match &self.backend {
            SecretBackend::Keyring => self.manifest_read(),
            SecretBackend::EncryptedFile { path, key_env } => {
                if !path.exists() {
                    return Ok(Vec::new());
                }
                let map = read_encrypted_map(path, key_env)?;
                let mut keys: Vec<String> = map.keys().cloned().collect();
                keys.sort();
                Ok(keys)
            }
        }
    }

    // ---- keyring sidecar manifest ----------------------------------------

    fn manifest_path() -> Result<PathBuf> {
        let home = home_dir()?;
        let dir = home.join(".altevra");
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        Ok(dir.join(KEYRING_MANIFEST_NAME))
    }

    fn manifest_read(&self) -> Result<Vec<String>> {
        let path = Self::manifest_path()?;
        if !path.exists() {
            return Ok(Vec::new());
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("read manifest {}", path.display()))?;
        if raw.trim().is_empty() {
            return Ok(Vec::new());
        }
        let manifest: Manifest = serde_json::from_str(&raw).context("parse keyring manifest")?;
        let mut keys = manifest
            .services
            .get(&self.service)
            .cloned()
            .unwrap_or_default();
        keys.sort();
        keys.dedup();
        Ok(keys)
    }

    fn manifest_add(&self, key: &str) -> Result<()> {
        let path = Self::manifest_path()?;
        let mut manifest: Manifest = if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("read manifest {}", path.display()))?;
            if raw.trim().is_empty() {
                Manifest::default()
            } else {
                serde_json::from_str(&raw).unwrap_or_default()
            }
        } else {
            Manifest::default()
        };
        let entry = manifest.services.entry(self.service.clone()).or_default();
        if !entry.iter().any(|k| k == key) {
            entry.push(key.to_string());
            entry.sort();
        }
        fs::write(&path, serde_json::to_string_pretty(&manifest)?)
            .with_context(|| format!("write manifest {}", path.display()))?;
        Ok(())
    }

    fn manifest_remove(&self, key: &str) -> Result<()> {
        let path = Self::manifest_path()?;
        if !path.exists() {
            return Ok(());
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("read manifest {}", path.display()))?;
        if raw.trim().is_empty() {
            return Ok(());
        }
        let mut manifest: Manifest = serde_json::from_str(&raw).unwrap_or_default();
        if let Some(entry) = manifest.services.get_mut(&self.service) {
            entry.retain(|k| k != key);
        }
        fs::write(&path, serde_json::to_string_pretty(&manifest)?)
            .with_context(|| format!("write manifest {}", path.display()))?;
        Ok(())
    }
}

// ---- helpers --------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    /// service name -> list of key names
    #[serde(default)]
    services: BTreeMap<String, Vec<String>>,
}

fn home_dir() -> Result<PathBuf> {
    // `std::env::home_dir` is deprecated; replicate the simple HOME lookup.
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("could not determine home directory (HOME/USERPROFILE unset)"))
}

fn derive_key(passphrase: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(passphrase.as_bytes());
    let out = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&out);
    key
}

fn random_nonce() -> [u8; 24] {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Best-effort entropy: hash (clock || process id || hash of self addr).
    // For production we would prefer `getrandom`, but we avoid adding the dep
    // here — the nonce uniqueness is the only requirement and SHA-256 of a
    // monotonic counter + clock is sufficient for distinct writes in process.
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mut hasher = Sha256::new();
    hasher.update(ns.to_le_bytes());
    hasher.update(pid.to_le_bytes());
    // Mix in a per-call counter using a static atomic.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    hasher.update(c.to_le_bytes());
    let digest = hasher.finalize();
    let mut nonce = [0u8; 24];
    nonce.copy_from_slice(&digest[..24]);
    nonce
}

fn read_encrypted_map(path: &Path, key_env: &str) -> Result<BTreeMap<String, String>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let passphrase = std::env::var(key_env)
        .with_context(|| format!("env var {key_env} not set for encrypted secrets file"))?;
    let key_bytes = derive_key(&passphrase);
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&key_bytes));

    let blob_hex = fs::read_to_string(path)
        .with_context(|| format!("read encrypted secrets file {}", path.display()))?;
    let blob_hex = blob_hex.trim();
    if blob_hex.is_empty() {
        return Ok(BTreeMap::new());
    }
    let blob = hex::decode(blob_hex).context("decode hex blob")?;
    if blob.len() < 24 {
        return Err(anyhow!("encrypted secrets blob too short"));
    }
    let (nonce_bytes, ciphertext) = blob.split_at(24);
    let nonce = XNonce::from_slice(nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow!("decrypt secrets blob: {e}"))?;
    let map: BTreeMap<String, String> =
        serde_json::from_slice(&plaintext).context("parse decrypted secrets json")?;
    Ok(map)
}

fn write_encrypted_map(path: &Path, key_env: &str, map: &BTreeMap<String, String>) -> Result<()> {
    let passphrase = std::env::var(key_env)
        .with_context(|| format!("env var {key_env} not set for encrypted secrets file"))?;
    let key_bytes = derive_key(&passphrase);
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&key_bytes));
    let nonce_bytes = random_nonce();
    let nonce = XNonce::from_slice(&nonce_bytes);
    let plaintext = serde_json::to_vec(map).context("serialize secrets map")?;
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_ref())
        .map_err(|e| anyhow!("encrypt secrets blob: {e}"))?;
    let mut blob = Vec::with_capacity(24 + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    fs::write(path, hex::encode(&blob))
        .with_context(|| format!("write encrypted secrets file {}", path.display()))?;
    Ok(())
}

// ---- tests ----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const TEST_ENV: &str = "ALTEVRA_TEST_SECRETS_KEY";

    fn enc_store(tmp: &TempDir, env_name: &str) -> SecretStore {
        // Ensure passphrase is set for this env var.
        std::env::set_var(env_name, "test-passphrase-do-not-use-in-prod");
        let path = tmp.path().join("secrets.enc");
        SecretStore::new_encrypted_file("altevra-test", path, env_name)
    }

    #[test]
    fn encrypted_file_set_get_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let store = enc_store(&tmp, "ALTEVRA_TEST_ENC_KEY_RT");
        store.set("OPENAI_API_KEY", "sk-secret-value").unwrap();
        let got = store.get("OPENAI_API_KEY").unwrap();
        assert_eq!(got.as_deref(), Some("sk-secret-value"));
    }

    #[test]
    fn encrypted_file_missing_key_is_none() {
        let tmp = TempDir::new().unwrap();
        let store = enc_store(&tmp, "ALTEVRA_TEST_ENC_KEY_MISS");
        assert!(store.get("DOES_NOT_EXIST").unwrap().is_none());
    }

    #[test]
    fn encrypted_file_delete_removes_value() {
        let tmp = TempDir::new().unwrap();
        let store = enc_store(&tmp, "ALTEVRA_TEST_ENC_KEY_DEL");
        store.set("FOO", "bar").unwrap();
        store.set("BAZ", "qux").unwrap();
        store.delete("FOO").unwrap();
        assert!(store.get("FOO").unwrap().is_none());
        assert_eq!(store.get("BAZ").unwrap().as_deref(), Some("qux"));
    }

    #[test]
    fn encrypted_file_list_keys_sorted() {
        let tmp = TempDir::new().unwrap();
        let store = enc_store(&tmp, "ALTEVRA_TEST_ENC_KEY_LIST");
        store.set("ZULU", "z").unwrap();
        store.set("ALPHA", "a").unwrap();
        store.set("MIKE", "m").unwrap();
        let keys = store.list_keys().unwrap();
        assert_eq!(keys, vec!["ALPHA", "MIKE", "ZULU"]);
    }

    #[test]
    fn encrypted_file_overwrite_updates_value() {
        let tmp = TempDir::new().unwrap();
        let store = enc_store(&tmp, "ALTEVRA_TEST_ENC_KEY_OVR");
        store.set("KEY", "v1").unwrap();
        store.set("KEY", "v2").unwrap();
        assert_eq!(store.get("KEY").unwrap().as_deref(), Some("v2"));
    }

    #[test]
    fn encrypted_file_wrong_passphrase_fails() {
        let tmp = TempDir::new().unwrap();
        let env_name = "ALTEVRA_TEST_ENC_KEY_WRONG";
        std::env::set_var(env_name, "right-passphrase");
        let path = tmp.path().join("secrets.enc");
        let store = SecretStore::new_encrypted_file("altevra-test", path.clone(), env_name);
        store.set("KEY", "value").unwrap();

        // Switch passphrase and attempt to read — must fail (AEAD auth tag).
        std::env::set_var(env_name, "WRONG-passphrase");
        let bad_store = SecretStore::new_encrypted_file("altevra-test", path, env_name);
        let err = bad_store.get("KEY").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("decrypt"),
            "expected decrypt error, got: {msg}"
        );
    }

    #[test]
    fn encrypted_file_blob_is_not_plaintext() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("secrets.enc");
        std::env::set_var(TEST_ENV, "passphrase-1");
        let store = SecretStore::new_encrypted_file("altevra-test", path.clone(), TEST_ENV);
        store.set("API_KEY", "totally-secret-value-xyz").unwrap();

        let blob = std::fs::read_to_string(&path).unwrap();
        assert!(
            !blob.contains("totally-secret-value-xyz"),
            "ciphertext leaked plaintext"
        );
        assert!(!blob.contains("API_KEY"), "ciphertext leaked key name");
    }

    #[test]
    fn derive_key_is_deterministic() {
        let a = derive_key("hello-world");
        let b = derive_key("hello-world");
        let c = derive_key("hello-world!");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn random_nonce_changes_per_call() {
        let a = random_nonce();
        let b = random_nonce();
        assert_ne!(a, b, "nonces must differ across calls");
    }

    // Keyring tests are gated behind `#[ignore]` because most CI runners do
    // not expose an OS keyring daemon. Run manually with:
    //   cargo test -p altevra-secrets -- --ignored
    #[test]
    #[ignore = "requires OS keyring (Linux Secret Service / macOS Keychain / Windows Credential Manager)"]
    fn keyring_set_get_delete_roundtrip() {
        let store = SecretStore::new_keyring("altevra-secrets-test-suite");
        let key = "ALTEVRA_TEST_KEYRING_KEY";
        let _ = store.delete(key);
        store.set(key, "keyring-value").unwrap();
        assert_eq!(store.get(key).unwrap().as_deref(), Some("keyring-value"));
        let listed = store.list_keys().unwrap();
        assert!(listed.iter().any(|k| k == key));
        store.delete(key).unwrap();
        assert!(store.get(key).unwrap().is_none());
    }
}
