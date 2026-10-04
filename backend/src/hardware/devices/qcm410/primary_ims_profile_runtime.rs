//! Production exact-family leases for the tested primary QCM410 topology.
//!
//! This is deliberately not the maintenance/probe entry point. Admission is
//! read-only, a family attempt always creates a fresh AT definition, and the
//! device flock follows the opaque bearer handle (including retained failures).
//! Unknown results retain the durable intent and stop the caller's family loop.

use super::*;
use crate::hardware::devices::transport::{
    ImsBearerFailureHint, ImsBearerHandle, ImsBearerInfo, ImsBearerTransport, ImsPcscfDiscovery,
    TransportFuture,
};
use std::pin::Pin;
use std::sync::Weak;
use tokio::sync::{oneshot, OwnedMutexGuard};

#[path = "primary_ims_profile_retirement.rs"]
mod retirement;

pub(super) async fn retire_absent(
    action: &str,
    io: &MmProfileIo,
    store: &DiskStore,
    receipt: Receipt,
    apn: &str,
    family: u32,
    expected_plan: Option<&str>,
) -> Result<serde_json::Value, String> {
    retirement::run(action, io, store, receipt, apn, family, expected_plan).await
}

const PRIMARY: &str = "/dev/wwan0qmi0";
const MODEM_PREFIX: &str = "/org/freedesktop/ModemManager1/Modem/";
const RUNTIME_ERROR: &str = "mm_ims_profile_runtime_unverified";
const MAX_RUNTIME_RECORD_BYTES: usize = 128 * 1024;

type Current = Arc<dyn Fn() -> bool + Send + Sync>;
type Established = (ImsBearerInfo, Box<dyn ImsBearerHandle + Send>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RuntimePhase {
    Profile,
    BearerPending,
    Active,
    Cleaning,
}

/// No raw line or subscriber identifiers are serialized. The complete bearer
/// network plan is mirrored before handoff/moves, so removal of its /run lease
/// cannot erase the evidence needed for a final read-only absence check.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeOwnership {
    version: u8,
    line_hash: String,
    generation: u64,
    process_id: u32,
    process_start: u64,
    boot_id: String,
    phase: RuntimePhase,
    abandoned: bool,
    bearer: Option<LeaseRecord>,
}

impl RuntimeOwnership {
    fn new(line: &str, generation: u64) -> Result<Self, String> {
        if line.is_empty() {
            return Err(RUNTIME_ERROR.into());
        }
        Ok(Self {
            version: 1,
            line_hash: fingerprint(&line)?,
            generation,
            process_id: std::process::id(),
            process_start: process_start(std::process::id())?.ok_or(RUNTIME_ERROR)?,
            boot_id: boot_id()?,
            phase: RuntimePhase::Profile,
            abandoned: false,
            bearer: None,
        })
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || self.line_hash.len() != 64
            || !self.line_hash.bytes().all(|b| b.is_ascii_hexdigit())
            || self.process_id == 0
            || self.process_start == 0
            || !valid_boot_id(&self.boot_id)
        {
            return Err(RUNTIME_ERROR.into());
        }
        if let Some(record) = &self.bearer {
            record.validate()?;
        }
        Ok(())
    }
}

fn validate_runtime_receipt(receipt: &Receipt) -> Result<&RuntimeOwnership, String> {
    if receipt.version != 2 || receipt.method != CreationMethod::At {
        return Err(RUNTIME_ERROR.into());
    }
    validate_request(&receipt.apn, receipt.requested_family)?;
    let owner = receipt.runtime.as_ref().ok_or(RUNTIME_ERROR)?;
    owner.validate()?;
    if receipt.before.stable_sim_fingerprint.is_none()
        || receipt.before.control_topology.is_none()
        || (owner.phase == RuntimePhase::Active && owner.bearer.is_none())
    {
        return Err(RUNTIME_ERROR.into());
    }
    if let Some(record) = &owner.bearer {
        if record.bus_id != receipt.before.bus_id
            || record.owner != receipt.before.owner
            || record.device != receipt.before.device
            || record.process_id != owner.process_id
            || record.process_start != owner.process_start
        {
            return Err(RUNTIME_ERROR.into());
        }
    }
    Ok(owner)
}

fn recovery_admitted(receipt: &Receipt, boot: &str, process_running: bool) -> Result<(), String> {
    let owner = validate_runtime_receipt(receipt)?;
    if owner.boot_id != boot
        || (!owner.abandoned && process_running)
        || owner.phase == RuntimePhase::BearerPending
        || matches!(receipt.phase, Phase::Creating | Phase::Probing)
    {
        return Err("mm_ims_profile_runtime_recovery_unresolved".into());
    }
    Ok(())
}

fn valid_boot_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

fn boot_id() -> Result<String, String> {
    let value = fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|_| RUNTIME_ERROR)?
        .trim()
        .to_string();
    if !valid_boot_id(&value) {
        return Err(RUNTIME_ERROR.into());
    }
    Ok(value)
}

fn canonical_modem(selector: &str) -> Result<String, String> {
    let selector = selector.trim();
    let id = selector.strip_prefix(MODEM_PREFIX).unwrap_or(selector);
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(RUNTIME_ERROR.into());
    }
    let id = id.parse::<u32>().map_err(|_| RUNTIME_ERROR)?;
    Ok(format!("{MODEM_PREFIX}{id}"))
}

fn failure(detail: impl Into<String>) -> ImsBearerError {
    ImsBearerError {
        kind: ImsBearerErrorKind::SessionStartFailed,
        hint: ImsBearerFailureHint::BindingChanged,
        detail: detail.into(),
    }
}

// Cleanup uncertainty must stop ordinary family fallback, but must not erase
// a fatal bearer classification. This only combines errors; the cleanup path
// remains responsible for retaining uncertain ownership receipts.
fn bearer_failure_after_cleanup(
    mut original: ImsBearerError,
    cleanup: Result<(), String>,
    current: impl FnOnce() -> Result<(), String>,
) -> ImsBearerError {
    let detail = match cleanup {
        Err(cleanup) => format!("{}:profile_cleanup_pending:{cleanup}", original.detail),
        Ok(()) => match current() {
            Ok(()) => return original,
            Err(error) if original.hint == ImsBearerFailureHint::BasebandWedged => {
                format!("{}:{error}", original.detail)
            }
            Err(error) => error,
        },
    };
    if original.hint == ImsBearerFailureHint::BasebandWedged {
        original.detail = detail;
        original
    } else {
        failure(detail)
    }
}

