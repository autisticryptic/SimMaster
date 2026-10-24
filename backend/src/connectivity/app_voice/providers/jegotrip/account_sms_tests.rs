//! Actions-only shared SMS/inbox transaction tests. The fixture helper invokes
//! the real inbox transaction but is cfg(test); no network or live SMS origin.
use super::*;
use super::super::{account_inbox::{self,InboxScope}, account_runtime::local_nonce,
    account_test_support::config, native_events::IncomingMessage, rpc_incoming::IncomingBinding,
    rpc_pending::RpcConnectionScope, session::HostingSession, smgw_text::SMGW_RECEIVER,
    tls_engine::tests::Certificates};
use crate::platform::db::Database;

fn setup()->(Database,String,InboxScope) {
    let cert=Certificates::generate("DNS:fixture.invalid");let db=Database::new(std::path::PathBuf::from(":memory:")).unwrap();
    let imported=config(&cert.root);let line=format!("app-account-{}",local_nonce().unwrap());
    db.with_conn(|conn|account_store::create(conn,&line,&imported,100)).unwrap();
    let mut session=HostingSession::new(line.clone());session.import(imported.credentials,100).unwrap();let owner=session.begin_login(100).unwrap();
    let context=super::super::account_test_support::registered_inbox_scope(&db,&line,1,&owner,&RpcConnectionScope::fresh())
        .with_receiver(IncomingBinding {object_id:b"#fixture-client",domain:b"fixture-domain",receiver_kind:SMGW_RECEIVER}).unwrap();
    (db,line,context)
}
fn text()->IncomingMessage {
    IncomingMessage {body:"验证码 fixture 🥘\nsecond line".as_bytes().to_vec(),from:Some(b"fixture-sender".to_vec()),time:Some(b"1700000000123".to_vec())}
}
fn count(db:&Database,table:&str)->i64 {
    db.with_conn(|conn|conn.query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|row|row.get(0)).map_err(|_|STORAGE)).unwrap()
}

