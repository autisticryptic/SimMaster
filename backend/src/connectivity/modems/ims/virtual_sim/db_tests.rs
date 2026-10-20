//! Persistence round-trip: sealing, rotation and startup installation.
use super::*;
use crate::platform::db::Database;

fn temp_database() -> (Database, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("simadmin-virtual-sim-{}", uuid_like()));
    let _ = std::fs::remove_file(&path);
    (Database::new(path.clone()).expect("database"), path)
}

fn uuid_like() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    format!("{nanos}-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn codes() -> FiveCodes {
    let mut ki = [0u8; 16];
    for (index, byte) in ki.iter_mut().enumerate() { *byte = index as u8; }
    let mut op = [0u8; 16];
    for (index, byte) in op.iter_mut().enumerate() { *byte = 0x80 + index as u8; }
    FiveCodes::new("8944000000000000001", "001010123456789", ki, OpMaterial::Op(op), Some("13800138000"))
        .expect("codes")
}

#[test]
fn the_master_key_is_created_once_and_reused() {
    let (database, path) = temp_database();
    database.with_conn(|conn| {
        let first = db::load_or_create_master_key(conn)?;
        let second = db::load_or_create_master_key(conn)?;
        assert_eq!(first.as_bytes(), second.as_bytes());
        Ok(())
    }).unwrap();
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_stored_profile_round_trips_and_deletes() {
    let (database, path) = temp_database();
    database.with_conn(|conn| {
        db::store_profile(conn, "virtual-1", "profile-1", &codes())?;
        assert_eq!(db::list_profiles(conn)?, vec![("virtual-1".to_string(), "profile-1".to_string())]);
        assert_eq!(db::open_profile(conn, "virtual-1")?, Some(codes()));
        assert!(db::open_profile(conn, "virtual-missing")?.is_none());
        assert!(db::delete_profile(conn, "virtual-1")?);
        assert!(!db::delete_profile(conn, "virtual-1")?);
        Ok(())
    }).unwrap();
    let _ = std::fs::remove_file(path);
}

#[test]
fn rotation_reencrypts_and_keeps_every_record_readable() {
    let (database, path) = temp_database();
    database.with_conn(|conn| {
        db::store_profile(conn, "virtual-1", "profile-1", &codes())?;
        let before = db::load_or_create_master_key(conn)?.as_bytes().to_vec();
        let (reencrypted, skipped) = db::rotate_master_key(conn)?;
        assert_eq!((reencrypted, skipped), (1, 0));
        let after = db::load_or_create_master_key(conn)?.as_bytes().to_vec();
        assert_ne!(before, after, "rotation must change the key");
        assert_eq!(db::open_profile(conn, "virtual-1")?, Some(codes()));
        Ok(())
    }).unwrap();
    let _ = std::fs::remove_file(path);
}

#[test]
fn startup_install_registers_each_line_and_reports_failures() {
    let (database, path) = temp_database();
    super::clear_for_test();
    database.with_conn(|conn| {
        db::store_profile(conn, "virtual-1", "profile-1", &codes())?;
        let (installed, failed) = db::install_all(conn)?;
        assert_eq!((installed, failed), (1, 0));
        Ok(())
    }).unwrap();
    let installed = for_line("virtual-1").expect("installed");
    assert_eq!(installed.codes().imsi, codes().imsi);
    assert!(for_line("virtual-2").is_none());
    super::clear_for_test();
    let _ = std::fs::remove_file(path);
}

#[test]
fn sealed_records_are_bound_to_their_profile_id() {
    let (database, path) = temp_database();
    database.with_conn(|conn| {
        db::store_profile(conn, "virtual-1", "profile-1", &codes())?;
        // Moving the blob to another profile id must not decrypt.
        let blob: Vec<u8> = conn
            .query_row("SELECT sealed FROM virtual_sim_profile WHERE line_id = 'virtual-1'", [], |row| row.get(0))
            .unwrap();
        let key = db::load_or_create_master_key(conn)?;
        assert!(store::open(&key, "profile-2", &blob).is_err());
        assert!(store::open(&key, "profile-1", &blob).is_ok());
        Ok(())
    }).unwrap();
    let _ = std::fs::remove_file(path);
}