fn check_current(current: &Current) -> Result<(), String> {
    if !current() || is_shutting_down() {
        return Err("mm_ims_profile_runtime_generation_changed".into());
    }
    Ok(())
}

fn ledger_file(device: &str) -> Result<PathBuf, String> {
    Ok(Path::new(DIRECTORY).join(format!("profile-{}.json", fingerprint(&device)?)))
}

fn read_receipt(file: &Path) -> Result<Option<Receipt>, String> {
    let metadata = match fs::symlink_metadata(file) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(RUNTIME_ERROR.into()),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != 0
        || metadata.mode() & 0o077 != 0
        || metadata.len() > 128 * 1024
    {
        return Err(RUNTIME_ERROR.into());
    }
    let input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(file)
        .map_err(|_| RUNTIME_ERROR)?;
    let opened = input.metadata().map_err(|_| RUNTIME_ERROR)?;
    if opened.ino() != metadata.ino() || opened.dev() != metadata.dev() {
        return Err(RUNTIME_ERROR.into());
    }
    serde_json::from_reader(std::io::Read::take(input, 128 * 1024 + 1))
        .map(Some)
        .map_err(|_| RUNTIME_ERROR.into())
}

/// A receipt that cannot be associated with this endpoint must not fall
/// through to generic APN reuse either. In particular a changed/absent device
/// string must not bypass a pending profile from the tested primary endpoint.
fn reject_other_receipts(file: &Path) -> Result<(), String> {
    for directory in ["/var/lib/simadmin", DIRECTORY] {
        match fs::symlink_metadata(directory) {
            Ok(metadata)
                if metadata.is_dir()
                    && !metadata.file_type().is_symlink()
                    && metadata.uid() == 0
                    && metadata.mode() & 0o022 == 0 => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(RUNTIME_ERROR.into()),
        }
    }
    match fs::read_dir(DIRECTORY) {
        Ok(entries) => {
            for entry in entries {
                let path = entry.map_err(|_| RUNTIME_ERROR)?.path();
                if path.extension().and_then(|s| s.to_str()) == Some("json") && path != file {
                    return Err("mm_ims_profile_runtime_other_receipt_pending".into());
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(RUNTIME_ERROR.into()),
    }
    Ok(())
}

pub(super) fn ensure_no_pending_profile(device: &str) -> Result<(), String> {
    let file = ledger_file(device.trim())?;
    reject_other_receipts(&file)?;
    if read_receipt(&file)?.is_some() {
        return Err("mm_ims_profile_runtime_receipt_pending".into());
    }
    // A runtime admission can hold the flock before writing its first intent.
    // Do not call that interval 'no pending profile' based on ENOENT alone.
    match OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(file.with_extension("lock"))
    {
        Ok(lock) => {
            let metadata = lock.metadata().map_err(|_| RUNTIME_ERROR)?;
            if !metadata.is_file()
                || metadata.uid() != 0
                || metadata.mode() & 0o077 != 0
                || metadata.nlink() != 1
                || unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0
            {
                return Err("mm_ims_profile_runtime_device_busy".into());
            }
            // Recheck after acquiring the same exclusion used by maintenance.
            if read_receipt(&file)?.is_some() {
                return Err(RUNTIME_ERROR.into());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(RUNTIME_ERROR.into()),
    }
    Ok(())
}

/// Reserve the ordinary preparation interval, not merely a point-in-time
/// absence check. Keep this fd through legacy bearer preparation so runtime
/// allocation cannot race APN-based reuse of an owned temporary profile.
pub(super) fn ordinary_profile_guard(device: &str) -> Result<fs::File, String> {
    let file = ledger_file(device.trim())?;
    reject_other_receipts(&file)?;
    let lock = device_lock(&file)?;
    reject_other_receipts(&file)?;
    if read_receipt(&file)?.is_some() {
        return Err("mm_ims_profile_runtime_receipt_pending".into());
    }
    Ok(lock)
}

/// Held from the post-cleanup proof until lpac AND the MM cycle finish. The
/// same nonblocking flock belongs to Context, so even a receipt-free cleanup
/// or an admission that has not written its intent yet prevents a switch.
#[must_use = "retain the drain guard until the complete eSIM switch finishes"]
pub struct EsimSwitchDrainGuard {
    _lock: fs::File,
}

/// This is an absence barrier, NOT recovery and NOT a multi-line drain. The
/// caller closes MM admission and drains its live session first, outside the
/// serial permit. The MM restart is global: reject ALL owned/pending bearer
/// work, including another line's work, without trying to clean it here.
pub(super) fn esim_switch_drain_guard() -> Result<EsimSwitchDrainGuard, String> {
    let file = ledger_file(PRIMARY)?;
    reject_other_receipts(&file)?;
    let lock = device_lock(&file)?;
    switch_drain_under_lock(&file, lock, || {
        reject_other_receipts(&file)?;
        // A different Context may still be before its first receipt or after
        // receipt removal. Do not mistake those intervals for a global drain.
        if live_contexts()
            .lock()
            .unwrap()
            .iter()
            .any(|context| context.strong_count() != 0)
        {
            return Err("mm_ims_profile_runtime_device_busy".into());
        }
        switch_bearer_work_absent(
            PENDING.load(Ordering::Acquire),
            !leases().lock().unwrap().is_empty(),
            Path::new(STATE_DIR),
        )
    })
}

fn switch_drain_under_lock(
    file: &Path,
    lock: fs::File,
    no_other_work: impl FnOnce() -> Result<(), String>,
) -> Result<EsimSwitchDrainGuard, String> {
    // Existence, not validity/owner/phase, is the boundary. Stale, truncated,
    // symlinked and otherwise damaged receipts must all keep lpac blocked.
    match fs::symlink_metadata(file) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => return Err("mm_ims_profile_runtime_receipt_pending".into()),
        Err(_) => return Err(RUNTIME_ERROR.into()),
    }
    no_other_work()?;
    Ok(EsimSwitchDrainGuard { _lock: lock })
}

fn switch_bearer_work_absent(
    pending: usize,
    live: bool,
    directory: &Path,
) -> Result<(), String> {
    if pending != 0 || live {
        return Err("mm_ims_profile_runtime_bearer_cleanup_pending".into());
    }
    // Validate the parent first: a dangling/replaced /run/simadmin symlink
    // must not turn a hidden bearer directory into an ENOENT absence proof.
    for path in [directory.parent().ok_or(RUNTIME_ERROR)?, directory] {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(metadata)
                if metadata.is_dir()
                    && !metadata.file_type().is_symlink()
                    && metadata.uid() == unsafe { libc::geteuid() }
                    && metadata.mode() & 0o022 == 0 => {}
            _ => return Err(RUNTIME_ERROR.into()),
        }
    }
    // Match the durable bearer cleanup helper's .json and pending .create
    // markers. Do not parse them: an unreadable/damaged marker is still work.
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(RUNTIME_ERROR.into()),
    };
    for entry in entries {
        let path = entry.map_err(|_| RUNTIME_ERROR)?.path();
        if matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("json" | "create")
        ) {
            return Err("mm_ims_profile_lease_bearer_receipt_remaining".into());
        }
    }
    Ok(())
}

fn device_lock(file: &Path) -> Result<fs::File, String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("mm_ims_profile_runtime_root_required".into());
    }
    ensure_directory(Path::new("/var/lib/simadmin"))?;
    ensure_directory(Path::new(DIRECTORY))?;
    open_device_lock(file, 0)
}

