//! Explicit, offline-first native receipt recovery. Old CIDs/handles are
//! historical evidence, NEVER commands to replay on a new control generation.
//! Only a receipt whose live owner confirmed full cleanup may be archived.
//! Unknown allocation/reset outcomes remain fenced for device-specific review.
use super::{
    config::NativeDeviceConfig,
    io::{PortIdentity, SystemNativeIo},
    NativeError,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const RECEIPT_DIRECTORY: &str = "/var/lib/simadmin/native-control";
pub const LOCK_DIRECTORY: &str = "/run/simadmin/native-control";
const MAX_RECEIPT_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OwnerInstance {
    boot_id: String,
    pid: u32,
    start_ticks: u64,
}
impl OwnerInstance {
    pub(super) fn current() -> Result<Self, NativeError> {
        Ok(Self {
            boot_id: boot_id()?,
            pid: std::process::id(),
            start_ticks: process_start(std::process::id())
                .ok_or_else(|| error("native_owner_instance_unavailable"))?,
        })
    }
    fn alive(&self) -> Result<bool, NativeError> {
        if self.boot_id != boot_id()? {
            return Ok(false);
        }
        let path = format!("/proc/{}/stat", self.pid);
        match std::fs::read_to_string(path) {
            Ok(stat) => Ok(parse_start(&stat)
                .ok_or_else(|| error("native_recovery_owner_state_unknown"))?
                == self.start_ticks),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(error("native_recovery_owner_state_unknown")),
        }
    }
}
fn parse_start(stat: &str) -> Option<u64> {
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}
fn process_start(pid: u32) -> Option<u64> {
    parse_start(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}
fn boot_id() -> Result<String, NativeError> {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|s| s.trim().to_string())
        .map_err(|_| error("native_boot_identity_unavailable"))
}
fn error(reason: &str) -> NativeError {
    NativeError::OwnerConflict(reason.into())
}
fn hash(bytes: &[u8]) -> String {
    super::direct_sms::digest(bytes)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    schema: u32,
    key: String,
    line_id: String,
    physical_key: String,
    slot: u8,
    anchor: PathBuf,
    owner: OwnerInstance,
    ports: Vec<PortIdentity>,
    generation: String,
    cleanup_confirmed: bool,
    payload: serde_json::Value,
}
impl Receipt {
    pub(super) fn new(
        key: &str,
        spec: &NativeDeviceConfig,
        anchor: &Path,
        owner: OwnerInstance,
        ports: Vec<PortIdentity>,
        payload: &[u8],
    ) -> Result<Self, NativeError> {
        let generation = generation(&owner.boot_id, &ports)?;
        Ok(Self {
            schema: 2,
            key: key.into(),
            line_id: spec.line_id(),
            physical_key: spec.hardware_key.clone(),
            slot: spec.uim_slot,
            anchor: anchor.into(),
            owner,
            ports,
            generation,
            cleanup_confirmed: false,
            payload: serde_json::from_slice(payload)
                .map_err(|_| error("native_receipt_payload_invalid"))?,
        })
    }
    fn validate(&self, key: &str) -> Result<(), NativeError> {
        if self.schema != 2
            || self.key != key
            || self.line_id.is_empty()
            || self.physical_key.is_empty()
            || self.ports.is_empty()
            || self.generation != generation(&self.owner.boot_id, &self.ports)?
        {
            return Err(error("native_receipt_identity_invalid"));
        }
        Ok(())
    }
}
fn generation(boot: &str, ports: &[PortIdentity]) -> Result<String, NativeError> {
    let mut ports = ports.to_vec();
    ports.sort_by(|a, b| a.device.cmp(&b.device));
    serde_json::to_vec(&(boot, ports))
        .map(|b| hash(&b))
        .map_err(|_| error("native_generation_encode_failed"))
}
fn valid_key(key: &str) -> bool {
    key.starts_with("session-")
        && key.len() <= 100
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

pub(crate) fn ensure_directory(directory: &Path) -> Result<(), NativeError> {
    std::fs::create_dir_all(directory)
        .map_err(|_| error("native_receipt_directory_unavailable"))?;
    let meta = std::fs::symlink_metadata(directory)
        .map_err(|_| error("native_receipt_directory_unavailable"))?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(error("native_receipt_directory_unsafe"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if meta.uid() != unsafe { libc::geteuid() } {
            return Err(error("native_receipt_directory_owner_mismatch"));
        }
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| error("native_receipt_permissions_failed"))?;
    }
    Ok(())
}
fn sync_directory(directory: &Path) -> Result<(), NativeError> {
    File::open(directory)
        .and_then(|f| f.sync_all())
        .map_err(|_| error("native_receipt_directory_sync_failed"))
}
fn read_private(path: &Path) -> Result<Vec<u8>, NativeError> {
    let before = std::fs::symlink_metadata(path)
        .map_err(|_| error("native_receipt_metadata_unavailable"))?;
    if !before.is_file() || before.file_type().is_symlink() {
        return Err(error("native_receipt_not_a_bounded_regular_file"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| error("native_receipt_read_failed"))?;
    let meta = file
        .metadata()
        .map_err(|_| error("native_receipt_metadata_unavailable"))?;
    if !meta.is_file() || meta.len() > MAX_RECEIPT_BYTES {
        return Err(error("native_receipt_not_a_bounded_regular_file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return Err(error("native_receipt_permissions_unsafe"));
        }
    }
    let mut bytes = Vec::new();
    file.take(MAX_RECEIPT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| error("native_receipt_read_failed"))?;
    if bytes.len() as u64 > MAX_RECEIPT_BYTES {
        return Err(error("native_receipt_size_limit"));
    }
    Ok(bytes)
}
fn create_private(path: &Path, bytes: &[u8]) -> Result<(), NativeError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options
        .open(path)
        .map_err(|_| error("native_receipt_create_failed"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| error("native_receipt_write_failed"))
}
fn write_record(
    directory: &Path,
    key: &str,
    record: &Receipt,
    create: bool,
) -> Result<(), NativeError> {
    if !valid_key(key) {
        return Err(error("native_receipt_key_invalid"));
    }
    let bytes = serde_json::to_vec(record).map_err(|_| error("native_receipt_encode_failed"))?;
    let path = directory.join(format!("{key}.json"));
    if create {
        create_private(&path, &bytes)?;
    } else {
        let temp = directory.join(format!("{key}.{}.tmp", std::process::id()));
        create_private(&temp, &bytes)?;
        std::fs::rename(&temp, &path).map_err(|_| error("native_receipt_commit_failed"))?;
    }
    sync_directory(directory)
}

pub(super) fn save_owned(
    directory: &Path,
    key: &str,
    record: &Receipt,
    create: bool,
) -> Result<(), NativeError> {
    record.validate(key)?;
    if !create {
        let previous: Receipt =
            serde_json::from_slice(&read_private(&directory.join(format!("{key}.json")))?)
                .map_err(|_| error("native_receipt_previous_state_unknown"))?;
        previous.validate(key)?;
        if previous.owner != record.owner
            || previous.generation != record.generation
            || previous.cleanup_confirmed
        {
            return Err(error("native_receipt_owner_or_generation_changed"));
        }
    }
    write_record(directory, key, record, create)
}

pub(super) fn clear_owned(
    directory: &Path,
    key: &str,
    owner: &OwnerInstance,
) -> Result<(), NativeError> {
    if !valid_key(key) {
        return Err(error("native_receipt_key_invalid"));
    }
    let path = directory.join(format!("{key}.json"));
    let mut receipt: Receipt = serde_json::from_slice(&read_private(&path)?)
        .map_err(|_| error("native_receipt_previous_state_unknown"))?;
    receipt.validate(key)?;
    if &receipt.owner != owner {
        return Err(error("native_receipt_owner_changed"));
    }
    // Every caller must have confirmed ALL its protocol/namespace releases.
    // A crash after this fsync leaves a recoverable terminal record, rather
    // than forcing a new process to replay numeric IDs on another generation.
    receipt.cleanup_confirmed = true;
    write_record(directory, key, &receipt, false)?;
    std::fs::remove_file(path).map_err(|_| error("native_receipt_remove_failed"))?;
    sync_directory(directory)
}

pub(super) fn pending_paths(directory: &Path) -> Result<Vec<PathBuf>, NativeError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(error("native_receipt_inventory_unreadable")),
    };
    let mut result = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| error("native_receipt_inventory_unreadable"))?;
        if entry.file_name().to_string_lossy().starts_with("session-") {
            result.push(entry.path());
        }
        if result.len() > 512 {
            return Err(error("native_receipt_inventory_limit"));
        }
    }
    result.sort();
    Ok(result)
}
pub(super) fn ensure_persistent_clear() -> Result<(), NativeError> {
    if !pending_paths(Path::new(RECEIPT_DIRECTORY))?.is_empty() {
        return Err(error("native_sessions_require_reconciliation_before_start"));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct InventoryEntry {
    pub file: String,
    pub location: &'static str,
    pub cleanup_confirmed: bool,
    pub review_required: bool,
}
pub fn inventory() -> Result<Vec<InventoryEntry>, NativeError> {
    let mut entries = Vec::new();
    for (directory, location) in [
        (RECEIPT_DIRECTORY, "persistent"),
        (LOCK_DIRECTORY, "legacy_runtime"),
    ] {
        for path in pending_paths(Path::new(directory))? {
            let key = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let receipt = read_private(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Receipt>(&bytes).ok())
                .filter(|r| r.validate(key).is_ok());
            let confirmed = receipt.as_ref().is_some_and(|r| r.cleanup_confirmed);
            entries.push(InventoryEntry {
                file: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                location,
                cleanup_confirmed: confirmed,
                review_required: !confirmed,
            });
        }
    }
    Ok(entries)
}

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryPlan {
    pub receipt: String,
    pub revision: String,
    pub line_id: Option<String>,
    pub physical_key: Option<String>,
    pub generation_changed: bool,
    pub eligible: bool,
    pub blockers: Vec<String>,
    pub action: &'static str,
}
struct CheckedPlan {
    public: RecoveryPlan,
    path: PathBuf,
    bytes: Vec<u8>,
    receipt: Receipt,
    current_ports: Vec<PortIdentity>,
}

fn evaluate(
    key: &str,
    bytes: &[u8],
    receipt: &Receipt,
    current_generation: &str,
    owner_alive: bool,
    scope_matches: bool,
) -> RecoveryPlan {
    let mut blockers = Vec::new();
    if !receipt.cleanup_confirmed {
        blockers.push("unconfirmed_resources_require_device_specific_review".into());
    }
    if receipt
        .payload
        .pointer("/owner/external_channels_unknown")
        .and_then(|v| v.as_bool())
        == Some(true)
    {
        blockers.push("external_helper_channel_cleanup_not_proven".into());
    }
    if owner_alive {
        blockers.push("original_owner_still_running".into());
    }
    if !scope_matches {
        blockers.push("configured_physical_scope_changed".into());
    }
    let revision = hash(
        &serde_json::to_vec(&(
            key,
            hash(bytes),
            current_generation,
            owner_alive,
            scope_matches,
        ))
        .expect("plain recovery revision"),
    );
    RecoveryPlan {
        receipt: format!("{key}.json"),
        revision,
        line_id: Some(receipt.line_id.clone()),
        physical_key: Some(receipt.physical_key.clone()),
        generation_changed: receipt.generation != current_generation,
        eligible: blockers.is_empty(),
        blockers,
        action: "archive_confirmed_cleanup_only",
    }
}
fn checked_plan(file: &str, specs: &[NativeDeviceConfig]) -> Result<CheckedPlan, NativeError> {
    let key = file
        .strip_suffix(".json")
        .filter(|s| valid_key(s))
        .ok_or_else(|| error("native_recovery_receipt_name_invalid"))?;
    let paths = [
        Path::new(RECEIPT_DIRECTORY).join(file),
        Path::new(LOCK_DIRECTORY).join(file),
    ]
    .into_iter()
    .filter(|p| p.exists())
    .collect::<Vec<_>>();
    if paths.len() != 1 {
        return Err(error("native_recovery_receipt_missing_or_ambiguous"));
    }
    let path = paths[0].clone();
    let bytes = read_private(&path)?;
    let receipt: Receipt = serde_json::from_slice(&bytes)
        .map_err(|_| error("native_recovery_legacy_or_partial_receipt_requires_review"))?;
    receipt.validate(key)?;
    let matching = specs
        .iter()
        .filter(|s| s.line_id() == receipt.line_id)
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return Err(error("native_recovery_exact_configured_line_required"));
    }
    let spec = matching[0];
    let anchor = std::fs::canonicalize(&spec.sysfs_anchor)
        .map_err(|_| error("native_recovery_physical_anchor_absent"))?;
    let paths = std::iter::once(spec.control_device.as_str())
        .chain(spec.at_device.as_deref())
        .chain(spec.ims.iter().map(|e| e.control_device.as_str()))
        .chain(spec.data.iter().map(|e| e.control_device.as_str()))
        .collect::<std::collections::BTreeSet<_>>();
    let current_ports = paths
        .into_iter()
        .map(|p| super::io::identity(p, &anchor))
        .collect::<Result<Vec<_>, _>>()?;
    let current_generation = generation(&boot_id()?, &current_ports)?;
    let scope_matches = receipt.anchor == anchor
        && receipt.physical_key == spec.hardware_key
        && receipt.slot == spec.uim_slot;
    let public = evaluate(
        key,
        &bytes,
        &receipt,
        &current_generation,
        receipt.owner.alive()?,
        scope_matches,
    );
    Ok(CheckedPlan {
        public,
        path,
        bytes,
        receipt,
        current_ports,
    })
}
pub fn plan(file: &str, specs: &[NativeDeviceConfig]) -> Result<RecoveryPlan, NativeError> {
    checked_plan(file, specs).map(|p| p.public)
}

#[cfg(unix)]
fn recovery_locks(record: &CheckedPlan) -> Result<Vec<File>, NativeError> {
    use std::os::unix::{fs::OpenOptionsExt, io::AsRawFd};
    ensure_directory(Path::new(LOCK_DIRECTORY))?;
    let mut names = record
        .receipt
        .ports
        .iter()
        .chain(&record.current_ports)
        .map(|p| format!("port-{}", p.rdev))
        .collect::<std::collections::BTreeSet<_>>();
    names.insert(format!(
        "physical-{:x}",
        md5::compute(record.receipt.anchor.as_os_str().as_encoded_bytes())
    ));
    let mut locks = Vec::new();
    for name in names {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(Path::new(LOCK_DIRECTORY).join(name))
            .map_err(|_| error("native_recovery_lock_unavailable"))?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(error("native_recovery_owner_lock_busy"));
        }
        locks.push(file);
    }
    Ok(locks)
}
fn archive_exact(record: &CheckedPlan) -> Result<(), NativeError> {
    if !record.public.eligible || !record.receipt.cleanup_confirmed {
        return Err(error("native_recovery_cleanup_not_confirmed"));
    }
    let directory = record
        .path
        .parent()
        .ok_or_else(|| error("native_recovery_path_invalid"))?;
    if read_private(&record.path)? != record.bytes {
        return Err(error("native_recovery_receipt_changed"));
    }
    let archive = directory.join("resolved");
    ensure_directory(&archive)?;
    let stem = format!("{}-{}", record.receipt.key, record.public.revision);
    let proof = archive.join(format!("{stem}.resolution.json"));
    let bytes = serde_json::to_vec(&record.public)
        .map_err(|_| error("native_recovery_evidence_encode_failed"))?;
    if proof.exists() {
        if read_private(&proof)? != bytes {
            return Err(error("native_recovery_resolution_collision"));
        }
    } else {
        create_private(&proof, &bytes)?;
    }
    sync_directory(&archive)?;
    let target = archive.join(format!("{stem}.receipt.json"));
    // Hard-link with no overwrite, fsync evidence, then unlink only the exact
    // source. Interrupted apply is resumable with identical checked evidence.
    if target.exists() {
        if read_private(&target)? != record.bytes {
            return Err(error("native_recovery_archive_collision"));
        }
    } else {
        std::fs::hard_link(&record.path, &target)
            .map_err(|_| error("native_recovery_archive_failed"))?;
    }
    sync_directory(&archive)?;
    if read_private(&record.path)? != record.bytes {
        return Err(error("native_recovery_receipt_changed"));
    }
    std::fs::remove_file(&record.path)
        .map_err(|_| error("native_recovery_source_retirement_failed"))?;
    sync_directory(directory)
}

pub async fn apply(
    file: &str,
    specs: &[NativeDeviceConfig],
    revision: &str,
    line: &str,
    physical_key: &str,
) -> Result<RecoveryPlan, NativeError> {
    let first = checked_plan(file, specs)?;
    if !first.public.eligible
        || first.public.revision != revision
        || first.receipt.line_id != line
        || first.receipt.physical_key != physical_key
    {
        return Err(error("native_recovery_confirmation_or_revision_mismatch"));
    }
    #[cfg(unix)]
    let _locks = recovery_locks(&first)?;
    #[cfg(not(unix))]
    return Err(error("native_recovery_requires_linux"));
    let conn = zbus::Connection::system()
        .await
        .map_err(|_| error("native_recovery_owner_check_unavailable"))?;
    SystemNativeIo::verify_manager_absent(&conn).await?;
    let latest = checked_plan(file, specs)?;
    if !latest.public.eligible || latest.public.revision != revision {
        return Err(error("native_recovery_state_changed"));
    }
    SystemNativeIo::verify_manager_absent(&conn).await?;
    archive_exact(&latest)?;
    // Never restart a service, reset a modem, move a netdev or recreate a CID.
    Ok(latest.public)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Receipt {
        let owner = OwnerInstance {
            boot_id: "fixture-boot".into(),
            pid: 123,
            start_ticks: 7,
        };
        let ports = vec![PortIdentity {
            device: "/dev/fixture".into(),
            canonical: "/dev/fixture".into(),
            sysfs: "/sys/devices/fixture/port".into(),
            rdev: 10,
            inode: 11,
        }];
        Receipt {
            schema: 2,
            key: "session-fixture-sim".into(),
            line_id: "line-fixture".into(),
            physical_key: "fixture".into(),
            slot: 1,
            anchor: "/sys/devices/fixture".into(),
            generation: generation(&owner.boot_id, &ports).unwrap(),
            owner,
            ports,
            cleanup_confirmed: false,
            payload: serde_json::json!({"client_id":7}),
        }
    }
    fn temp() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "native-recovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        ensure_directory(&path).unwrap();
        path
    }
    #[test]
    fn another_generation_is_not_proof_of_protocol_resource_cleanup() {
        let mut record = fixture();
        let bytes = serde_json::to_vec(&record).unwrap();
        let plan = evaluate(&record.key, &bytes, &record, "new-generation", false, true);
        assert!(plan.generation_changed);
        assert!(!plan.eligible);
        record.cleanup_confirmed = true;
        let bytes = serde_json::to_vec(&record).unwrap();
        assert!(evaluate(&record.key, &bytes, &record, "new-generation", false, true).eligible);
        assert!(!evaluate(&record.key, &bytes, &record, "new-generation", true, true).eligible);
        assert!(!evaluate(&record.key, &bytes, &record, "new-generation", false, false).eligible);
    }
    #[test]
    fn revision_binds_receipt_generation_scope_and_owner_liveness() {
        let record = fixture();
        let bytes = serde_json::to_vec(&record).unwrap();
        let revision = evaluate(&record.key, &bytes, &record, "generation-a", false, true).revision;
        for value in [
            evaluate(&record.key, &bytes, &record, "generation-b", false, true),
            evaluate(&record.key, &bytes, &record, "generation-a", true, true),
            evaluate(
                &record.key,
                b"changed",
                &record,
                "generation-a",
                false,
                true,
            ),
        ] {
            assert_ne!(revision, value.revision);
        }
    }
    #[test]
    fn updates_require_the_original_owner_and_a_valid_complete_envelope() {
        let directory = temp();
        let mut record = fixture();
        save_owned(&directory, &record.key, &record, true).unwrap();
        record.owner.start_ticks += 1;
        assert!(save_owned(&directory, &record.key, &record, false).is_err());
        assert!(clear_owned(&directory, &record.key, &record.owner).is_err());
        assert_eq!(pending_paths(&directory).unwrap().len(), 1);
        record.owner.start_ticks -= 1;
        clear_owned(&directory, &record.key, &record.owner).unwrap();
        assert!(pending_paths(&directory).unwrap().is_empty());
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn confirmed_terminal_record_is_archived_without_replaying_payload_ids() {
        let directory = temp();
        let mut receipt = fixture();
        receipt.cleanup_confirmed = true;
        write_record(&directory, &receipt.key, &receipt, true).unwrap();
        let bytes = serde_json::to_vec(&receipt).unwrap();
        let public = evaluate(
            &receipt.key,
            &bytes,
            &receipt,
            "new-generation",
            false,
            true,
        );
        let plan = CheckedPlan {
            path: directory.join(format!("{}.json", receipt.key)),
            bytes,
            receipt,
            current_ports: Vec::new(),
            public,
        };
        archive_exact(&plan).unwrap();
        assert!(pending_paths(&directory).unwrap().is_empty());
        assert_eq!(
            std::fs::read_dir(directory.join("resolved"))
                .unwrap()
                .count(),
            2
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn archive_refuses_changed_or_unconfirmed_source_and_keeps_original() {
        let directory = temp();
        let mut receipt = fixture();
        receipt.cleanup_confirmed = true;
        write_record(&directory, &receipt.key, &receipt, true).unwrap();
        let bytes = serde_json::to_vec(&receipt).unwrap();
        let public = evaluate(
            &receipt.key,
            &bytes,
            &receipt,
            "new-generation",
            false,
            true,
        );
        let mut plan = CheckedPlan {
            path: directory.join(format!("{}.json", receipt.key)),
            bytes,
            receipt,
            current_ports: Vec::new(),
            public,
        };
        plan.public.eligible = false;
        assert!(archive_exact(&plan).is_err());
        plan.public.eligible = true;
        std::fs::write(&plan.path, b"changed").unwrap();
        assert!(archive_exact(&plan).is_err());
        assert!(plan.path.exists());
        assert!(!directory.join("resolved").exists());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn partial_foreign_line_and_legacy_dji_files_all_block_startup_inventory() {
        let directory = temp();
        for name in [
            "session-another-line-ims.json",
            "session-dji-usb-maintenance.json",
            "session-fixture-sim.123.tmp",
        ] {
            create_private(&directory.join(name), b"partial").unwrap();
        }
        assert_eq!(pending_paths(&directory).unwrap().len(), 3);
        for path in pending_paths(&directory).unwrap() {
            assert!(serde_json::from_slice::<Receipt>(&read_private(&path).unwrap()).is_err());
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn pid_start_parser_handles_parentheses_in_command_names() {
        let tail = (3..=22)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(parse_start(&format!("42 (test ) name) {tail}")), Some(22));
    }
}
