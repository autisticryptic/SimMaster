use super::*;

fn fixture() -> (Receipt, Snapshot, String) {
    let profile = Profile {
        id: 4,
        family: 1,
        apn: "ims".into(),
        name: "".into(),
        fingerprint: "owned-profile".into(),
    };
    let before = Snapshot {
        bus_id: "a".repeat(32),
        owner: ":1.25".into(),
        modem: format!("{MODEM_PREFIX}77"),
        device: PRIMARY.into(),
        sim_fingerprint: "old-sim-object".into(),
        stable_sim_fingerprint: Some("old-card".into()),
        control_topology: Some("same-physical-control".into()),
        eps_fingerprint: "old-eps".into(),
        profiles: BTreeMap::new(),
        definitions: BTreeMap::new(),
        reporting: (1..=16).map(|id| (id, [0, 0, 0])).collect(),
    };
    let boot = "12345678-1234-1234-1234-123456789abc".to_string();
    let record = LeaseRecord {
        version: 1,
        bus_id: before.bus_id.clone(),
        owner: before.owner.clone(),
        modem: before.modem.clone(),
        bearer: "/org/freedesktop/ModemManager1/Bearer/155".into(),
        device: PRIMARY.into(),
        interface: "wwan0".into(),
        process_id: 119010,
        process_start: 555,
        namespace: None,
        network: Some(NetdevConfig {
            address: "192.0.2.2".parse().unwrap(),
            prefix: 30,
            mtu: Some(1500),
            probe_target: None,
        }),
        additional_networks: vec![],
    };
    let receipt = Receipt {
        version: 2,
        method: CreationMethod::At,
        phase: Phase::Probed,
        before: before.clone(),
        requested_family: 1,
        apn: "ims".into(),
        tag: "sa1234567890abcd".into(),
        owned: Some(profile),
        owned_definition: Some(Definition {
            family: 1,
            apn: "ims".into(),
            fingerprint: "owned-definition".into(),
        }),
        runtime: Some(RuntimeOwnership {
            version: 1,
            line_hash: "b".repeat(64),
            generation: 0,
            process_id: 119010,
            process_start: 555,
            boot_id: boot.clone(),
            phase: RuntimePhase::Cleaning,
            abandoned: true,
            bearer: Some(record),
        }),
    };
    let current = Snapshot {
        owner: ":1.99".into(),
        modem: format!("{MODEM_PREFIX}0"),
        sim_fingerprint: "new-sim-object".into(),
        stable_sim_fingerprint: Some("new-card".into()),
        eps_fingerprint: "new-sim-eps".into(),
        ..before
    };
    (receipt, current, boot)
}

#[test]
fn runtime_retirement_accepts_absence_not_cross_owner_adoption() {
    let (receipt, current, boot) = fixture();
    assert_eq!(
        validate_absent_profile(&receipt, &current, &boot, false).unwrap(),
        4
    );
    // New-card settings need not equal old-card settings: only metadata is
    // archived. The two CURRENT snapshots, however, must be identical.
    verify_snapshot_pair(&current, &current).unwrap();
    assert!(validate_absent_profile(&receipt, &current, &boot, true).is_err());
    for change in 0..4 {
        let mut present = current.clone();
        match change {
            0 => {
                present.profiles.insert(4, receipt.owned.clone().unwrap());
            }
            1 => {
                present
                    .definitions
                    .insert(4, receipt.owned_definition.clone().unwrap());
            }
            2 => {
                let mut other = receipt.owned.clone().unwrap();
                other.apn = "new-card-data".into();
                present.profiles.insert(4, other);
            }
            _ => {
                present.reporting.insert(4, [1, 1, 1]);
            }
        }
        assert!(validate_absent_profile(&receipt, &present, &boot, false).is_err());
    }
}

#[test]
fn runtime_retirement_rejects_uncertain_phases_and_incomplete_ownership() {
    let (receipt, current, boot) = fixture();
    for phase in [
        Phase::Creating,
        Phase::Probing,
        Phase::RestoringReporting,
        Phase::Deleting,
        Phase::Rejected,
    ] {
        let mut bad = receipt.clone();
        bad.phase = phase;
        assert!(validate_absent_profile(&bad, &current, &boot, false).is_err());
    }
    for change in 0..9 {
        let mut bad = receipt.clone();
        match change {
            0 => bad.runtime.as_mut().unwrap().phase = RuntimePhase::BearerPending,
            1 => bad.runtime.as_mut().unwrap().bearer = None,
            2 => {
                bad.runtime
                    .as_mut()
                    .unwrap()
                    .bearer
                    .as_mut()
                    .unwrap()
                    .network = None
            }
            3 => bad.before.control_topology = None,
            4 => bad.before.device = "/dev/wwan1qmi0".into(),
            5 => bad.owned_definition = None,
            6 => bad.owned.as_mut().unwrap().family = 2,
            7 => {
                bad.before.profiles.insert(4, bad.owned.clone().unwrap());
            }
            _ => bad.version = 1,
        }
        assert!(
            validate_absent_profile(&bad, &current, &boot, false).is_err(),
            "change={change}"
        );
    }
    let mut changed = current.clone();
    changed.control_topology = Some("other-device".into());
    assert!(validate_absent_profile(&receipt, &changed, &boot, false).is_err());
}

