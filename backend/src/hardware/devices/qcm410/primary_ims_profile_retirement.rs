//! Explicit metadata-only retirement of absent or provably uncreated profiles.
//! Never delete/modify a modem profile across a changed MM owner or SIM.

use super::*;
use std::io::{Read, Write};
use std::time::{Duration, SystemTime};

const RETIRE_ERROR: &str = "mm_ims_profile_retirement_unverified";
const UNCREATED_SETTLE: Duration = Duration::from_secs(120);

/// Explicit reconciliation only: automatic recovery must still retain Creating.
fn validate_uncreated_profile(
    receipt: &Receipt,
    current: &Snapshot,
    current_boot: &str,
    creator_alive: bool,
) -> Result<(), String> {
    let owner = validate_runtime_receipt(receipt)?;
    if receipt.phase != Phase::Creating
        || receipt.owned.is_some()
        || receipt.owned_definition.is_some()
        || owner.phase != RuntimePhase::Profile
        || owner.bearer.is_some()
        || owner.boot_id != current_boot
        || creator_alive
        || &receipt.before != current
    {
        // Compare the COMPLETE original inventory, not just the requested APN.
        return Err(RETIRE_ERROR.into());
    }
    Ok(())
}

async fn observe_uncreated(io: &MmProfileIo, receipt: &Receipt) -> Result<Snapshot, String> {
    require_stopped()?;
    no_bearer_work()?;
    let owner = validate_runtime_receipt(receipt)?;
    let boot = boot_id()?;
    let alive = process_start(owner.process_id)? == Some(owner.process_start);
    validate_uncreated_profile(receipt, &receipt.before, &boot, alive)?;
    if !io.bus.owner_is_current().await? {
        return Err(RETIRE_ERROR.into());
    }
    let snapshot = io.snapshot().await?;
    let boot = boot_id()?;
    let alive = process_start(owner.process_id)? == Some(owner.process_start);
    validate_uncreated_profile(receipt, &snapshot, &boot, alive)?;
    if !io.bus.owner_is_current().await? {
        return Err(RETIRE_ERROR.into());
    }
    require_stopped()?;
    no_bearer_work()?;
    Ok(snapshot)
}

fn validate_settled_source(
    modified: SystemTime,
    inspection_started: SystemTime,
) -> Result<(), String> {
    if inspection_started
        .duration_since(modified)
        .map_err(|_| RETIRE_ERROR)?
        < UNCREATED_SETTLE
    {
        return Err("mm_ims_profile_retirement_source_not_settled".into());
    }
    Ok(())
}

fn settled_source(file: &Path, source: &[u8], started: SystemTime) -> Result<SystemTime, String> {
    let (current, metadata) = read_source_with_metadata(file)?;
    if current != source {
        return Err(RETIRE_ERROR.into());
    }
    let modified = metadata.modified().map_err(|_| RETIRE_ERROR)?;
    validate_settled_source(modified, started)?;
    Ok(modified)
}

fn uncreated_plan(
    source: &[u8],
    snapshot: &Snapshot,
    boot: &str,
    modified: SystemTime,
) -> Result<String, String> {
    let modified = modified
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| RETIRE_ERROR)?;
    fingerprint(&(
        "retire-uncreated-v1",
        fingerprint(&source)?,
        boot,
        snapshot,
        modified.as_secs(),
        modified.subsec_nanos(),
    ))
}

fn require_uncreated_plan(plan: &str, expected: Option<&str>) -> Result<(), String> {
    if expected != Some(plan) {
        return Err("mm_ims_profile_retirement_expected_plan_changed".into());
    }
    Ok(())
}

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

/// Automatic boot recovery is narrower than explicit maintenance: the card
/// must be unchanged, and a real boot change is required. All existing absence
/// and network proofs still apply; this never adopts or deletes a modem object.
fn validate_reboot_absence(
    receipt: &Receipt,
    current: &Snapshot,
    boot: &str,
) -> Result<(), String> {
    let owner = validate_runtime_receipt(receipt)?;
    if !valid_boot_id(boot)
        || owner.boot_id == boot
        || receipt.before.stable_sim_fingerprint != current.stable_sim_fingerprint
    {
        return Err("mm_ims_profile_reboot_reconciliation_unverified".into());
    }
    validate_absent_profile(receipt, current, boot, false)?;
    Ok(())
}

pub(super) async fn retire_reboot_absence(
    io: &MmProfileIo,
    store: &DiskStore,
    receipt: &Receipt,
) -> Result<(), String> {
    let boot = boot_id()?;
    let source = read_source(&store.file)?;
    let decoded: Receipt = serde_json::from_slice(&source).map_err(|_| RETIRE_ERROR)?;
    if fingerprint(&decoded)? != fingerprint(receipt)? {
        return Err(RETIRE_ERROR.into());
    }
    // These require no other manager/worker, no bearer receipts, absent old
    // namespace/network state and a stable, current MM owner. Call at startup,
    // before line_registry creates namespaces with the same deterministic name.
    let first = observe_absence(io, receipt).await?;
    validate_reboot_absence(receipt, &first, &boot)?;
    let second = observe_absence(io, receipt).await?;
    validate_reboot_absence(receipt, &second, &boot)?;
    verify_snapshot_pair(&first, &second)?;
    if boot_id()? != boot {
        return Err(RETIRE_ERROR.into());
    }
    require_stopped()?;
    no_bearer_work()?;
    old_owner_absent(io, receipt).await?;
    archive_source(&store.file, &source)?;
    tracing::info!(
        "Archived prior-boot IMS ownership after same-SIM double absence proof; modem unchanged"
    );
    Ok(())
}

