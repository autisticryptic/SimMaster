//! Actions-only SQLite tests. No APK, provider, modem or real credentials.
use super::{accounts, device_identity as device, Error};
use device::Snapshot;
use rusqlite::Connection;

fn key(value: &str) -> Vec<u8> { ring::digest::digest(&ring::digest::SHA256,value.as_bytes()).as_ref().to_vec() }
fn memory() -> Connection {
    let conn=Connection::open_in_memory().unwrap();
    conn.pragma_update(None,"foreign_keys",true).unwrap();
    accounts::initialize(&conn).unwrap();device::initialize(&conn).unwrap();conn
}
fn reserve(conn:&Connection,alias:&[u8],preferred:Option<&str>,seed:Option<&str>)->Result<Snapshot,Error> {
    let tx=rusqlite::Transaction::new_unchecked(conn,rusqlite::TransactionBehavior::Immediate).unwrap();
    let result=device::resolve(&tx,alias,preferred,seed,100)?;
    tx.commit().unwrap();Ok(result)
}
fn confirm(conn:&Connection,alias:&[u8],snapshot:&Snapshot,account:&[u8])->Result<Snapshot,Error> {
    let tx=rusqlite::Transaction::new_unchecked(conn,rusqlite::TransactionBehavior::Immediate).unwrap();
    let result=device::confirm(&tx,alias,snapshot,account,101)?;
    tx.commit().unwrap();Ok(result)
}
fn summary(conn:&Connection,account:&[u8])->String { accounts::record(conn,account,"u***","***0123",1,101).unwrap() }
fn total(conn:&Connection,table:&str)->i64 { conn.query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|r|r.get(0)).unwrap() }