#[test]
fn runtime_retirement_plan_requires_stable_current_snapshot() {
    let (_, current, _) = fixture();
    for change in 0..5 {
        let mut after = current.clone();
        match change {
            0 => after.owner = ":1.100".into(),
            1 => after.stable_sim_fingerprint = Some("third-card".into()),
            2 => after.eps_fingerprint = "changed".into(),
            3 => after.modem = format!("{MODEM_PREFIX}2"),
            _ => {
                after.reporting.insert(1, [1, 0, 0]);
            }
        }
        assert!(verify_snapshot_pair(&current, &after).is_err());
    }
}

fn uncreated_fixture() -> (Receipt, Snapshot, String) {
    let (mut receipt, _, boot) = fixture();
    receipt.phase = Phase::Creating;
    receipt.owned = None;
    receipt.owned_definition = None;
    let owner = receipt.runtime.as_mut().unwrap();
    owner.phase = RuntimePhase::Profile;
    owner.bearer = None;
    let current = receipt.before.clone();
    (receipt, current, boot)
}

#[test]
fn uncreated_retirement_requires_dead_creator_same_boot_and_exact_shape() {
    let (receipt, current, boot) = uncreated_fixture();
    validate_uncreated_profile(&receipt, &current, &boot, false).unwrap();
    assert!(validate_uncreated_profile(&receipt, &current, &boot, true).is_err());
    assert!(validate_uncreated_profile(
        &receipt,
        &current,
        "abcdefab-1234-1234-1234-123456789abc",
        false
    )
    .is_err());
    for phase in [
        Phase::Rejected,
        Phase::Owned,
        Phase::Probing,
        Phase::Probed,
        Phase::RestoringReporting,
        Phase::Deleting,
    ] {
        let mut bad = receipt.clone();
        bad.phase = phase;
        assert!(validate_uncreated_profile(&bad, &current, &boot, false).is_err());
    }
    for phase in [
        RuntimePhase::BearerPending,
        RuntimePhase::Active,
        RuntimePhase::Cleaning,
    ] {
        let mut bad = receipt.clone();
        bad.runtime.as_mut().unwrap().phase = phase;
        assert!(validate_uncreated_profile(&bad, &current, &boot, false).is_err());
    }
    let (owned, _, _) = fixture();
    for change in 0..10 {
        let mut bad = receipt.clone();
        match change {
            0 => bad.version = 1,
            1 => bad.method = CreationMethod::Qmi,
            2 => bad.runtime = None,
            3 => bad.owned = owned.owned.clone(),
            4 => bad.owned_definition = owned.owned_definition.clone(),
            5 => {
                bad.runtime.as_mut().unwrap().bearer =
                    owned.runtime.as_ref().unwrap().bearer.clone()
            }
            6 => bad.runtime.as_mut().unwrap().process_start = 0,
            7 => bad.runtime.as_mut().unwrap().process_id = 0,
            8 => bad.before.stable_sim_fingerprint = None,
            _ => bad.before.control_topology = None,
        }
        assert!(
            validate_uncreated_profile(&bad, &current, &boot, false).is_err(),
            "change={change}"
        );
    }
    // The new explicit path does not broaden existing absence proofs.
    assert!(validate_absent_profile(&receipt, &current, &boot, false).is_err());
}

#[test]
fn uncreated_retirement_rejects_any_snapshot_drift() {
    let (receipt, current, boot) = uncreated_fixture();
    let (owned, _, _) = fixture();
    for change in 0..12 {
        let mut changed = current.clone();
        match change {
            0 => changed.bus_id = "c".repeat(32),
            1 => changed.owner = ":1.99".into(),
            2 => changed.modem = format!("{MODEM_PREFIX}0"),
            3 => changed.device = "/dev/wwan1qmi0".into(),
            4 => changed.sim_fingerprint = "new-sim-object".into(),
            5 => changed.stable_sim_fingerprint = Some("new-card".into()),
            6 => changed.control_topology = Some("new-topology".into()),
            7 => changed.eps_fingerprint = "new-eps".into(),
            8 => {
                changed.profiles.insert(4, owned.owned.clone().unwrap());
            }
            9 => {
                changed
                    .definitions
                    .insert(4, owned.owned_definition.clone().unwrap());
            }
            10 => {
                changed.reporting.insert(4, [1, 0, 0]);
            }
            _ => {
                changed.reporting.remove(&4);
            }
        }
        assert!(
            validate_uncreated_profile(&receipt, &changed, &boot, false).is_err(),
            "change={change}"
        );
        assert!(verify_snapshot_pair(&current, &changed).is_err());
    }
}

