//! Actions-only SQLite/原 scope 不变量；夹具不连接手机、服务或 Android SDK。
use super::*;
use super::super::{account_store, account_test_support, rpc_stream::{Writer, RpcProperty}, session::HostingSession, tls_engine::tests::Certificates};
use std::sync::Arc;
fn setup() -> (Arc<Database>, String, InboxScope) {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let database = Arc::new(Database::new(std::path::PathBuf::from(":memory:")).unwrap());
    let config = account_test_support::config(&cert.root);
    let line = format!("app-account-{}", super::super::account_runtime::local_nonce().unwrap());
    database.with_conn(|conn| account_store::create(conn, &line, &config, 100)).unwrap();
    let mut session = HostingSession::new(line.clone()); session.import(config.credentials, 100).unwrap();
    let owner = session.begin_login(100).unwrap(); let scope = RpcConnectionScope::fresh();
    let context = account_test_support::registered_inbox_scope(&database, &line, 1, &owner, &scope).with_receiver(super::super::rpc_incoming::IncomingBinding {
        object_id: b"#fixture-offline", domain: b"fixture-domain", receiver_kind: b"Sip.Request",
    }).unwrap();
    (database, line, context)
}
fn record(kind: &[u8], body: &[u8]) -> Vec<u8> {
    let mut writer = Writer::new(); writer.bytes(kind).unwrap(); writer.properties(&[
        RpcProperty { key: b"Sip.Message.From", value: b"fixture-sender" }, RpcProperty { key: b"Sip.Message.Time", value: b"1700000000000" },
        RpcProperty { key: b"unknown.private.property", value: &[0xff, 0, 7] },
    ]).unwrap(); writer.bytes(body).unwrap(); writer.finish().expose_to_transport().to_vec()
}
fn page(context: &InboxScope, user: &[u8], request: i64, records: &[(i64, &[u8])], position: i64) -> OwnedOfflinePage {
    let mut writer = Writer::new(); writer.boolean(true).unwrap(); writer.u32(records.len() as u32).unwrap();
    for (position, raw) in records { writer.i64(*position).unwrap(); writer.bytes(raw).unwrap(); }
    writer.i64(position).unwrap(); writer.bytes(b"").unwrap();
    OwnedOfflinePage::original(context.owner.clone(), context.scope.clone(), 17, request, user, b"fixture-selector", Some(b"fixture-entry"), writer.finish().expose_to_transport()).unwrap()
}
fn count(database: &Database, table: &str) -> i64 {
    database.with_conn(|conn| conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0)).map_err(|_| STORAGE)).unwrap()
}
#[test]
fn offline_raw_sms_event_and_position_commit_atomically_without_remote_ack_or_channels() {
    let (database, line, context) = setup(); let raw = record(b"Sip.Request", b"fixture-text");
    let value = page(&context, b"user-a", -1, &[(0x1_0000_0001, &raw)], 0x1_0000_0009);
    let proof = context.commit_offline(&database, &value).unwrap();
    assert!(proof.matches(&context.owner, &context.scope)); assert_eq!(proof.next_position(), 0x1_0000_0002); assert!(proof.more());
    assert_eq!(count(&database, "sms_messages"), 1); assert_eq!(count(&database, "app_voice_inbox"), 1);
    let saved: Vec<u8> = database.with_conn(|conn| conn.query_row("SELECT offline_envelope FROM app_voice_inbox WHERE line_id=?1", [&line], |row| row.get(0)).map_err(|_| STORAGE)).unwrap();
    assert_eq!(saved, raw);
    let metadata = database.with_conn(|conn| account_inbox::metadata(conn, &line, 10)).unwrap();
    assert_eq!(metadata[0].reply_state, "offline_committed"); assert_eq!(metadata[0].classification, "smgw_text");
    let json = serde_json::to_string(&metadata).unwrap(); for secret in ["fixture", "token", "position", "envelope", "identity"] { assert!(!json.contains(secret)); }
    assert_eq!(format!("{proof:?}"), "CommittedOfflineCursor(<redacted>)");
}
#[test]
fn cursor_write_failure_rolls_back_raw_sms_notifications_and_events_together() {
    let (database, _line, context) = setup(); let raw = record(b"Sip.Request", b"fixture-text");
    database.with_conn(|conn| conn.execute_batch("CREATE TRIGGER reject_offline_cursor BEFORE INSERT ON app_voice_offline_cursors BEGIN SELECT RAISE(ABORT,'fixture'); END;").map_err(|_| STORAGE)).unwrap();
    assert_eq!(context.commit_offline(&database, &page(&context, b"user-a", -1, &[(1, &raw)], 1)).err(), Some(STORAGE));
    for table in ["app_voice_inbox", "sms_messages", "app_voice_offline_cursors", "app_voice_notification_jobs", "app_voice_sms_projection"] { assert_eq!(count(&database, table), 0); }
}
#[test]
fn unsupported_json_stream_is_quarantined_without_cursor_sms_or_consumer_ack() {
    let (database, _line, context) = setup(); let json = b"\x02{\"body\":\"private\"}";
    let value = page(&context, b"user-a", -1, &[(1, json)], 1);
    for _ in 0..2 { assert_eq!(context.commit_offline(&database, &value).err(), Some("app_account_offline_stream_unsupported")); }
    assert_eq!(count(&database, "app_voice_offline_quarantine"), 1);
    assert_eq!(count(&database, "app_voice_offline_cursors"), 0); assert_eq!(count(&database, "sms_messages"), 0);
    let raw: Vec<u8> = database.with_conn(|conn| conn.query_row("SELECT envelope FROM app_voice_offline_quarantine", [], |row| row.get(0)).map_err(|_| STORAGE)).unwrap();
    assert_eq!(raw, value.raw());
}
#[test]
fn unknown_kind_is_retained_unclassified_instead_of_promoted_using_the_configured_kind() {
    let (database, line, context) = setup(); let raw = record(b"Other.Kind", b"fixture-text");
    context.commit_offline(&database, &page(&context, b"user-a", -1, &[(0, &raw)], 0)).unwrap();
    assert_eq!(count(&database, "app_voice_inbox"), 1); assert_eq!(count(&database, "sms_messages"), 0);
    assert_eq!(database.with_conn(|conn| account_inbox::metadata(conn, &line, 10)).unwrap()[0].classification, "unclassified");
}
#[test]
fn committed_positions_are_not_reprojected_after_reconnect_or_user_sms_deletion() {
    let (database, line, context) = setup(); let raw = record(b"Sip.Request", b"fixture-text");
    context.commit_offline(&database, &page(&context, b"user-a", -1, &[(1, &raw)], 1)).unwrap();
    database.with_conn(|conn| conn.execute("DELETE FROM sms_messages WHERE transport='app_voice' AND line_id=?1", [&line]).map(|_| ()).map_err(|_| STORAGE)).unwrap();
    let wiped: (Vec<u8>, Option<Vec<u8>>) = database.with_conn(|conn| conn.query_row("SELECT body,offline_envelope FROM app_voice_inbox", [], |row| Ok((row.get(0)?, row.get(1)?))).map_err(|_| STORAGE)).unwrap();
    assert!(wiped.0.is_empty()); assert!(wiped.1.is_none());
    let owner = context.owner.clone(); let scope = RpcConnectionScope::fresh();
    let reconnect = account_test_support::registered_inbox_scope(&database, &line, 1, &owner, &scope).with_receiver(super::super::rpc_incoming::IncomingBinding {
        object_id: b"#fixture-offline", domain: b"fixture-domain", receiver_kind: b"Sip.Request",
    }).unwrap();
    let proof = reconnect.commit_offline(&database, &page(&reconnect, b"user-a", -1, &[(1, &raw)], 1)).unwrap();
    assert_eq!(proof.next_position(), 2); assert_eq!(count(&database, "sms_messages"), 0);
    assert_eq!(count(&database, "app_voice_inbox"), 0); assert_eq!(count(&database, "app_voice_offline_cursors"), 1);
}
#[test]
fn another_native_principal_does_not_borrow_the_old_cursor_even_on_the_same_line_revision() {
    let (database, _line, context) = setup(); let raw = record(b"Sip.Request", b"fixture-text");
    context.commit_offline(&database, &page(&context, b"user-a", -1, &[(1, &raw)], 1)).unwrap();
    context.commit_offline(&database, &page(&context, b"user-b", -1, &[(1, &raw)], 1)).unwrap();
    assert_eq!(count(&database, "app_voice_offline_cursors"), 2); assert_eq!(count(&database, "sms_messages"), 2);
}
#[test]
fn stale_scope_revision_and_live_run_fence_are_checked_before_any_new_consumption() {
    let (database, line, context) = setup(); let raw = record(b"Sip.Request", b"fixture-text");
    let value = page(&context, b"user-a", -1, &[(1, &raw)], 1);
    context.scope.close(); assert_eq!(context.commit_offline(&database, &value).err(), Some(SCOPE));
    assert_eq!(count(&database, "sms_messages"), 0);
    let scope = RpcConnectionScope::fresh(); let owner = context.owner.clone();
    let replacement = account_test_support::registered_inbox_scope(&database, &line, 1, &owner, &scope).with_receiver(super::super::rpc_incoming::IncomingBinding {
        object_id: b"#fixture-offline", domain: b"fixture-domain", receiver_kind: b"Sip.Request",
    }).unwrap();
    let value = page(&replacement, b"user-a", -1, &[(1, &raw)], 1);
    database.with_conn(|conn| conn.execute("UPDATE app_voice_accounts SET revision=2 WHERE line_id=?1", [&line]).map(|_| ()).map_err(|_| STORAGE)).unwrap();
    assert_eq!(replacement.commit_offline(&database, &value).err(), Some(SCOPE)); assert_eq!(count(&database, "app_voice_offline_cursors"), 0);
}
#[test]
fn full_unknown_inbox_rejects_whole_page_without_eviction_or_partial_cursor() {
    let (database, _line, context) = setup();
    let unknown = IncomingMessage { body: vec![0xff], from: None, time: None };
    for id in 1..=128 { context.commit_fixture(&database, id, &unknown).unwrap(); }
    let raw = record(b"Sip.Request", b"fixture-text");
    assert_eq!(context.commit_offline(&database, &page(&context, b"user-a", -1, &[(1, &raw)], 1)).err(), Some(FULL));
    assert_eq!(count(&database, "app_voice_inbox"), 128); assert_eq!(count(&database, "app_voice_offline_cursors"), 0); assert_eq!(count(&database, "sms_messages"), 0);
}
#[test]
fn empty_page_advances_only_the_matched_server_position_and_regression_is_not_reset_silently() {
    let (database, _line, context) = setup();
    let proof = context.commit_offline(&database, &page(&context, b"user-a", -1, &[], 0x1_0000_0000)).unwrap();
    assert_eq!(proof.next_position(), 0x1_0000_0001); assert!(!proof.more());
    assert_eq!(context.commit_offline(&database, &page(&context, b"user-a", -1, &[], 1)).err(), Some("app_account_offline_cursor_conflict"));
    assert_eq!(count(&database, "sms_messages"), 0);
}
#[test]
fn old_check_schema_migration_preserves_original_bytes_ids_and_autoincrement_frontier() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE app_voice_inbox(id INTEGER PRIMARY KEY AUTOINCREMENT,line_id TEXT NOT NULL,revision INTEGER NOT NULL,
        run_nonce TEXT NOT NULL,source_kind TEXT NOT NULL CHECK(source_kind IN ('direct','batch')),peer_epoch INTEGER NOT NULL,sequence INTEGER NOT NULL,
        fingerprint TEXT NOT NULL,body BLOB NOT NULL,sender BLOB,opaque_time BLOB,reply_state TEXT NOT NULL DEFAULT 'pending'
        CHECK(reply_state IN ('pending','direct_submitted','batch_queued')),created_at TEXT NOT NULL);
        INSERT INTO app_voice_inbox(id,line_id,revision,run_nonce,source_kind,peer_epoch,sequence,fingerprint,body,created_at)
        VALUES(17,'fixture-line',1,'fixture-run','direct',0,19,'fixture-digest',X'00FF07','fixture-stamp');
        UPDATE sqlite_sequence SET seq=200 WHERE name='app_voice_inbox';").unwrap();
    account_inbox::initialize_schema(&conn).unwrap();
    let saved: (i64, Vec<u8>, i64) = conn.query_row("SELECT id,body,sequence FROM app_voice_inbox", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
    assert_eq!(saved, (17, vec![0, 0xff, 7], 19));
    let frontier: i64 = conn.query_row("SELECT seq FROM sqlite_sequence WHERE name='app_voice_inbox'", [], |row| row.get(0)).unwrap(); assert_eq!(frontier, 200);
    conn.execute("INSERT INTO app_voice_inbox(line_id,revision,run_nonce,source_kind,peer_epoch,sequence,fingerprint,body,reply_state,created_at)
        VALUES('fixture-line',1,'fixture-new','offline',0,4294967297,'fixture-digest',X'00','offline_committed','fixture-stamp')", []).unwrap();
    assert_eq!(conn.last_insert_rowid(), 201);
    account_inbox::initialize_schema(&conn).unwrap();
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM app_voice_inbox", [], |row| row.get::<_, i64>(0)).unwrap(), 2);
}
