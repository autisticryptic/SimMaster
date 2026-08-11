//! Per-SIM entitlement state and secret stores.
//!
//! These stores are deliberately separate from the user `SimOverrideStore`:
//! background query/provisioning work may only ever update *these* files and
//! must never rewrite the user's address override file.
//!
//! Layout under the store root (`E911_STATE_DIR`, default `/data/simadmin/e911`):
//!   `<root>/state/<binding-sha256>.json`   — non-secret entitlement record
//!   `<root>/secret/<binding-sha256>.json`  — encrypted secrets (token, cookie,
//!                                            ServiceFlow URL/user data)
//!   `<root>/secret.key`                    — device-local AES-256-GCM key
//!
//! Secrets are encrypted at rest with the device key; the key file is created
//! once with 0600 permissions. A lost key makes old secrets unreadable (fail
//! closed), which is safer than persisting plaintext.

use std::path::{Path, PathBuf};

use ring::aead::{LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

use crate::connectivity::core::entitlement::E911EntitlementRecord;
use crate::connectivity::modems::ims::profile_override::SimBindingKey;

pub const E911_STATE_SCHEMA_VERSION: u32 = 1;

/// Error codes for the E911 state store. Keep them greppable and short.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E911StoreError {
    code: String,
}

impl E911StoreError {
    fn new(code: impl Into<String>) -> Self {
        Self { code: code.into() }
    }

    pub fn code(&self) -> &str {
        &self.code
    }
}

impl std::fmt::Display for E911StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.code)
    }
}

impl std::error::Error for E911StoreError {}

/// On-disk wrapper for the non-secret record.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StateFile {
    schema_version: u32,
    binding: StoredBinding,
    #[serde(flatten)]
    record: E911EntitlementRecord,
}

/// Mirror of the binding snapshot so a state file is never applied to a
/// different SIM.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredBinding {
    kind: String,
    iccid: String,
    eid: Option<String>,
    profile_iccid: Option<String>,
}

impl StoredBinding {
    fn from(key: &SimBindingKey) -> Self {
        match key {
            SimBindingKey::Plain { iccid } => Self {
                kind: "plain".to_string(),
                iccid: iccid.clone(),
                eid: None,
                profile_iccid: None,
            },
            SimBindingKey::Euicc { eid, profile_iccid } => Self {
                kind: "euicc".to_string(),
                iccid: String::new(),
                eid: Some(eid.clone()),
                profile_iccid: Some(profile_iccid.clone()),
            },
        }
    }

    fn matches(&self, key: &SimBindingKey) -> bool {
        match key {
            SimBindingKey::Plain { iccid } => self.kind == "plain" && self.iccid == *iccid,
            SimBindingKey::Euicc { eid, profile_iccid } => {
                self.kind == "euicc"
                    && self.eid.as_deref() == Some(eid.as_str())
                    && self.profile_iccid.as_deref() == Some(profile_iccid.as_str())
            }
        }
    }
}

/// What a secret file carries. Token, cookie and `ServiceFlow_UserData` are
/// treated as secrets per the research doc §12.3; the flow URL is kept beside
/// them so operation creation cannot substitute the entitlement endpoint.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct E911Secrets {
    pub entitlement_token: Option<String>,
    pub configuration_version: Option<u64>,
    pub cookie: Option<String>,
    /// SSRF-validated carrier websheet URL. Kept encrypted with the associated
    /// user data so operation creation never substitutes the entitlement URL.
    pub server_flow_url: Option<String>,
    pub server_flow_user_data: Option<String>,
}

impl E911Secrets {
    pub fn is_empty(&self) -> bool {
        self.entitlement_token.is_none()
            && self.configuration_version.is_none()
            && self.cookie.is_none()
            && self.server_flow_url.is_none()
            && self.server_flow_user_data.is_none()
    }
}

/// Device-local encrypted secret store + plaintext state store.
#[derive(Debug, Clone)]
pub struct E911StateStore {
    state_dir: PathBuf,
    secret_dir: PathBuf,
    key_path: PathBuf,
}

