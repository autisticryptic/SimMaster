//! Persistence round-trip for five-code records.
use super::*;
use crate::platform::db::Database;

fn temp_database() -> (Database, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("simadmin-virtual-sim-{}", unique()));
    let _ = std::fs::remove_file(&path);
    (Database::new(path.clone()).expect("database"), path)
}

fn unique() -> String {
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
fn storing_again_replaces_the_previous_record() {
    let (database, path) = temp_database();
    database.with_conn(|conn| {
        db::store_profile(conn, "virtual-1", "profile-1", &codes())?;
        let updated = FiveCodes::new("8944000000000000002", "001010987654321",
            [9u8; 16], OpMaterial::Opc([7u8; 16]), None).unwrap();
        db::store_profile(conn, "virtual-1", "profile-2", &updated)?;
        assert_eq!(db::list_profiles(conn)?.len(), 1);
        assert_eq!(db::open_profile(conn, "virtual-1")?, Some(updated));
        Ok(())
    }).unwrap();
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_corrupt_record_is_reported_instead_of_guessed() {
    let (database, path) = temp_database();
    database.with_conn(|conn| {
        db::store_profile(conn, "virtual-1", "profile-1", &codes())?;
        conn.execute("UPDATE virtual_sim_profile SET sealed = X'00' WHERE line_id = 'virtual-1'", []).unwrap();
        assert!(db::open_profile(conn, "virtual-1").is_err());
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