fn read_source(file: &Path) -> Result<Vec<u8>, String> {
    Ok(read_source_with_metadata(file)?.0)
}

fn open_validated_source(file: &Path) -> Result<fs::File, String> {
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
    Ok(input)
}

fn read_source_with_metadata(file: &Path) -> Result<(Vec<u8>, fs::Metadata), String> {
    let input = open_validated_source(file)?;
    let meta = input.metadata().map_err(|_| RETIRE_ERROR)?;
    let mut bytes = Vec::new();
    input
        .take(MAX_RUNTIME_RECORD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| RETIRE_ERROR)?;
    if bytes.len() > MAX_RUNTIME_RECORD_BYTES {
        return Err(RETIRE_ERROR.into());
    }
    Ok((bytes, meta))
}

fn verify_snapshot_pair(before: &Snapshot, after: &Snapshot) -> Result<(), String> {
    if before != after {
        return Err("mm_ims_profile_retirement_snapshot_changed".into());
    }
    Ok(())
}

/// Preserve an exact copy of the ownership metadata BEFORE clearing its active
/// filename. No firmware command, configuration/DB backup, or budget deletion.
pub(super) fn archive_source(file: &Path, source: &[u8]) -> Result<PathBuf, String> {
    if read_source(file)? != source {
        return Err(RETIRE_ERROR.into());
    }
    let directory = file.parent().ok_or(RETIRE_ERROR)?;
    let archive_dir = directory.join("retired");
    ensure_directory(&archive_dir)?;
    let archive = archive_dir.join(format!("absent-{}.receipt", fingerprint(&source)?));
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&archive)
    {
        Ok(mut output) => output
            .write_all(source)
            .and_then(|_| output.sync_all())
            .map_err(|_| RETIRE_ERROR)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // A crash may leave the durable archive AND the active name. Resume
            // only an exact, private, single-link copy; never overwrite evidence
            // or accept a partial write, symlink, or unrelated file.
            let input = open_validated_source(&archive)?;
            let mut bytes = Vec::new();
            (&input)
                .take(MAX_RUNTIME_RECORD_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| RETIRE_ERROR)?;
            if bytes != source {
                return Err(RETIRE_ERROR.into());
            }
            input.sync_all().map_err(|_| RETIRE_ERROR)?;
        }
        Err(_) => return Err("mm_ims_profile_retirement_archive_pending".into()),
    }
    fs::File::open(&archive_dir)
        .and_then(|dir| dir.sync_all())
        .map_err(|_| RETIRE_ERROR)?;
    // Also persist the newly created retired/ entry in its parent BEFORE
    // removing the only active name for the ownership record.
    fs::File::open(directory)
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
    let inspection_started = SystemTime::now();
    let source = read_source(&store.file)?;
    let decoded: Receipt = serde_json::from_slice(&source).map_err(|_| RETIRE_ERROR)?;
    if fingerprint(&decoded)? != fingerprint(&receipt)? {
        return Err(RETIRE_ERROR.into());
    }
    if matches!(action, "inspect-uncreated" | "retire-uncreated") {
        // A newly written Creating intent could still have an AT command in
        // flight after its creator dies. Never wait here to age it into proof.
        let modified = settled_source(&store.file, &source, inspection_started)?;
        let before = observe_uncreated(io, &receipt).await?;
        let after = observe_uncreated(io, &receipt).await?;
        verify_snapshot_pair(&before, &after)?;
        if settled_source(&store.file, &source, inspection_started)? != modified {
            return Err(RETIRE_ERROR.into());
        }
        let plan = uncreated_plan(&source, &after, &boot_id()?, modified)?;
        if action == "inspect-uncreated" {
            return Ok(serde_json::json!({"action":action,"plan":plan,
                "profile_uncreated_verified":true,"metadata_only":true,
                "mutated_modem":false,"active_receipt_retained":true}));
        }
        require_uncreated_plan(&plan, expected_plan)?;
        require_stopped()?;
        no_bearer_work()?;
        if !io.bus.owner_is_current().await?
            || settled_source(&store.file, &source, inspection_started)? != modified
        {
            return Err(RETIRE_ERROR.into());
        }
        let archive = archive_source(&store.file, &source)?;
        return Ok(serde_json::json!({"action":action,"archive":archive,
            "profile_uncreated_verified":true,"metadata_only":true,
            "mutated_modem":false,"active_receipt_retired":true}));
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