fn open_device_lock(file: &Path, uid: u32) -> Result<fs::File, String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(file.with_extension("lock"))
        .map_err(|_| RUNTIME_ERROR)?;
    let metadata = file.metadata().map_err(|_| RUNTIME_ERROR)?;
    // Separate open file descriptions make flock exclude threads in this
    // process too. Do not clone a globally cached fd or unlink the lock file.
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
        || unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0
    {
        return Err("mm_ims_profile_runtime_device_busy".into());
    }
    Ok(file)
}

struct Stored {
    receipt: Option<Receipt>,
    ownership: RuntimeOwnership,
    poisoned: bool,
}

struct RuntimeStore {
    disk: DiskStore,
    state: Mutex<Stored>,
}

impl RuntimeStore {
    fn new(file: PathBuf, ownership: RuntimeOwnership, receipt: Option<Receipt>) -> Self {
        Self {
            disk: DiskStore { file },
            state: Mutex::new(Stored {
                receipt,
                ownership,
                poisoned: false,
            }),
        }
    }

    fn receipt(&self) -> Result<Option<Receipt>, String> {
        let state = self.state.lock().unwrap();
        if state.poisoned {
            return Err("mm_ims_profile_runtime_store_unverified".into());
        }
        Ok(state.receipt.clone())
    }

    fn update(
        &self,
        update: impl FnOnce(&mut Receipt, &mut RuntimeOwnership),
    ) -> Result<(), String> {
        let mut receipt = self.receipt()?.ok_or(RUNTIME_ERROR)?;
        let mut ownership = receipt.runtime.take().ok_or(RUNTIME_ERROR)?;
        update(&mut receipt, &mut ownership);
        receipt.runtime = Some(ownership);
        self.save(&receipt)
    }
}

impl Store for RuntimeStore {
    fn save(&self, receipt: &Receipt) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if state.poisoned {
            return Err("mm_ims_profile_runtime_store_unverified".into());
        }
        let mut next = receipt.clone();
        next.version = 2;
        next.runtime = Some(
            next.runtime
                .take()
                .unwrap_or_else(|| state.ownership.clone()),
        );
        // Keep the last possible intent in memory even if rename/fsync fails.
        // A later call in this process must never reinterpret ENOENT as safe.
        state.receipt = Some(next.clone());
        let encoded = serde_json::to_vec(&next).map_err(|_| RUNTIME_ERROR)?;
        if encoded.len() > MAX_RUNTIME_RECORD_BYTES {
            state.poisoned = true;
            return Err("mm_ims_profile_runtime_record_too_large".into());
        }
        if let Err(error) = self.disk.save(&next) {
            state.poisoned = true;
            return Err(error);
        }
        Ok(())
    }

    fn remove(&self) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if state.poisoned {
            return Err("mm_ims_profile_runtime_store_unverified".into());
        }
        if let Err(error) = self.disk.remove() {
            state.poisoned = true;
            // Best effort restore after a failed directory sync, never delete
            // an uncertain ledger merely to unlock a subsequent attempt.
            if let Some(receipt) = &state.receipt {
                let _ = self.disk.save(receipt);
            }
            return Err(error);
        }
        state.receipt = None;
        Ok(())
    }
}

struct Context {
    bus: Arc<MmBus>,
    apn: String,
    sim: (String, u8),
    current: Current,
    store: RuntimeStore,
    attempt: Arc<tokio::sync::Mutex<()>>,
    cleanup_lock: tokio::sync::Mutex<()>,
    _lock: fs::File,
}

/// Owned admission, not a reusable profile ID. Keep this value alive for the
/// caller's existing family loop; each invocation has its own create/delete.
pub struct RuntimeProfileTransport {
    context: Arc<Context>,
}

fn live_contexts() -> &'static Mutex<Vec<Weak<Context>>> {
    static CONTEXTS: OnceLock<Mutex<Vec<Weak<Context>>>> = OnceLock::new();
    CONTEXTS.get_or_init(Mutex::default)
}

fn register_context(context: Arc<Context>) -> Arc<Context> {
    let mut contexts = live_contexts().lock().unwrap();
    contexts.retain(|old| old.strong_count() != 0);
    contexts.push(Arc::downgrade(&context));
    context
}

fn shutdown_phase_ready(phase: RuntimePhase, recorded_bearer: bool) -> bool {
    recorded_bearer && matches!(phase, RuntimePhase::Active | RuntimePhase::Cleaning)
}

fn shutdown_profile_ready(receipt: &Receipt) -> bool {
    !matches!(receipt.phase, Phase::Creating | Phase::Probing)
        && validate_runtime_receipt(receipt)
            .is_ok_and(|owner| shutdown_phase_ready(owner.phase, owner.bearer.is_some()))
}

/// Called by the device shutdown hook AFTER its retained bearer drain. Main
/// exits explicitly, so relying only on RuntimeHandle::drop would leave every
/// active profile behind. This never turns on global shutdown itself and does
/// not delete while profile creation/reporting or bearer setup is uncertain.
pub async fn shutdown_profiles() {
    let contexts: Vec<_> = live_contexts()
        .lock()
        .unwrap()
        .iter()
        .filter_map(Weak::upgrade)
        .collect();
    for context in contexts {
        let ready = context
            .store
            .receipt()
            .ok()
            .flatten()
            .is_some_and(|r| shutdown_profile_ready(&r));
        if ready {
            if let Err(error) = finish_after_bearer(&context, false).await {
                tracing::warn!(error, "Runtime IMS profile shutdown cleanup deferred");
            }
        }
    }
}

