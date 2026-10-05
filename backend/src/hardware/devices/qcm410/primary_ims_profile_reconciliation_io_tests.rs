use super::*;
use std::os::unix::fs::symlink;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("simadmin-reconcile-{}-{}", std::process::id(), profile_tag().unwrap()));
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        Self(dir)
    }
    fn file(&self) -> PathBuf { self.0.join("profile.json") }
}
impl Drop for Temp { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
fn private_file(path: &Path, bytes: &[u8]) {
    let mut f = OpenOptions::new().create_new(true).write(true).mode(0o600).open(path).unwrap();
    f.write_all(bytes).unwrap();
}
fn sample_journal(source: &[u8]) -> Journal {
    Journal {
        version: 1, source_fingerprint: fingerprint(&source).unwrap(), step: Step::AbsentVerified,
        current: Observation {
            snapshot: Snapshot {
                bus_id: "a".repeat(32), owner: ":1.99".into(), modem: format!("{MODEM_PREFIX}0"),
                device: PRIMARY.into(), sim_fingerprint: "b".repeat(64),
                stable_sim_fingerprint: Some("c".repeat(64)), control_topology: Some("d".repeat(64)),
                eps_fingerprint: "e".repeat(64), profiles: BTreeMap::new(), definitions: BTreeMap::new(),
                reporting: (1..=16).map(|id| (id, [0,0,0])).collect(),
            }, boot: "12345678-1234-1234-1234-123456789abc".into(), activity: BTreeMap::new(),
        },
    }
}
#[test]
fn reconciliation_activity_requires_explicit_present_target_and_strict_rows() {
    let parsed = parse_activity("+CGACT: 1,1\n+CGACT: 4,0\nOK\n").unwrap();
    assert_eq!(parsed.get(&4), Some(&false));
    assert_eq!(parsed.get(&3), None); // absent is not false
    for text in ["", "OK", "+CGACT: 4,0\n+CGACT: 4,0", "+CGACT: 4,2", "+CGACT: 0,0", "+CGACT: 17,0", "+CGACT: 4,0,1", "ERROR"] {
        assert!(parse_activity(text).is_err(), "{text}");
    }
}
#[test]
fn orphan_reconciliation_marker_blocks_ordinary_and_switch_admission() {
    let temp = Temp::new();
    let file = temp.file();
    assert!(!has_pending(&file).unwrap());
    private_file(&journal_path(&file), b"damaged");
    assert!(has_pending(&file).unwrap());
    assert!(ensure_no_pending(&file).is_err());
    let lock = open_device_lock(&file, unsafe { libc::geteuid() }).unwrap();
    assert!(switch_drain_under_lock(&file, lock, || Ok(())).is_err());
    assert!(JournalStore { file }.finish_archived().is_err());
}
#[test]
fn damaged_linked_or_source_mismatched_journal_never_resets_budget() {
    let temp = Temp::new(); let file = temp.file();
    private_file(&file, b"source");
    let store = JournalStore { file: file.clone() };
    let j = sample_journal(b"source"); store.save(&j).unwrap();
    assert!(store.load(&"f".repeat(64)).is_err());
    fs::write(&file, b"changed source").unwrap();
    assert!(store.save(&j).is_err());
    fs::remove_file(journal_path(&file)).unwrap();
    symlink(temp.0.join("missing"), journal_path(&file)).unwrap();
    assert!(has_pending(&file).unwrap());
    assert!(store.load(&j.source_fingerprint).is_err());
}
#[test]
fn active_source_prevents_orphan_marker_retirement() {
    let temp = Temp::new(); let file = temp.file();
    private_file(&file, b"source"); let store = JournalStore { file };
    store.save(&sample_journal(b"source")).unwrap();
    assert!(store.finish_archived().is_err());
    assert!(has_pending(&store.file).unwrap());
}
#[test]
fn terminal_orphan_marker_requires_exact_durable_source_archive() {
    let temp = Temp::new(); let file = temp.file(); let store = JournalStore { file: file.clone() };
    let source = b"original source bytes"; private_file(&file, source);
    let j = sample_journal(source); store.save(&j).unwrap();
    fs::remove_file(&file).unwrap();
    assert!(store.finish_archived().is_err());
    let archive = temp.0.join("retired").join(format!("absent-{}.receipt", j.source_fingerprint));
    private_file(&archive, source);
    store.finish_archived().unwrap();
    assert!(!has_pending(&file).unwrap());
    assert_eq!(fs::read(archive).unwrap(), source);
    let archived = temp.0.join("retired").join(format!("reconciled-{}.journal", j.source_fingerprint));
    assert_eq!(serde_json::from_slice::<Journal>(&fs::read(archived).unwrap()).unwrap(), j);
}
#[test]
fn restoring_a_retired_source_never_replenishes_the_command_budget() {
    let temp = Temp::new(); let file = temp.file(); let store = JournalStore { file: file.clone() };
    let source = b"original source"; private_file(&file, source);
    let j = sample_journal(source); store.save(&j).unwrap();
    fs::remove_file(&file).unwrap();
    let retired = temp.0.join("retired"); fs::create_dir(&retired).unwrap();
    private_file(&retired.join(format!("absent-{}.receipt", j.source_fingerprint)), source);
    store.finish_archived().unwrap();
    private_file(&file, source); // a mistakenly restored backup is still the old transaction
    assert!(store.load(&j.source_fingerprint).is_err());
    assert!(store.save(&j).is_err());
    assert!(!has_pending(&file).unwrap());
    assert_eq!(fs::read(&file).unwrap(), source);
}

#[test]
fn unsafe_archive_directory_cannot_hide_retired_transactions() {
    let temp = Temp::new(); let file = temp.file(); let store = JournalStore { file: file.clone() };
    private_file(&file, b"source");
    symlink(temp.0.join("missing"), temp.0.join("retired")).unwrap();
    assert!(store.load(&fingerprint(&b"source").unwrap()).is_err());
}

#[test]
fn incomplete_or_conflicting_archive_keeps_active_marker() {
    let temp = Temp::new(); let file = temp.file(); let store = JournalStore { file: file.clone() };
    let source = b"original"; private_file(&file, source); let mut j = sample_journal(source);
    j.step = Step::DeleteDispatched; store.save(&j).unwrap(); fs::remove_file(&file).unwrap();
    assert!(store.finish_archived().is_err()); assert!(has_pending(&file).unwrap());
}
