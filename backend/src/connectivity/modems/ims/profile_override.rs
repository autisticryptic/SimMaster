//! Per-SIM user overrides for IMS connection facts.
//!
//! The read-only carrier catalog is the baseline. A user may explicitly
//! override a small set of connection facts for one physical SIM. Those edits
//! are persisted in a per-binding SQLite row in production and layered over
//! the catalog at connect time. A file backend remains available for tests and
//! recovery tooling. Everything else (automation, retries, network probes)
//! reads only the catalog.
//!
//! Binding model (see `DEVELOPMENT_PLAN.md` P1):
//!   - a plain removable SIM is bound by its normalized ICCID;
//!   - an eUICC is bound by its EID plus the currently enabled profile ICCID,
//!     so switching eSIM profiles yields a different binding key;
//!   - when no ICCID can be read the caller receives `sim_identity_not_ready`
//!     and must not fall back to `line_id`, modem path, IMEI or IMSI.
//!
//! Storage contract:
//!   - production uses one `ims_sim_overrides` row per binding hash;
//!   - the compatibility backend uses `<dir>/<binding-sha256>.json`;
//!   - fields are stored only when the user explicitly changed them;
//!   - an empty override removes the row/file; no record is the normal state;
//!   - unsupported schema, binding mismatch or corrupt JSON fails closed with
//!     a diagnosable error and never silently reuses a stale cache.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::platform::utils::normalize_iccid;

pub const OVERRIDE_SCHEMA_VERSION: u32 = 1;

/// Where a value originated. Returned to the API so the UI can show whether a
/// field came from the catalog, the SIM override, the modem, or a network
/// response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverrideSource {
    Catalog,
    SimOverride,
    Modem,
    Network,
}

/// Stable identity of one physical SIM for the purposes of user overrides.
///
/// Deliberately independent of `line_id` (which anchors runtime state to a
/// hardware slot): moving the SIM to another modem must still resolve the same
/// user override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimBindingKey {
    /// A plain removable SIM, or a eUICC for which no EID could be read yet.
    /// The current profile ICCID is the only usable anchor in that case.
    Plain { iccid: String },
    /// A eUICC with both its EID and the currently enabled profile ICCID.
    Euicc { eid: String, profile_iccid: String },
}

impl SimBindingKey {
    /// Build the binding key from the SIM identity parts that are available.
    ///
    /// * `iccid` may be `None` when the reader could not produce it yet. In that
    ///   case we fail closed with `sim_identity_not_ready`.
    /// * `eid` is accepted only as a normalized 32-digit decimal value. A
    ///   removable eSIM whose EID cannot be read degrades to `Plain` and is
    ///   still re-anchored on profile switch because the ICCID changes.
    pub fn resolve(iccid: Option<&str>, eid: Option<&str>) -> Result<Self, OverrideError> {
        let iccid = normalize_iccid(iccid.unwrap_or(""));
        if iccid.is_empty() {
            return Err(OverrideError::identity_not_ready());
        }
        match eid.map(normalize_eid).filter(|eid| !eid.is_empty()) {
            Some(eid) => Ok(Self::Euicc {
                eid,
                profile_iccid: iccid,
            }),
            None => Ok(Self::Plain { iccid }),
        }
    }

    /// Normalized 32-digit EID, or an empty string when the value is invalid.
    pub fn normalized_eid(&self) -> Option<&str> {
        match self {
            Self::Euicc { eid, .. } => Some(eid),
            Self::Plain { .. } => None,
        }
    }

    /// ICCID component of the binding key. For a eUICC this is the currently
    /// enabled profile's ICCID, so it changes when the profile switches.
    pub fn iccid(&self) -> &str {
        match self {
            Self::Plain { iccid } => iccid,
            Self::Euicc { profile_iccid, .. } => profile_iccid,
        }
    }

    /// Stable short filename hash. Sensitive identifiers are never used
    /// verbatim as file names.
    pub fn sha256(&self) -> String {
        let canonical = match self {
            Self::Plain { iccid } => format!("plain\0{iccid}"),
            Self::Euicc { eid, profile_iccid } => {
                format!("euicc\0{eid}\0{profile_iccid}")
            }
        };
        sha256_hex(canonical.as_bytes())
    }
}

/// Normalized EID: digits only, uppercase, exactly 32 digits, or empty.
pub fn normalize_eid(eid: &str) -> String {
    let cleaned = eid
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    if cleaned.len() == 32 && cleaned.bytes().all(|byte| byte.is_ascii_digit()) {
        cleaned
    } else {
        String::new()
    }
}

/// Connection facts the user may override for one access.
///
/// All fields are optional: `None` means "inherit the carrier catalog". The
/// VoLTE adapter ignores the ePDG fields and the VoWiFi adapter ignores none of
/// them; both share the IMS bearer fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImsAccessOverride {
    /// Pin to a specific carrier profile by `profile_id`.
    pub profile_id: Option<String>,
    /// IMS APN override.
    pub apn: Option<String>,
    /// IMS domain override.
    pub domain: Option<String>,
    /// IMS authentication realm override.
    pub realm: Option<String>,
    /// IMS registrar URI or host override.
    pub registrar: Option<String>,
    /// Ordered P-CSCF candidates override.
    pub pcscf: Option<Vec<String>>,
    /// VoWiFi only: ePDG host override.
    pub epdg_host: Option<String>,
    /// VoWiFi only: ePDG port override.
    pub epdg_port: Option<u16>,
    /// Requested IMS IP family for VoLTE/VoWiFi, for example "ipv4" or
    /// "ipv6". The VoLTE runtime uses this as an explicit profile override;
    /// otherwise the LTE catalog `access.ip_family` is only a fallback hint.
    pub ip_stack: Option<String>,
    /// VoWiFi only: DNS server override.
    pub dns: Option<Vec<String>>,
    /// VoWiFi only: explicitly present a different subscriber identity to the
    /// carrier. This changes profile matching and IMS/IKE identities, but it
    /// never changes the SIM/UIM used for AKA authentication.
    #[serde(default)]
    pub spoof_imsi: bool,
    /// IMSI to present when `spoof_imsi` is enabled (5-16 decimal digits).
    #[serde(default)]
    pub custom_imsi: Option<String>,
}

