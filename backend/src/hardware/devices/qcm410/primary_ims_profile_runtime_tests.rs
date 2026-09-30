use super::*;

fn ownership() -> RuntimeOwnership {
    RuntimeOwnership {
        version: 1,
        line_hash: "a".repeat(64),
        generation: 41,
        process_id: 123,
        process_start: 456,
        boot_id: "12345678-1234-1234-1234-123456789abc".into(),
        phase: RuntimePhase::Profile,
        abandoned: false,
        bearer: None,
    }
}

fn baseline() -> Snapshot {
    Snapshot {
        bus_id: "a".repeat(32),
        owner: ":1.42".into(),
        modem: format!("{MODEM_PREFIX}0"),
        device: PRIMARY.into(),
        sim_fingerprint: "object-and-sim".into(),
        stable_sim_fingerprint: Some("stable-card-slot".into()),
        control_topology: Some("physical-control".into()),
        eps_fingerprint: "original-eps".into(),
        profiles: BTreeMap::from([(
            1,
            Profile {
                id: 1,
                apn: "internet".into(),
                family: 4,
                name: String::new(),
                fingerprint: "original-profile".into(),
            },
        )]),
        definitions: BTreeMap::from([(
            1,
            Definition {
                family: 4,
                apn: "internet".into(),
                fingerprint: "original-definition".into(),
            },
        )]),
        reporting: (1..=16).map(|id| (id, [0, 0, 0])).collect(),
    }
}

fn runtime_receipt() -> Receipt {
    Receipt {
        version: 2,
        method: CreationMethod::At,
        phase: Phase::Owned,
        before: baseline(),
        requested_family: 4,
        apn: "ims".into(),
        tag: "sa-test".into(),
        owned: None,
        owned_definition: None,
        runtime: Some(ownership()),
    }
}

#[test]
fn runtime_profile_phase_ownership_blocks_live_unknown_and_rebooted_receipts() {
    let receipt = runtime_receipt();
    let boot = ownership().boot_id;
    assert!(recovery_admitted(&receipt, &boot, false).is_ok());
    assert!(recovery_admitted(&receipt, &boot, true).is_err());
    assert!(recovery_admitted(&receipt, "other-boot", false).is_err());
    for phase in [Phase::Creating, Phase::Probing] {
        let mut receipt = receipt.clone();
        receipt.phase = phase;
        assert!(recovery_admitted(&receipt, &boot, false).is_err());
    }
    for change in 0..8 {
        let mut receipt = receipt.clone();
        match change {
            0 => receipt.runtime = None,
            1 => receipt.version = 1,
            2 => receipt.before.stable_sim_fingerprint = None,
            3 => receipt.before.control_topology = None,
            4 => receipt.runtime.as_mut().unwrap().phase = RuntimePhase::BearerPending,
            5 => receipt.runtime.as_mut().unwrap().phase = RuntimePhase::Active,
            6 => receipt.runtime.as_mut().unwrap().process_start = 0,
            _ => receipt.method = CreationMethod::Qmi,
        }
        assert!(
            recovery_admitted(&receipt, &boot, false).is_err(),
            "change {change}"
        );
    }
    let encoded = serde_json::to_string(&receipt).unwrap();
    assert!(encoded.contains("line_hash"));
    assert!(!encoded.contains("iccid"));
    assert!(!encoded.contains("line_id"));
    let mut unknown = serde_json::to_value(&receipt).unwrap();
    unknown["runtime"]["unrecognized_owner"] = true.into();
    assert!(serde_json::from_value::<Receipt>(unknown).is_err());
}

#[test]
fn runtime_profile_canonical_modem_and_family_contract() {
    assert_eq!(
        canonical_modem(" 0005 ").unwrap(),
        format!("{MODEM_PREFIX}5")
    );
    assert_eq!(
        canonical_modem(&format!("{MODEM_PREFIX}5")).unwrap(),
        format!("{MODEM_PREFIX}5")
    );
    for invalid in ["", "-1", "/Modem/5", "5/", "4294967296", "5x"] {
        assert!(canonical_modem(invalid).is_err());
    }
    assert_eq!(requested_family(&[4, 6]).unwrap(), 4);
    assert_eq!(requested_family(&[6, 4]).unwrap(), 4);
    assert_eq!(requested_family(&[6]).unwrap(), 2);
    assert_eq!(requested_family(&[4]).unwrap(), 1);
    for invalid in [&[][..], &[4, 4], &[6, 6], &[4, 6, 4], &[0]] {
        assert!(requested_family(invalid).is_err());
    }
}

#[derive(Default)]
struct MemoryStore(Mutex<Option<Receipt>>);
impl Store for MemoryStore {
    fn save(&self, receipt: &Receipt) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(receipt.clone());
        Ok(())
    }
    fn remove(&self) -> Result<(), String> {
        self.0.lock().unwrap().take();
        Ok(())
    }
}