/// None is possible only before any modem mutation, with no unresolved profile
/// receipt. Call this BEFORE legacy profile preparation/reporting, not after it.
pub async fn prepare(
    device: &str,
    modem: &str,
    apn: &str,
    expected_sim: (&str, u8),
    line_id: &str,
    generation: u64,
    is_current: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<Option<RuntimeProfileTransport>, String> {
    check_current(&is_current)?;
    let device = device.trim();
    let file = ledger_file(device)?;
    reject_other_receipts(&file)?;
    let existing = read_receipt(&file)?;
    if device != PRIMARY {
        return if existing.is_some() {
            Err(RUNTIME_ERROR.into())
        } else {
            Ok(None)
        };
    }
    // A missing expected endpoint is known non-applicability only without an
    // ownership record. Ambiguous physical topology is an error, not fallback.
    match fs::metadata(device) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && existing.is_none() => {
            return Ok(None)
        }
        Err(_) => return Err(RUNTIME_ERROR.into()),
        Ok(_) => {}
    }
    let interface = netdev::primary_netdev_for_qmi(device).ok_or(RUNTIME_ERROR)?;
    netdev::verify_mm_data_interface(device, &interface)?;
    let modem = canonical_modem(modem)?;
    validate_request(apn, 4)?;
    if expected_sim.0.is_empty() || expected_sim.1 == 0 {
        return Err(RUNTIME_ERROR.into());
    }
    let lock = device_lock(&file)?;
    reject_other_receipts(&file)?;
    let existing = read_receipt(&file)?; // re-read under the shared maintenance flock
    if existing.is_some() {
        // Recovery is a separate, shielded capability. Do not recover and then
        // return None: nonapplicability must remain a pre-mutation decision.
        return Err("mm_ims_profile_runtime_recovery_required".into());
    }
    let bus = MmBus::new(device, &modem, &interface).await?;
    check_current(&is_current)?;
    bus.pin_sim_binding().await?;
    check_current(&is_current)?;
    bus.verify_expected_sim(expected_sim.0, expected_sim.1)
        .await?;
    check_current(&is_current)?;
    let bearers = bus.bearers().await?;
    check_current(&is_current)?;
    // Refuse even disconnected competing MM objects. Never stop ordinary data
    // or secondary service to make runtime admission possible.
    if !bearers.is_empty() {
        return Ok(None);
    }
    no_bearer_work()?;
    let io = MmProfileIo {
        bus: Arc::clone(&bus),
        method: CreationMethod::At,
        topology: physical_control_topology,
    };
    io.snapshot().await?;
    check_current(&is_current)?;
    let ownership = RuntimeOwnership::new(line_id, generation)?;
    Ok(Some(RuntimeProfileTransport {
        context: register_context(Arc::new(Context {
            bus,
            apn: apn.into(),
            sim: (expected_sim.0.into(), expected_sim.1),
            current: is_current,
            store: RuntimeStore::new(file, ownership, None),
            attempt: Arc::new(tokio::sync::Mutex::new(())),
            cleanup_lock: tokio::sync::Mutex::new(()),
            _lock: lock,
        })),
    }))
}

fn no_bearer_work() -> Result<(), String> {
    if PENDING.load(Ordering::Acquire) != 0 || !leases().lock().unwrap().is_empty() {
        return Err("mm_ims_profile_runtime_bearer_cleanup_pending".into());
    }
    require_no_bearer_receipts()
}

/// Generation/cancellation checks are repeated inside the serial permit, not
/// just before waiting for it. Cleanup deliberately uses the original owner
/// and SIM rather than authorizing mutation using a newer line generation.
struct RuntimeIo {
    inner: MmProfileIo,
    current: Current,
    definition_dispatched: AtomicBool,
    reporting_dispatched: AtomicBool,
}

impl RuntimeIo {
    fn observation_current(&self) -> Result<(), String> {
        let current = check_current(&self.current);
        // After a dispatched mutation, complete readback even if the caller
        // vanished. Ownership must be resolved before deciding what to clean.
        // This never authorizes another write or a bearer on a stale generation.
        if self.definition_dispatched.load(Ordering::Acquire)
            || self.reporting_dispatched.load(Ordering::Acquire)
        {
            Ok(())
        } else {
            current
        }
    }

    async fn command(&self, command: &str) -> Result<String, String> {
        let definition = command.starts_with("AT+CGDCONT=") && !command.ends_with('?');
        let reporting = command.starts_with("AT$QCPDPIMSCFGE=");
        let write = definition || reporting;
        if write {
            check_current(&self.current)?;
        } else {
            self.observation_current()?;
        }
        let dispatched = AtomicBool::new(false);
        let result = recovery::command_checked(&self.inner.bus, command, || async {
            if self.inner.quiescent().await.is_err() {
                return false;
            }
            if write && !dispatched.load(Ordering::Acquire) {
                if check_current(&self.current).is_err() {
                    return false;
                }
                dispatched.store(true, Ordering::Release);
                if definition {
                    self.definition_dispatched.store(true, Ordering::Release);
                }
                if reporting {
                    self.reporting_dispatched.store(true, Ordering::Release);
                }
                true
            } else {
                self.observation_current().is_ok()
            }
        })
        .await;
        self.observation_current()?;
        result
    }
}

impl ProfileIo for RuntimeIo {
    fn method(&self) -> CreationMethod {
        CreationMethod::At
    }
    async fn snapshot(&self) -> Result<Snapshot, String> {
        self.observation_current()?;
        let result = self.inner.snapshot().await;
        self.observation_current()?;
        result
    }
    async fn create(&self, apn: &str, family: u32, _tag: &str) -> Result<Profile, String> {
        create_at_with(self, apn, family, |command| async move {
            self.command(&command).await
        })
        .await
    }
    async fn restore_reporting(&self, id: i32, flags: [u8; 3]) -> Result<(), String> {
        if !(2..=16).contains(&id) || flags.iter().any(|flag| *flag > 1) {
            return Err(RUNTIME_ERROR.into());
        }
        self.command(&format!(
            "AT$QCPDPIMSCFGE={id},{},{},{}",
            flags[0], flags[1], flags[2]
        ))
        .await?;
        Ok(())
    }
    async fn delete(&self, _id: i32) -> Result<(), String> {
        Err("mm_ims_profile_runtime_setup_cannot_delete".into())
    }
}

