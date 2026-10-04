//! Stopped/quiescent capability and private durable store for reconciliation.
//! Never stop a service, remove a namespace, or redirect an old bearer here.
use super::*;
use super::reconciliation::{Journal, Observation, ReconcileIo, ReconcileStore, Step};
use std::io::{Read, Write};

const ERROR: &str = "mm_ims_profile_reconcile_unverified";
const PENDING: &str = "mm_ims_profile_reconcile_pending";

pub(super) fn journal_path(file: &Path) -> PathBuf { file.with_extension("recovery") }

pub(super) fn has_pending(file: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(journal_path(file)) {
        Ok(_) => Ok(true), // damaged/link also blocks ordinary allocation
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(ERROR.into()),
    }
}

pub(super) fn ensure_no_pending(file: &Path) -> Result<(), String> {
    if has_pending(file)? { Err(PENDING.into()) } else { Ok(()) }
}

fn read_private(file: &Path) -> Result<Vec<u8>, String> {
    let f = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(file).map_err(|_| ERROR)?;
    let meta = f.metadata().map_err(|_| ERROR)?;
    if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0 || meta.nlink() != 1
        || meta.len() > MAX_RUNTIME_RECORD_BYTES as u64
    { return Err(ERROR.into()); }
    let mut bytes = Vec::new();
    f.take(MAX_RUNTIME_RECORD_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|_| ERROR)?;
    if bytes.len() > MAX_RUNTIME_RECORD_BYTES { return Err(ERROR.into()); }
    Ok(bytes)
}

fn sync_dir(path: &Path) -> Result<(), String> {
    fs::File::open(path).and_then(|f| f.sync_all()).map_err(|_| ERROR.into())
}

pub(super) struct JournalStore { pub(super) file: PathBuf }
impl JournalStore {
    fn journal(&self) -> Result<Option<Journal>, String> {
        let path = journal_path(&self.file);
        if !has_pending(&self.file)? { return Ok(None); }
        let bytes = read_private(&path)?;
        serde_json::from_slice(&bytes).map(Some).map_err(|_| ERROR.into())
    }

    /// Only clear an orphan active marker if the original source was already
    /// archived durably and the journal has the terminal absence proof.
    pub(super) fn finish_archived(&self) -> Result<(), String> {
        let Some(journal) = self.journal()? else { return Ok(()); };
        if journal.version != 1 || journal.step != Step::AbsentVerified
            || journal.source_fingerprint.len() != 64
            || !journal.source_fingerprint.bytes().all(|b| b.is_ascii_hexdigit())
        { return Err(PENDING.into()); }
        if !matches!(fs::symlink_metadata(&self.file), Err(e) if e.kind() == std::io::ErrorKind::NotFound) {
            return Err(PENDING.into());
        }
        let parent = self.file.parent().ok_or(ERROR)?;
        let retired = parent.join("retired");
        ensure_directory(&retired)?;
        let source = read_private(&retired.join(format!("absent-{}.receipt", journal.source_fingerprint)))?;
        if fingerprint(&source)? != journal.source_fingerprint { return Err(ERROR.into()); }
        let active = journal_path(&self.file);
        let bytes = read_private(&active)?;
        let archived = retired.join(format!("reconciled-{}.journal", journal.source_fingerprint));
        match OpenOptions::new().write(true).create_new(true).mode(0o600)
            .custom_flags(libc::O_NOFOLLOW).open(&archived)
        {
            Ok(mut out) => out.write_all(&bytes).and_then(|_| out.sync_all()).map_err(|_| ERROR)?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if read_private(&archived)? != bytes { return Err(ERROR.into()); }
                fs::File::open(&archived).and_then(|f| f.sync_all()).map_err(|_| ERROR)?;
            }
            Err(_) => return Err(ERROR.into()),
        }
        sync_dir(&retired)?;
        sync_dir(parent)?;
        if read_private(&active)? != bytes { return Err(ERROR.into()); }
        fs::remove_file(&active).map_err(|_| ERROR)?;
        sync_dir(parent)
    }
}
impl ReconcileStore for JournalStore {
    fn source(&self) -> Result<Vec<u8>, String> { read_private(&self.file) }
    fn load(&self, source_fingerprint: &str) -> Result<Option<Journal>, String> {
        // One fixed active marker per endpoint, NOT a new path selected by
        // source content: rewriting a receipt cannot reset the command budget.
        let value = self.journal()?;
        if value.is_none() {
            // Restoring an old receipt from a backup must not manufacture a
            // fresh command budget after its transaction was already retired.
            let retired = self.file.parent().ok_or(ERROR)?.join("retired");
            match fs::symlink_metadata(&retired) {
                Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink()
                    && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o022 == 0 => {},
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                _ => return Err(PENDING.into()),
            }
            for name in [format!("absent-{source_fingerprint}.receipt"),
                         format!("reconciled-{source_fingerprint}.journal")] {
                match fs::symlink_metadata(retired.join(name)) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                    _ => return Err(PENDING.into()),
                }
            }
        }
        if value.as_ref().is_some_and(|j| j.source_fingerprint != source_fingerprint) {
            return Err(ERROR.into());
        }
        Ok(value)
    }
    fn save(&self, journal: &Journal) -> Result<(), String> {
        if fingerprint(&self.source()?)? != journal.source_fingerprint { return Err(ERROR.into()); }
        let path = journal_path(&self.file);
        self.load(&journal.source_fingerprint)?;
        write_record(&path, journal)?;
        sync_dir(path.parent().ok_or(ERROR)?)
    }
    fn retire(&self, source: &[u8]) -> Result<(), String> {
        let journal = self.journal()?.ok_or(ERROR)?;
        if journal.step != Step::AbsentVerified || journal.source_fingerprint != fingerprint(&source)? {
            return Err(ERROR.into());
        }
        retirement::archive_source(&self.file, source)?;
        self.finish_archived()
    }
}