impl ImsAccessOverride {
    pub fn is_empty(&self) -> bool {
        self.profile_id.is_none()
            && self.apn.is_none()
            && self.domain.is_none()
            && self.realm.is_none()
            && self.registrar.is_none()
            && self.pcscf.is_none()
            && self.epdg_host.is_none()
            && self.epdg_port.is_none()
            && self.ip_stack.is_none()
            && self.dns.is_none()
            && !self.spoof_imsi
            && self.custom_imsi.is_none()
    }
}

/// Facts that follow the SIM regardless of access.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImsCommonOverride {
    /// 15-digit IMEI to present as the device identity. `None` or user-cleared
    /// means "use this device's own IMEI".
    pub custom_imei: Option<String>,
    /// Voicemail retrieval number for this SIM.
    pub voicemail_number: Option<String>,
}

impl ImsCommonOverride {
    pub fn is_empty(&self) -> bool {
        self.custom_imei.is_none() && self.voicemail_number.is_none()
    }
}

/// Supplementary service preferences that follow the SIM.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServicesOverride {
    /// Request call waiting for this SIM.
    pub call_waiting: Option<bool>,
    /// Request caller-id restriction (CLIR) for outbound calls.
    pub caller_id_restriction: Option<bool>,
}

impl ServicesOverride {
    pub fn is_empty(&self) -> bool {
        self.call_waiting.is_none() && self.caller_id_restriction.is_none()
    }
}

/// Emergency provisioning facts the user explicitly entered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmergencyOverride {
    /// User-entered civic address intent. It is not proof that the carrier has
    /// confirmed the address.
    pub e911_address: Option<String>,
}

impl EmergencyOverride {
    pub fn is_empty(&self) -> bool {
        self.e911_address.is_none()
    }
}

/// The complete user override for one SIM.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimOverride {
    #[serde(default)]
    pub ims_common: ImsCommonOverride,
    #[serde(default)]
    pub ims_volte: ImsAccessOverride,
    #[serde(default)]
    pub ims_vowifi: ImsAccessOverride,
    #[serde(default)]
    pub services: ServicesOverride,
    #[serde(default)]
    pub emergency: EmergencyOverride,
}

impl SimOverride {
    /// Whether the override carries no user-entered facts at all. An empty
    /// override is deleted from disk rather than persisted.
    pub fn is_empty(&self) -> bool {
        self.ims_common.is_empty()
            && self.ims_volte.is_empty()
            && self.ims_vowifi.is_empty()
            && self.services.is_empty()
            && self.emergency.is_empty()
    }

    pub fn sanitized(&self) -> Self {
        self.clone()
    }
}

/// An error produced by the override store or binding resolution. The message
/// is safe for diagnostics and API responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverrideError {
    pub code: String,
}

impl OverrideError {
    fn new(code: impl Into<String>) -> Self {
        Self { code: code.into() }
    }

    fn identity_not_ready() -> Self {
        Self::new("sim_identity_not_ready")
    }

    pub fn code(&self) -> &str {
        &self.code
    }
}

impl std::fmt::Display for OverrideError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.code)
    }
}

impl std::error::Error for OverrideError {}

/// Snapshot of the binding stored inside an override file so a mismatch can be
/// detected on load and fail closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredBinding {
    Plain { iccid: String },
    Euicc { eid: String, profile_iccid: String },
}

impl From<&SimBindingKey> for StoredBinding {
    fn from(key: &SimBindingKey) -> Self {
        match key {
            SimBindingKey::Plain { iccid } => Self::Plain {
                iccid: iccid.clone(),
            },
            SimBindingKey::Euicc { eid, profile_iccid } => Self::Euicc {
                eid: eid.clone(),
                profile_iccid: profile_iccid.clone(),
            },
        }
    }
}

impl StoredBinding {
    fn matches(&self, key: &SimBindingKey) -> bool {
        self == &StoredBinding::from(key)
    }

    fn sha256(&self) -> String {
        match self {
            Self::Plain { iccid } => SimBindingKey::Plain {
                iccid: iccid.clone(),
            }
            .sha256(),
            Self::Euicc { eid, profile_iccid } => SimBindingKey::Euicc {
                eid: eid.clone(),
                profile_iccid: profile_iccid.clone(),
            }
            .sha256(),
        }
    }
}

/// On-disk wrapper that carries the binding snapshot alongside the override.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct StoredImsOverrides {
    common: ImsCommonOverride,
    volte: ImsAccessOverride,
    vowifi: ImsAccessOverride,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OverrideFile {
    schema_version: u32,
    binding: StoredBinding,
    #[serde(default)]
    ims: StoredImsOverrides,
    #[serde(default)]
    services: ServicesOverride,
    #[serde(default)]
    emergency: EmergencyOverride,
}