fn requested_family(families: &[u8]) -> Result<u32, String> {
    match families {
        [4] => Ok(1),
        [6] => Ok(2),
        [4, 6] | [6, 4] => Ok(4),
        _ => Err(RUNTIME_ERROR.into()),
    }
}

impl ImsBearerTransport for RuntimeProfileTransport {
    fn endpoint_available(&self, device: &str) -> bool {
        device.trim() == self.context.bus.device && (self.context.current)()
    }

    fn establish_ims_bearer<'a>(
        &'a self,
        device: &'a str,
        modem: &'a str,
        apn: &'a str,
        profile_id: Option<u32>,
        _cid: u8,
        families: &'a [u8],
        allow_roaming: bool,
        expected_sim: Option<(&'a str, u8)>,
    ) -> TransportFuture<'a, Result<Established, ImsBearerError>> {
        Box::pin(async move {
            let context = &self.context;
            check_current(&context.current).map_err(failure)?;
            if device.trim() != context.bus.device
                || canonical_modem(modem).map_err(failure)? != context.bus.modem
                || apn != context.apn
                || profile_id.is_some()
                || expected_sim != Some((context.sim.0.as_str(), context.sim.1))
            {
                return Err(failure("mm_ims_profile_runtime_request_changed"));
            }
            let family = requested_family(families).map_err(failure)?;
            let permit = Arc::clone(&context.attempt)
                .try_lock_owned()
                .map_err(|_| failure("mm_ims_profile_runtime_attempt_active"))?;
            let context = Arc::clone(context);
            let families = families.to_vec();
            let cancelled = Arc::new(AtomicBool::new(false));
            let _cancellation = Cancel(Arc::clone(&cancelled));
            let current: Current = {
                let context = Arc::clone(&context);
                Arc::new(move || !cancelled.load(Ordering::Acquire) && (context.current)())
            };
            let (sender, receiver) = oneshot::channel();
            // A dropped caller must not abort AT, Connect or network cleanup.
            tokio::spawn(async move {
                let result =
                    attempt(context, permit, family, families, allow_roaming, current).await;
                deliver(sender, result).await;
            });
            tokio::time::timeout(Duration::from_secs(120), receiver)
                .await
                .map_err(|_| failure("mm_ims_profile_runtime_setup_timeout_unverified"))?
                .map_err(|_| failure("mm_ims_profile_runtime_task_failed"))?
        })
    }
}

