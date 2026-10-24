//! Actions-only private inbox/SQLite invariants. These data tests do not
//! authenticate a peer, classify SMS, produce fake channels or run local peers.
use super::*;
use super::super::{account_runtime::{local_nonce, unix_now}, account_test_support::{config, TempDatabase},
    session::HostingSession, tls_engine::tests::Certificates};
use std::sync::Arc;

fn setup() -> (Arc<Database>, String, InboxScope) {
    let cert=Certificates::generate("DNS:fixture.invalid");
    let database=Arc::new(Database::new(std::path::PathBuf::from(":memory:")).unwrap());
    let imported=config(&cert.root); let line=format!("app-account-{}",local_nonce().unwrap());
    database.with_conn(|conn|account_store::create(conn,&line,&imported,100)).unwrap();
    let mut session=HostingSession::new(line.clone());session.import(imported.credentials,100).unwrap();
    let owner=session.begin_login(100).unwrap();let scope=RpcConnectionScope::fresh();
    let context=InboxScope::original(&line,1,&local_nonce().unwrap(),&owner,&scope).unwrap();
    (database,line,context)
}
fn message()->IncomingMessage {
    IncomingMessage{body:vec![0xff,0,3,0xfe],from:Some(vec![0,0xff,2]),time:Some(b"fixture-opaque-time".to_vec())}
}
fn count(database:&Database,table:&str)->i64 {
    database.with_conn(|conn|conn.query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|row|row.get(0)).map_err(|_|STORAGE)).unwrap()
}

#[test]
fn generic_bytes_and_redacted_event_commit_atomically_without_sms_or_readiness() {
    let (database,line,context)=setup();let mut broadcast=database.subscribe_app_events();
    let receipt=context.commit(&database,"direct",0,1,&message()).unwrap();
    assert_eq!(count(&database,"app_voice_inbox"),1);assert_eq!(count(&database,"sms_messages"),0);
    let private=private_message(&database,&line,receipt.id).unwrap();
    assert_eq!(private.body.as_slice(),[0xff,0,3,0xfe]);assert_eq!(private.sender.as_deref().map(|b|b.as_slice()),Some([0,0xff,2].as_slice()));
    assert_eq!(private.time.as_deref().map(|b|b.as_slice()),Some(b"fixture-opaque-time".as_slice()));
    let event=broadcast.try_recv().unwrap();assert_eq!(event.event_type,"app_voice.inbox_stored");
    assert_eq!(event.line_id.as_deref(),Some(line.as_str()));assert_eq!(event.transport.as_deref(),Some(super::super::PROVIDER_ID));
    assert_eq!(serde_json::from_str::<serde_json::Value>(&event.payload_json).unwrap(),serde_json::json!({"inbox_id":receipt.id,"classification":"unclassified"}));
    // Broadcast cannot precede the row's durable transaction.
    assert!(private_message(&database,&line,receipt.id).is_ok());
    let public=database.with_conn(|conn|metadata(conn,&line,50)).unwrap();
    assert_eq!(public[0].classification,"unclassified");assert_eq!(public[0].reply_state,"pending");
    let json=serde_json::to_string(&public).unwrap();
    for forbidden in ["fixture", "sender", "opaque_time", "nonce", "epoch", "sequence", "fingerprint", "body"] {assert!(!json.contains(forbidden));}
    assert_eq!(format!("{context:?}"),"InboxScope(<redacted>)");assert_eq!(format!("{receipt:?}"),"CommittedInbox(<redacted>)");
}