impl OverrideFile {
    fn from_override(key: &SimBindingKey, override_: &SimOverride) -> Self {
        Self {
            schema_version: OVERRIDE_SCHEMA_VERSION,
            binding: StoredBinding::from(key),
            ims: StoredImsOverrides {
                common: override_.ims_common.clone(),
                volte: override_.ims_volte.clone(),
                vowifi: override_.ims_vowifi.clone(),
            },
            services: override_.services.clone(),
            emergency: override_.emergency.clone(),
        }
    }

    fn into_override(self) -> SimOverride {
        SimOverride {
            ims_common: self.ims.common,
            ims_volte: self.ims.volte,
            ims_vowifi: self.ims.vowifi,
            services: self.services,
            emergency: self.emergency,
        }
    }
}

/// Persistent store for per-SIM overrides.
///
/// Production keeps one row per binding in the application database, beside the
/// other per-line records, so a normal device backup captures overrides and no
/// separate file needs its own lifecycle. The file backend remains for recovery
/// and tests, where being able to read or hand-edit a single override matters
/// more than sharing a transaction with the rest of the configuration.
#[derive(Clone)]
pub struct SimOverrideStore {
    backend: Backend,
}

#[derive(Clone)]
enum Backend {
    /// One `<binding-sha256>.json` per override. The lock serializes writers,
    /// which the filesystem does not do for us.
    File {
        dir: PathBuf,
        write_lock: Arc<Mutex<()>>,
    },
    /// Rows in `ims_sim_overrides`. No lock here: the shared connection is
    /// already behind a mutex, and each operation is a single statement or an
    /// immediate transaction.
    Database(Arc<crate::platform::db::Database>),
}

// `Database` does not implement `Debug`, so the derive cannot be used. The
// manual impl reports which backend is active, which is what a log line needs.
impl std::fmt::Debug for SimOverrideStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.backend {
            Backend::File { dir, .. } => formatter
                .debug_struct("SimOverrideStore")
                .field("backend", &"file")
                .field("dir", dir)
                .finish(),
            Backend::Database(_) => formatter
                .debug_struct("SimOverrideStore")
                .field("backend", &"database")
                .finish(),
        }
    }
}