struct Cancel(Arc<AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

async fn deliver(
    sender: oneshot::Sender<Result<Established, ImsBearerError>>,
    result: Result<Established, ImsBearerError>,
) {
    if let Err(Ok((_info, handle))) = sender.send(result) {
        handle.release().await;
    }
}

async fn arm<I: ProfileIo, S: Store>(
    io: &I,
    store: &S,
    mut receipt: Receipt,
) -> Result<(), String> {
    let current = io.snapshot().await?;
    let id = owned_matches(&receipt, &current)?;
    receipt.phase = Phase::Probing; // unknown reporting write is never replayed
    store.save(&receipt)?;
    io.restore_reporting(id, [1, 1, 1]).await?;
    let armed = io.snapshot().await?;
    owned_matches(&receipt, &armed)?;
    if armed.reporting.get(&id) != Some(&[1, 1, 1]) || io.snapshot().await? != armed {
        return Err(RUNTIME_ERROR.into());
    }
    receipt.phase = Phase::Probed;
    store.save(&receipt)
}

async fn attempt(
    context: Arc<Context>,
    permit: OwnedMutexGuard<()>,
    family: u32,
    families: Vec<u8>,
    allow_roaming: bool,
    current: Current,
) -> Result<Established, ImsBearerError> {
    let io = RuntimeIo {
        inner: MmProfileIo {
            bus: Arc::clone(&context.bus),
            method: CreationMethod::At,
            topology: physical_control_topology,
        },
        current: Arc::clone(&current),
        definition_dispatched: AtomicBool::new(false),
        reporting_dispatched: AtomicBool::new(false),
    };
    let setup = async {
        check_current(&current)?;
        if context.store.receipt()?.is_some() {
            return Err("mm_ims_profile_runtime_receipt_pending".into());
        }
        no_bearer_work()?;
        let before = io.snapshot().await?;
        let plan = fingerprint(&(context.apn.as_str(), family, &before))?;
        acquire_with(&io, &context.store, &context.apn, family, &plan).await?;
        let receipt = context.store.receipt()?.ok_or(RUNTIME_ERROR)?;
        check_current(&current)?;
        arm(&io, &context.store, receipt).await?;
        check_current(&current)?;
        context
            .store
            .update(|_, owner| owner.phase = RuntimePhase::BearerPending)?;
        Ok::<(), String>(())
    }
    .await;
    if let Err(error) = setup {
        // The running task may prove a command was never dispatched. A
        // restarted process cannot infer that fact from the same disk phase.
        let cleanup = async {
            if let Some(receipt) = context.store.receipt()? {
                if receipt.phase == Phase::Creating
                    && !io.definition_dispatched.load(Ordering::Acquire)
                {
                    if io.inner.snapshot().await? != receipt.before
                        || io.inner.snapshot().await? != receipt.before
                    {
                        return Err(RUNTIME_ERROR.into());
                    }
                    no_bearer_work()?;
                    context.store.remove()?;
                } else if receipt.phase == Phase::Probing
                    && !io.reporting_dispatched.load(Ordering::Acquire)
                {
                    context
                        .store
                        .update(|receipt, _| receipt.phase = Phase::Owned)?;
                }
            }
            finish_profile(&context, false).await
        }
        .await;
        if cleanup.is_ok() && check_current(&current).is_ok() {
            return Err(ImsBearerError {
                kind: ImsBearerErrorKind::SessionStartFailed,
                hint: ImsBearerFailureHint::None,
                detail: error,
            });
        }
        return Err(failure(error));
    }
    let receipt = context
        .store
        .receipt()
        .map_err(failure)?
        .ok_or_else(|| failure(RUNTIME_ERROR))?;
    let id = receipt
        .owned
        .as_ref()
        .ok_or_else(|| failure(RUNTIME_ERROR))?
        .id;
    if let Err(error) = check_current(&current) {
        // No bearer dispatch occurred after the durable pending transition.
        // This task knows that fact; restart recovery deliberately does not.
        let _ = finish_profile(&context, true).await;
        return Err(failure(error));
    }
    let attempt_bus = match context.bus.for_profile_attempt(Arc::clone(&current)) {
        Ok(bus) => bus,
        Err(error) => {
            let _ = finish_profile(&context, true).await;
            return Err(failure(error));
        }
    };
    let result = crate::hardware::devices::qcm410::ims_bearer::establish_with_bus(
        &context.bus.device,
        &context.bus.modem,
        &context.apn,
        Some(id as u32),
        id as u8,
        &families,
        allow_roaming,
        Some((&context.sim.0, context.sim.1)),
        Some(attempt_bus),
    )
    .await;
    match result {
        Err(original) => {
            // Recover only abandoned/dead leases, never global shutdown. A late
            // setup is still counted by PENDING and cannot authorize deletion.
            let cleanup = finish_after_bearer(&context, true).await;
            Err(bearer_failure_after_cleanup(original, cleanup, || {
                check_current(&current)
            }))
        }
        Ok((info, inner)) => {
            let bearer = match info.path_handle.strip_prefix("mm:") {
                Some(path) => path.to_string(),
                None => {
                    inner.release().await;
                    let _ = finish_profile(&context, true).await;
                    return Err(failure("mm_ims_profile_runtime_bearer_identity_missing"));
                }
            };
            let mut handle = RuntimeHandle {
                bearer,
                inner: Some(inner),
                context: Some(Arc::clone(&context)),
                permit: Some(permit),
            };
            if let Err(error) = handle
                .capture(RuntimePhase::Active)
                .and_then(|_| check_current(&current))
            {
                Box::new(handle).release().await;
                return Err(failure(error));
            }
            Ok((info, Box::new(handle)))
        }
    }
}

fn owned_record(context: &Context, bearer: &str) -> Result<LeaseRecord, String> {
    let candidates: Vec<_> = leases()
        .lock()
        .unwrap()
        .values()
        .filter(|lease| {
            lease.path() == bearer
                && lease.bus.bus_id == context.bus.bus_id
                && lease.bus.owner == context.bus.owner
                && lease.bus.modem == context.bus.modem
                && lease.bus.device == context.bus.device
        })
        .cloned()
        .collect();
    let [lease] = candidates.as_slice() else {
        return Err(RUNTIME_ERROR.into());
    };
    let record = lease.record.lock().unwrap().clone();
    record.validate()?;
    Ok(record)
}

struct RuntimeHandle {
    bearer: String,
    inner: Option<Box<dyn ImsBearerHandle + Send>>,
    context: Option<Arc<Context>>,
    permit: Option<OwnedMutexGuard<()>>,
}

impl RuntimeHandle {
    fn capture(&mut self, phase: RuntimePhase) -> Result<(), String> {
        let context = self.context.as_ref().ok_or(RUNTIME_ERROR)?;
        let record = owned_record(context, &self.bearer)?;
        context.store.update(|_, owner| {
            owner.phase = phase;
            owner.bearer = Some(record);
        })
    }
    fn current(&self) -> Result<(), ImsBearerError> {
        check_current(&self.context.as_ref().expect("retained context").current).map_err(failure)
    }
}

impl ImsBearerHandle for RuntimeHandle {
    fn check_liveness(&mut self) -> Result<(), ImsBearerError> {
        self.current()?;
        self.inner
            .as_mut()
            .expect("retained handle")
            .check_liveness()
    }
    fn verify_mm_sim_binding<'a>(
        &'a mut self,
        iccid: &'a str,
        slot: u8,
    ) -> TransportFuture<'a, Result<(), ImsBearerError>> {
        Box::pin(async move {
            self.current()?;
            let result = self
                .inner
                .as_mut()
                .expect("retained handle")
                .verify_mm_sim_binding(iccid, slot)
                .await;
            self.current()?;
            result
        })
    }
    fn discover_pcscf(
        &mut self,
    ) -> TransportFuture<'_, Result<Option<ImsPcscfDiscovery>, ImsBearerError>> {
        Box::pin(async move {
            self.current()?;
            let result = self
                .inner
                .as_mut()
                .expect("retained handle")
                .discover_pcscf()
                .await;
            self.current()?;
            result
        })
    }
    fn prepare_namespace_move(&mut self, namespace: &str) -> Result<Box<dyn Send>, ImsBearerError> {
        self.current()?;
        let guard = self
            .inner
            .as_mut()
            .expect("retained handle")
            .prepare_namespace_move(namespace)?;
        self.capture(RuntimePhase::Active).map_err(failure)?;
        Ok(guard)
    }
    fn confirm_namespace_restore<'a>(
        &'a mut self,
        namespace: &'a str,
    ) -> TransportFuture<'a, Result<(), ImsBearerError>> {
        Box::pin(async move {
            // Restoration is cleanup and must also work after invalidation.
            let result = self
                .inner
                .as_mut()
                .expect("retained handle")
                .confirm_namespace_restore(namespace)
                .await;
            self.capture(RuntimePhase::Active).map_err(failure)?;
            result
        })
    }
    fn release(mut self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        // Schedule immediately: even dropping the returned future cannot drop
        // the ownership/permit ahead of a dispatched cleanup.
        let task = self.schedule_cleanup();
        Box::pin(async move {
            if let Some(task) = task {
                let _ = task.await;
            }
        })
    }
}

