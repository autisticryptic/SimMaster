use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

fn hash(s: &str) -> String { fingerprint(&s).unwrap() }

fn fixture() -> (Receipt, Observation) {
    let baseline_profile = Profile {
        id: 1, apn: "internet".into(), family: 4, name: String::new(),
        fingerprint: hash("eps-profile"),
    };
    let baseline_definition = Definition {
        apn: "internet".into(), family: 4, fingerprint: hash("eps-definition"),
    };
    let before = Snapshot {
        bus_id: "a".repeat(32), owner: ":1.25".into(),
        modem: format!("{MODEM_PREFIX}77"), device: PRIMARY.into(),
        sim_fingerprint: hash("old-sim-path"),
        stable_sim_fingerprint: Some(hash("old-card")),
        control_topology: Some(hash("physical-port")), eps_fingerprint: hash("eps"),
        profiles: BTreeMap::from([(1, baseline_profile)]),
        definitions: BTreeMap::from([(1, baseline_definition)]),
        reporting: (1..=16).map(|id| (id, [0, 0, 0])).collect(),
    };
    let boot = "12345678-1234-1234-1234-123456789abc".to_string();
    let receipt = Receipt {
        version: 2, method: CreationMethod::At, phase: Phase::Probed,
        before: before.clone(), requested_family: 1, apn: "ims".into(),
        tag: "sa1234567890abcd".into(),
        owned: Some(Profile {
            id: 4, apn: "ims".into(), family: 1, name: String::new(),
            fingerprint: hash("owned-profile"),
        }),
        owned_definition: Some(Definition {
            apn: "ims".into(), family: 1, fingerprint: hash("owned-definition"),
        }),
        runtime: Some(RuntimeOwnership {
            version: 1, line_hash: hash("line"), generation: 1,
            process_id: 119010, process_start: 555, boot_id: boot.clone(),
            phase: RuntimePhase::Profile, abandoned: true, bearer: None,
        }),
    };
    let mut snapshot = Snapshot {
        owner: ":1.99".into(), modem: format!("{MODEM_PREFIX}0"),
        sim_fingerprint: hash("new-sim-path"),
        stable_sim_fingerprint: Some(hash("new-card")), ..before
    };
    snapshot.profiles.insert(4, receipt.owned.clone().unwrap());
    snapshot.definitions.insert(4, receipt.owned_definition.clone().unwrap());
    snapshot.reporting.insert(4, [1, 1, 1]);
    (receipt, Observation { snapshot, boot, activity: BTreeMap::from([(1, false), (4, false)]) })
}

struct FakeIo {
    state: Mutex<Observation>,
    restores: AtomicUsize,
    deletes: AtomicUsize,
    observations: AtomicUsize,
    drift_at: AtomicUsize,
    behavior: AtomicUsize,
}
impl FakeIo {
    fn new(o: Observation) -> Self {
        Self { state: Mutex::new(o), restores: AtomicUsize::new(0), deletes: AtomicUsize::new(0),
            observations: AtomicUsize::new(0), drift_at: AtomicUsize::new(0), behavior: AtomicUsize::new(0) }
    }
    fn counts(&self) -> (usize, usize) {
        (self.restores.load(Ordering::SeqCst), self.deletes.load(Ordering::SeqCst))
    }
    fn absent(&self) {
        let mut o = self.state.lock().unwrap();
        o.snapshot.profiles.remove(&4);
        o.snapshot.definitions.remove(&4);
        o.snapshot.reporting.insert(4, [0, 0, 0]);
        o.activity.remove(&4);
    }
}
impl ReconcileIo for FakeIo {
    async fn observe(&self, _: &Receipt) -> Result<Observation, String> {
        let count = self.observations.fetch_add(1, Ordering::SeqCst) + 1;
        let mut o = self.state.lock().unwrap().clone();
        if self.drift_at.load(Ordering::SeqCst) == count {
            o.activity.insert(2, false);
        }
        Ok(o)
    }
    async fn restore_reporting(&self, id: i32, flags: [u8; 3]) -> Result<(), String> {
        assert_eq!((id, flags), (4, [0, 0, 0]));
        self.restores.fetch_add(1, Ordering::SeqCst);
        let behavior = self.behavior.load(Ordering::SeqCst);
        if behavior == 1 { return Err("timeout-before-effect".into()); }
        if behavior == 3 { std::future::pending::<()>().await; }
        self.state.lock().unwrap().snapshot.reporting.insert(id, flags);
        if behavior == 2 { return Err("reply-lost".into()); }
        Ok(())
    }
    async fn delete(&self, id: i32) -> Result<(), String> {
        assert_eq!(id, 4);
        self.deletes.fetch_add(1, Ordering::SeqCst);
        let behavior = self.behavior.load(Ordering::SeqCst);
        if behavior == 4 { return Err("timeout-before-effect".into()); }
        if behavior == 6 { std::future::pending::<()>().await; }
        self.absent();
        if behavior == 5 { return Err("reply-lost".into()); }
        Ok(())
    }
}