impl SimOverrideStore {
    /// Create a store rooted at `dir`. The directory is created lazily on the
    /// first write; reads against a missing directory simply report no file.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            backend: Backend::File {
                dir: dir.into(),
                write_lock: Arc::new(Mutex::new(())),
            },
        }
    }

    /// The production store. One row per `SimBindingKey`; the binding hash is
    /// only an index, and the full binding snapshot is still checked on read.
    pub fn database(database: Arc<crate::platform::db::Database>) -> Self {
        Self {
            backend: Backend::Database(database),
        }
    }

    /// Pick the backend for this process.
    ///
    /// `SIMADMIN_OVERRIDES_DIR` is the recovery escape hatch: it selects the file
    /// backend so an operator can inspect or hand-edit one override without a
    /// SQL client. Otherwise overrides live in the application database.
    pub fn resolve(database: Arc<crate::platform::db::Database>) -> Self {
        if let Some(dir) = std::env::var_os("SIMADMIN_OVERRIDES_DIR") {
            if !dir.is_empty() {
                return Self::new(PathBuf::from(dir));
            }
        }
        Self::database(database)
    }

    /// Directory backing this store, or an empty path when it is database-backed.
    pub fn dir(&self) -> &Path {
        match &self.backend {
            Backend::File { dir, .. } => dir,
            Backend::Database(_) => Path::new(""),
        }
    }

    fn path_for(&self, key: &SimBindingKey) -> PathBuf {
        self.dir().join(format!("{}.json", key.sha256()))
    }

    /// Load the override for `key`, returning `Ok(None)` when the SIM has no
    /// override (the normal state). Corrupt JSON, an unsupported schema or a
    /// binding mismatch fail closed.
    pub fn load(&self, key: &SimBindingKey) -> Result<Option<SimOverride>, OverrideError> {
        let (dir, write_lock) = match &self.backend {
            Backend::Database(database) => return self.load_from_database(database, key),
            Backend::File { dir, write_lock } => (dir, write_lock),
        };
        let _guard = write_lock
            .lock()
            .map_err(|_| OverrideError::new("sim_override_lock_poisoned"))?;
        self.ensure_safe_dir(false)?;
        let _ = dir;
        let path = self.path_for(key);
        if !safe_regular_file_exists(&path)? {
            return Ok(None);
        }
        let bytes = read_no_follow(&path)
            .map_err(|error| OverrideError::new(format!("sim_override_read_failed:{error}")))?;
        let file = parse_override_file(&bytes).map_err(|error| {
            OverrideError::new(format!(
                "sim_override_corrupt:{}",
                redact_json_error(&error)
            ))
        })?;
        if file.schema_version != OVERRIDE_SCHEMA_VERSION {
            return Err(OverrideError::new(format!(
                "sim_override_unsupported_schema:{}",
                file.schema_version
            )));
        }
        if !file.binding.matches(key) {
            return Err(OverrideError::new("sim_override_binding_mismatch"));
        }
        Ok(Some(file.into_override()))
    }

    /// Persist `override_` for `key`. An empty override removes the record: no
    /// override is the normal state, so an emptied one leaves nothing behind.
    pub fn save(&self, key: &SimBindingKey, override_: &SimOverride) -> Result<(), OverrideError> {
        let (dir, write_lock) = match &self.backend {
            Backend::Database(database) => {
                if override_.is_empty() {
                    return self.delete_from_database(database, key);
                }
                return self.save_to_database(database, key, override_);
            }
            Backend::File { dir, write_lock } => (dir, write_lock),
        };
        let _guard = write_lock
            .lock()
            .map_err(|_| OverrideError::new("sim_override_lock_poisoned"))?;
        if override_.is_empty() {
            return self.delete_unlocked(key);
        }
        std::fs::create_dir_all(dir)
            .map_err(|error| OverrideError::new(format!("sim_override_mkdir_failed:{error}")))?;
        self.ensure_safe_dir(true)?;
        #[cfg(unix)]
        set_dir_permissions(dir)?;

        let file = OverrideFile::from_override(key, override_);
        let content = serde_json::to_string_pretty(&file).map_err(|error| {
            OverrideError::new(format!("sim_override_serialize_failed:{error}"))
        })?;

        let path = self.path_for(key);
        let _ = safe_regular_file_exists(&path)?;
        let (temp_path, mut temp_file) = create_unique_temp_file(dir, &key.sha256())?;
        use std::io::Write;
        if let Err(error) = temp_file.write_all(content.as_bytes()) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(OverrideError::new(format!(
                "sim_override_write_tmp_failed:{error}"
            )));
        }
        if let Err(error) = temp_file.sync_all() {
            let _ = std::fs::remove_file(&temp_path);
            return Err(OverrideError::new(format!(
                "sim_override_sync_failed:{error}"
            )));
        }
        drop(temp_file);

        if let Err(error) = std::fs::rename(&temp_path, &path) {
            if cfg!(windows) && path.exists() {
                std::fs::copy(&temp_path, &path).map_err(|copy_error| {
                    OverrideError::new(format!("sim_override_replace_failed:{error}:{copy_error}"))
                })?;
                let _ = std::fs::remove_file(&temp_path);
            } else {
                return Err(OverrideError::new(format!(
                    "sim_override_rename_failed:{error}"
                )));
            }
        }
        #[cfg(unix)]
        sync_dir(self.dir())?;
        Ok(())
    }

    /// Remove the override for `key`. A missing record is not an error.
    pub fn delete(&self, key: &SimBindingKey) -> Result<(), OverrideError> {
        let write_lock = match &self.backend {
            Backend::Database(database) => return self.delete_from_database(database, key),
            Backend::File { write_lock, .. } => write_lock,
        };
        let _guard = write_lock
            .lock()
            .map_err(|_| OverrideError::new("sim_override_lock_poisoned"))?;
        self.delete_unlocked(key)
    }

    fn load_from_database(
        &self,
        database: &Arc<crate::platform::db::Database>,
        key: &SimBindingKey,
    ) -> Result<Option<SimOverride>, OverrideError> {
        let binding_hash = key.sha256();
        let row = database
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT schema_version, document_json
                     FROM ims_sim_overrides WHERE binding_hash = ?1",
                    [&binding_hash],
                    |row| Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
            })
            .map_err(|error| {
                OverrideError::new(format!("sim_override_database_read_failed:{error}"))
            })?;
        let Some((schema_version, document)) = row else {
            return Ok(None);
        };
        read_stored_override(schema_version, &document, key).map(Some)
    }

    fn save_to_database(
        &self,
        database: &Arc<crate::platform::db::Database>,
        key: &SimBindingKey,
        override_: &SimOverride,
    ) -> Result<(), OverrideError> {
        let binding_hash = key.sha256();
        let file = OverrideFile::from_override(key, override_);
        let document = serde_json::to_string(&file).map_err(|error| {
            OverrideError::new(format!("sim_override_serialize_failed:{error}"))
        })?;
        let updated_at = chrono::Utc::now().to_rfc3339();
        database
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO ims_sim_overrides
                         (binding_hash, schema_version, document_json, updated_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(binding_hash) DO UPDATE SET
                         schema_version = excluded.schema_version,
                         document_json = excluded.document_json,
                         updated_at = excluded.updated_at",
                    rusqlite::params![
                        &binding_hash,
                        OVERRIDE_SCHEMA_VERSION,
                        &document,
                        &updated_at
                    ],
                )
                .map(|_| ())
            })
            .map_err(|error| {
                OverrideError::new(format!("sim_override_database_write_failed:{error}"))
            })
    }

    fn delete_from_database(
        &self,
        database: &Arc<crate::platform::db::Database>,
        key: &SimBindingKey,
    ) -> Result<(), OverrideError> {
        let binding_hash = key.sha256();
        database
            .with_connection(|conn| {
                conn.execute(
                    "DELETE FROM ims_sim_overrides WHERE binding_hash = ?1",
                    [&binding_hash],
                )
                .map(|_| ())
            })
            .map_err(|error| {
                OverrideError::new(format!("sim_override_database_delete_failed:{error}"))
            })
    }

    fn delete_unlocked(&self, key: &SimBindingKey) -> Result<(), OverrideError> {
        self.ensure_safe_dir(false)?;
        let path = self.path_for(key);
        if safe_regular_file_exists(&path)? {
            std::fs::remove_file(&path).map_err(|error| {
                OverrideError::new(format!("sim_override_delete_failed:{error}"))
            })?;
            #[cfg(unix)]
            sync_dir(self.dir())?;
        }
        Ok(())
    }

    fn ensure_safe_dir(&self, must_exist: bool) -> Result<(), OverrideError> {
        match std::fs::symlink_metadata(self.dir()) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                Err(OverrideError::new("sim_override_dir_symlink_rejected"))
            }
            Ok(metadata) if !metadata.is_dir() => {
                Err(OverrideError::new("sim_override_dir_not_directory"))
            }
            Ok(_) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound && !must_exist => Ok(()),
            Err(error) => Err(OverrideError::new(format!(
                "sim_override_dir_metadata_failed:{error}"
            ))),
        }
    }
}

fn parse_override_file(bytes: &[u8]) -> Result<OverrideFile, serde_json::Error> {
    serde_json::from_slice(bytes)
}