#[test]
fn recovered_text_projection_is_original_account_scoped_and_has_no_fake_pdu() {
    let (db,line,context)=setup();let inbox_id=context.commit_fixture(&db,1,&text()).unwrap();
    let rows=db.with_conn(|conn|list(conn,&line,50)).unwrap();assert_eq!(rows.len(),1);let row=&rows[0];
    assert_eq!(row.classification,"smgw_text");assert_eq!(row.timestamp_source,"sender");assert_eq!(row.sent_at_unix_millis,Some(1_700_000_000_123));
    assert_eq!(row.message.content,"验证码 fixture 🥘\nsecond line");assert_eq!(row.message.phone_number,"fixture-sender");
    assert_eq!(row.message.timestamp,"2023-11-14T22:13:20.123Z");assert_eq!(row.message.line_id.as_deref(),Some(line.as_str()));
    assert_eq!(row.message.transport,super::super::PROVIDER_ID);assert!(row.message.pdu.is_none());assert_eq!(row.message.direction,"incoming");
    let shared=db.get_sms_messages_for_channel(50,0,None,Some(&line)).unwrap();assert_eq!(shared.len(),1);assert_eq!(shared[0].id,row.message.id);
    assert!(db.get_sms_messages_for_channel(50,0,None,Some("another-line")).unwrap().is_empty());
    let raw=db.with_conn(|conn|account_inbox::metadata(conn,&line,50)).unwrap();assert_eq!(raw[0].inbox_id,inbox_id);assert_eq!(raw[0].classification,"smgw_text");
    assert_eq!(format!("{row:?}"),"AccountSmsMessage(<redacted>)");
}
#[test]
fn raw_and_sms_events_commit_together_and_neither_contains_private_content() {
    let (db,line,context)=setup();let mut receiver=db.subscribe_app_events();context.commit_fixture(&db,1,&text()).unwrap();
    let one=receiver.try_recv().unwrap();let two=receiver.try_recv().unwrap();assert!(receiver.try_recv().is_err());
    assert_eq!(one.event_type,"app_voice.inbox_stored");assert_eq!(two.event_type,"sms.received");assert!(one.id<two.id);
    for event in [one,two] {
        assert_eq!(event.line_id.as_deref(),Some(line.as_str()));assert_eq!(event.transport.as_deref(),Some("app_voice"));
        for secret in ["验证码", "fixture-sender", "root_der", "phone_login_token", "#fixture-client"] {assert!(!event.payload_json.contains(secret));}
        assert_eq!(count(&db,"sms_messages"),1);assert_eq!(count(&db,"app_voice_inbox"),1);
    }
}
#[test]
fn exact_duplicate_does_not_recreate_sms_projection_or_publish_an_extra_event() {
    let (db,line,context)=setup();let id=context.commit_fixture(&db,7,&text()).unwrap();
    let sms=db.with_conn(|conn|list(conn,&line,50)).unwrap()[0].message.id;
    assert_eq!(context.commit_fixture(&db,7,&text()).unwrap(),id);assert_eq!(count(&db,"sms_messages"),1);assert_eq!(count(&db,"app_voice_sms_projection"),1);assert_eq!(count(&db,"app_events"),2);
    assert_eq!(db.with_conn(|conn|list(conn,&line,50)).unwrap()[0].message.id,sms);
}
#[test]
fn unsupported_kind_sender_utf8_or_time_stays_only_in_private_generic_inbox() {
    let (db,line,_)=setup();let cert=Certificates::generate("DNS:fixture.invalid");let imported=config(&cert.root);
    let mut session=HostingSession::new(line.clone());session.import(imported.credentials,100).unwrap();let owner=session.begin_login(100).unwrap();
    let unknown=super::super::account_test_support::registered_inbox_scope(&db,&line,1,&owner,&RpcConnectionScope::fresh())
        .with_receiver(IncomingBinding{object_id:b"#fixture-client",domain:b"fixture-domain",receiver_kind:b"SomeOther.Kind"}).unwrap();
    unknown.commit_fixture(&db,1,&text()).unwrap();assert_eq!(count(&db,"sms_messages"),0);
    let known=super::super::account_test_support::registered_inbox_scope(&db,&line,1,&owner,&RpcConnectionScope::fresh())
        .with_receiver(IncomingBinding{object_id:b"#fixture-client",domain:b"fixture-domain",receiver_kind:SMGW_RECEIVER}).unwrap();
    let mut invalid=text();invalid.body=vec![0xff];known.commit_fixture(&db,1,&invalid).unwrap();
    let mut invalid=text();invalid.from=None;known.commit_fixture(&db,2,&invalid).unwrap();
    let mut invalid=text();invalid.time=Some(b"not-a-time".to_vec());known.commit_fixture(&db,3,&invalid).unwrap();
    assert_eq!(count(&db,"app_voice_inbox"),4);assert_eq!(count(&db,"app_events"),4);assert_eq!(count(&db,"sms_messages"),0);
    assert!(db.with_conn(|conn|list(conn,&line,50)).unwrap().is_empty());
}
#[test]
fn absent_sent_time_uses_recorded_receive_timestamp_with_an_explicit_source() {
    let (db,line,context)=setup();let mut message=text();message.time=None;let id=context.commit_fixture(&db,1,&message).unwrap();
    let raw=db.with_conn(|conn|account_inbox::metadata(conn,&line,50)).unwrap();assert_eq!(raw[0].inbox_id,id);
    let rows=db.with_conn(|conn|list(conn,&line,50)).unwrap();assert_eq!(rows[0].timestamp_source,"received");assert_eq!(rows[0].sent_at_unix_millis,None);
    let recorded=chrono::DateTime::parse_from_rfc3339(&raw[0].created_at).unwrap().timestamp_millis();
    assert_eq!(chrono::DateTime::parse_from_rfc3339(&rows[0].message.timestamp).unwrap().timestamp_millis(),recorded);
}
#[test]
fn sms_insert_failure_rolls_back_raw_projection_and_both_events() {
    let (db,_,context)=setup();let mut receiver=db.subscribe_app_events();
    db.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_app_sms BEFORE INSERT ON sms_messages WHEN NEW.transport='app_voice'
        BEGIN SELECT RAISE(ABORT,'fixture-private-error');END;").map_err(|_|STORAGE)).unwrap();
    assert_eq!(context.commit_fixture(&db,1,&text()).err(),Some(STORAGE));
    for table in ["app_voice_inbox","app_voice_sms_projection","sms_messages","app_events"]{assert_eq!(count(&db,table),0);}
    assert!(receiver.try_recv().is_err());
}
#[test]
fn sms_event_failure_rolls_back_even_the_preceding_generic_event_and_raw_data() {
    let (db,_,context)=setup();let mut receiver=db.subscribe_app_events();
    db.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_app_sms_event BEFORE INSERT ON app_events WHEN NEW.event_type='sms.received'
        BEGIN SELECT RAISE(ABORT,'fixture-private-error');END;").map_err(|_|STORAGE)).unwrap();
    assert_eq!(context.commit_fixture(&db,1,&text()).err(),Some(STORAGE));
    for table in ["app_voice_inbox","app_voice_sms_projection","sms_messages","app_events"]{assert_eq!(count(&db,table),0);}
    assert!(receiver.try_recv().is_err());
}
#[test]
fn deleting_sms_wipes_private_raw_copy_and_identical_retry_does_not_resurrect_it() {
    let (db,line,context)=setup();let id=context.commit_fixture(&db,1,&text()).unwrap();let sms=db.with_conn(|conn|list(conn,&line,50)).unwrap()[0].message.id;
    assert_eq!(db.delete_sms_for_channel(sms,&line).unwrap(),1);
    let wiped:bool=db.with_conn(|conn|conn.query_row("SELECT length(body)=0 AND sender IS NULL AND opaque_time IS NULL FROM app_voice_inbox WHERE id=?1",[id],|row|row.get(0)).map_err(|_|STORAGE)).unwrap();assert!(wiped);
    assert!(db.with_conn(|conn|list(conn,&line,50)).unwrap().is_empty());assert_eq!(db.with_conn(|conn|account_inbox::metadata(conn,&line,50)).unwrap()[0].classification,"smgw_text_deleted");
    assert_eq!(context.commit_fixture(&db,1,&text()).unwrap(),id);assert_eq!(count(&db,"sms_messages"),0);assert_eq!(count(&db,"app_events"),2);
    let mut changed=text();changed.body.push(b'!');assert_eq!(context.commit_fixture(&db,1,&changed).err(),Some("app_account_inbox_duplicate_conflict"));
    let wiped:bool=db.with_conn(|conn|conn.query_row("SELECT length(body)=0 AND sender IS NULL AND opaque_time IS NULL FROM app_voice_inbox WHERE id=?1",[id],|row|row.get(0)).map_err(|_|STORAGE)).unwrap();assert!(wiped);
}
#[test]
fn deleting_another_transport_does_not_touch_the_application_inbox_or_projection() {
    let (db,line,context)=setup();context.commit_fixture(&db,1,&text()).unwrap();
    let other=db.insert_sms_with_transport_for_line("incoming","other-number","other-text","received",None,"modem",Some("line-other")).unwrap();
    assert_eq!(db.delete_sms_for_channel(other,"line-other").unwrap(),1);
    assert_eq!(db.with_conn(|conn|list(conn,&line,50)).unwrap().len(),1);assert_eq!(count(&db,"app_voice_inbox"),1);
    assert_eq!(db.with_conn(|conn|account_inbox::metadata(conn,&line,50)).unwrap()[0].classification,"smgw_text");
}
#[test]
fn deleting_account_purges_its_private_envelope_but_keeps_existing_sms_history() {
    let (db,line,context)=setup();context.commit_fixture(&db,1,&text()).unwrap();
    db.with_conn(|conn|account_store::delete(conn,&line,1)).unwrap();assert_eq!(count(&db,"app_voice_inbox"),0);
    let history=db.get_sms_messages_for_channel(50,0,None,Some(&line)).unwrap();assert_eq!(history.len(),1);assert_eq!(history[0].transport,"app_voice");
    assert_eq!(db.delete_sms_for_channel(history[0].id,&line).unwrap(),1);
    assert!(db.get_sms_messages_for_channel(50,0,None,Some(&line)).unwrap().is_empty());
}
#[test]
fn original_raw_scope_required_for_promotion_and_no_sms_from_an_unrelated_row() {
    let (db,line,_)=setup();let message=text();let text=super::super::smgw_text::classify(SMGW_RECEIVER,&message).unwrap();
    let result=db.with_conn(|conn| {
        let tx=rusqlite::Transaction::new_unchecked(conn,rusqlite::TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
        promote_new(&tx,999,&line,1,&text,"2023-11-14T22:13:20Z",None).map(|_|())
    });
    assert_eq!(result,Err(SCOPE));assert_eq!(count(&db,"sms_messages"),0);assert_eq!(count(&db,"app_events"),0);
}
#[test]
fn same_text_on_two_accounts_does_not_deduplicate_or_delete_across_original_lines() {
    let (db,line,context)=setup();context.commit_fixture(&db,1,&text()).unwrap();
    let cert=Certificates::generate("DNS:fixture.invalid");let imported=config(&cert.root);let other=format!("app-account-{}",local_nonce().unwrap());
    db.with_conn(|conn|account_store::create(conn,&other,&imported,100)).unwrap();
    let mut session=HostingSession::new(other.clone());session.import(imported.credentials,100).unwrap();let owner=session.begin_login(100).unwrap();
    let context=super::super::account_test_support::registered_inbox_scope(&db,&other,1,&owner,&RpcConnectionScope::fresh())
        .with_receiver(IncomingBinding{object_id:b"#fixture-client",domain:b"fixture-domain",receiver_kind:SMGW_RECEIVER}).unwrap();
    context.commit_fixture(&db,1,&text()).unwrap();
    let a=db.with_conn(|conn|list(conn,&line,50)).unwrap();let b=db.with_conn(|conn|list(conn,&other,50)).unwrap();
    assert_eq!(a.len(),1);assert_eq!(b.len(),1);assert_ne!(a[0].message.id,b[0].message.id);
    assert_eq!(db.delete_sms_for_channel(b[0].message.id,&line).unwrap(),0);
    assert_eq!(db.clear_sms_for_channel(&line).unwrap(),1);assert!(db.with_conn(|conn|list(conn,&line,50)).unwrap().is_empty());
    assert_eq!(db.with_conn(|conn|list(conn,&other,50)).unwrap().len(),1);assert_eq!(count(&db,"app_events"),4);
}
#[test]
fn a_different_timestamp_or_retired_config_cannot_project_an_existing_raw_observation() {
    let (db,line,context)=setup();let id=context.commit_fixture(&db,1,&text()).unwrap();
    let mut changed=text();changed.time=Some(b"1700000000124".to_vec());let classified=super::super::smgw_text::classify(SMGW_RECEIVER,&changed).unwrap();
    let result=db.with_conn(|conn|{let tx=rusqlite::Transaction::new_unchecked(conn,rusqlite::TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
        promote_new(&tx,id,&line,1,&classified,"2023-11-14T22:13:20Z",None).map(|_|())});assert_eq!(result,Err(SCOPE));
    db.with_conn(|conn|conn.execute("UPDATE app_voice_accounts SET revision=2 WHERE line_id=?1",[&line]).map(|_|()).map_err(|_|STORAGE)).unwrap();
    let original=text();let classified=super::super::smgw_text::classify(SMGW_RECEIVER,&original).unwrap();
    let result=db.with_conn(|conn|{let tx=rusqlite::Transaction::new_unchecked(conn,rusqlite::TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
        promote_new(&tx,id,&line,1,&classified,"2023-11-14T22:13:20Z",None).map(|_|())});assert_eq!(result,Err(SCOPE));
    assert_eq!(count(&db,"sms_messages"),1);assert_eq!(count(&db,"app_events"),2);
}
#[test]
fn schema_initialization_does_not_reinterpret_preexisting_generic_history() {
    let (db,line,context)=setup();let mut unknown=text();unknown.time=Some(b"opaque-time".to_vec());context.commit_fixture(&db,1,&unknown).unwrap();
    db.with_conn(initialize_schema).unwrap();db.with_conn(initialize_schema).unwrap();
    assert_eq!(count(&db,"app_voice_inbox"),1);assert_eq!(count(&db,"app_events"),1);assert_eq!(count(&db,"sms_messages"),0);
    assert!(db.with_conn(|conn|list(conn,&line,50)).unwrap().is_empty());
}