impl E911StateStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            state_dir: root.join("state"),
            secret_dir: root.join("secret"),
            key_path: root.join("secret.key"),
        }
    }

    pub fn default() -> Self {
        if let Some(dir) = std::env::var_os("E911_STATE_DIR") {
            if !dir.is_empty() {
                return Self::new(PathBuf::from(dir));
            }
        }
        Self::new("/data/simadmin/e911")
    }

    pub fn root(&self) -> PathBuf {
        self.state_dir.parent().unwrap().to_path_buf()
    }

    fn state_path(&self, key: &SimBindingKey) -> PathBuf {
        self.state_dir.join(format!("{}.json", key.sha256()))
    }

    fn secret_path(&self, key: &SimBindingKey) -> PathBuf {
        self.secret_dir.join(format!("{}.json", key.sha256()))
    }

    /// Load the non-secret record. Missing file is the normal state.
    pub fn load(&self, key: &SimBindingKey) -> Result<E911EntitlementRecord, E911StoreError> {
        let path = self.state_path(key);
        if !path.exists() {
            return Ok(E911EntitlementRecord::default());
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| E911StoreError::new(format!("e911_state_read_failed:{error}")))?;
        let file: StateFile = serde_json::from_slice(&bytes)
            .map_err(|error| E911StoreError::new(format!("e911_state_corrupt:{error}")))?;
        if file.schema_version != E911_STATE_SCHEMA_VERSION {
            return Err(E911StoreError::new(format!(
                "e911_state_unsupported_schema:{}",
                file.schema_version
            )));
        }
        if !file.binding.matches(key) {
            return Err(E911StoreError::new("e911_state_binding_mismatch"));
        }
        Ok(file.record)
    }

    /// Save the non-secret record (atomic temp+rename).
    pub fn save(
        &self,
        key: &SimBindingKey,
        record: &E911EntitlementRecord,
    ) -> Result<(), E911StoreError> {
        std::fs::create_dir_all(&self.state_dir)
            .map_err(|error| E911StoreError::new(format!("e911_state_mkdir_failed:{error}")))?;
        #[cfg(unix)]
        set_dir_permissions(&self.state_dir)?;

        let file = StateFile {
            schema_version: E911_STATE_SCHEMA_VERSION,
            binding: StoredBinding::from(key),
            record: record.clone(),
        };
        let content = serde_json::to_string_pretty(&file)
            .map_err(|error| E911StoreError::new(format!("e911_state_serialize_failed:{error}")))?;
        write_atomic(
            &self.state_dir,
            &self.state_path(key),
            content.as_bytes(),
            "e911_state",
        )?;
        Ok(())
    }

    /// Load secrets for a binding. A corrupted or unreadable secret file fails
    /// closed (no fallback to plaintext).
    pub fn load_secrets(&self, key: &SimBindingKey) -> Result<E911Secrets, E911StoreError> {
        let path = self.secret_path(key);
        if !path.exists() {
            return Ok(E911Secrets::default());
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| E911StoreError::new(format!("e911_secret_read_failed:{error}")))?;
        let key_bytes = self.load_or_create_key()?;
        let plaintext = decrypt(&key_bytes, &bytes)
            .map_err(|error| E911StoreError::new(format!("e911_secret_decrypt_failed:{error}")))?;
        serde_json::from_slice(&plaintext)
            .map_err(|error| E911StoreError::new(format!("e911_secret_corrupt:{error}")))
    }

    /// Persist secrets for a binding, encrypted with the device key.
    pub fn save_secrets(
        &self,
        key: &SimBindingKey,
        secrets: &E911Secrets,
    ) -> Result<(), E911StoreError> {
        std::fs::create_dir_all(&self.secret_dir)
            .map_err(|error| E911StoreError::new(format!("e911_secret_mkdir_failed:{error}")))?;
        #[cfg(unix)]
        set_dir_permissions(&self.secret_dir)?;
        let key_bytes = self.load_or_create_key()?;
        let content = serde_json::to_vec(secrets).map_err(|error| {
            E911StoreError::new(format!("e911_secret_serialize_failed:{error}"))
        })?;
        let ciphertext = encrypt(&key_bytes, &content)
            .map_err(|error| E911StoreError::new(format!("e911_secret_encrypt_failed:{error}")))?;
        write_atomic(
            &self.secret_dir,
            &self.secret_path(key),
            &ciphertext,
            "e911_secret",
        )?;
        Ok(())
    }

    /// Remove both files for a binding (e.g. SIM removed).
    pub fn delete(&self, key: &SimBindingKey) -> Result<(), E911StoreError> {
        for path in [self.state_path(key), self.secret_path(key)] {
            if path.exists() {
                std::fs::remove_file(&path)
                    .map_err(|error| E911StoreError::new(format!("e911_delete_failed:{error}")))?;
            }
        }
        Ok(())
    }

    fn load_or_create_key(&self) -> Result<Vec<u8>, E911StoreError> {
        if let Ok(bytes) = std::fs::read(&self.key_path) {
            if bytes.len() == 32 {
                return Ok(bytes);
            }
            // Wrong-length key is fatal: do not overwrite silently.
            return Err(E911StoreError::new("e911_key_corrupt"));
        }
        let parent = self
            .key_path
            .parent()
            .ok_or_else(|| E911StoreError::new("e911_key_no_parent"))?;
        std::fs::create_dir_all(parent)
            .map_err(|error| E911StoreError::new(format!("e911_key_mkdir_failed:{error}")))?;
        let mut key = [0u8; 32];
        SystemRandom::new()
            .fill(&mut key)
            .map_err(|_| E911StoreError::new("e911_key_random_failed"))?;
        write_atomic(parent, &self.key_path, &key, "e911_key")?;
        Ok(key.to_vec())
    }
}

