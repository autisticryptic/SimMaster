//! Explicit, bounded cross-owner profile reconciliation; never ordinary recovery.
//!
//! The caller must hold the device/global exclusion for this entire future and
//! preserve the original receipt until `retire` succeeds. IO must use a pinned
//! current owner/SIM, not reusable modem/profile selectors. No old bearer RPCs.
use super::*;

pub(super) const RECONCILE_UNVERIFIED: &str = "mm_ims_profile_reconcile_unverified";
pub(super) const RECONCILE_SOURCE_CHANGED: &str = "mm_ims_profile_reconcile_source_changed";
pub(super) const RECONCILE_JOURNAL_INVALID: &str = "mm_ims_profile_reconcile_journal_invalid";
pub(super) const RECONCILE_BINDING_CHANGED: &str = "mm_ims_profile_reconcile_binding_changed";
pub(super) const RECONCILE_OBSERVATION_CHANGED: &str = "mm_ims_profile_reconcile_observation_changed";
pub(super) const RECONCILE_MANUAL_REQUIRED: &str = "mm_ims_profile_reconcile_manual_required";
pub(super) const RECONCILE_STORE_FAILED: &str = "mm_ims_profile_reconcile_store_failed";
pub(super) const RECONCILE_OBSERVATION_FAILED: &str = "mm_ims_profile_reconcile_observation_failed";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Observation {
    pub(super) snapshot: Snapshot,
    pub(super) boot: String,
    pub(super) activity: BTreeMap<i32, bool>,
}

pub(super) trait ReconcileIo {
    /// Each observation must enforce: no competing process/worker/bearer or
    /// calls; original owner absent; original topology; no namespace, XFRM or
    /// network leftovers; pinned current owner+SIM before AND after reads;
    /// original process dead, or this same main process explicitly abandoned.
    /// Parse CGACT strictly (duplicates/unknown rows are errors). Do not infer
    /// inactivity from a missing row. All operations must have bounded timeouts.
    fn observe(&self, receipt: &Receipt)
        -> impl Future<Output = Result<Observation, String>> + Send;
    /// Recheck pinned identity and global exclusion immediately at dispatch.
    /// Exactly one command, no transport-level write retries, even on timeout.
    fn restore_reporting(&self, id: i32, flags: [u8; 3])
        -> impl Future<Output = Result<(), String>> + Send;
    fn delete(&self, id: i32) -> impl Future<Output = Result<(), String>> + Send;
}