pub(super) fn parse_activity(text: &str) -> Result<BTreeMap<i32, bool>, String> {
    if text.len() > 16384 { return Err(ERROR.into()); }
    let mut values = BTreeMap::new();
    for line in text.lines().map(str::trim).filter(|s| !s.is_empty()) {
        if matches!(line, "OK" | "AT+CGACT?") { continue; }
        let (id, active) = line.strip_prefix("+CGACT:").and_then(|s| s.split_once(',')).ok_or(ERROR)?;
        let id: i32 = id.trim().parse().map_err(|_| ERROR)?;
        let active = match active.trim() { "0" => false, "1" => true, _ => return Err(ERROR.into()) };
        if !(1..=16).contains(&id) || values.insert(id, active).is_some() { return Err(ERROR.into()); }
    }
    if values.is_empty() { return Err(ERROR.into()); }
    Ok(values)
}

pub(super) struct CurrentIo { io: MmProfileIo, receipt: Receipt }
impl CurrentIo {
    async fn guard(&self, receipt: &Receipt) -> Result<(), String> {
        require_stopped()?; // excludes this caller; rejects every other manager/worker
        no_bearer_work()?;
        if live_contexts().lock().unwrap().iter().any(|c| c.strong_count() != 0) {
            return Err(PENDING.into());
        }
        let owner = validate_runtime_receipt(receipt)?;
        let current_boot = boot_id()?;
        if current_boot == owner.boot_id && process_start(owner.process_id)? == Some(owner.process_start)
            && (owner.process_id != std::process::id() || !owner.abandoned)
        { return Err(PENDING.into()); }
        retirement::old_owner_absent(&self.io, receipt).await?;
        self.io.bus.ensure_sim_binding().await?;
        if !self.io.bus.bearers().await?.is_empty() { return Err(PENDING.into()); }
        let objects: ManagedObjects = timed(5, async {
            self.io.bus.proxy("/org/freedesktop/ModemManager1", "org.freedesktop.DBus.ObjectManager")
                .await?.call("GetManagedObjects", &()).await.map_err(bus_error)
        }).await?;
        if objects.values().filter(|interfaces| interfaces.contains_key(MODEM)).count() != 1 {
            return Err(PENDING.into());
        }
        let calls: Vec<OwnedObjectPath> = timed(5, async {
            self.io.bus.proxy(&self.io.bus.modem, "org.freedesktop.ModemManager1.Modem.Voice")
                .await?.call("ListCalls", &()).await.map_err(bus_error)
        }).await?;
        if !calls.is_empty() { return Err(PENDING.into()); }
        if let Some(record) = &owner.bearer { verify_retired_network_readonly(record).await?; }
        // A still-named namespace, even empty, is not ours to sweep here. Also
        // exclude detached namespaces and their XFRM state by requiring every
        // process to share our root network namespace. This deliberately rejects
        // containers/other tenants rather than guessing which resources are safe.
        let root = fs::metadata("/proc/self/ns/net").map_err(|_| ERROR)?.ino();
        let host = fs::metadata("/proc/1/ns/net").map_err(|_| ERROR)?.ino();
        if root != host { return Err(PENDING.into()); }
        match fs::read_dir("/run/netns") {
            Ok(mut entries) => if entries.next().is_some() { return Err(PENDING.into()); },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(_) => return Err(ERROR.into()),
        }
        for entry in fs::read_dir("/proc").map_err(|_| ERROR)? {
            let entry = entry.map_err(|_| ERROR)?;
            if entry.file_name().to_str().is_none_or(|n| n.parse::<u32>().is_err()) { continue; }
            match fs::metadata(entry.path().join("ns/net")) {
                Ok(meta) if meta.ino() == root => {},
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                _ => return Err(PENDING.into()),
            }
        }
        for kind in ["state", "policy"] {
            let output = tokio::time::timeout(Duration::from_secs(5),
                tokio::process::Command::new("ip").args(["xfrm", kind, "list"]).output())
                .await.map_err(|_| ERROR)?.map_err(|_| ERROR)?;
            if !output.status.success() || !output.stdout.is_empty() || !output.stderr.is_empty() {
                return Err(PENDING.into());
            }
        }
        self.io.bus.ensure_sim_binding().await?;
        retirement::old_owner_absent(&self.io, receipt).await
    }
}
impl ReconcileIo for CurrentIo {
    async fn observe(&self, receipt: &Receipt) -> Result<Observation, String> {
        self.guard(receipt).await?;
        let boot = boot_id()?;
        let snapshot = self.io.snapshot().await?;
        let activity = parse_activity(&self.io.command("AT+CGACT?").await?)?;
        self.guard(receipt).await?;
        if boot_id()? != boot || self.io.snapshot().await? != snapshot { return Err(ERROR.into()); }
        Ok(Observation { snapshot, boot, activity })
    }
    async fn restore_reporting(&self, id: i32, flags: [u8; 3]) -> Result<(), String> {
        // MmProfileIo::command rechecks its pinned owner/SIM inside the serial
        // permit; the transaction has just performed the global proof twice.
        self.guard(&self.receipt).await?;
        if parse_activity(&self.io.command("AT+CGACT?").await?)?.get(&id) != Some(&false) {
            return Err(PENDING.into());
        }
        self.io.restore_reporting(id, flags).await
    }
    async fn delete(&self, id: i32) -> Result<(), String> {
        self.guard(&self.receipt).await?;
        if parse_activity(&self.io.command("AT+CGACT?").await?)?.get(&id) != Some(&false) {
            return Err(PENDING.into());
        }
        self.io.delete(id).await
    }
}

pub(super) async fn run(
    io: MmProfileIo, file: PathBuf, receipt: &Receipt, inspect: bool, expected: Option<&str>,
) -> Result<serde_json::Value, String> {
    let io = CurrentIo { io, receipt: receipt.clone() };
    let store = JournalStore { file };
    if inspect { return reconciliation::inspect(&io, &store, receipt).await; }
    reconciliation::reconcile(&io, &store, receipt, expected).await?;
    Ok(serde_json::json!({"reconciled": true, "original_receipt_archived": true,
        "bounded_commands": true, "original_binding_rewritten": false}))
}

#[cfg(test)]
#[path = "primary_ims_profile_reconciliation_io_tests.rs"]
mod tests;