/// Write a file atomically (temp + fsync + rename + dir fsync).
fn write_atomic(dir: &Path, path: &Path, content: &[u8], tag: &str) -> Result<(), E911StoreError> {
    use std::io::Write;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let temp_path = dir.join(format!(".{file_name}.tmp"));
    {
        let mut temp = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temp_path)
            .map_err(|error| E911StoreError::new(format!("{tag}_write_tmp_failed:{error}")))?;
        temp.write_all(content)
            .map_err(|error| E911StoreError::new(format!("{tag}_write_tmp_failed:{error}")))?;
        temp.sync_all()
            .map_err(|error| E911StoreError::new(format!("{tag}_sync_failed:{error}")))?;
    }
    #[cfg(unix)]
    set_file_permissions(&temp_path)?;
    if let Err(error) = std::fs::rename(&temp_path, path) {
        if cfg!(windows) && path.exists() {
            std::fs::copy(&temp_path, path).map_err(|copy_error| {
                E911StoreError::new(format!("{tag}_replace_failed:{error}:{copy_error}"))
            })?;
            let _ = std::fs::remove_file(&temp_path);
        } else {
            return Err(E911StoreError::new(format!("{tag}_rename_failed:{error}")));
        }
    }
    #[cfg(unix)]
    sync_dir(dir)?;
    Ok(())
}

fn encrypt(key_bytes: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, ring::error::Unspecified> {
    let unbound = UnboundKey::new(&AES_256_GCM, key_bytes).map_err(|_| ring::error::Unspecified)?;
    let key = LessSafeKey::new(unbound);
    let mut nonce_bytes = [0u8; 12];
    SystemRandom::new().fill(&mut nonce_bytes)?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    let mut in_out = plaintext.to_vec();
    let tag = key.seal_in_place_separate_tag(nonce, ring::aead::Aad::empty(), &mut in_out)?;
    let mut out = nonce_bytes.to_vec();
    out.extend_from_slice(tag.as_ref());
    out.extend_from_slice(&in_out);
    Ok(out)
}

fn decrypt(key_bytes: &[u8], blob: &[u8]) -> Result<Vec<u8>, ring::error::Unspecified> {
    let unbound = UnboundKey::new(&AES_256_GCM, key_bytes).map_err(|_| ring::error::Unspecified)?;
    let key = LessSafeKey::new(unbound);
    if blob.len() < 12 + 16 {
        return Err(ring::error::Unspecified);
    }
    let mut nonce_bytes = [0u8; 12];
    nonce_bytes.copy_from_slice(&blob[..12]);
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    // Blob layout: nonce(12) || tag(16) || ciphertext. ring's open_in_place
    // expects ciphertext || tag, so rebuild that order.
    let tag = &blob[12..28];
    let ciphertext = &blob[28..];
    let mut in_out = Vec::with_capacity(ciphertext.len() + 16);
    in_out.extend_from_slice(ciphertext);
    in_out.extend_from_slice(tag);
    key.open_in_place(nonce, ring::aead::Aad::empty(), &mut in_out)?;
    in_out.truncate(in_out.len() - 16);
    Ok(in_out)
}

#[cfg(unix)]
fn set_dir_permissions(dir: &Path) -> Result<(), E911StoreError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| E911StoreError::new(format!("e911_dir_permissions_failed:{error}")))
}