#[test]
fn two_aliases_of_one_confirmed_number_converge_without_lending_the_provisional_id() {
    let conn=memory();let a=key("realm/email");let b=key("realm/phone");let number=key("realm/+447700900123");
    let first=reserve(&conn,&a,None,None).unwrap();let first=confirm(&conn,&a,&first,&number).unwrap();let id=summary(&conn,&number);
    let other=reserve(&conn,&b,None,None).unwrap();assert_ne!(other.device_id.expose_to_backend(),first.device_id.expose_to_backend());
    let canonical=confirm(&conn,&b,&other,&number).unwrap();assert_eq!(canonical.id,first.id);
    assert_eq!(reserve(&conn,&b,None,None).unwrap().id,first.id);
    let rows=device::list(&conn).unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].bound_account_id.as_deref(),Some(id.as_str()));
    assert_eq!(total(&conn,"jegotrip_device_aliases"),2);
    assert_eq!(device::set(&conn,None,0,None,Some(other.device_id.expose_to_backend()),102).unwrap_err().code,"app_login_device_identity_conflict");
}
#[test]
fn an_alias_returning_another_number_keeps_its_original_ownership_and_fails_closed() {
    let conn=memory();let alias=key("realm/email");let old=key("old-number");
    let used=reserve(&conn,&alias,None,None).unwrap();let used=confirm(&conn,&alias,&used,&old).unwrap();summary(&conn,&old);
    assert_eq!(confirm(&conn,&alias,&used,&key("new-number")).unwrap_err().code,"app_login_identity_mismatch");
    let row=device::list(&conn).unwrap().remove(0);assert_eq!(row.device_id,used.device_id.expose_to_backend());
    assert!(row.bound_account_id.is_some());assert_eq!(row.revision,used.revision);
}
#[test]
fn omitting_an_account_in_a_manual_update_never_clears_or_reassigns_number_ownership() {
    let conn=memory();let alias=key("alias-a");let account=key("number-a");
    let used=reserve(&conn,&alias,None,None).unwrap();let used=confirm(&conn,&alias,&used,&account).unwrap();let id=summary(&conn,&account);
    device::set(&conn,Some(&used.id),used.revision,None,Some("OWN-DEVICE-0001"),102).unwrap();
    let row=device::list(&conn).unwrap().remove(0);assert_eq!(row.bound_account_id.as_deref(),Some(id.as_str()));
    assert_eq!(row.revision,used.revision+1);assert_eq!(row.source,"manual");
    assert_eq!(reserve(&conn,&alias,None,None).unwrap().device_id.expose_to_backend(),"OWN-DEVICE-0001");
    assert_eq!(device::set(&conn,Some(&used.id),row.revision,Some(&key("number-b")),Some("OWN-DEVICE-0002"),103).unwrap_err().code,"app_login_device_identity_conflict");
    assert_eq!(device::list(&conn).unwrap().remove(0).device_id,"OWN-DEVICE-0001");
}
#[test]
fn a_prepared_manual_identity_can_be_assigned_from_sql_null_without_stealing_another_account() {
    let conn=memory();device::set(&conn,None,0,None,Some("PREPARED-0001"),100).unwrap();
    let prepared=device::list(&conn).unwrap().remove(0);assert!(!prepared.reserved);assert!(prepared.bound_account_id.is_none());
    let number=key("number");let id=summary(&conn,&number);
    device::set(&conn,Some(&prepared.id),prepared.revision,Some(&number),Some("PREPARED-0001"),101).unwrap();
    assert_eq!(device::list(&conn).unwrap().remove(0).bound_account_id.as_deref(),Some(id.as_str()));
    assert_eq!(device::set(&conn,None,0,Some(&key("foreign")),Some("prepared-0001"),102).unwrap_err().code,"app_login_device_identity_conflict");
}
#[test]
fn automatic_selection_does_not_consume_an_arbitrary_unassigned_manual_value() {
    let conn=memory();device::set(&conn,None,0,None,Some("PREPARED-0001"),100).unwrap();
    let automatic=reserve(&conn,&key("automatic"),None,None).unwrap();assert_ne!(automatic.device_id.expose_to_backend(),"PREPARED-0001");
    let selected=reserve(&conn,&key("explicit"),Some("PREPARED-0001"),None).unwrap();assert_eq!(selected.device_id.expose_to_backend(),"PREPARED-0001");
    assert_eq!(reserve(&conn,&key("another"),Some("PREPARED-0001"),None).unwrap_err().code,"app_login_device_identity_conflict");
}
#[test]
fn rotating_and_deleting_an_identity_retains_all_old_values_for_the_original_identity_only() {
    let conn=memory();let a=key("alias");let account=key("number");
    let used=reserve(&conn,&a,Some("OWN-OLD-0001"),None).unwrap();let used=confirm(&conn,&a,&used,&account).unwrap();summary(&conn,&account);
    device::set(&conn,Some(&used.id),used.revision,None,Some("OWN-NEW-0001"),102).unwrap();
    assert_eq!(device::set(&conn,None,0,None,Some("own-old-0001"),103).unwrap_err().code,"app_login_device_identity_conflict");
    let current=device::list(&conn).unwrap().remove(0);
    device::remove(&conn,&used.id,current.revision,104).unwrap();assert!(device::list(&conn).unwrap().is_empty());
    assert_eq!(device::set(&conn,None,0,None,Some("OWN-NEW-0001"),105).unwrap_err().code,"app_login_device_identity_conflict");
    let next=reserve(&conn,&a,None,None).unwrap();assert_ne!(next.id,used.id);
    assert_ne!(next.device_id.expose_to_backend(),"OWN-NEW-0001");
    assert_eq!(total(&conn,"jegotrip_device_identity_history"),3);
}
#[test]
fn stale_cas_update_and_delete_are_explicit_conflicts_with_no_partial_history_writes() {
    let conn=memory();device::set(&conn,None,0,None,Some("MANUAL-DEVICE-1"),100).unwrap();
    let original=device::list(&conn).unwrap().remove(0);
    assert_eq!(device::set(&conn,Some(&original.id),original.revision+1,None,Some("MANUAL-DEVICE-2"),101).unwrap_err().code,"app_login_device_identity_revision_conflict");
    assert_eq!(device::remove(&conn,&original.id,original.revision+1,102).unwrap_err().code,"app_login_device_identity_revision_conflict");
    assert_eq!(total(&conn,"jegotrip_device_identity_history"),1);assert_eq!(device::list(&conn).unwrap().remove(0).device_id,original.device_id);
    let error=device::set(&conn,None,1,None,Some("MANUAL-DEVICE-2"),103).unwrap_err();assert_eq!(error.code,"app_login_device_identity_revision_conflict");
    assert!(!format!("{error:?}").contains("MANUAL-DEVICE"));
}
#[test]
fn random_generation_has_honest_provenance_and_uuid_v4_bits() {
    let conn=memory();device::set(&conn,None,0,None,None,100).unwrap();
    let view=device::list(&conn).unwrap().remove(0);assert_eq!(view.source,"generated");
    assert_eq!(view.device_id.len(),36);assert_eq!(&view.device_id[14..15],"4");
    assert!(matches!(&view.device_id[19..20],"8"|"9"|"A"|"B"));
    assert!(view.device_id.chars().all(|c| c=='-' || c.is_ascii_digit() || ('A'..='F').contains(&c)));
    assert!(!format!("{view:?}").contains(&view.device_id));
}
#[test]
fn the_first_legacy_installation_id_is_preserved_but_never_shared_with_the_next_alias() {
    let conn=memory();let first=reserve(&conn,&key("first"),None,Some("LEGACY-DEVICE-1")).unwrap();
    assert_eq!(first.device_id.expose_to_backend(),"LEGACY-DEVICE-1");
    let second=reserve(&conn,&key("second"),None,Some("LEGACY-DEVICE-1")).unwrap();
    assert_ne!(second.device_id.expose_to_backend(),first.device_id.expose_to_backend());
    assert_eq!(device::list(&conn).unwrap().iter().filter(|v|v.source=="legacy").count(),1);
}
#[test]
fn static_or_case_variant_device_ids_do_not_bypass_number_exclusivity() {
    let conn=memory();let a=key("realm-a/login");let used=reserve(&conn,&a,Some("STATIC-DEVICE-1"),None).unwrap();
    let used=confirm(&conn,&a,&used,&key("number-a")).unwrap();
    assert_eq!(reserve(&conn,&a,Some("static-device-1"),None).unwrap().id,used.id);
    assert_eq!(reserve(&conn,&key("realm-b/login"),Some("STATIC-DEVICE-1"),None).unwrap_err().code,"app_login_device_identity_conflict");
    assert_eq!(total(&conn,"jegotrip_device_aliases"),1);
}
#[test]
fn an_unreleased_native_runtime_fence_blocks_device_rotation_and_retirement() {
    let conn=memory();let a=key("alias");let account=key("number");
    let used=reserve(&conn,&a,None,None).unwrap();let used=confirm(&conn,&a,&used,&account).unwrap();let id=summary(&conn,&account);
    let line="app-account-0123456789abcdef0123456789abcdef";
    accounts::link_voice_line(&conn,&id,line).unwrap();
    super::super::account_runs::initialize_schema(&conn).unwrap();
    conn.execute("INSERT INTO app_voice_runtime_leases(line_id,revision,run_nonce) VALUES(?1,1,?2)",rusqlite::params![line,"0123456789abcdef0123456789abcdef"]).unwrap();
    assert!(device::list(&conn).unwrap().remove(0).in_use);
    assert_eq!(device::set(&conn,Some(&used.id),used.revision,None,Some("OWN-REPLACEMENT-1"),102).unwrap_err().code,"app_login_device_identity_in_use");
    assert_eq!(device::remove(&conn,&used.id,used.revision,103).unwrap_err().code,"app_login_device_identity_in_use");
    // An empty lease table is not release if an original inbox run is unjoined.
    conn.execute("DELETE FROM app_voice_runtime_leases WHERE line_id=?1",[line]).unwrap();
    conn.execute("INSERT INTO app_voice_inbox_runs(line_id,revision,run_nonce,state) VALUES(?1,1,?2,'live')",rusqlite::params![line,"0123456789abcdef0123456789abcdef"]).unwrap();
    assert!(device::list(&conn).unwrap().remove(0).in_use);
    assert_eq!(device::remove(&conn,&used.id,used.revision,104).unwrap_err().code,"app_login_device_identity_in_use");
}
#[test]
fn allocation_and_retained_history_limits_fail_without_overwriting_owned_identities() {
    let conn=memory();for n in 0..device::MAX_IDENTITIES { reserve(&conn,&key(&format!("alias-{n}")),None,None).unwrap(); }
    assert_eq!(reserve(&conn,&key("over-limit"),None,None).unwrap_err().code,"app_login_device_identity_limit");
    assert_eq!(device::list(&conn).unwrap().len(),device::MAX_IDENTITIES);
    assert_eq!(total(&conn,"jegotrip_device_aliases"),device::MAX_IDENTITIES as i64);
    let conn=memory();device::set(&conn,None,0,None,Some("HISTORY-DEVICE-1"),100).unwrap();let view=device::list(&conn).unwrap().remove(0);
    for n in 1..512 {conn.execute("INSERT INTO jegotrip_device_identity_history(device_id,identity_id) VALUES(?1,?2)",rusqlite::params![format!("HISTORY-RESERVED-{n}"),view.id]).unwrap();}
    assert_eq!(device::set(&conn,Some(&view.id),view.revision,None,Some("NEW-HISTORY-DEVICE"),101).unwrap_err().code,"app_login_device_identity_limit");
    assert_eq!(device::list(&conn).unwrap().remove(0).device_id,view.device_id);
}
#[test]
fn alias_capacity_and_corrupt_stored_metadata_do_not_silently_truncate_or_expose_values() {
    let conn=memory();let first=reserve(&conn,&key("first"),None,None).unwrap();
    for n in 1..256 {conn.execute("INSERT INTO jegotrip_device_aliases(alias_key,identity_id) VALUES(?1,?2)",rusqlite::params![key(&format!("alias-{n}")),first.id]).unwrap();}
    assert_eq!(reserve(&conn,&key("overflow"),None,None).unwrap_err().code,"app_login_device_identity_limit");
    assert_eq!(total(&conn,"jegotrip_device_identities"),1);
    let number=key("confirmed");confirm(&conn,&key("first"),&first,&number).unwrap();let id=summary(&conn,&number);
    conn.execute("UPDATE jegotrip_accounts SET masked_number='private-number-that-must-not-escape' WHERE account_id=?1",[id]).unwrap();
    assert_eq!(device::list(&conn).unwrap_err(),"app_login_storage_failed");
}
#[test]
fn persisted_identity_survives_database_reopen_and_concurrent_alias_admission() {
    let path=std::env::temp_dir().join(format!("jegotrip-device-{}.sqlite",super::super::account_runtime::local_nonce().unwrap()));
    {
        let conn=Connection::open(&path).unwrap();accounts::initialize(&conn).unwrap();device::initialize(&conn).unwrap();
    }
    let barrier=std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles:Vec<_>=(0..2).map(|_| {
        let path=path.clone();let barrier=barrier.clone();
        std::thread::spawn(move || {
            let conn=Connection::open(path).unwrap();conn.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
            barrier.wait();let snapshot=reserve(&conn,&key("same-realm/same-alias"),None,None).unwrap();
            (snapshot.id,snapshot.device_id.expose_to_backend().to_owned())
        })
    }).collect();
    let mut values=handles.into_iter().map(|h|h.join().unwrap());let first=values.next().unwrap();assert_eq!(values.next().unwrap(),first);
    {
        let conn=Connection::open(&path).unwrap();let snapshot=reserve(&conn,&key("same-realm/same-alias"),None,None).unwrap();
        assert_eq!(snapshot.id,first.0);assert_eq!(snapshot.device_id.expose_to_backend(),first.1);assert_eq!(total(&conn,"jegotrip_device_identities"),1);
    }
    std::fs::remove_file(path).unwrap();
}
#[test]
fn an_older_linked_receiver_remains_fenced_after_a_newer_login_replaces_the_summary_link() {
    let conn=memory();super::store::initialize(&conn,"fixture-owner",100).unwrap();
    let a=key("alias");let account=key("number");let used=reserve(&conn,&a,None,None).unwrap();
    let used=confirm(&conn,&a,&used,&account).unwrap();let id=summary(&conn,&account);
    let older="app-account-00000000000000000000000000000001";let newer="app-account-00000000000000000000000000000002";
    for (attempt,line) in [("00000000000000000000000000000001",older),("00000000000000000000000000000002",newer)] {
        super::store::insert(&conn,attempt,"fixture-owner",100).unwrap();
        super::store::set_account(&conn,attempt,"fixture-owner",&id,100).unwrap();
        super::store::set_line(&conn,attempt,"fixture-owner",line,100).unwrap();
    }
    assert_eq!(accounts::list(&conn).unwrap()[0].voice_line_id.as_deref(),Some(newer));
    super::super::account_runs::initialize_schema(&conn).unwrap();
    conn.execute("INSERT INTO app_voice_runtime_leases(line_id,revision,run_nonce) VALUES(?1,1,?2)",rusqlite::params![older,"0123456789abcdef0123456789abcdef"]).unwrap();
    assert!(device::list(&conn).unwrap().remove(0).in_use);
    assert!(device::native_ownership_pending(&conn,&account).unwrap());
    assert_eq!(device::set(&conn,Some(&used.id),used.revision,None,Some("OWN-REPLACEMENT-1"),102).unwrap_err().code,"app_login_device_identity_in_use");
    assert_eq!(device::remove(&conn,&used.id,used.revision,102).unwrap_err().code,"app_login_device_identity_in_use");
}
#[test]
fn oversized_or_wrong_type_persisted_device_columns_are_rejected_before_owned_copy_or_reallocation() {
    let conn=memory();let a=key("alias");let used=reserve(&conn,&a,None,None).unwrap();
    conn.pragma_update(None,"ignore_check_constraints",true).unwrap();
    conn.execute("UPDATE jegotrip_device_identities SET device_id=?1 WHERE id=?2",rusqlite::params!["Q".repeat(1024*1024),used.id]).unwrap();
    assert_eq!(device::list(&conn).unwrap_err(),"app_login_storage_failed");
    assert_eq!(reserve(&conn,&a,None,None).unwrap_err().code,"app_login_storage_failed");
    conn.execute("UPDATE jegotrip_device_identities SET device_id=?1,account_key=?2 WHERE id=?3",
        rusqlite::params![used.device_id.expose_to_backend(),vec![7u8;1024*1024],used.id]).unwrap();
    assert_eq!(device::list(&conn).unwrap_err(),"app_login_storage_failed");
    conn.execute("UPDATE jegotrip_device_identities SET account_key=?1 WHERE id=?2",rusqlite::params!["x".repeat(32),used.id]).unwrap();
    assert_eq!(reserve(&conn,&a,None,None).unwrap_err().code,"app_login_storage_failed");
    assert_eq!(total(&conn,"jegotrip_device_identities"),1);assert_eq!(total(&conn,"jegotrip_device_identity_history"),1);
}
#[test]
fn corrupt_account_hints_and_account_attempt_line_ids_are_never_reflected_in_public_views() {
    let conn=memory();super::store::initialize(&conn,"fixture-owner",100).unwrap();
    let account=key("number");let id=summary(&conn,&account);
    conn.execute("UPDATE jegotrip_accounts SET masked_login_name='private-login@example.invalid' WHERE account_id=?1",[&id]).unwrap();
    assert_eq!(accounts::list(&conn).unwrap_err(),"app_login_storage_failed");
    conn.execute("UPDATE jegotrip_accounts SET masked_login_name='u***',voice_line_id=?1 WHERE account_id=?2",rusqlite::params!["private-token".repeat(10000),id]).unwrap();
    assert_eq!(accounts::list(&conn).unwrap_err(),"app_login_storage_failed");
    let attempt="0123456789abcdef0123456789abcdef";super::store::insert(&conn,attempt,"fixture-owner",100).unwrap();
    conn.execute("UPDATE jegotrip_login_attempts SET line_id=?1 WHERE attempt_id=?2",rusqlite::params!["private-token".repeat(10000),attempt]).unwrap();
    assert_eq!(super::store::get(&conn,attempt).unwrap_err(),"app_login_storage_failed");
    assert_eq!(super::store::list(&conn).unwrap_err(),"app_login_storage_failed");
}
#[test]
fn server_rejection_diagnostics_never_echo_reflected_device_ids_tokens_or_reason_fields() {
    let doc=super::crypto::JsonDoc(serde_json::json!({"code":506,"token":"PRIVATE-TOKEN","device_id":"PRIVATE-DEVICE",
        "message":"PRIVATE-TOKEN PRIVATE-DEVICE","body":{"password":"PRIVATE-PASSWORD"}}));
    let line=super::http::rejected_response_diagnostic("api/service/voip/v1/setVoip",&doc).unwrap();
    assert_eq!(line,"app_login_reject_fields endpoint=api/service/voip/v1/setVoip code=506");
    assert!(!line.contains("PRIVATE"));assert_eq!(super::crypto::success(&doc).unwrap_err().server_code,Some(506));
}
#[test]
fn device_trust_shape_logs_only_known_lengths_and_small_public_codes() {
    let body=serde_json::json!({"enrollId":"PRIVATE-ENROLL-ID","expireTime":"PRIVATE-EXPIRY","authentication_code":2,
        "token":"PRIVATE-TOKEN","device_id":"PRIVATE-DEVICE","PRIVATE-FIELD-KEY":"PRIVATE-FIELD-VALUE"});
    let line=super::http::device_trust_shape("enroll",body.as_object().unwrap());
    assert!(line.contains("authentication_code=num:2"));assert!(line.contains("enrollId=str:17"));assert!(!line.contains("PRIVATE"));
    let invalid=serde_json::json!({"authentication_code":"123456789012345"});
    assert_eq!(super::http::device_trust_shape("security_verify",invalid.as_object().unwrap()),
        "app_login_device_trust_shape step=security_verify authentication_code=unknown");
}