#[test]
fn original_receiver_binding_is_preserved_when_newer_configuration_replaces_it() {
    let (database,line,context)=setup();
    let context=context.with_receiver(super::super::rpc_incoming::IncomingBinding {
        object_id:b"#original-object",domain:b"original-domain",receiver_kind:b"original.kind"
    }).unwrap();
    let receipt=context.commit(&database,"direct",0,1,&message()).unwrap();
    database.with_conn(|conn|conn.execute("UPDATE app_voice_accounts SET revision=2 WHERE line_id=?1",[&line]).map(|_|()).map_err(|_|STORAGE)).unwrap();
    let values:(Vec<u8>,Vec<u8>,Vec<u8>)=database.with_conn(|conn|conn.query_row(
        "SELECT receiver_object,receiver_domain,receiver_kind FROM app_voice_inbox WHERE id=?1 AND line_id=?2",
        params![receipt.id,&line],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(|_|STORAGE)).unwrap();
    assert_eq!(values.0,b"#original-object");assert_eq!(values.1,b"original-domain");assert_eq!(values.2,b"original.kind");
    assert!(context.commit(&database,"direct",0,2,&message()).is_err());
    let json=serde_json::to_string(&database.with_conn(|conn|metadata(conn,&line,50)).unwrap()).unwrap();
    assert!(!json.contains("original"));assert!(!json.contains("receiver"));assert!(json.contains("unclassified"));
}

#[test]
fn changed_receiver_context_is_a_conflict_not_an_equivalent_duplicate() {
    let (database,line,context)=setup();let nonce=context.nonce.clone();let owner=context.owner.clone();let scope=context.scope.clone();
    let context=context.with_receiver(super::super::rpc_incoming::IncomingBinding {
        object_id:b"#original-object",domain:b"original-domain",receiver_kind:b"original.kind"
    }).unwrap();
    context.commit(&database,"direct",0,1,&message()).unwrap();
    let changed=InboxScope::original(&line,1,&nonce,&owner,&scope).unwrap().with_receiver(super::super::rpc_incoming::IncomingBinding {
        object_id:b"#original-object",domain:b"original-domain",receiver_kind:b"different.kind"
    }).unwrap();
    assert_eq!(changed.commit(&database,"direct",0,1,&message()).err(),Some("app_account_inbox_duplicate_conflict"));
    assert_eq!(count(&database,"app_voice_inbox"),1);assert_eq!(count(&database,"app_events"),1);
}

#[test]
fn legacy_inbox_binding_columns_stay_unknown_instead_of_using_current_profile() {
    let conn=Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE app_voice_inbox(
        id INTEGER PRIMARY KEY AUTOINCREMENT,line_id TEXT NOT NULL,revision INTEGER NOT NULL,
        run_nonce TEXT NOT NULL,source_kind TEXT NOT NULL,peer_epoch INTEGER NOT NULL,
        sequence INTEGER NOT NULL,fingerprint TEXT NOT NULL,body BLOB NOT NULL,sender BLOB,opaque_time BLOB,
        reply_state TEXT NOT NULL DEFAULT 'pending',created_at TEXT NOT NULL,
        UNIQUE(line_id,revision,run_nonce,source_kind,peer_epoch,sequence));
        INSERT INTO app_voice_inbox(line_id,revision,run_nonce,source_kind,peer_epoch,sequence,fingerprint,body,created_at)
        VALUES('old-local-line',1,'old-run','direct',0,1,'old-fingerprint',X'FF0003','old-time');").unwrap();
    initialize_schema(&conn).unwrap();initialize_schema(&conn).unwrap();
    let row:(Vec<u8>,bool)=conn.query_row("SELECT body,receiver_object IS NULL AND receiver_domain IS NULL AND receiver_kind IS NULL FROM app_voice_inbox WHERE id=1",
        [],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
    assert_eq!(row.0,[0xff,0,3]);assert!(row.1);
}

#[test]
fn equal_original_delivery_is_idempotent_and_does_not_emit_an_extra_event() {
    let (database,_,context)=setup();let first=context.commit(&database,"direct",0,7,&message()).unwrap();
    let duplicate=context.commit(&database,"direct",0,7,&message()).unwrap();assert_eq!(first.id,duplicate.id);
    assert_eq!(count(&database,"app_voice_inbox"),1);assert_eq!(count(&database,"app_events"),1);
    let mut changed=message();changed.body.push(9);
    assert_eq!(context.commit(&database,"direct",0,7,&changed).err(),Some("app_account_inbox_duplicate_conflict"));
    assert_eq!(count(&database,"app_voice_inbox"),1);assert_eq!(count(&database,"app_events"),1);
}

#[test]
fn absent_and_empty_binary_sender_or_time_are_distinct_not_lossy_text() {
    let (database,_,context)=setup();let mut absent=message();absent.from=None;absent.time=None;
    context.commit(&database,"direct",0,1,&absent).unwrap();
    let mut empty=message();empty.from=Some(vec![]);empty.time=Some(vec![]);
    assert_ne!(fingerprint(&absent),fingerprint(&empty));
    assert_eq!(context.commit(&database,"direct",0,1,&empty).err(),Some("app_account_inbox_duplicate_conflict"));
    assert!(context.commit(&database,"direct",0,2,&empty).is_ok());
    assert_eq!(count(&database,"app_voice_inbox"),2);
}

#[test]
fn same_sequence_on_another_run_kind_or_actual_batch_peer_epoch_is_not_a_remote_uuid() {
    let (database,line,context)=setup();let body=message();
    context.commit(&database,"direct",0,1,&body).unwrap();
    context.commit(&database,"batch",9,1,&body).unwrap();context.commit(&database,"batch",10,1,&body).unwrap();
    let next=InboxScope::original(&line,1,&local_nonce().unwrap(),&context.owner,&context.scope).unwrap();
    next.commit(&database,"direct",0,1,&body).unwrap();assert_eq!(count(&database,"app_voice_inbox"),4);
    let mut session=HostingSession::new("line-wrong".into());let mut imported=config(&Certificates::generate("DNS:fixture.invalid").root);
    imported.credentials.sdk.expires_at_unix_seconds=None;session.import(imported.credentials,unix_now()).unwrap();let wrong=session.begin_login(unix_now()).unwrap();
    assert_eq!(InboxScope::original(&line,1,&context.nonce,&wrong,&context.scope).err(),Some(SCOPE));
    assert_eq!(count(&database,"app_voice_inbox"),4);
}

#[test]
fn failed_event_insert_rolls_back_private_bytes_and_broadcast_without_consumer_ack() {
    let (database,_,context)=setup();let mut broadcast=database.subscribe_app_events();
    database.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_generic_event BEFORE INSERT ON app_events
        WHEN NEW.event_type='app_voice.inbox_stored' BEGIN SELECT RAISE(ABORT,'fixture-private-do-not-echo');END;").map_err(|_|STORAGE)).unwrap();
    assert_eq!(context.commit(&database,"direct",0,1,&message()).err(),Some(STORAGE));
    assert_eq!(count(&database,"app_voice_inbox"),0);assert_eq!(count(&database,"app_events"),0);assert!(broadcast.try_recv().is_err());
}

#[test]
fn changed_configuration_and_closed_physical_scope_cannot_stage_new_or_mark_old_replies() {
    let (database,line,context)=setup();let committed=context.commit(&database,"direct",0,1,&message()).unwrap();
    database.with_conn(|conn|conn.execute("UPDATE app_voice_accounts SET revision=2 WHERE line_id=?1",[&line]).map(|_|()).map_err(|_|STORAGE)).unwrap();
    assert_eq!(context.commit(&database,"direct",0,2,&message()).err(),Some(SCOPE));
    assert_eq!(committed.record_reply(&database,"direct_submitted"),Err(SCOPE));
    assert_eq!(count(&database,"app_voice_inbox"),1);
    context.scope.close();assert_eq!(context.commit(&database,"direct",0,3,&message()).err(),Some(SCOPE));
    assert_eq!(InboxScope::original(&line,2,&context.nonce,&context.owner,&context.scope).err(),Some(SCOPE));
}

#[test]
fn unknown_committed_bytes_are_not_evicted_at_per_account_capacity() {
    let (database,line,context)=setup();for sequence in 1..=MAX_PER_ACCOUNT as i32 {context.commit(&database,"direct",0,sequence,&message()).unwrap();}
    assert_eq!(context.commit(&database,"direct",0,129,&message()).err(),Some(FULL));
    assert_eq!(count(&database,"app_voice_inbox"),MAX_PER_ACCOUNT);assert_eq!(count(&database,"app_events"),MAX_PER_ACCOUNT);
    assert!(context.commit(&database,"direct",0,1,&message()).is_ok()); // exact retry remains admissible at cap
    let rows=database.with_conn(|conn|metadata(conn,&line,usize::MAX)).unwrap();assert_eq!(rows.len(),128);
}

#[test]
fn shared_payload_byte_quota_keeps_previous_accounts_and_events_unchanged() {
    let (database,line,context)=setup();let large=IncomingMessage{body:vec![0xff;MAX_FIELD_BYTES],from:None,time:None};
    for sequence in 1..=128 {context.commit(&database,"direct",0,sequence,&large).unwrap();}
    let cert=Certificates::generate("DNS:fixture.invalid");let imported=config(&cert.root);let other=format!("app-account-{}",local_nonce().unwrap());
    database.with_conn(|conn|account_store::create(conn,&other,&imported,100)).unwrap();
    let mut session=HostingSession::new(other.clone());session.import(imported.credentials,100).unwrap();let owner=session.begin_login(100).unwrap();
    let other_scope=InboxScope::original(&other,1,&local_nonce().unwrap(),&owner,&RpcConnectionScope::fresh()).unwrap();
    for sequence in 1..=128 {other_scope.commit(&database,"direct",0,sequence,&large).unwrap();}
    let third=format!("app-account-{}",local_nonce().unwrap());let imported=config(&cert.root);
    database.with_conn(|conn|account_store::create(conn,&third,&imported,100)).unwrap();let mut session=HostingSession::new(third.clone());
    session.import(imported.credentials,100).unwrap();let owner=session.begin_login(100).unwrap();
    let third_scope=InboxScope::original(&third,1,&local_nonce().unwrap(),&owner,&RpcConnectionScope::fresh()).unwrap();
    assert_eq!(third_scope.commit(&database,"direct",0,1,&message()).err(),Some(FULL));
    assert_eq!(count(&database,"app_voice_inbox"),256);assert_eq!(count(&database,"app_events"),256);
    assert_eq!(database.with_conn(|conn|metadata(conn,&line,128)).unwrap().len(),128);
}

#[test]
fn total_row_quota_does_not_evict_another_accounts_unknown_deliveries() {
    let (database,_,first)=setup();let cert=Certificates::generate("DNS:fixture.invalid");let bytes=message();
    for index in 0..16 {
        if index==0 {for sequence in 1..=128 {first.commit(&database,"direct",0,sequence,&bytes).unwrap();}continue;}
        let imported=config(&cert.root);let line=format!("app-account-{}",local_nonce().unwrap());
        database.with_conn(|conn|account_store::create(conn,&line,&imported,100)).unwrap();
        let mut session=HostingSession::new(line.clone());session.import(imported.credentials,100).unwrap();let owner=session.begin_login(100).unwrap();
        let context=InboxScope::original(&line,1,&local_nonce().unwrap(),&owner,&RpcConnectionScope::fresh()).unwrap();
        for sequence in 1..=128 {context.commit(&database,"direct",0,sequence,&bytes).unwrap();}
    }
    let imported=config(&cert.root);let line=format!("app-account-{}",local_nonce().unwrap());database.with_conn(|conn|account_store::create(conn,&line,&imported,100)).unwrap();
    let mut session=HostingSession::new(line.clone());session.import(imported.credentials,100).unwrap();let owner=session.begin_login(100).unwrap();
    let extra=InboxScope::original(&line,1,&local_nonce().unwrap(),&owner,&RpcConnectionScope::fresh()).unwrap();
    assert_eq!(extra.commit(&database,"direct",0,1,&bytes).err(),Some(FULL));
    assert_eq!(count(&database,"app_voice_inbox"),MAX_TOTAL);assert_eq!(count(&database,"app_events"),MAX_TOTAL);
    assert!(first.commit(&database,"direct",0,1,&bytes).is_ok());
}

#[test]
fn message_field_and_sequence_bounds_fail_before_partial_inbox_or_event_writes() {
    let (database,_,context)=setup();for index in 0..3 {
        let mut invalid=message();match index {0=>invalid.body=vec![0;MAX_FIELD_BYTES+1],1=>invalid.from=Some(vec![0;MAX_FIELD_BYTES+1]),_=>invalid.time=Some(vec![0;MAX_FIELD_BYTES+1])}
        assert_eq!(context.commit(&database,"direct",0,1,&invalid).err(),Some(INVALID));
    }
    for sequence in [0,-1] {assert_eq!(context.commit(&database,"direct",0,sequence,&message()).err(),Some(SCOPE));}
    assert_eq!(count(&database,"app_voice_inbox"),0);assert_eq!(count(&database,"app_events"),0);
}

#[test]
fn direct_submission_and_batch_queued_markers_never_claim_peer_acknowledgement() {
    let (database,line,context)=setup();let direct=context.commit(&database,"direct",0,1,&message()).unwrap();
    let batch=context.commit(&database,"batch",9,1,&message()).unwrap();
    assert_eq!(direct.record_reply(&database,"batch_queued"),Err(INVALID));assert_eq!(batch.record_reply(&database,"direct_submitted"),Err(INVALID));
    direct.record_reply(&database,"direct_submitted").unwrap();batch.record_reply(&database,"batch_queued").unwrap();
    let rows=database.with_conn(|conn|metadata(conn,&line,50)).unwrap();assert_eq!(rows[0].reply_state,"batch_queued");assert_eq!(rows[1].reply_state,"direct_submitted");
    let json=serde_json::to_string(&rows).unwrap();assert!(!json.contains("confirmed"));assert!(!json.contains("sms.received"));
}

#[test]
fn failed_reply_marker_does_not_undo_durable_payload_or_reclassify_as_sms() {
    let (database,line,context)=setup();let receipt=context.commit(&database,"direct",0,1,&message()).unwrap();
    database.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_reply_marker BEFORE UPDATE ON app_voice_inbox BEGIN SELECT RAISE(ABORT,'fixture-private-marker');END;").map_err(|_|STORAGE)).unwrap();
    assert_eq!(receipt.record_reply(&database,"direct_submitted"),Err(STORAGE));
    assert_eq!(count(&database,"app_voice_inbox"),1);assert_eq!(count(&database,"app_events"),1);assert_eq!(count(&database,"sms_messages"),0);
    assert!(private_message(&database,&line,receipt.id).is_ok());assert_eq!(database.with_conn(|conn|metadata(conn,&line,50)).unwrap()[0].reply_state,"pending");
}

#[test]
fn account_delete_purges_only_its_private_inbox_after_cas_and_reopen_preserves_binary_data() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let database=temp.database.clone();let imported=config(&cert.root);
    let line=format!("app-account-{}",local_nonce().unwrap());database.with_conn(|conn|account_store::create(conn,&line,&imported,100)).unwrap();
    let mut session=HostingSession::new(line.clone());session.import(imported.credentials,100).unwrap();let owner=session.begin_login(100).unwrap();
    let context=InboxScope::original(&line,1,&local_nonce().unwrap(),&owner,&RpcConnectionScope::fresh()).unwrap();
    let receipt=context.commit(&database,"direct",0,1,&message()).unwrap();
    let reopened=Database::new(temp.path.clone()).unwrap();let loaded=private_message(&reopened,&line,receipt.id).unwrap();assert_eq!(loaded.body.as_slice(),message().body.as_slice());
    assert_eq!(database.with_conn(|conn|account_store::delete(conn,&line,2)).err(),Some("app_account_revision_conflict"));
    assert_eq!(count(&database,"app_voice_inbox"),1);
    database.with_conn(|conn|account_store::delete(conn,&line,1)).unwrap();assert_eq!(count(&database,"app_voice_inbox"),0);
    assert_eq!(count(&database,"app_events"),1); // redacted durable historical event, not raw data
}