impl RuntimeHandle {
    fn schedule_cleanup(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        let context = self.context.take()?;
        let inner = self.inner.take();
        let permit = self.permit.take();
        // Capture latest namespace/network intent before the inner release can
        // erase it. A failed mirror blocks profile deletion, not bearer cleanup.
        let captured = owned_record(&context, &self.bearer)
            .or_else(|_| {
                // The ordinary shutdown may already have released this exact
                // bearer. Its last mirrored network plan still needs read-only
                // verification; absence of the live registry is not proof.
                let receipt = context.store.receipt()?.ok_or(RUNTIME_ERROR)?;
                validate_runtime_receipt(&receipt)?
                    .bearer
                    .clone()
                    .filter(|record| record.bearer == self.bearer)
                    .ok_or_else(|| RUNTIME_ERROR.to_string())
            })
            .and_then(|record| {
                context.store.update(|_, owner| {
                    owner.bearer = Some(record);
                    owner.phase = RuntimePhase::Cleaning;
                    owner.abandoned = true;
                })
            });
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => Some(runtime.spawn(async move {
                let _permit = permit;
                if let Some(inner) = inner {
                    inner.release().await;
                }
                if captured.is_ok() {
                    if let Err(error) = finish_after_bearer(&context, true).await {
                        tracing::warn!(
                            error,
                            "Runtime IMS profile cleanup pending; ledger retained"
                        );
                    }
                }
            })),
            Err(_) => {
                // No executor: inner Drop and durable receipts retain recovery
                // ownership. Never attempt a synchronous profile-only delete.
                drop(inner);
                drop(permit);
                None
            }
        }
    }
}
impl Drop for RuntimeHandle {
    fn drop(&mut self) {
        let _ = self.schedule_cleanup();
    }
}

/// Use ObjectManager on the ORIGINAL unique owner for profile-only discovery.
/// No old bearer RPC is redirected to this new modem path.
async fn identity_io(receipt: &Receipt) -> Result<MmProfileIo, String> {
    identity_io_with(receipt, physical_control_topology).await
}

async fn identity_io_with(
    receipt: &Receipt,
    topology: fn(&str) -> Result<String, String>,
) -> Result<MmProfileIo, String> {
    let old = &receipt.before;
    let interface = netdev::primary_netdev_for_qmi(&old.device).ok_or(RUNTIME_ERROR)?;
    let original = MmBus::new(&old.device, &old.modem, &interface).await?;
    if original.bus_id != old.bus_id || original.owner != old.owner {
        return Err("mm_ims_profile_runtime_owner_changed".into());
    }
    let objects: ManagedObjects = timed(10, async {
        original
            .proxy(
                "/org/freedesktop/ModemManager1",
                "org.freedesktop.DBus.ObjectManager",
            )
            .await?
            .call("GetManagedObjects", &())
            .await
            .map_err(bus_error)
    })
    .await?;
    let mut paths = objects.iter().filter_map(|(path, interfaces)| {
        let properties = interfaces.get(MODEM)?;
        let port = properties
            .get("PrimaryPort")
            .and_then(|v| <&str>::try_from(v).ok())?;
        (Some(port) == old.device.strip_prefix("/dev/")).then(|| path.to_string())
    });
    let modem = paths.next().ok_or(RUNTIME_ERROR)?;
    if paths.next().is_some() {
        return Err(RUNTIME_ERROR.into());
    }
    let bus = Arc::new(MmBus {
        connection: original.connection.clone(),
        bus_id: original.bus_id.clone(),
        owner: original.owner.clone(),
        modem,
        device: old.device.clone(),
        interface,
        selected_interface: OnceLock::new(),
        sim_binding: OnceLock::new(),
        setup_current: None,
    });
    bus.pin_sim_binding().await?;
    let io = MmProfileIo {
        bus,
        method: CreationMethod::At,
        topology,
    };
    let sim = io.bus.read_sim_binding().await?;
    let stable = fingerprint(&(&sim.id, sim.slot))?;
    let full = fingerprint(&(&sim.path, &sim.id, sim.slot))?;
    if old.stable_sim_fingerprint.as_ref() != Some(&stable)
        || old.control_topology.as_ref() != Some(&(io.topology)(&old.device)?)
        || (old.modem == io.bus.modem && old.sim_fingerprint != full)
        || (old.modem != io.bus.modem && !original_modem_absent(&io, &old.modem).await?)
    {
        return Err(RUNTIME_ERROR.into());
    }
    Ok(io)
}

fn cleanup_may_settle(receipt: &Receipt, setup_finished: bool) -> bool {
    matches!(receipt.phase, Phase::Owned | Phase::Probed)
        && validate_runtime_receipt(receipt)
            .is_ok_and(|owner| owner.phase != RuntimePhase::BearerPending || setup_finished)
}

