//! Explicit metadata-only retirement after an old runtime profile has vanished.
//! Never delete/modify a modem profile across a changed MM owner or SIM.

use super::*;
use std::io::{Read, Write};

const RETIRE_ERROR: &str = "mm_ims_profile_retirement_unverified";

fn validate_absent_profile(
    receipt: &Receipt,
    current: &Snapshot,
    current_boot: &str,
    creator_alive: bool,
) -> Result<i32, String> {
    let owner = validate_runtime_receipt(receipt)?;
    if !matches!(receipt.phase, Phase::Owned | Phase::Probed)
        || !matches!(
            owner.phase,
            RuntimePhase::Profile | RuntimePhase::Active | RuntimePhase::Cleaning
        )
        || (owner.boot_id == current_boot && creator_alive)
        || receipt.before.device != current.device
        || receipt.before.control_topology.is_none()
        || receipt.before.control_topology != current.control_topology
        || current.stable_sim_fingerprint.is_none()
    {
        return Err(RETIRE_ERROR.into());
    }
    let owned = receipt.owned.as_ref().ok_or(RETIRE_ERROR)?;
    if !(2..=16).contains(&owned.id)
        || owned.apn != receipt.apn
        || owned.family != receipt.requested_family
        || receipt.before.profiles.contains_key(&owned.id)
        || receipt.before.definitions.contains_key(&owned.id)
        || receipt.owned_definition.as_ref().is_none_or(|definition| {
            definition.apn != owned.apn || definition.family != owned.family
        })
        || current.profiles.contains_key(&owned.id)
        || current.definitions.contains_key(&owned.id)
    {
        // Even a changed APN/family at this ID is PRESENT, not proof of absence.
        return Err("mm_ims_profile_retirement_profile_not_absent".into());
    }
    if current.reporting.get(&owned.id) != receipt.before.reporting.get(&owned.id)
        || current.reporting.get(&owned.id) != Some(&[0, 0, 0])
    {
        return Err("mm_ims_profile_retirement_reporting_remaining".into());
    }
    if matches!(owner.phase, RuntimePhase::Active | RuntimePhase::Cleaning)
        && owner.bearer.is_none()
    {
        return Err(RETIRE_ERROR.into());
    }
    if let Some(bearer) = &owner.bearer {
        // v2 already validates bus/owner/device/process; preserve the original
        // modem binding too. No current-owner bearer RPC is issued for it.
        if bearer.modem != receipt.before.modem
            || (matches!(owner.phase, RuntimePhase::Active | RuntimePhase::Cleaning)
                && bearer.network.is_none())
        {
            return Err(RETIRE_ERROR.into());
        }
    }
    Ok(owned.id)
}

pub(super) async fn old_owner_absent(io: &MmProfileIo, receipt: &Receipt) -> Result<(), String> {
    if !io.bus.owner_is_current().await? {
        return Err(RETIRE_ERROR.into());
    }
    // Different bus IDs cannot contain the original objects, even if a unique
    // name string was reused. On the same bus require actual NameHasOwner=false,
    // not merely replacement of the MM well-known name.
    if io.bus.bus_id == receipt.before.bus_id {
        let owner_present = timed(5, async {
            let manager = zbus::fdo::DBusProxy::new(&io.bus.connection)
                .await
                .map_err(bus_error)?;
            manager
                .name_has_owner(
                    receipt
                        .before
                        .owner
                        .as_str()
                        .try_into()
                        .map_err(bus_error)?,
                )
                .await
                .map_err(bus_error)
        })
        .await?;
        if owner_present {
            return Err("mm_ims_profile_retirement_old_owner_still_present".into());
        }
    }
    if !io.bus.owner_is_current().await? {
        return Err(RETIRE_ERROR.into());
    }
    Ok(())
}

async fn observe_absence(io: &MmProfileIo, receipt: &Receipt) -> Result<Snapshot, String> {
    require_stopped()?;
    no_bearer_work()?;
    let owner = validate_runtime_receipt(receipt)?;
    old_owner_absent(io, receipt).await?;
    let snapshot = io.snapshot().await?;
    let boot = boot_id()?;
    let alive =
        owner.boot_id == boot && process_start(owner.process_id)? == Some(owner.process_start);
    validate_absent_profile(receipt, &snapshot, &boot, alive)?;
    if let Some(record) = &owner.bearer {
        // An existing old namespace needs separate reconciliation, including
        // any IPsec state. This command does not sweep or remove namespaces.
        if let Some(namespace) = &record.namespace {
            let name =
                crate::platform::netns::NetnsName::adopt(namespace).map_err(|_| RETIRE_ERROR)?;
            match fs::symlink_metadata(Path::new("/run/netns").join(name.as_str())) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err("mm_ims_profile_retirement_namespace_remaining".into()),
            }
        }
        verify_retired_network_readonly(record).await?;
    }
    old_owner_absent(io, receipt).await?;
    require_stopped()?;
    no_bearer_work()?;
    Ok(snapshot)
}