struct FakeStore {
    source: Mutex<Vec<u8>>,
    journal: Mutex<Option<Journal>>,
    saves: AtomicUsize,
    fail_at: AtomicUsize,
    retired: AtomicUsize,
    fail_retire: AtomicUsize,
    persist_failed_save: AtomicUsize,
}
impl FakeStore {
    fn new(r: &Receipt) -> Self {
        Self { source: Mutex::new(serde_json::to_vec(r).unwrap()), journal: Mutex::new(None),
            saves: AtomicUsize::new(0), fail_at: AtomicUsize::new(0), retired: AtomicUsize::new(0),
            fail_retire: AtomicUsize::new(0), persist_failed_save: AtomicUsize::new(0) }
    }
    fn step(&self) -> Step { self.journal.lock().unwrap().as_ref().unwrap().step }
}
impl ReconcileStore for FakeStore {
    fn source(&self) -> Result<Vec<u8>, String> { Ok(self.source.lock().unwrap().clone()) }
    fn load(&self, _: &str) -> Result<Option<Journal>, String> { Ok(self.journal.lock().unwrap().clone()) }
    fn save(&self, j: &Journal) -> Result<(), String> {
        let count = self.saves.fetch_add(1, Ordering::SeqCst) + 1;
        let fail = self.fail_at.load(Ordering::SeqCst) == count;
        if !fail || self.persist_failed_save.load(Ordering::SeqCst) != 0 {
            *self.journal.lock().unwrap() = Some(j.clone());
        }
        if fail { return Err("fsync-failed".into()); }
        Ok(())
    }
    fn retire(&self, source: &[u8]) -> Result<(), String> {
        assert_eq!(self.source.lock().unwrap().as_slice(), source);
        assert_eq!(self.step(), Step::AbsentVerified);
        if self.fail_retire.load(Ordering::SeqCst) != 0 { return Err("archive-failed".into()); }
        self.retired.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// The protocol tests below explicitly authorize the exact initial fixture.
// Production recovery passes None and cannot delete an untagged present CID.
async fn reconcile(io: &FakeIo, store: &FakeStore, r: &Receipt) -> Result<(), String> {
    let token = plan(&store.source()?, &io.state.lock().unwrap().clone())?;
    super::reconcile(io, store, r, Some(&token)).await
}

#[tokio::test]
async fn identical_profile_cannot_be_automatically_adopted_without_plan() {
    let (r, o) = fixture();
    let io = FakeIo::new(o);
    let store = FakeStore::new(&r);
    assert_eq!(super::reconcile(&io, &store, &r, None).await.unwrap_err(), RECONCILE_MANUAL_REQUIRED);
    assert_eq!(io.counts(), (0, 0));
    assert_eq!(store.saves.load(Ordering::SeqCst), 0);
    assert!(super::reconcile(&io, &store, &r, Some("wrong-plan")).await.is_err());
    let inspected = inspect(&io, &store, &r).await.unwrap();
    assert_eq!(inspected["requires_explicit_approval"], true);
    assert_eq!(store.saves.load(Ordering::SeqCst), 0);
    super::reconcile(&io, &store, &r, inspected["plan"].as_str()).await.unwrap();
    assert_eq!(io.counts(), (1, 1));
}

#[tokio::test]
async fn absent_profile_auto_retires_without_plan_or_modem_mutation() {
    let (r, o) = fixture();
    let io = FakeIo::new(o);
    io.absent();
    let store = FakeStore::new(&r);
    super::reconcile(&io, &store, &r, None).await.unwrap();
    assert_eq!(io.counts(), (0, 0));
    assert_eq!(store.retired.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn changed_owner_and_sim_exact_owned_profile_restores_and_deletes_once() {
    for prior_boot in [false, true] {
        let (r, mut o) = fixture();
        if prior_boot { o.boot = "abcdefab-1234-1234-1234-123456789abc".into(); }
        let io = FakeIo::new(o);
        let store = FakeStore::new(&r);
        let source = store.source().unwrap();
        reconcile(&io, &store, &r).await.unwrap();
        assert_eq!(io.counts(), (1, 1));
        assert_eq!(store.retired.load(Ordering::SeqCst), 1);
        assert_eq!(store.source().unwrap(), source);
        assert_eq!(store.step(), Step::AbsentVerified);
        // Retained evidence permits metadata-only completion, never commands.
        reconcile(&io, &store, &r).await.unwrap();
        assert_eq!(io.counts(), (1, 1));
    }
}

#[tokio::test]
async fn invalid_current_evidence_has_zero_persistent_or_modem_writes() {
    for case in 0..20 {
        let (r, mut o) = fixture();
        match case {
            0 => o.snapshot.owner = r.before.owner.clone(),
            1 => { o.activity.insert(4, true); }
            2 => { o.activity.remove(&4); }
            3 => o.snapshot.profiles.get_mut(&4).unwrap().fingerprint = hash("reused-id"),
            4 => o.snapshot.definitions.get_mut(&4).unwrap().fingerprint = hash("changed-row"),
            5 => o.snapshot.profiles.get_mut(&1).unwrap().fingerprint = hash("other-inventory"),
            6 => o.snapshot.eps_fingerprint = hash("different-eps"),
            7 => o.snapshot.control_topology = Some(hash("different-port")),
            8 => o.snapshot.stable_sim_fingerprint = Some(String::new()),
            9 => o.snapshot.sim_fingerprint = "malformed".into(),
            10 => { o.snapshot.reporting.insert(4, [0, 0, 0]); }
            11 => { o.snapshot.reporting.insert(4, [1, 0, 0]); }
            12 => { o.snapshot.reporting.insert(1, [1, 1, 1]); }
            13 => { o.snapshot.profiles.remove(&4); }
            14 => { o.snapshot.definitions.remove(&4); }
            15 => o.snapshot.bus_id.clear(),
            16 => o.snapshot.owner.clear(),
            17 => o.snapshot.device.clear(),
            18 => o.snapshot.modem.clear(),
            _ => o.boot.clear(),
        }
        let io = FakeIo::new(o);
        let store = FakeStore::new(&r);
        assert!(reconcile(&io, &store, &r).await.is_err(), "case={case}");
        assert_eq!(io.counts(), (0, 0), "case={case}");
        assert_eq!(store.saves.load(Ordering::SeqCst), 0, "case={case}");
    }
}

#[tokio::test]
async fn invalid_original_receipts_have_zero_writes() {
    for case in 0..18 {
        let (mut r, o) = fixture();
        match case {
            0 => r.version = 1,
            1 => r.method = CreationMethod::Qmi,
            2 => r.phase = Phase::Creating,
            3 => r.phase = Phase::Probing,
            4 => r.phase = Phase::Deleting,
            5 => r.phase = Phase::RestoringReporting,
            6 => r.phase = Phase::Rejected,
            7 => r.runtime.as_mut().unwrap().phase = RuntimePhase::BearerPending,
            8 => r.runtime = None,
            9 => r.owned.as_mut().unwrap().id = 1,
            10 => { r.before.profiles.insert(4, r.owned.clone().unwrap()); }
            11 => r.owned.as_mut().unwrap().fingerprint.clear(),
            12 => r.owned_definition.as_mut().unwrap().fingerprint.clear(),
            13 => r.before.eps_fingerprint.clear(),
            14 => r.before.stable_sim_fingerprint = None,
            15 => r.tag.clear(),
            16 => r.before.owner.clear(),
            _ => r.runtime.as_mut().unwrap().process_start = 0,
        }
        let io = FakeIo::new(o);
        let store = FakeStore::new(&r);
        assert!(reconcile(&io, &store, &r).await.is_err(), "case={case}");
        assert_eq!(io.counts(), (0, 0));
        assert_eq!(store.saves.load(Ordering::SeqCst), 0);
    }
    let (r, _) = fixture();
    let mut value = serde_json::to_value(r).unwrap();
    value["phase"] = serde_json::json!("unknown_phase");
    assert!(serde_json::from_value::<Receipt>(value).is_err());
}

#[tokio::test]
async fn already_absent_is_metadata_only_and_owned_zero_flags_skips_restore() {
    let (r, o) = fixture();
    let io = FakeIo::new(o);
    io.absent();
    let store = FakeStore::new(&r);
    reconcile(&io, &store, &r).await.unwrap();
    assert_eq!(io.counts(), (0, 0));
    assert_eq!(store.retired.load(Ordering::SeqCst), 1);
    let (mut r, mut o) = fixture();
    r.phase = Phase::Owned;
    o.snapshot.reporting.insert(4, [0, 0, 0]);
    let io = FakeIo::new(o);
    reconcile(&io, &FakeStore::new(&r), &r).await.unwrap();
    assert_eq!(io.counts(), (0, 1));
}

#[tokio::test]
async fn first_double_proof_includes_entire_activity_inventory() {
    let (r, o) = fixture();
    let io = FakeIo::new(o);
    io.drift_at.store(2, Ordering::SeqCst);
    let store = FakeStore::new(&r);
    assert_eq!(reconcile(&io, &store, &r).await.unwrap_err(), RECONCILE_OBSERVATION_CHANGED);
    assert_eq!(io.counts(), (0, 0));
    assert_eq!(store.saves.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn timeout_or_lost_reply_never_replays_a_dispatched_command() {
    for behavior in [1, 2, 4, 5] {
        let (r, o) = fixture();
        let io = FakeIo::new(o);
        io.behavior.store(behavior, Ordering::SeqCst);
        let store = FakeStore::new(&r);
        assert_eq!(reconcile(&io, &store, &r).await.unwrap_err(), RECONCILE_MANUAL_REQUIRED);
        let before = io.counts();
        io.behavior.store(0, Ordering::SeqCst);
        let result = reconcile(&io, &store, &r).await;
        if behavior == 2 || behavior == 5 {
            result.unwrap();
            assert_eq!(io.counts(), (1, 1));
        } else {
            assert_eq!(result.unwrap_err(), RECONCILE_MANUAL_REQUIRED);
            assert_eq!(io.counts(), before);
            assert_eq!(store.retired.load(Ordering::SeqCst), 0);
        }
    }
}

#[tokio::test]
async fn cancellation_keeps_intent_and_only_late_exact_readback_can_advance() {
    for behavior in [3, 6] {
        let (r, o) = fixture();
        let io = FakeIo::new(o);
        io.behavior.store(behavior, Ordering::SeqCst);
        let store = FakeStore::new(&r);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(10),
            reconcile(&io, &store, &r)).await.is_err());
        assert_eq!(store.step(), if behavior == 3 { Step::ReportingDispatched } else { Step::DeleteDispatched });
        let before = io.counts();
        io.behavior.store(0, Ordering::SeqCst);
        assert_eq!(reconcile(&io, &store, &r).await.unwrap_err(), RECONCILE_MANUAL_REQUIRED);
        assert_eq!(io.counts(), before);
        if behavior == 3 {
            io.state.lock().unwrap().snapshot.reporting.insert(4, [0, 0, 0]);
        } else { io.absent(); }
        reconcile(&io, &store, &r).await.unwrap();
        assert_eq!(io.counts(), (1, 1));
    }
}

#[tokio::test]
async fn every_failed_checkpoint_stops_io_and_resumes_without_replay() {
    for persist_failed in [false, true] {
        for fail_at in 1..=5 {
            let (r, o) = fixture();
            let io = FakeIo::new(o);
            let store = FakeStore::new(&r);
            store.fail_at.store(fail_at, Ordering::SeqCst);
            store.persist_failed_save.store(persist_failed as usize, Ordering::SeqCst);
            assert_eq!(reconcile(&io, &store, &r).await.unwrap_err(), RECONCILE_STORE_FAILED);
            assert_eq!(io.counts(), match fail_at { 1 | 2 => (0, 0), 3 | 4 => (1, 0), _ => (1, 1) });
            store.fail_at.store(0, Ordering::SeqCst);
            let result = reconcile(&io, &store, &r).await;
            if persist_failed && matches!(fail_at, 2 | 4) {
                // Checkpoint may be durable although command was never polled:
                // safety sacrifices liveness rather than guessing/retrying.
                assert_eq!(result.unwrap_err(), RECONCILE_MANUAL_REQUIRED);
            } else { result.unwrap(); }
            let (restores, deletes) = io.counts();
            assert!(restores <= 1 && deletes <= 1);
        }
    }
}

#[tokio::test]
async fn interrupted_journal_pins_source_owner_boot_sim_and_full_inventory() {
    for case in 0..8 {
        let (r, o) = fixture();
        let io = FakeIo::new(o);
        let store = FakeStore::new(&r);
        io.behavior.store(2, Ordering::SeqCst);
        assert!(reconcile(&io, &store, &r).await.is_err());
        io.behavior.store(0, Ordering::SeqCst);
        match case {
            0 => store.journal.lock().unwrap().as_mut().unwrap().source_fingerprint = hash("other-source"),
            1 => io.state.lock().unwrap().snapshot.owner = ":1.100".into(),
            2 => io.state.lock().unwrap().boot = "abcdefab-1234-1234-1234-123456789abc".into(),
            3 => io.state.lock().unwrap().snapshot.stable_sim_fingerprint = Some(hash("third-card")),
            4 => store.source.lock().unwrap().push(b' '), // even semantically identical bytes changed
            5 => store.journal.lock().unwrap().as_mut().unwrap().version = 99,
            6 => { io.state.lock().unwrap().activity.insert(2, false); }
            _ => store.journal.lock().unwrap().as_mut().unwrap().current.snapshot.eps_fingerprint = hash("forged"),
        }
        assert!(reconcile(&io, &store, &r).await.is_err(), "case={case}");
        assert_eq!(io.counts(), (1, 0));
        assert_eq!(store.retired.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn retirement_failure_keeps_blocking_evidence_and_does_not_repeat_commands() {
    let (r, o) = fixture();
    let io = FakeIo::new(o);
    let store = FakeStore::new(&r);
    store.fail_retire.store(1, Ordering::SeqCst);
    assert_eq!(reconcile(&io, &store, &r).await.unwrap_err(), RECONCILE_STORE_FAILED);
    assert_eq!(store.step(), Step::AbsentVerified);
    assert_eq!(store.retired.load(Ordering::SeqCst), 0);
    store.fail_retire.store(0, Ordering::SeqCst);
    reconcile(&io, &store, &r).await.unwrap();
    assert_eq!(io.counts(), (1, 1));
    assert_eq!(store.retired.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn changed_boot_alone_is_not_an_owner_transition_but_new_bus_is() {
    let (r, mut o) = fixture();
    o.snapshot.owner = r.before.owner.clone();
    o.boot = "abcdefab-1234-1234-1234-123456789abc".into();
    let io = FakeIo::new(o.clone());
    let store = FakeStore::new(&r);
    assert!(reconcile(&io, &store, &r).await.is_err());
    assert_eq!(store.saves.load(Ordering::SeqCst), 0);
    // Unique name strings can be reused on a genuinely different bus.
    o.snapshot.bus_id = "b".repeat(32);
    let io = FakeIo::new(o);
    reconcile(&io, &FakeStore::new(&r), &r).await.unwrap();
    assert_eq!(io.counts(), (1, 1));
}

#[tokio::test]
async fn deletion_requires_double_absence_before_retirement() {
    let (r, o) = fixture();
    let io = FakeIo::new(o);
    let store = FakeStore::new(&r);
    io.drift_at.store(10, Ordering::SeqCst);
    assert_eq!(reconcile(&io, &store, &r).await.unwrap_err(), RECONCILE_MANUAL_REQUIRED);
    assert_eq!(io.counts(), (1, 1));
    assert_eq!(store.step(), Step::DeleteDispatched);
    assert_eq!(store.retired.load(Ordering::SeqCst), 0);
    io.drift_at.store(0, Ordering::SeqCst);
    reconcile(&io, &store, &r).await.unwrap();
    assert_eq!(io.counts(), (1, 1));
}

#[tokio::test]
async fn source_bytes_must_decode_to_the_supplied_original_receipt() {
    let (r, o) = fixture();
    let io = FakeIo::new(o);
    let store = FakeStore::new(&r);
    let mut other = r.clone();
    other.tag = "different-source".into();
    *store.source.lock().unwrap() = serde_json::to_vec(&other).unwrap();
    assert_eq!(reconcile(&io, &store, &r).await.unwrap_err(), RECONCILE_SOURCE_CHANGED);
    assert_eq!(io.counts(), (0, 0));
    assert_eq!(store.saves.load(Ordering::SeqCst), 0);
}

#[test]
fn journal_schema_rejects_unknown_steps_and_fields() {
    let (r, o) = fixture();
    let j = Journal { version: 1, source_fingerprint: hash("source"), current: o, step: Step::Prepared };
    let mut value = serde_json::to_value(&j).unwrap();
    value["step"] = serde_json::json!("retry_delete");
    assert!(serde_json::from_value::<Journal>(value).is_err());
    let mut value = serde_json::to_value(&j).unwrap();
    value["extra"] = serde_json::json!(true);
    assert!(serde_json::from_value::<Journal>(value).is_err());
    assert!(validate_source(&r).is_ok());
}