pub(super) trait ReconcileStore {
    /// Read exact original bytes under exclusion, with private-file validation
    /// and a size bound. This source must never be rebound or rewritten.
    fn source(&self) -> Result<Vec<u8>, String>;
    /// Use a private recovery subdirectory (not a top-level .json receipt),
    /// keyed by original source fingerprint. Missing ONLY means never prepared;
    /// corrupt/unreadable/partial journals must error, never return None.
    /// Lookup MUST also reject any existing journal for this active source path
    /// with a different fingerprint (scan/index the recovery directory). A changed
    /// source must not select a new empty journal and reset the one-shot budget.
    fn load(&self, source_fingerprint: &str) -> Result<Option<Journal>, String>;
    /// Durable atomic replace + file and directory fsync BEFORE returning Ok.
    /// Preserve/poison uncertain writes; never roll back or remove a journal.
    fn save(&self, journal: &Journal) -> Result<(), String>;
    /// Compare exact source again and archive it using retirement::archive_source.
    /// Only success unlocks a new allocation; retain journal evidence forever.
    fn retire(&self, source: &[u8]) -> Result<(), String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Step {
    Prepared,
    ReportingDispatched,
    ReportingConfirmed,
    DeleteDispatched,
    AbsentVerified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Journal {
    pub(super) version: u8,
    pub(super) source_fingerprint: String,
    /// Immutable initial double proof, including the full activity inventory.
    pub(super) current: Observation,
    pub(super) step: Step,
}

fn digest_valid(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn snapshot_valid(s: &Snapshot) -> bool {
    s.bus_id.len() == 32
        && s.bus_id.bytes().all(|b| b.is_ascii_hexdigit())
        && s.owner.starts_with(':') && s.owner.len() > 1
        && !s.owner.bytes().any(|b| b.is_ascii_whitespace())
        && canonical_modem(&s.modem).is_ok_and(|path| path == s.modem)
        && s.device == PRIMARY
        && digest_valid(&s.sim_fingerprint)
        && s.stable_sim_fingerprint.as_deref().is_some_and(digest_valid)
        && s.control_topology.as_deref().is_some_and(digest_valid)
        && digest_valid(&s.eps_fingerprint)
        && s.profiles.keys().eq(s.definitions.keys())
        && s.profiles.iter().all(|(id, p)| {
            (1..=16).contains(id) && p.id == *id && digest_valid(&p.fingerprint)
                && matches!(p.family, 1 | 2 | 4)
                && s.definitions.get(id).is_some_and(|d| {
                    d.apn == p.apn && d.family == p.family && digest_valid(&d.fingerprint)
                })
        })
        && s.reporting.keys().copied().eq(1..=16)
        && s.reporting.values().all(|flags| flags.iter().all(|v| *v <= 1))
}

fn validate_source(receipt: &Receipt) -> Result<i32, String> {
    let owner = validate_runtime_receipt(receipt).map_err(|_| RECONCILE_UNVERIFIED)?;
    let p = receipt.owned.as_ref().ok_or(RECONCILE_UNVERIFIED)?;
    let d = receipt.owned_definition.as_ref().ok_or(RECONCILE_UNVERIFIED)?;
    if !matches!(receipt.phase, Phase::Owned | Phase::Probed)
        || owner.phase == RuntimePhase::BearerPending
        || !snapshot_valid(&receipt.before) || receipt.tag.trim().is_empty()
        || !(2..=16).contains(&p.id) || p.apn != receipt.apn
        || p.family != receipt.requested_family || !digest_valid(&p.fingerprint)
        || d.apn != p.apn || d.family != p.family || !digest_valid(&d.fingerprint)
        || receipt.before.profiles.contains_key(&p.id)
        || receipt.before.definitions.contains_key(&p.id)
        || receipt.before.reporting.get(&p.id) != Some(&[0, 0, 0])
        || (matches!(owner.phase, RuntimePhase::Active | RuntimePhase::Cleaning)
            && owner.bearer.is_none())
        || owner.bearer.as_ref().is_some_and(|b| {
            b.modem != receipt.before.modem
                || (matches!(owner.phase, RuntimePhase::Active | RuntimePhase::Cleaning)
                    && b.network.is_none())
        })
    {
        return Err(RECONCILE_UNVERIFIED.into());
    }
    Ok(p.id)
}

/// Compare inventories without ever rebinding the source ownership record.
fn validate_observation(receipt: &Receipt, o: &Observation, id: i32) -> Result<bool, String> {
    let old = &receipt.before;
    let s = &o.snapshot;
    if !snapshot_valid(s) || !valid_boot_id(&o.boot)
        || (s.bus_id == old.bus_id && s.owner == old.owner)
        || s.device != old.device || s.control_topology != old.control_topology
        || s.eps_fingerprint != old.eps_fingerprint
        || o.activity.keys().any(|id| !(1..=16).contains(id))
    {
        return Err(RECONCILE_UNVERIFIED.into());
    }
    let mut profiles = s.profiles.clone();
    let mut definitions = s.definitions.clone();
    let p = profiles.remove(&id);
    let d = definitions.remove(&id);
    let mut reporting = s.reporting.clone();
    reporting.insert(id, [0, 0, 0]);
    if profiles != old.profiles || definitions != old.definitions || reporting != old.reporting {
        return Err(RECONCILE_UNVERIFIED.into());
    }
    match (p, d) {
        (None, None) if s.reporting.get(&id) == Some(&[0, 0, 0])
            && o.activity.get(&id) != Some(&true) => Ok(false),
        (Some(p), Some(d)) if Some(&p) == receipt.owned.as_ref()
            && Some(&d) == receipt.owned_definition.as_ref()
            && o.activity.get(&id) == Some(&false) => Ok(true),
        _ => Err(RECONCILE_UNVERIFIED.into()),
    }
}

fn validate_initial(receipt: &Receipt, o: &Observation, id: i32) -> Result<bool, String> {
    let present = validate_observation(receipt, o, id)?;
    if present && o.snapshot.reporting.get(&id) != Some(&[1, 1, 1])
        && !(receipt.phase == Phase::Owned
            && o.snapshot.reporting.get(&id) == Some(&[0, 0, 0]))
    {
        return Err(RECONCILE_UNVERIFIED.into());
    }
    Ok(present)
}

fn read_source<S: ReconcileStore>(store: &S, receipt: &Receipt) -> Result<Vec<u8>, String> {
    let bytes = store.source().map_err(|_| RECONCILE_SOURCE_CHANGED)?;
    if bytes.len() > MAX_RUNTIME_RECORD_BYTES {
        return Err(RECONCILE_SOURCE_CHANGED.into());
    }
    let decoded: Receipt = serde_json::from_slice(&bytes).map_err(|_| RECONCILE_SOURCE_CHANGED)?;
    if fingerprint(&decoded).map_err(|_| RECONCILE_SOURCE_CHANGED)?
        != fingerprint(receipt).map_err(|_| RECONCILE_SOURCE_CHANGED)?
    {
        return Err(RECONCILE_SOURCE_CHANGED.into());
    }
    Ok(bytes)
}

fn unchanged_source<S: ReconcileStore>(store: &S, source: &[u8]) -> Result<(), String> {
    if store.source().map_err(|_| RECONCILE_SOURCE_CHANGED)? != source {
        return Err(RECONCILE_SOURCE_CHANGED.into());
    }
    Ok(())
}

fn checkpoint<S: ReconcileStore>(store: &S, source: &[u8], j: &mut Journal, step: Step) -> Result<(), String> {
    unchanged_source(store, source)?;
    j.step = step;
    store.save(j).map_err(|_| RECONCILE_STORE_FAILED.into())
}

async fn double_observe<I: ReconcileIo>(io: &I, receipt: &Receipt) -> Result<Observation, String> {
    let first = io.observe(receipt).await.map_err(|_| RECONCILE_OBSERVATION_FAILED)?;
    let second = io.observe(receipt).await.map_err(|_| RECONCILE_OBSERVATION_FAILED)?;
    if first != second {
        return Err(RECONCILE_OBSERVATION_CHANGED.into());
    }
    Ok(second)
}

fn expected(j: &Journal, o: &Observation, id: i32, absent: bool, restored: bool) -> Result<(), String> {
    let mut wanted = j.current.clone();
    if !same_binding(&wanted.snapshot, &o.snapshot) || wanted.boot != o.boot {
        return Err(RECONCILE_BINDING_CHANGED.into());
    }
    if restored { wanted.snapshot.reporting.insert(id, [0, 0, 0]); }
    if absent {
        wanted.snapshot.profiles.remove(&id);
        wanted.snapshot.definitions.remove(&id);
        // Firmware may omit an absent CID from CGACT; all other activity is pinned.
        if o.activity.get(&id).is_none() { wanted.activity.remove(&id); }
    }
    if wanted != *o { return Err(RECONCILE_MANUAL_REQUIRED.into()); }
    Ok(())
}

/// The AT profile has no unique owner tag: identical fingerprints cannot rule
/// out deletion/recreation by another writer. Present resources therefore need
/// an explicit plan token; absence can be retired automatically. A persisted
/// approved transaction may resume readback, never replay a dispatched command.
pub(super) fn plan(source: &[u8], current: &Observation) -> Result<String, String> {
    fingerprint(&("explicit-stale-profile-v1", fingerprint(&source)?, current))
}

pub(super) async fn inspect<I: ReconcileIo, S: ReconcileStore>(
    io: &I, store: &S, receipt: &Receipt,
) -> Result<serde_json::Value, String> {
    let id = validate_source(receipt)?;
    let source = read_source(store, receipt)?;
    let current = double_observe(io, receipt).await?;
    let present = validate_initial(receipt, &current, id)?;
    unchanged_source(store, &source)?;
    Ok(serde_json::json!({"plan": plan(&source, &current)?,
        "profile_present": present, "requires_explicit_approval": present,
        "mutated_modem": false, "profile_id": id}))
}

pub(super) async fn reconcile<I: ReconcileIo, S: ReconcileStore>(
    io: &I, store: &S, receipt: &Receipt, expected_plan: Option<&str>,
) -> Result<(), String> {
    let id = validate_source(receipt)?;
    let source = read_source(store, receipt)?;
    let source_fingerprint = fingerprint(&source).map_err(|_| RECONCILE_SOURCE_CHANGED)?;
    let loaded = store.load(&source_fingerprint).map_err(|_| RECONCILE_JOURNAL_INVALID)?;
    let mut journal = match loaded {
        Some(j) => {
            if j.version != 1 || j.source_fingerprint != source_fingerprint
                || !digest_valid(&j.source_fingerprint)
            { return Err(RECONCILE_JOURNAL_INVALID.into()); }
            let present = validate_initial(receipt, &j.current, id)
                .map_err(|_| RECONCILE_JOURNAL_INVALID)?;
            if (!present && !matches!(j.step, Step::Prepared | Step::AbsentVerified))
                || (j.step == Step::ReportingDispatched
                    && j.current.snapshot.reporting.get(&id) != Some(&[1, 1, 1]))
            { return Err(RECONCILE_JOURNAL_INVALID.into()); }
            j
        }
        None => {
            let current = double_observe(io, receipt).await?;
            if validate_initial(receipt, &current, id)?
                && expected_plan != Some(plan(&source, &current)?.as_str())
            {
                return Err(RECONCILE_MANUAL_REQUIRED.into());
            }
            let mut j = Journal { version: 1, source_fingerprint, current, step: Step::Prepared };
            checkpoint(store, &source, &mut j, Step::Prepared)?;
            j
        }
    };
    // No retry loops: at most one reporting command and one Delete per source.
    // A dispatched marker is persisted before constructing/polling its IO future.
    if journal.step == Step::Prepared {
        let o = double_observe(io, receipt).await?;
        let present = validate_initial(receipt, &o, id)?;
        expected(&journal, &o, id, false, false)?;
        if !present {
            checkpoint(store, &source, &mut journal, Step::AbsentVerified)?;
        } else if o.snapshot.reporting.get(&id) == Some(&[1, 1, 1]) {
            checkpoint(store, &source, &mut journal, Step::ReportingDispatched)?;
            io.restore_reporting(id, [0, 0, 0]).await.map_err(|_| RECONCILE_MANUAL_REQUIRED)?;
        } else {
            checkpoint(store, &source, &mut journal, Step::ReportingConfirmed)?;
        }
    }
    if journal.step == Step::ReportingDispatched {
        let o = double_observe(io, receipt).await.map_err(|_| RECONCILE_MANUAL_REQUIRED)?;
        validate_observation(receipt, &o, id).map_err(|_| RECONCILE_MANUAL_REQUIRED)?;
        expected(&journal, &o, id, false, true)?;
        checkpoint(store, &source, &mut journal, Step::ReportingConfirmed)?;
    }
    if journal.step == Step::ReportingConfirmed {
        let o = double_observe(io, receipt).await?;
        validate_observation(receipt, &o, id).map_err(|_| RECONCILE_MANUAL_REQUIRED)?;
        expected(&journal, &o, id, false, true)?;
        checkpoint(store, &source, &mut journal, Step::DeleteDispatched)?;
        io.delete(id).await.map_err(|_| RECONCILE_MANUAL_REQUIRED)?;
    }
    if journal.step == Step::DeleteDispatched || journal.step == Step::AbsentVerified {
        let o = double_observe(io, receipt).await.map_err(|_| RECONCILE_MANUAL_REQUIRED)?;
        if validate_observation(receipt, &o, id).map_err(|_| RECONCILE_MANUAL_REQUIRED)? {
            return Err(RECONCILE_MANUAL_REQUIRED.into());
        }
        expected(&journal, &o, id, true, true)?;
        if journal.step != Step::AbsentVerified {
            checkpoint(store, &source, &mut journal, Step::AbsentVerified)?;
        }
        unchanged_source(store, &source)?;
        return store.retire(&source).map_err(|_| RECONCILE_STORE_FAILED.into());
    }
    Err(RECONCILE_MANUAL_REQUIRED.into())
}

#[cfg(test)]
#[path = "primary_ims_profile_reconciliation_tests.rs"]
mod tests;