/// Validate and decode one stored override document for a live SIM.
///
/// Shared by both backends so a row and a file are held to the same standard.
/// The binding is checked with `matches`, not by comparing hashes: the hash is
/// only an index, and confirming the full snapshot is what stops a stored
/// override from ever being applied to a different SIM.
fn read_stored_override(
    schema_version: u32,
    document: &str,
    key: &SimBindingKey,
) -> Result<SimOverride, OverrideError> {
    if schema_version != OVERRIDE_SCHEMA_VERSION {
        return Err(OverrideError::new(format!(
            "sim_override_unsupported_schema:{schema_version}"
        )));
    }
    let file = parse_override_file(document.as_bytes()).map_err(|error| {
        OverrideError::new(format!(
            "sim_override_corrupt:{}",
            redact_json_error(&error)
        ))
    })?;
    if file.schema_version != schema_version {
        return Err(OverrideError::new("sim_override_schema_mismatch"));
    }
    if !file.binding.matches(key) {
        return Err(OverrideError::new("sim_override_binding_mismatch"));
    }
    Ok(file.into_override())
}

/// Validate one serialized SQLite override row without loading it for a
/// particular live SIM. Maintenance import uses this before opening a write
/// transaction, so a forged binding hash or malformed document can never be
/// committed and later become visible to a different SIM.
pub(crate) fn validate_stored_override_document(
    binding_hash: &str,
    schema_version: u32,
    document: &serde_json::Value,
) -> Result<(), OverrideError> {
    if schema_version != OVERRIDE_SCHEMA_VERSION {
        return Err(OverrideError::new("sim_override_unsupported_schema"));
    }
    let file: OverrideFile = serde_json::from_value(document.clone())
        .map_err(|_| OverrideError::new("sim_override_corrupt"))?;
    if file.schema_version != schema_version {
        return Err(OverrideError::new("sim_override_schema_mismatch"));
    }
    if file.binding.sha256() != binding_hash {
        return Err(OverrideError::new("sim_override_binding_hash_mismatch"));
    }
    Ok(())
}

fn safe_regular_file_exists(path: &Path) -> Result<bool, OverrideError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(OverrideError::new("sim_override_symlink_rejected"))
        }
        Ok(metadata) if !metadata.is_file() => {
            Err(OverrideError::new("sim_override_not_regular_file"))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(OverrideError::new(format!(
            "sim_override_metadata_failed:{error}"
        ))),
    }
}

fn read_no_follow(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    let mut bytes = Vec::new();
    use std::io::Read;
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn create_unique_temp_file(
    dir: &Path,
    binding_hash: &str,
) -> Result<(PathBuf, std::fs::File), OverrideError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

    for _ in 0..16 {
        let temp_id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(
            ".{binding_hash}.{}.{}.tmp",
            std::process::id(),
            temp_id
        ));
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(OverrideError::new(format!(
                    "sim_override_write_tmp_failed:{error}"
                )))
            }
        }
    }
    Err(OverrideError::new("sim_override_temp_name_exhausted"))
}

#[cfg(unix)]
fn set_dir_permissions(dir: &Path) -> Result<(), OverrideError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| OverrideError::new(format!("sim_override_dir_permissions_failed:{error}")))
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> Result<(), OverrideError> {
    use std::os::unix::io::AsRawFd;
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .open(dir)
        .map_err(|error| OverrideError::new(format!("sim_override_open_dir_failed:{error}")))?;
    let rc = unsafe { libc::fsync(directory.as_raw_fd()) };
    if rc != 0 {
        return Err(OverrideError::new("sim_override_sync_dir_failed"));
    }
    Ok(())
}

fn redact_json_error(error: &serde_json::Error) -> String {
    // serde_json errors never embed file contents, but we keep the helper so
    // future error paths that could leak secrets have a single redaction point.
    error.to_string()
}