/// Returning the data interface can briefly remove the MM object. Retry only
/// before a reporting/Delete mutation starts; uncertain writes are never
/// replayed. The original-owner identity checks remain inside each attempt.
async fn finish_after_bearer(context: &Context, setup_finished: bool) -> Result<(), String> {
    for attempt in 0..4 {
        let error = match finish_profile(context, setup_finished).await {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        let retry = context
            .store
            .receipt()?
            .as_ref()
            .is_some_and(|receipt| cleanup_may_settle(receipt, setup_finished));
        if !retry || attempt == 3 {
            return Err(error);
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    Err(RUNTIME_ERROR.into())
}

async fn finish_profile(context: &Context, setup_finished: bool) -> Result<(), String> {
    let _cleanup = context.cleanup_lock.lock().await;
    let Some(receipt) = context.store.receipt()? else {
        return Ok(());
    };
    let owner = validate_runtime_receipt(&receipt)?;
    if matches!(receipt.phase, Phase::Creating | Phase::Probing)
        || (owner.phase == RuntimePhase::BearerPending && !setup_finished)
    {
        return Err("mm_ims_profile_runtime_mutation_unresolved".into());
    }
    if PENDING.load(Ordering::Acquire) != 0 {
        return Err(RUNTIME_ERROR.into());
    }
    // Pin/check identity BEFORE the recovery helper (which intentionally may
    // retire abandoned bearer receipts after an MM owner change).
    let io = identity_io(&receipt).await?;
    let allowed = owner.bearer.as_ref().map(|r| r.bearer.as_str());
    if io
        .bus
        .bearers()
        .await?
        .iter()
        .any(|path| Some(path.as_str()) != allowed)
    {
        return Err("mm_ims_profile_runtime_competing_bearer".into());
    }
    let network = owner.bearer.clone();
    cleanup_before_profile(
        || async {
            io.bus.ensure_sim_binding().await?;
            recover_owned().await?;
            io.bus.ensure_sim_binding().await
        },
        || async {
            no_bearer_work()?;
            if let Some(record) = &network {
                verify_retired_network_readonly(record).await?;
            }
            Ok(())
        },
        || async {
            if receipt.before.modem != io.bus.modem {
                reconcile_profile_modem(&io, &context.store, receipt).await?;
            }
            // Durable proof boundary: there are no remaining bearer mutations
            // or network resources before profile reporting/Delete can start.
            context.store.update(|_, owner| {
                owner.phase = RuntimePhase::Cleaning;
                owner.abandoned = true;
            })?;
            let receipt = context.store.receipt()?.ok_or(RUNTIME_ERROR)?;
            release_with(&io, &context.store, receipt).await
        },
    )
    .await?;
    no_bearer_work()?;
    if read_receipt(&context.store.disk.file)?.is_some() {
        return Err(RUNTIME_ERROR.into());
    }
    Ok(())
}

async fn cleanup_before_profile<B, BF, N, NF, P, PF>(
    bearer: B,
    absence: N,
    profile: P,
) -> Result<(), String>
where
    B: FnOnce() -> BF,
    BF: Future<Output = Result<(), String>>,
    N: FnOnce() -> NF,
    NF: Future<Output = Result<(), String>>,
    P: FnOnce() -> PF,
    PF: Future<Output = Result<(), String>>,
{
    bearer().await?;
    absence().await?;
    profile().await
}

fn prior_boot_receipt(receipt: Option<&Receipt>, boot: &str) -> Result<bool, String> {
    let Some(receipt) = receipt else {
        return Ok(false);
    };
    if receipt.version != 2 {
        return Ok(false); // No change to the explicit legacy recovery contract.
    }
    let owner = validate_runtime_receipt(receipt)?;
    if !valid_boot_id(boot) {
        return Err(RUNTIME_ERROR.into());
    }
    Ok(owner.boot_id != boot)
}

/// This hint performs no modem or filesystem mutation. Corrupt/ambiguous
/// metadata after a failed startup proof must not authorize new namespaces.
pub(crate) fn requires_pre_namespace_recovery() -> bool {
    (|| {
        let file = ledger_file(PRIMARY)?;
        reject_other_receipts(&file)?;
        prior_boot_receipt(read_receipt(&file)?.as_ref(), &boot_id()?)
    })()
    .unwrap_or(true)
}

/// Resolve the current modem only for metadata-only prior-boot absence proof.
/// Never use this new owner to delete/disconnect an old owner's resource.
async fn reboot_absence_io(receipt: &Receipt) -> Result<MmProfileIo, String> {
    require_stopped()?;
    no_bearer_work()?;
    let old = &receipt.before;
    let interface = netdev::primary_netdev_for_qmi(&old.device).ok_or(RUNTIME_ERROR)?;
    netdev::verify_mm_data_interface(&old.device, &interface)?;
    let discovery = MmBus::new(&old.device, &old.modem, &interface).await?;
    let objects: ManagedObjects = timed(10, async {
        discovery
            .proxy(
                "/org/freedesktop/ModemManager1",
                "org.freedesktop.DBus.ObjectManager",
            )
            .await?
            .call("GetManagedObjects", &())
            .await
            .map_err(bus_error)
    })
    .await?;
    let mut modems = objects.iter().filter_map(|(path, interfaces)| {
        let properties = interfaces.get(MODEM)?;
        let port = properties
            .get("PrimaryPort")
            .and_then(|v| <&str>::try_from(v).ok())?;
        (Some(port) == old.device.strip_prefix("/dev/")).then(|| path.to_string())
    });
    let modem = modems.next().ok_or(RUNTIME_ERROR)?;
    if modems.next().is_some() || !discovery.owner_is_current().await? {
        return Err(RUNTIME_ERROR.into());
    }
    let bus = MmBus::new(&old.device, &modem, &interface).await?;
    if bus.bus_id != discovery.bus_id || bus.owner != discovery.owner {
        return Err(RUNTIME_ERROR.into());
    }
    bus.pin_sim_binding().await?;
    Ok(MmProfileIo {
        bus,
        method: CreationMethod::At,
        topology: physical_control_topology,
    })
}

/// Startup/new-attempt recovery. Same-boot cleanup retains the original-owner
/// contract. A prior-boot record can ONLY be archived after strict same-SIM,
/// topology, profile/reporting, bearer, namespace and network absence proofs.
/// Legacy, unknown, mid-AT and still-present cross-owner resources remain blocked.
/// Callers re-run prepare afterwards; recovery does not narrow family fallback.
pub async fn recover(device: &str) -> Result<(), String> {
    let device = device.trim().to_string();
    let (sender, receiver) = oneshot::channel();
    tokio::spawn(async move {
        let _ = sender.send(recover_inner(&device).await);
    });
    receiver.await.map_err(|_| RUNTIME_ERROR.to_string())?
}

async fn recover_inner(device: &str) -> Result<(), String> {
    let file = ledger_file(device)?;
    reject_other_receipts(&file)?;
    let lock = device_lock(&file)?;
    let Some(receipt) = read_receipt(&file)? else {
        // Do not let admission's no-bearer guard bypass legacy recovery that
        // normally happens at PrimaryImsSession startup. The established helper
        // only reaps abandoned/dead leases using its original-owner rules.
        return recover_owned().await;
    };
    if device != PRIMARY
        || receipt.before.device != device
        || receipt.version != 2
        || receipt.method != CreationMethod::At
    {
        return Err(RUNTIME_ERROR.into());
    }
    let owner = validate_runtime_receipt(&receipt)?.clone();
    let boot = boot_id()?;
    if owner.boot_id != boot {
        let io = reboot_absence_io(&receipt).await?;
        return retirement::retire_reboot_absence(&io, &DiskStore { file }, &receipt).await;
    }
    recovery_admitted(
        &receipt,
        &boot,
        process_start(owner.process_id)? == Some(owner.process_start),
    )?;
    // Discovery is read-only and bound to the same unique owner, stable SIM
    // and physical control topology before abandoned bearer cleanup is allowed.
    // Only Modem.Bearers count here (not the separate Initial EPS object).
    let io = identity_io(&receipt).await?;
    let context = Context {
        bus: io.bus,
        apn: receipt.apn.clone(),
        sim: (String::new(), 0),
        current: Arc::new(|| false),
        store: RuntimeStore::new(file, owner, Some(receipt)),
        attempt: Arc::default(),
        cleanup_lock: tokio::sync::Mutex::new(()),
        _lock: lock,
    };
    finish_profile(&context, true).await
}

#[cfg(test)]
#[path = "primary_ims_profile_runtime_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "primary_ims_switch_drain_tests.rs"]
mod switch_drain_tests;

#[cfg(test)]
#[path = "primary_ims_profile_runtime_dbus_tests.rs"]
mod dbus_tests;
