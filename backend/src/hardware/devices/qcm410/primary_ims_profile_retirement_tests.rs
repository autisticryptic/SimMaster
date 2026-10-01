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