#[test]
fn uncreated_retirement_requires_settled_mtime_before_inspection() {
    let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    assert!(validate_settled_source(modified, modified).is_err());
    assert!(validate_settled_source(modified, modified - Duration::from_secs(1)).is_err());
    assert!(validate_settled_source(modified, modified + Duration::from_secs(119)).is_err());
    validate_settled_source(modified, modified + Duration::from_secs(120)).unwrap();
    validate_settled_source(modified, modified + Duration::from_secs(121)).unwrap();
}

#[test]
fn uncreated_retirement_token_binds_source_snapshot_boot_mtime_and_action() {
    let (receipt, current, boot) = uncreated_fixture();
    let source = serde_json::to_vec(&receipt).unwrap();
    let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let plan = uncreated_plan(&source, &current, &boot, modified).unwrap();
    assert_eq!(
        plan,
        uncreated_plan(&source, &current, &boot, modified).unwrap()
    );
    require_uncreated_plan(&plan, Some(&plan)).unwrap();
    assert!(require_uncreated_plan(&plan, None).is_err());
    let mut changed = current.clone();
    changed.reporting.insert(4, [1, 0, 0]);
    let absent = fingerprint(&(
        "retire-absent-v1",
        fingerprint(&source).unwrap(),
        &boot,
        &current,
    ))
    .unwrap();
    for other in [
        uncreated_plan(b"changed-source", &current, &boot, modified).unwrap(),
        uncreated_plan(&source, &changed, &boot, modified).unwrap(),
        uncreated_plan(&source, &current, "other-boot", modified).unwrap(),
        uncreated_plan(&source, &current, &boot, modified + Duration::from_nanos(1)).unwrap(),
        absent,
    ] {
        assert_ne!(plan, other);
        assert!(require_uncreated_plan(&plan, Some(&other)).is_err());
    }
}

fn temporary_source() -> (PathBuf, PathBuf, Vec<u8>) {
    let dir = std::env::temp_dir().join(format!(
        "simadmin-retire-test-{}-{}",
        std::process::id(),
        profile_tag().unwrap()
    ));
    fs::create_dir(&dir).unwrap();
    let source = dir.join("profile.json");
    let bytes = serde_json::to_vec(&fixture().0).unwrap();
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&source)
        .unwrap();
    out.write_all(&bytes).unwrap();
    out.sync_all().unwrap();
    (dir, source, bytes)
}

#[test]
fn runtime_retirement_archives_exact_metadata_before_removing_active_name() {
    let (dir, source, bytes) = temporary_source();
    let archive = archive_source(&source, &bytes).unwrap();
    assert!(!source.exists());
    assert_eq!(fs::read(&archive).unwrap(), bytes);
    assert_eq!(fs::metadata(&archive).unwrap().mode() & 0o777, 0o600);
    assert_eq!(archive.parent().unwrap(), dir.join("retired"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn runtime_retirement_archive_failure_or_changed_source_never_unlocks() {
    let (dir, source, bytes) = temporary_source();
    assert!(archive_source(&source, b"different-record").is_err());
    assert_eq!(fs::read(&source).unwrap(), bytes);
    let archive_dir = dir.join("retired");
    ensure_directory(&archive_dir).unwrap();
    let archive = archive_dir.join(format!("absent-{}.receipt", fingerprint(&bytes).unwrap()));
    fs::write(&archive, b"prior-evidence-do-not-overwrite").unwrap();
    assert!(archive_source(&source, &bytes).is_err());
    assert_eq!(fs::read(&source).unwrap(), bytes);
    assert_eq!(
        fs::read(&archive).unwrap(),
        b"prior-evidence-do-not-overwrite"
    );
    fs::remove_file(archive).unwrap();
    fs::remove_dir(&archive_dir).unwrap();
    fs::write(&archive_dir, b"not-a-directory").unwrap();
    assert!(archive_source(&source, &bytes).is_err());
    assert_eq!(fs::read(&source).unwrap(), bytes);
    fs::remove_dir_all(dir).unwrap();
}