struct MockIo {
    state: Mutex<Snapshot>,
    commands: Mutex<Vec<String>>,
    next: AtomicUsize,
    reporting_unknown: AtomicBool,
}
impl MockIo {
    fn new() -> Self {
        Self {
            state: Mutex::new(baseline()),
            commands: Mutex::default(),
            next: AtomicUsize::new(7),
            reporting_unknown: AtomicBool::new(false),
        }
    }
    fn at(&self, command: &str, family: u32, apn: &str) -> Result<String, String> {
        self.commands.lock().unwrap().push(command.into());
        let id = self.next.load(Ordering::Acquire) as i32;
        if command == "AT+CGDCONT=?" {
            let family = match family {
                1 => "IP",
                2 => "IPV6",
                _ => "IPV4V6",
            };
            return Ok(format!("+CGDCONT: ({id}-16),\"{family}\""));
        }
        if command == "AT+CGACT?" {
            return Ok("+CGACT: 1,0".into());
        }
        assert!(command.starts_with(&format!("AT+CGDCONT={id},")));
        let mut state = self.state.lock().unwrap();
        assert!(!state.profiles.contains_key(&id));
        state.profiles.insert(
            id,
            Profile {
                id,
                apn: apn.into(),
                family,
                name: String::new(),
                fingerprint: format!("created-{id}-{family}"),
            },
        );
        state.definitions.insert(
            id,
            Definition {
                apn: apn.into(),
                family,
                fingerprint: format!("definition-{id}-{family}"),
            },
        );
        Ok("OK".into())
    }
}
impl ProfileIo for MockIo {
    fn method(&self) -> CreationMethod {
        CreationMethod::At
    }
    async fn snapshot(&self) -> Result<Snapshot, String> {
        Ok(self.state.lock().unwrap().clone())
    }
    async fn create(&self, apn: &str, family: u32, _tag: &str) -> Result<Profile, String> {
        create_at_with(self, apn, family, |command| {
            std::future::ready(self.at(&command, family, apn))
        })
        .await
    }
    async fn restore_reporting(&self, id: i32, flags: [u8; 3]) -> Result<(), String> {
        self.commands
            .lock()
            .unwrap()
            .push(format!("report:{id}:{flags:?}"));
        self.state.lock().unwrap().reporting.insert(id, flags);
        if self.reporting_unknown.load(Ordering::Acquire) {
            return Err("lost-reporting-reply".into());
        }
        Ok(())
    }
    async fn delete(&self, id: i32) -> Result<(), String> {
        self.commands.lock().unwrap().push(format!("Delete:{id}"));
        let mut state = self.state.lock().unwrap();
        state.profiles.remove(&id);
        state.definitions.remove(&id);
        self.next.fetch_add(2, Ordering::AcqRel);
        Ok(())
    }
}

#[tokio::test]
async fn runtime_profile_each_family_creates_new_unused_definition_and_returned_id() {
    let io = MockIo::new();
    let store = MemoryStore::default();
    for (families, id) in [(&[6, 4][..], 7), (&[6][..], 9), (&[4][..], 11)] {
        let family = requested_family(families).unwrap();
        let before = io.snapshot().await.unwrap();
        let plan = fingerprint(&("ims", family, &before)).unwrap();
        let receipt = acquire_with(&io, &store, "ims", family, &plan)
            .await
            .unwrap();
        assert_eq!(receipt.owned.as_ref().unwrap().id, id);
        assert_eq!(receipt.owned.as_ref().unwrap().family, family);
        arm(&io, &store, receipt).await.unwrap();
        let receipt = store.0.lock().unwrap().clone().unwrap();
        assert_eq!(receipt.phase, Phase::Probed);
        release_with(&io, &store, receipt).await.unwrap();
        assert_eq!(io.snapshot().await.unwrap(), before);
        assert!(store.0.lock().unwrap().is_none());
    }
    let commands = io.commands.lock().unwrap();
    assert!(commands
        .iter()
        .any(|c| c == "AT+CGDCONT=7,\"IPV4V6\",\"ims\""));
    assert!(commands
        .iter()
        .any(|c| c == "AT+CGDCONT=9,\"IPV6\",\"ims\""));
    assert!(commands.iter().any(|c| c == "AT+CGDCONT=11,\"IP\",\"ims\""));
    assert_eq!(
        commands.iter().filter(|c| c.starts_with("Delete:")).count(),
        3
    );
}