#[cfg(unix)]
fn set_file_permissions(path: &Path) -> Result<(), E911StoreError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| E911StoreError::new(format!("e911_file_permissions_failed:{error}")))
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> Result<(), E911StoreError> {
    use std::os::unix::io::AsRawFd;
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .open(dir)
        .map_err(|error| E911StoreError::new(format!("e911_open_dir_failed:{error}")))?;
    let rc = unsafe { libc::fsync(directory.as_raw_fd()) };
    if rc != 0 {
        return Err(E911StoreError::new("e911_sync_dir_failed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::core::entitlement::{
        E911State, E911StateSource, EntitlementStatusValue,
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_root() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "simadmin-e911-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ))
    }

    fn plain_key(iccid: &str) -> SimBindingKey {
        SimBindingKey::Plain {
            iccid: iccid.to_string(),
        }
    }

    #[test]
    fn state_round_trips_per_binding() {
        let store = E911StateStore::new(temp_root());
        let key = plain_key("8986000111111111111");
        assert_eq!(store.load(&key).unwrap(), E911EntitlementRecord::default());

        let record = E911EntitlementRecord {
            state: E911State::Provisioned,
            source: E911StateSource::CarrierConfirmed,
            addr_status: EntitlementStatusValue::Set,
            confirmed_at_epoch: Some(1_700_000_000),
            provider_reference: Some("ref-x".to_string()),
            ..Default::default()
        };
        store.save(&key, &record).unwrap();
        assert_eq!(store.load(&key).unwrap(), record);

        // A different binding must not see the first one's state.
        let other = plain_key("8986000111111111112");
        assert_eq!(
            store.load(&other).unwrap(),
            E911EntitlementRecord::default()
        );
        store.delete(&key).unwrap();
        assert_eq!(store.load(&key).unwrap(), E911EntitlementRecord::default());
    }

    #[test]
    fn secret_store_round_trips_encrypted() {
        let store = E911StateStore::new(temp_root());
        let key = plain_key("8986000111111111111");
        let secrets = E911Secrets {
            entitlement_token: Some("secret-token".to_string()),
            cookie: Some("session".to_string()),
            server_flow_user_data: Some("csrf-state".to_string()),
            ..E911Secrets::default()
        };
        store.save_secrets(&key, &secrets).unwrap();

        // The on-disk file must not contain the plaintext secret.
        let raw = std::fs::read(store.secret_path(&key)).unwrap();
        let raw_text = String::from_utf8_lossy(&raw);
        assert!(!raw_text.contains("secret-token"));
        assert!(!raw_text.contains("csrf-state"));

        assert_eq!(store.load_secrets(&key).unwrap(), secrets);
    }

    #[test]
    fn secrets_fail_closed_when_key_rotates() {
        let root = temp_root();
        let store = E911StateStore::new(&root);
        let key = plain_key("8986000111111111111");
        store
            .save_secrets(
                &key,
                &E911Secrets {
                    entitlement_token: Some("t".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

        // Replace the key with garbage of the same length: load must fail,
        // never return plaintext or a blank.
        let key_path = store.key_path.clone();
        std::fs::write(key_path, [0u8; 32]).unwrap();
        assert!(store.load_secrets(&key).is_err());
    }

    #[test]
    fn binding_mismatch_fails_closed() {
        let store = E911StateStore::new(temp_root());
        let key = plain_key("8986000111111111111");
        let record = E911EntitlementRecord {
            state: E911State::Provisioned,
            ..Default::default()
        };
        store.save(&key, &record).unwrap();

        // Hand-craft a file with a wrong binding snapshot.
        let path = store.state_path(&key);
        let wrong = serde_json::json!({
            "schema_version": 1,
            "binding": { "kind": "plain", "iccid": "other-iccid" },
            "state": "provisioned",
        });
        std::fs::write(&path, serde_json::to_vec(&wrong).unwrap()).unwrap();
        assert!(store.load(&key).is_err());
    }
}