fn sha256_hex(data: &[u8]) -> String {
    use ring::digest;
    let digest = digest::digest(&digest::SHA256, data);
    let mut out = String::with_capacity(digest.as_ref().len() * 2);
    for byte in digest.as_ref() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> SimOverrideStore {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "simadmin-override-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        SimOverrideStore::new(&dir)
    }

    fn sample_override() -> SimOverride {
        SimOverride {
            ims_common: ImsCommonOverride {
                custom_imei: Some("351234567890123".to_string()),
                voicemail_number: None,
            },
            ims_vowifi: ImsAccessOverride {
                profile_id: Some("cn-cmcc-vowifi".to_string()),
                epdg_host: Some("epdg.example.com".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn plain_binding_normalizes_iccid() {
        let key = SimBindingKey::resolve(Some("898600F1234567890123"), None).unwrap();
        assert_eq!(
            key,
            SimBindingKey::Plain {
                iccid: "8986001234567890123".to_string()
            }
        );
        assert_eq!(key.iccid(), "8986001234567890123");
        assert!(key.normalized_eid().is_none());
    }

    #[test]
    fn euicc_binding_uses_eid_and_profile_iccid() {
        let key = SimBindingKey::resolve(
            Some("8986001234567890123"),
            Some("89086030123456789012345678901234"),
        )
        .unwrap();
        match &key {
            SimBindingKey::Euicc { eid, profile_iccid } => {
                assert_eq!(eid, "89086030123456789012345678901234");
                assert_eq!(profile_iccid, "8986001234567890123");
            }
            _ => panic!("expected euicc binding"),
        }
        assert_eq!(
            key.normalized_eid(),
            Some("89086030123456789012345678901234")
        );
        assert_eq!(key.iccid(), "8986001234567890123");
    }

    #[test]
    fn invalid_eid_degrades_to_plain_binding() {
        let key = SimBindingKey::resolve(Some("8986001234567890123"), Some("short")).unwrap();
        assert_eq!(
            key,
            SimBindingKey::Plain {
                iccid: "8986001234567890123".to_string()
            }
        );
        let key2 = SimBindingKey::resolve(
            Some("8986001234567890123"),
            Some("abcdef012345678901234567890123456"),
        )
        .unwrap();
        assert_eq!(
            key2,
            SimBindingKey::Plain {
                iccid: "8986001234567890123".to_string()
            }
        );
    }

    #[test]
    fn missing_iccid_fails_closed() {
        let error =
            SimBindingKey::resolve(None, Some("89086030123456789012345678901234")).unwrap_err();
        assert_eq!(error.code(), "sim_identity_not_ready");
        let error = SimBindingKey::resolve(Some("   "), None).unwrap_err();
        assert_eq!(error.code(), "sim_identity_not_ready");
    }

    #[test]
    fn switching_profile_iccid_changes_euicc_binding() {
        let profile_a = SimBindingKey::resolve(
            Some("8986001111111111111"),
            Some("89086030123456789012345678901234"),
        )
        .unwrap();
        let profile_b = SimBindingKey::resolve(
            Some("8986002222222222222"),
            Some("89086030123456789012345678901234"),
        )
        .unwrap();
        assert_ne!(profile_a, profile_b);
        assert_ne!(profile_a.sha256(), profile_b.sha256());
    }

    #[test]
    fn round_trip_persists_only_user_fields() {
        let store = temp_store();
        let key = SimBindingKey::resolve(Some("8986001234567890123"), None).unwrap();
        let override_ = sample_override();
        store.save(&key, &override_).unwrap();

        let loaded = store.load(&key).unwrap().expect("override should exist");
        assert_eq!(
            loaded.ims_common.custom_imei.as_deref(),
            Some("351234567890123")
        );
        assert_eq!(
            loaded.ims_vowifi.profile_id.as_deref(),
            Some("cn-cmcc-vowifi")
        );
        assert_eq!(
            loaded.ims_vowifi.epdg_host.as_deref(),
            Some("epdg.example.com")
        );
        assert!(loaded.ims_volte.is_empty());
        assert!(loaded.services.is_empty());
    }

    #[test]
    fn empty_override_removes_file() {
        let store = temp_store();
        let key = SimBindingKey::resolve(Some("8986001234567890123"), None).unwrap();
        store.save(&key, &sample_override()).unwrap();
        assert!(store.load(&key).unwrap().is_some());

        store.save(&key, &SimOverride::default()).unwrap();
        assert!(store.load(&key).unwrap().is_none());
        assert!(!store.path_for(&key).exists());
    }

    #[test]
    fn delete_missing_file_is_not_an_error() {
        let store = temp_store();
        let key = SimBindingKey::resolve(Some("8986001234567890123"), None).unwrap();
        store.delete(&key).unwrap();
    }

    #[test]
    fn corrupt_file_fails_closed() {
        let store = temp_store();
        let key = SimBindingKey::resolve(Some("8986001234567890123"), None).unwrap();
        std::fs::create_dir_all(store.dir()).unwrap();
        std::fs::write(store.path_for(&key), b"{not json").unwrap();
        let error = store.load(&key).unwrap_err();
        assert!(error.code().starts_with("sim_override_corrupt"));
    }

    #[test]
    fn unsupported_schema_fails_closed() {
        let store = temp_store();
        let key = SimBindingKey::resolve(Some("8986001234567890123"), None).unwrap();
        std::fs::create_dir_all(store.dir()).unwrap();
        std::fs::write(
            store.path_for(&key),
            r#"{"schema_version":999,"binding":{"kind":"plain","iccid":"8986001234567890123"}}"#,
        )
        .unwrap();
        let error = store.load(&key).unwrap_err();
        assert!(error.code().starts_with("sim_override_unsupported_schema"));
    }

    #[test]
    fn binding_mismatch_fails_closed() {
        let store = temp_store();
        let key = SimBindingKey::resolve(Some("8986001234567890123"), None).unwrap();
        let other = SimBindingKey::resolve(Some("8986009999999999999"), None).unwrap();
        // Simulate a file copied from another SIM onto this binding's path: the
        // stored binding snapshot must not match the lookup key.
        std::fs::create_dir_all(store.dir()).unwrap();
        let file = OverrideFile {
            schema_version: OVERRIDE_SCHEMA_VERSION,
            binding: StoredBinding::from(&other),
            ..OverrideFile::from_override(&key, &sample_override())
        };
        std::fs::write(store.path_for(&key), serde_json::to_vec(&file).unwrap()).unwrap();
        let error = store.load(&key).unwrap_err();
        assert_eq!(error.code(), "sim_override_binding_mismatch");
    }

    #[test]
    fn filename_hashes_sensitive_identifiers() {
        let key = SimBindingKey::resolve(Some("8986001234567890123"), None).unwrap();
        let hash = key.sha256();
        assert_eq!(hash.len(), 64);
        assert!(!hash.contains("8986001234567890123"));
        assert!(store_path_is_within_dir(&key));
    }

    fn store_path_is_within_dir(key: &SimBindingKey) -> bool {
        let dir = PathBuf::from("/data/simadmin/ims-overrides");
        let path = SimOverrideStore::new(&dir).path_for(key);
        path.parent() == Some(dir.as_path())
    }

    #[test]
    fn normalize_eid_accepts_only_32_digits() {
        assert_eq!(
            normalize_eid("89086030123456789012345678901234"),
            "89086030123456789012345678901234"
        );
        assert_eq!(
            normalize_eid(" 8908 6030 1234 5678 9012 3456 7890 1234 "),
            "89086030123456789012345678901234"
        );
        assert_eq!(normalize_eid("8908603012345678901234567890123"), "");
        assert_eq!(normalize_eid("abc"), "");
    }

    #[test]
    fn disk_schema_groups_ims_sections() {
        let store = temp_store();
        let key = SimBindingKey::resolve(Some("8986001234567890123"), None).unwrap();
        store.save(&key, &sample_override()).unwrap();

        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(store.path_for(&key)).unwrap()).unwrap();
        assert!(value.get("ims").and_then(|ims| ims.get("common")).is_some());
        assert!(value.get("ims").and_then(|ims| ims.get("volte")).is_some());
        assert!(value.get("ims").and_then(|ims| ims.get("vowifi")).is_some());
        assert!(value.get("ims_common").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn save_enforces_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let store = temp_store();
        let key = SimBindingKey::resolve(Some("8986001234567890123"), None).unwrap();
        store.save(&key, &sample_override()).unwrap();

        assert_eq!(
            std::fs::metadata(store.dir()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(store.path_for(&key))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn override_file_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let store = temp_store();
        let key = SimBindingKey::resolve(Some("8986001234567890123"), None).unwrap();
        std::fs::create_dir_all(store.dir()).unwrap();
        let target = store.dir().join("outside.json");
        std::fs::write(&target, b"do not follow").unwrap();
        symlink(&target, store.path_for(&key)).unwrap();

        assert_eq!(
            store.load(&key).unwrap_err().code(),
            "sim_override_symlink_rejected"
        );
        assert_eq!(
            store.save(&key, &sample_override()).unwrap_err().code(),
            "sim_override_symlink_rejected"
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"do not follow");
    }

    #[test]
    fn concurrent_writes_leave_one_complete_document() {
        let store = temp_store();
        let key = SimBindingKey::resolve(Some("8986001234567890123"), None).unwrap();
        let mut first = sample_override();
        first.ims_common.voicemail_number = Some("111".to_string());
        let mut second = sample_override();
        second.ims_common.voicemail_number = Some("222".to_string());

        let left_store = store.clone();
        let left_key = key.clone();
        let left = std::thread::spawn(move || left_store.save(&left_key, &first));
        let right_store = store.clone();
        let right_key = key.clone();
        let right = std::thread::spawn(move || right_store.save(&right_key, &second));
        left.join().unwrap().unwrap();
        right.join().unwrap().unwrap();

        let loaded = store.load(&key).unwrap().unwrap();
        assert!(matches!(
            loaded.ims_common.voicemail_number.as_deref(),
            Some("111" | "222")
        ));
        let temp_files = std::fs::read_dir(store.dir())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(temp_files, 0);
    }

    /// A database-backed store on a throwaway `data.db`.
    ///
    /// Returns the `Arc<Database>` as well so a test can reopen the same store
    /// (modelling rediscovery after hotplug) without a second connection to the
    /// file competing for write locks.
    fn temp_database_store(
        name: &str,
    ) -> (
        SimOverrideStore,
        Arc<crate::platform::db::Database>,
        PathBuf,
    ) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "simadmin-override-db-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let database = Arc::new(
            crate::platform::db::Database::new(dir.join("data.db")).expect("open test database"),
        );
        (
            SimOverrideStore::database(Arc::clone(&database)),
            database,
            dir,
        )
    }

    #[test]
    fn database_store_round_trips_distinct_bindings() {
        let (store, database, dir) = temp_database_store("roundtrip");
        let first = SimBindingKey::resolve(Some("8986001111111111111"), None).unwrap();
        let second = SimBindingKey::resolve(Some("8986002222222222222"), None).unwrap();
        let mut first_override = sample_override();
        first_override.ims_common.voicemail_number = Some("111".to_string());
        let mut second_override = sample_override();
        second_override.ims_common.voicemail_number = Some("222".to_string());

        store.save(&first, &first_override).unwrap();
        store.save(&second, &second_override).unwrap();
        assert_eq!(
            store
                .load(&first)
                .unwrap()
                .unwrap()
                .ims_common
                .voicemail_number
                .as_deref(),
            Some("111")
        );
        assert_eq!(
            store
                .load(&second)
                .unwrap()
                .unwrap()
                .ims_common
                .voicemail_number
                .as_deref(),
            Some("222")
        );

        // Count through the same handle: a second connection to a WAL database
        // may not see the writes yet.
        let count = database
            .with_connection(|conn| {
                conn.query_row("SELECT COUNT(*) FROM ims_sim_overrides", [], |row| {
                    row.get::<_, i64>(0)
                })
            })
            .unwrap();
        assert_eq!(count, 2);
        drop(database);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn automatic_resolution_does_not_rewrite_the_override_row() {
        use crate::connectivity::modems::ims::{
            effective_profile::{
                resolve_effective_device_identity, resolve_effective_emergency,
                resolve_effective_ims_profile, resolve_effective_vowifi_ims_profile,
                resolve_effective_vowifi_profile,
            },
            vowifi::profiles::GB_EE_23433,
        };

        let (store, database, dir) = temp_database_store("read-only-resolution");
        let key = SimBindingKey::resolve(Some("8986001234567890123"), None).unwrap();
        let override_ = sample_override();
        store.save(&key, &override_).unwrap();

        let row = || {
            database
                .with_connection(|conn| {
                    conn.query_row(
                        "SELECT document_json, updated_at FROM ims_sim_overrides
                         WHERE binding_hash = ?1",
                        [key.sha256()],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )
                })
                .unwrap()
        };
        let before = row();

        for _ in 0..3 {
            let loaded = store.load(&key).unwrap().unwrap();
            let _ = resolve_effective_vowifi_profile(&GB_EE_23433, Some(&loaded));
            let _ = resolve_effective_ims_profile(&GB_EE_23433, Some(&loaded));
            let _ = resolve_effective_vowifi_ims_profile(&GB_EE_23433, Some(&loaded));
            let _ = resolve_effective_device_identity(Some(&loaded), Some("999999999999999"));
            let _ = resolve_effective_emergency(Some(&loaded));
        }

        assert_eq!(row(), before);
        drop(database);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn same_catalog_multi_sim_access_and_hotplug_contract_stays_isolated() {
        use crate::connectivity::modems::ims::{
            effective_profile::{
                resolve_effective_device_identity, resolve_effective_emergency,
                resolve_effective_ims_profile, resolve_effective_vowifi_ims_profile,
            },
            vowifi::profiles::GB_EE_23433,
        };

        let (store, database, dir) = temp_database_store("multi-sim-contract");
        let first = SimBindingKey::resolve(Some("8986001111111111111"), None).unwrap();
        let second = SimBindingKey::resolve(Some("8986002222222222222"), None).unwrap();
        let make_override = |imei: &str, suffix: &str| SimOverride {
            ims_common: ImsCommonOverride {
                custom_imei: Some(imei.to_string()),
                voicemail_number: Some(format!("*8{suffix}")),
            },
            ims_volte: ImsAccessOverride {
                domain: Some(format!("volte-{suffix}.ims.example")),
                ..Default::default()
            },
            ims_vowifi: ImsAccessOverride {
                domain: Some(format!("vowifi-{suffix}.ims.example")),
                epdg_host: Some(format!("epdg-{suffix}.example")),
                ..Default::default()
            },
            emergency: EmergencyOverride {
                e911_address: Some(format!("address-{suffix}")),
            },
            ..Default::default()
        };
        store
            .save(&first, &make_override("490154203237518", "a"))
            .unwrap();
        store
            .save(&second, &make_override("351234567890124", "b"))
            .unwrap();

        // A new store instance models rediscovery after reader/modem hotplug:
        // the lookup is repeated from the SIM key rather than a previous line.
        let reconnected = SimOverrideStore::database(Arc::clone(&database));
        let first_loaded = reconnected.load(&first).unwrap().unwrap();
        let second_loaded = reconnected.load(&second).unwrap().unwrap();
        for (loaded, suffix, imei) in [
            (&first_loaded, "a", "490154203237518"),
            (&second_loaded, "b", "351234567890124"),
        ] {
            assert_eq!(
                resolve_effective_ims_profile(&GB_EE_23433, Some(loaded))
                    .domain
                    .value,
                format!("volte-{suffix}.ims.example")
            );
            assert_eq!(
                resolve_effective_vowifi_ims_profile(&GB_EE_23433, Some(loaded))
                    .domain
                    .value,
                format!("vowifi-{suffix}.ims.example")
            );
            assert_eq!(
                resolve_effective_device_identity(Some(loaded), None)
                    .imei
                    .as_deref(),
                Some(imei)
            );
            assert_eq!(
                resolve_effective_emergency(Some(loaded))
                    .e911_address
                    .as_deref(),
                Some(format!("address-{suffix}").as_str())
            );
        }

        let eid = "89086030123456789012345678901234";
        let esim_profile_a =
            SimBindingKey::resolve(Some("8986003333333333333"), Some(eid)).unwrap();
        let esim_profile_b =
            SimBindingKey::resolve(Some("8986004444444444444"), Some(eid)).unwrap();
        reconnected
            .save(&esim_profile_a, &make_override("490154203237518", "esim-a"))
            .unwrap();
        assert!(reconnected.load(&esim_profile_b).unwrap().is_none());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn database_empty_override_deletes_row() {
        let (store, database, dir) = temp_database_store("delete");
        let key = SimBindingKey::resolve(Some("8986001234567890123"), None).unwrap();
        store.save(&key, &sample_override()).unwrap();
        store.save(&key, &SimOverride::default()).unwrap();
        assert!(store.load(&key).unwrap().is_none());
        // Emptying an override must remove the row, not leave an empty document
        // behind: no record is how "this SIM has no override" is represented.
        let count = database
            .with_connection(|conn| {
                conn.query_row("SELECT COUNT(*) FROM ims_sim_overrides", [], |row| {
                    row.get::<_, i64>(0)
                })
            })
            .unwrap();
        assert_eq!(count, 0);
        drop(database);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// `SIMADMIN_OVERRIDES_DIR` is the documented recovery escape hatch, so the
    /// selector must actually honour it rather than always returning the
    /// database backend.
    #[test]
    fn resolve_prefers_the_override_dir_escape_hatch() {
        let (_, database, dir) = temp_database_store("resolve");
        let override_dir = dir.join("overrides");

        // Not set: database backend, so `dir()` is empty.
        std::env::remove_var("SIMADMIN_OVERRIDES_DIR");
        let from_database = SimOverrideStore::resolve(Arc::clone(&database));
        assert_eq!(from_database.dir(), Path::new(""));

        std::env::set_var("SIMADMIN_OVERRIDES_DIR", &override_dir);
        let from_env = SimOverrideStore::resolve(Arc::clone(&database));
        assert_eq!(from_env.dir(), override_dir.as_path());
        std::env::remove_var("SIMADMIN_OVERRIDES_DIR");

        drop(database);
        let _ = std::fs::remove_dir_all(dir);
    }
}