#[tokio::test]
async fn runtime_profile_ambiguous_reporting_retains_intent_and_never_deletes() {
    let io = MockIo::new();
    let store = MemoryStore::default();
    let plan = fingerprint(&("ims", 4_u32, io.snapshot().await.unwrap())).unwrap();
    let receipt = acquire_with(&io, &store, "ims", 4, &plan).await.unwrap();
    io.reporting_unknown.store(true, Ordering::Release);
    assert!(arm(&io, &store, receipt).await.is_err());
    let receipt = store.0.lock().unwrap().clone().unwrap();
    assert_eq!(receipt.phase, Phase::Probing);
    assert!(release_with(&io, &store, receipt).await.is_err());
    assert!(!io
        .commands
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.starts_with("Delete:")));
    assert!(store.0.lock().unwrap().is_some());
}

#[tokio::test]
async fn runtime_profile_delete_requires_bearer_cleanup_and_network_absence() {
    for fail in ["none", "bearer", "network"] {
        let calls = Mutex::new(Vec::new());
        let result = cleanup_before_profile(
            || async {
                calls.lock().unwrap().push("bearer");
                if fail == "bearer" {
                    Err("pending".into())
                } else {
                    Ok(())
                }
            },
            || async {
                calls.lock().unwrap().push("network");
                if fail == "network" {
                    Err("residue".into())
                } else {
                    Ok(())
                }
            },
            || async {
                calls.lock().unwrap().push("profile");
                Ok(())
            },
        )
        .await;
        assert_eq!(result.is_ok(), fail == "none");
        let expected = match fail {
            "bearer" => vec!["bearer"],
            "network" => vec!["bearer", "network"],
            _ => vec!["bearer", "network", "profile"],
        };
        assert_eq!(*calls.lock().unwrap(), expected);
    }
    let deleted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&deleted);
    let (done, wait) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        cleanup_before_profile(
            || async {
                wait.await.unwrap();
                Ok(())
            },
            || async { Ok(()) },
            || async {
                flag.store(true, Ordering::Release);
                Ok(())
            },
        )
        .await
        .unwrap();
    });
    tokio::task::yield_now().await;
    assert!(!deleted.load(Ordering::Acquire));
    done.send(()).unwrap();
    task.await.unwrap();
    assert!(deleted.load(Ordering::Acquire));
}

struct LateHandle(Arc<AtomicUsize>);
impl ImsBearerHandle for LateHandle {
    fn check_liveness(&mut self) -> Result<(), ImsBearerError> {
        Ok(())
    }
    fn release(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        Box::pin(async move {
            tokio::task::yield_now().await;
            self.0.fetch_add(1, Ordering::AcqRel);
        })
    }
}
#[tokio::test]
async fn runtime_profile_late_cancelled_setup_releases_successful_handle_once() {
    let released = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = oneshot::channel();
    drop(receiver);
    deliver(
        sender,
        Ok((
            ImsBearerInfo::default(),
            Box::new(LateHandle(Arc::clone(&released))),
        )),
    )
    .await;
    assert_eq!(released.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn runtime_profile_generation_is_checked_before_and_after_waits() {
    let live = Arc::new(AtomicBool::new(true));
    let current: Current = {
        let live = Arc::clone(&live);
        Arc::new(move || live.load(Ordering::Acquire))
    };
    check_current(&current).unwrap();
    // Cancel uses true as cancelled; the runtime combines it with !flag. Test
    // the same captured predicate rather than manufacturing a newer generation.
    let cancelled = Arc::new(AtomicBool::new(false));
    let token = Cancel(Arc::clone(&cancelled));
    let generation: Current = Arc::new(move || !cancelled.load(Ordering::Acquire));
    check_current(&generation).unwrap();
    tokio::task::yield_now().await;
    drop(token);
    assert!(check_current(&generation).is_err());
    live.store(false, Ordering::Release);
    assert!(check_current(&current).is_err());
}

#[test]
fn runtime_profile_rebind_preserves_original_bearer_identity_and_rejects_unknown_sim() {
    let mut receipt = runtime_receipt();
    let owned = Profile {
        id: 7,
        apn: "ims".into(),
        family: 4,
        name: String::new(),
        fingerprint: "new".into(),
    };
    let definition = Definition {
        family: 4,
        apn: "ims".into(),
        fingerprint: "new-definition".into(),
    };
    receipt.owned = Some(owned.clone());
    receipt.owned_definition = Some(definition.clone());
    let mut observed = receipt.before.clone();
    observed.modem = format!("{MODEM_PREFIX}2");
    observed.sim_fingerprint = "new-sim-object-same-card".into();
    observed.profiles.insert(7, owned);
    observed.definitions.insert(7, definition);
    let rebound = rebind_profile_receipt(&receipt, &observed).unwrap();
    assert_eq!(rebound.before.modem, observed.modem);
    assert_eq!(rebound.runtime.as_ref().unwrap().generation, 41);
    assert_eq!(receipt.before.modem, format!("{MODEM_PREFIX}0"));
    for change in 0..4 {
        let mut observed = observed.clone();
        match change {
            0 => observed.stable_sim_fingerprint = None,
            1 => observed.stable_sim_fingerprint = Some("other-card".into()),
            2 => observed.owner = ":1.99".into(),
            _ => observed.control_topology = None,
        }
        assert!(rebind_profile_receipt(&receipt, &observed).is_err());
    }
}

fn temporary_directory() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "simadmin-runtime-profile-test-{}-{}",
        std::process::id(),
        profile_tag().unwrap()
    ));
    fs::create_dir(&path).unwrap();
    path
}

