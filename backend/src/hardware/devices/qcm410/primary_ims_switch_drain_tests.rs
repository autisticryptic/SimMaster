//! No modem, D-Bus, serial, root privileges or process-global test state.
use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "simadmin-ims-switch-drain-{}-{}",
            std::process::id(),
            profile_tag().unwrap()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn profile(&self) -> PathBuf {
        self.0.join("profile.json")
    }

    fn bearers(&self) -> PathBuf {
        self.0.join("bearers")
    }

    fn lock(&self) -> Result<fs::File, String> {
        open_device_lock(&self.profile(), unsafe { libc::geteuid() })
    }

    fn drain(&self, pending: usize, live: bool) -> Result<EsimSwitchDrainGuard, String> {
        switch_drain_under_lock(&self.profile(), self.lock()?, || {
            switch_bearer_work_absent(pending, live, &self.bearers())
        })
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn stale_receipt() -> Receipt {
    Receipt {
        version: 2,
        method: CreationMethod::At,
        phase: Phase::Probed,
        before: Snapshot {
            bus_id: "a".repeat(32),
            owner: ":1.18".into(),
            modem: format!("{MODEM_PREFIX}4"),
            device: PRIMARY.into(),
            sim_fingerprint: "old-sim-object".into(),
            stable_sim_fingerprint: Some("old-sim".into()),
            control_topology: Some("primary-control".into()),
            eps_fingerprint: "eps".into(),
            profiles: BTreeMap::new(),
            definitions: BTreeMap::new(),
            reporting: (1..=16).map(|id| (id, [0, 0, 0])).collect(),
        },
        requested_family: 4,
        apn: "ims".into(),
        tag: "sa-stale".into(),
        owned: Some(Profile {
            id: 3,
            apn: "ims".into(),
            family: 4,
            name: String::new(),
            fingerprint: "cid3".into(),
        }),
        owned_definition: Some(Definition {
            family: 4,
            apn: "ims".into(),
            fingerprint: "cid3-definition".into(),
        }),
        runtime: Some(RuntimeOwnership {
            version: 1,
            line_hash: "b".repeat(64),
            generation: 41,
            process_id: 123,
            process_start: 456,
            boot_id: "12345678-1234-1234-1234-123456789abc".into(),
            phase: RuntimePhase::Cleaning,
            abandoned: true,
            bearer: None,
        }),
    }
}

#[test]
fn ims_switch_drain_clean_absence_reserves_flock_until_drop() {
    let temp = TempDirectory::new();
    let guard = temp.drain(0, false).unwrap();
    assert!(!temp.profile().exists());
    assert!(temp.lock().is_err(), "guard must exclude a new Context");
    assert!(temp.drain(0, false).is_err(), "switches also exclude each other");
    drop(guard);
    assert!(temp.lock().is_ok());
    assert!(temp.profile().with_extension("lock").exists(), "never unlink the flock");
    assert!(temp.drain(0, false).is_ok());
}

#[test]
fn ims_switch_drain_receipt_free_cleanup_or_admission_is_busy() {
    let temp = TempDirectory::new();
    // Model Context before its first intent OR after receipt removal while
    // cleanup still owns the fd. ENOENT alone cannot authorize lpac.
    let context_lock = temp.lock().unwrap();
    assert!(!temp.profile().exists());
    assert_eq!(
        temp.drain(0, false).err().unwrap(),
        "mm_ims_profile_runtime_device_busy"
    );
    drop(context_lock);
    assert!(temp.drain(0, false).is_ok());
}

#[test]
fn ims_switch_drain_stale_and_damaged_receipts_are_never_adopted_or_removed() {
    let stale = serde_json::to_vec(&stale_receipt()).unwrap();
    assert!(serde_json::from_slice::<Receipt>(&stale).is_ok());
    for bytes in [stale, Vec::new(), b"{truncated".to_vec(), b"null".to_vec()] {
        let temp = TempDirectory::new();
        fs::write(temp.profile(), &bytes).unwrap();
        assert_eq!(
            temp.drain(0, false).err().unwrap(),
            "mm_ims_profile_runtime_receipt_pending"
        );
        assert_eq!(fs::read(temp.profile()).unwrap(), bytes);
        assert!(temp.lock().is_ok(), "an error must release only the guard fd");
    }
}

#[test]
fn ims_switch_drain_symlink_or_directory_receipt_is_not_absence() {
    for directory in [false, true] {
        let temp = TempDirectory::new();
        if directory {
            fs::create_dir(temp.profile()).unwrap();
        } else {
            symlink(temp.0.join("missing-receipt"), temp.profile()).unwrap();
        }
        assert!(temp.drain(0, false).is_err());
        assert!(fs::symlink_metadata(temp.profile()).is_ok());
    }
}

#[test]
fn ims_switch_drain_pending_or_live_global_bearer_blocks_without_receipt() {
    for (pending, live) in [(1, false), (0, true), (2, true)] {
        let temp = TempDirectory::new();
        assert_eq!(
            temp.drain(pending, live).err().unwrap(),
            "mm_ims_profile_runtime_bearer_cleanup_pending"
        );
        assert!(!temp.bearers().exists());
        assert!(temp.lock().is_ok());
    }
}

#[test]
fn ims_switch_drain_durable_bearer_and_pending_create_markers_block() {
    for marker in ["other-line.json", "old-owner.create"] {
        let temp = TempDirectory::new();
        fs::create_dir(temp.bearers()).unwrap();
        let path = temp.bearers().join(marker);
        // Intentionally damaged: neither successful parsing nor a live owner
        // is required to block the global MM reset.
        fs::write(&path, b"damaged").unwrap();
        assert_eq!(
            temp.drain(0, false).err().unwrap(),
            "mm_ims_profile_lease_bearer_receipt_remaining"
        );
        assert_eq!(fs::read(path).unwrap(), b"damaged");
        assert!(temp.lock().is_ok());
    }
}

#[test]
fn ims_switch_drain_unverifiable_bearer_directory_fails_closed() {
    for directory_symlink in [false, true] {
        let temp = TempDirectory::new();
        if directory_symlink {
            symlink(temp.0.join("missing"), temp.bearers()).unwrap();
        } else {
            fs::write(temp.bearers(), b"not a directory").unwrap();
        }
        assert!(temp.drain(0, false).is_err());
    }
    let temp = TempDirectory::new();
    fs::create_dir(temp.bearers()).unwrap();
    symlink(temp.0.join("missing"), temp.bearers().join("pending.create")).unwrap();
    assert!(temp.drain(0, false).is_err());
}

#[test]
fn ims_switch_drain_untrusted_parent_or_permissions_cannot_prove_absence() {
    let temp = TempDirectory::new();
    let parent = temp.0.join("parent");
    symlink(temp.0.join("missing"), &parent).unwrap();
    assert!(switch_bearer_work_absent(0, false, &parent.join("bearers")).is_err());
    fs::remove_file(&parent).unwrap();
    fs::create_dir(&parent).unwrap();
    let bearers = parent.join("bearers");
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(switch_bearer_work_absent(0, false, &bearers).is_err());
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(&bearers).unwrap();
    fs::set_permissions(&bearers, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(switch_bearer_work_absent(0, false, &bearers).is_err());
    fs::set_permissions(&bearers, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(switch_bearer_work_absent(0, false, &bearers).is_ok());
}

// Exercise the actual release/store proof, not a test that simply unlinks a
// receipt and calls that cleanup. Only the modem observations are substituted.
struct CleanupIo {
    snapshot: Mutex<Snapshot>,
    bad_readback: bool,
}

impl CleanupIo {
    fn new(receipt: &Receipt, bad_readback: bool) -> Self {
        let mut snapshot = receipt.before.clone();
        let owned = receipt.owned.as_ref().unwrap();
        snapshot.profiles.insert(owned.id, owned.clone());
        snapshot
            .definitions
            .insert(owned.id, receipt.owned_definition.clone().unwrap());
        snapshot.reporting.insert(owned.id, [1, 1, 1]);
        Self {
            snapshot: Mutex::new(snapshot),
            bad_readback,
        }
    }
}

impl ProfileIo for CleanupIo {
    fn method(&self) -> CreationMethod {
        CreationMethod::At
    }
    async fn snapshot(&self) -> Result<Snapshot, String> {
        Ok(self.snapshot.lock().unwrap().clone())
    }
    async fn create(&self, _: &str, _: u32, _: &str) -> Result<Profile, String> {
        panic!("cleanup must never create a profile")
    }
    async fn restore_reporting(&self, id: i32, flags: [u8; 3]) -> Result<(), String> {
        self.snapshot.lock().unwrap().reporting.insert(id, flags);
        Ok(())
    }
    async fn delete(&self, id: i32) -> Result<(), String> {
        let mut snapshot = self.snapshot.lock().unwrap();
        snapshot.profiles.remove(&id);
        snapshot.definitions.remove(&id);
        if self.bad_readback {
            snapshot.eps_fingerprint = "unverified-after-delete".into();
        }
        Ok(())
    }
}

#[tokio::test]
async fn ims_switch_drain_verified_cleanup_clears_receipt_but_retains_context_flock() {
    let temp = TempDirectory::new();
    let context_lock = temp.lock().unwrap();
    let receipt = stale_receipt();
    let store = RuntimeStore::new(temp.profile(), receipt.runtime.clone().unwrap(), None);
    store.save(&receipt).unwrap();
    let io = CleanupIo::new(&receipt, false);
    release_with(&io, &store, receipt).await.unwrap();
    assert!(store.receipt().unwrap().is_none());
    assert_eq!(
        fs::symlink_metadata(temp.profile()).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    assert!(temp.drain(0, false).is_err(), "cleanup still owns the Context fd");
    drop(context_lock);
    let guard = temp.drain(0, false).unwrap();
    assert!(temp.lock().is_err());
    drop(guard);
    assert!(
        temp.lock().is_ok(),
        "successful cleanup and switch must release flock"
    );
}

#[tokio::test]
async fn ims_switch_drain_failed_cleanup_retains_receipt_after_context_drop() {
    for bad_readback in [false, true] {
        let temp = TempDirectory::new();
        let context_lock = temp.lock().unwrap();
        let receipt = stale_receipt();
        let store = RuntimeStore::new(temp.profile(), receipt.runtime.clone().unwrap(), None);
        store.save(&receipt).unwrap();
        let io = CleanupIo::new(&receipt, bad_readback);
        let result = cleanup_before_profile(
            || async {
                if bad_readback {
                    Ok(())
                } else {
                    Err("bearer-cleanup-failed".into())
                }
            },
            || async { Ok(()) },
            || async { release_with(&io, &store, receipt).await },
        )
        .await;
        assert!(result.is_err());
        assert!(store.receipt().unwrap().is_some());
        let bytes = fs::read(temp.profile()).unwrap();
        drop(context_lock);
        assert_eq!(
            temp.drain(0, false).err().unwrap(),
            "mm_ims_profile_runtime_receipt_pending"
        );
        assert_eq!(fs::read(temp.profile()).unwrap(), bytes);
        assert!(temp.lock().is_ok(), "failed barrier must release its own fd");
    }
}

#[test]
fn ims_switch_drain_empty_bearer_directory_is_positive_proof() {
    let temp = TempDirectory::new();
    fs::create_dir(temp.bearers()).unwrap();
    assert!(temp.drain(0, false).is_ok());
}

#[test]
fn ims_switch_drain_checks_global_work_while_flock_is_retained() {
    let temp = TempDirectory::new();
    let result = switch_drain_under_lock(&temp.profile(), temp.lock().unwrap(), || {
        assert!(temp.lock().is_err());
        Err("other-context-cleanup-busy".into())
    });
    assert_eq!(result.err().unwrap(), "other-context-cleanup-busy");
    assert!(temp.lock().is_ok());
}

#[test]
fn ims_switch_drain_flock_also_excludes_another_process() {
    let temp = TempDirectory::new();
    let external = || {
        std::process::Command::new("flock")
            .arg("-n")
            .arg(temp.profile().with_extension("lock"))
            .arg("true")
            .status()
            .expect("Linux flock required in Actions")
    };
    let guard = temp.drain(0, false).unwrap();
    assert!(!external().success());
    drop(guard);
    assert!(external().success());
}

#[tokio::test]
async fn ims_switch_drain_guard_survives_lpac_and_mm_awaits() {
    let temp = TempDirectory::new();
    let guard = temp.drain(0, false).unwrap();
    let (lpac_done, lpac_wait) = oneshot::channel::<()>();
    let (mm_started, mm_start_wait) = oneshot::channel::<()>();
    let (mm_done, mm_wait) = oneshot::channel::<()>();
    let operation = tokio::spawn(async move {
        let _drain = guard;
        lpac_wait.await.unwrap();
        mm_started.send(()).unwrap();
        mm_wait.await.unwrap();
    });
    assert!(temp.lock().is_err());
    lpac_done.send(()).unwrap();
    mm_start_wait.await.unwrap();
    assert!(temp.lock().is_err(), "lpac completion must not release the barrier");
    mm_done.send(()).unwrap();
    operation.await.unwrap();
    assert!(temp.lock().is_ok());
}

#[tokio::test]
async fn ims_switch_drain_guard_releases_on_operation_cancellation() {
    let temp = TempDirectory::new();
    let guard = temp.drain(0, false).unwrap();
    let (started, wait_started) = oneshot::channel::<()>();
    let operation = tokio::spawn(async move {
        let _drain = guard;
        started.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    wait_started.await.unwrap();
    assert!(temp.lock().is_err());
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    assert!(temp.lock().is_ok());
}