fn read_source(file: &Path) -> Result<Vec<u8>, String> {
    let input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(file)
        .map_err(|_| RETIRE_ERROR)?;
    let meta = input.metadata().map_err(|_| RETIRE_ERROR)?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
        || meta.len() > MAX_RUNTIME_RECORD_BYTES as u64
        || meta.nlink() != 1
    {
        return Err(RETIRE_ERROR.into());
    }
    let mut bytes = Vec::new();
    input
        .take(MAX_RUNTIME_RECORD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| RETIRE_ERROR)?;
    if bytes.len() > MAX_RUNTIME_RECORD_BYTES {
        return Err(RETIRE_ERROR.into());
    }
    Ok(bytes)
}

fn verify_snapshot_pair(before: &Snapshot, after: &Snapshot) -> Result<(), String> {
    if before != after {
        return Err("mm_ims_profile_retirement_snapshot_changed".into());
    }
    Ok(())
}

/// Preserve an exact copy of the ownership metadata BEFORE clearing its active
/// filename. No firmware command, configuration/DB backup, or budget deletion.
fn archive_source(file: &Path, source: &[u8]) -> Result<PathBuf, String> {
    if read_source(file)? != source {
        return Err(RETIRE_ERROR.into());
    }
    let directory = file.parent().ok_or(RETIRE_ERROR)?;
    let archive_dir = directory.join("retired");
    ensure_directory(&archive_dir)?;
    let archive = archive_dir.join(format!("absent-{}.receipt", fingerprint(&source)?));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&archive)
        .map_err(|_| "mm_ims_profile_retirement_archive_pending")?;
    output
        .write_all(source)
        .and_then(|_| output.sync_all())
        .map_err(|_| RETIRE_ERROR)?;
    fs::File::open(&archive_dir)
        .and_then(|dir| dir.sync_all())
        .map_err(|_| RETIRE_ERROR)?;
    if read_source(file)? != source {
        return Err(RETIRE_ERROR.into());
    }
    fs::remove_file(file).map_err(|_| RETIRE_ERROR)?;
    if fs::File::open(directory)
        .and_then(|dir| dir.sync_all())
        .is_err()
    {
        // Preserve blocking state if the active-name removal was not durable.
        if let Ok(mut active) = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(file)
        {
            let _ = active.write_all(source).and_then(|_| active.sync_all());
            let _ = fs::File::open(directory).and_then(|dir| dir.sync_all());
        }
        return Err("mm_ims_profile_retirement_archive_unconfirmed".into());
    }
    Ok(archive)
}

pub(super) async fn run(
    action: &str,
    io: &MmProfileIo,
    store: &DiskStore,
    receipt: Receipt,
    apn: &str,
    family: u32,
    expected_plan: Option<&str>,
) -> Result<serde_json::Value, String> {
    if receipt.apn != apn || receipt.requested_family != family {
        return Err("mm_ims_profile_retirement_selector_mismatch".into());
    }
    let source = read_source(&store.file)?;
    let decoded: Receipt = serde_json::from_slice(&source).map_err(|_| RETIRE_ERROR)?;
    if fingerprint(&decoded)? != fingerprint(&receipt)? {
        return Err(RETIRE_ERROR.into());
    }
    let before = observe_absence(io, &receipt).await?;
    let after = observe_absence(io, &receipt).await?;
    verify_snapshot_pair(&before, &after)?;
    let plan = fingerprint(&(
        "retire-absent-v1",
        fingerprint(&source)?,
        boot_id()?,
        &after,
    ))?;
    if action == "inspect-retired" {
        return Ok(
            serde_json::json!({"action":action,"plan":plan,"profile_absence_verified":true,
            "metadata_only":true,"mutated_modem":false,"active_receipt_retained":true}),
        );
    }
    if action != "retire-absent" || expected_plan != Some(plan.as_str()) {
        return Err("mm_ims_profile_retirement_expected_plan_changed".into());
    }
    require_stopped()?;
    no_bearer_work()?;
    let archive = archive_source(&store.file, &source)?;
    Ok(
        serde_json::json!({"action":action,"archive":archive,"profile_absence_verified":true,
        "metadata_only":true,"mutated_modem":false,"active_receipt_retired":true}),
    )
}

#[cfg(test)]
#[path = "primary_ims_profile_retirement_tests.rs"]
mod tests;