#[test]
fn runtime_profile_settle_retries_never_replay_uncertain_writes() {
    let mut receipt = runtime_receipt();
    for phase in [
        Phase::Creating,
        Phase::Probing,
        Phase::RestoringReporting,
        Phase::Deleting,
        Phase::Rejected,
    ] {
        receipt.phase = phase;
        assert!(!cleanup_may_settle(&receipt, true));
    }
    receipt.phase = Phase::Probed;
    receipt.runtime.as_mut().unwrap().phase = RuntimePhase::BearerPending;
    assert!(!cleanup_may_settle(&receipt, false));
    assert!(cleanup_may_settle(&receipt, true));
    receipt.runtime.as_mut().unwrap().phase = RuntimePhase::Cleaning;
    assert!(cleanup_may_settle(&receipt, false));
}

#[test]
fn runtime_profile_shutdown_never_adopts_unrecorded_or_inflight_setup() {
    for phase in [
        RuntimePhase::Profile,
        RuntimePhase::BearerPending,
        RuntimePhase::Active,
        RuntimePhase::Cleaning,
    ] {
        assert!(!shutdown_phase_ready(phase, false));
        assert_eq!(
            shutdown_phase_ready(phase, true),
            matches!(phase, RuntimePhase::Active | RuntimePhase::Cleaning)
        );
    }
    let mut receipt = runtime_receipt();
    receipt.runtime.as_mut().unwrap().phase = RuntimePhase::Active;
    assert!(!shutdown_profile_ready(&receipt));
}

#[test]
fn runtime_profile_oversize_intent_is_rejected_before_disk_or_modem_work() {
    let directory = temporary_directory();
    let file = directory.join("profile.json");
    let store = RuntimeStore::new(file.clone(), ownership(), None);
    let mut receipt = runtime_receipt();
    receipt.phase = Phase::Creating;
    receipt.before.eps_fingerprint = "x".repeat(MAX_RUNTIME_RECORD_BYTES);
    assert_eq!(
        store.save(&receipt).unwrap_err(),
        "mm_ims_profile_runtime_record_too_large"
    );
    assert!(!file.exists());
    assert!(store.receipt().is_err());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn runtime_profile_flock_excludes_same_process_and_other_process_until_drop() {
    let directory = temporary_directory();
    let file = directory.join("profile.json");
    let first = open_device_lock(&file, unsafe { libc::geteuid() }).unwrap();
    assert!(open_device_lock(&file, unsafe { libc::geteuid() }).is_err());
    let external = || {
        std::process::Command::new("flock")
            .arg("-n")
            .arg(file.with_extension("lock"))
            .arg("true")
            .status()
            .expect("Linux flock required")
    };
    assert!(!external().success());
    drop(first);
    assert!(external().success());
    let second = open_device_lock(&file, unsafe { libc::geteuid() }).unwrap();
    drop(second);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn runtime_profile_store_stamps_ownership_before_dispatch_and_poison_blocks_reuse() {
    let directory = temporary_directory();
    let file = directory.join("profile.json");
    let store = RuntimeStore::new(file.clone(), ownership(), None);
    let mut receipt = runtime_receipt();
    receipt.version = 1;
    receipt.runtime = None;
    receipt.phase = Phase::Creating;
    store.save(&receipt).unwrap();
    let saved: Receipt = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    assert_eq!(saved.version, 2);
    assert_eq!(saved.phase, Phase::Creating);
    let owner = saved.runtime.unwrap();
    assert_eq!(owner.line_hash, ownership().line_hash);
    assert_eq!(owner.generation, 41);
    assert_eq!(owner.process_start, 456);
    assert_eq!(owner.boot_id, ownership().boot_id);
    let missing = directory.join("missing");
    let poisoned = RuntimeStore::new(missing.join("profile.json"), ownership(), None);
    assert!(poisoned.save(&receipt).is_err());
    fs::create_dir(&missing).unwrap();
    assert!(poisoned.receipt().is_err());
    assert!(poisoned.save(&receipt).is_err());
    assert!(poisoned.remove().is_err());
    assert!(poisoned.state.lock().unwrap().receipt.is_some());
    fs::remove_dir_all(directory).unwrap();
}
