//! Actions-only SQLite replay/GC proofs built by the original pure protocol.
//! Actual actor WRITE + manager durability is covered by long native fixtures.
use super::*;
use super::super::super::{arc_protocol::ArcProtocol, long_reception_test_support as wire};

fn original() -> (TempDatabase,String,ArcProtocol,AttemptOwner,InboxScope) {
    let temp=TempDatabase::new();let cert=Certificates::generate("DNS:fixture.invalid");let mut imported=config(&cert.root);
    imported.profile.incoming.as_mut().unwrap().receiver_kind=b"Sip.Request".to_vec();let line=format!("app-account-{}",local_nonce().unwrap());
    temp.database.with_conn(|conn|account_store::create(conn,&line,&imported,100)).unwrap();
    let mut session=HostingSession::new(line.clone());session.import(imported.credentials,100).unwrap();let owner=session.begin_login(100).unwrap();
    let protocol=wire::authenticated(owner.clone());let scope=protocol.connection_scope();let nonce=local_nonce().unwrap();
    temp.database.with_conn(|conn|account_runs::register_runtime(conn,&line,1,&nonce)).unwrap();
    let context=InboxScope::original(&line,1,&nonce,&owner,&scope).unwrap().with_receiver(super::super::super::rpc_incoming::IncomingBinding{
        object_id:b"#fixture-client",domain:b"fixture-domain",receiver_kind:b"Sip.Request"}).unwrap();
    (temp,line,protocol,owner,context)
}
fn complete(protocol:&mut ArcProtocol,message:&super::super::super::arc_protocol::IncomingProtocolMessage) {
    assert!(protocol.complete_incoming(message.reply_ticket(),true).unwrap().is_some());
}
fn receive(temp:&TempDatabase,protocol:&mut ArcProtocol,owner:&AttemptOwner,context:&InboxScope,id:i32)->CommittedInbox {
    let body=format!("fixture message {id}");let message=wire::incoming(protocol,owner,id,body.as_bytes());
    let committed=context.commit_direct(&temp.database,&message).unwrap();complete(protocol,&message);
    committed.record_reply_with_frontier(&temp.database,"direct_submitted",protocol.direct_replay_frontier().as_ref()).unwrap();committed
}

#[test]
fn more_than_512_original_direct_sms_compact_envelopes_but_preserve_all_history_and_events() {
    let(temp,line,mut protocol,owner,context)=original();
    for id in 1..=600 {
        receive(&temp,&mut protocol,&owner,&context,id);
        assert!(count(&temp.database,"app_voice_inbox")<=128);assert_eq!(count(&temp.database,"sms_messages"),i64::from(id));
    }
    assert_eq!(count(&temp.database,"app_voice_inbox"),128);assert_eq!(count(&temp.database,"app_voice_sms_projection"),600);
    assert_eq!(count(&temp.database,"app_events"),1200);assert!(protocol.connection_scope().is_open());
    let through:i32=temp.database.with_conn(|conn|conn.query_row("SELECT through_sequence FROM app_voice_direct_replay_fences WHERE line_id=?1",[&line],|row|row.get(0)).map_err(|_|STORAGE)).unwrap();
    assert_eq!(through,600);assert!(temp.database.with_conn(|conn|account_runs::require_live(conn,&line,1,&context.nonce)).is_ok());
}
#[test]
fn compacted_deleted_original_ticket_cannot_recreate_sms_or_notification_under_its_watermark() {
    let(temp,line,mut protocol,owner,context)=original();let first=wire::incoming(&mut protocol,&owner,1,b"deleted original");
    let first_commit=context.commit_direct(&temp.database,&first).unwrap();complete(&mut protocol,&first);
    first_commit.record_reply_with_frontier(&temp.database,"direct_submitted",protocol.direct_replay_frontier().as_ref()).unwrap();
    temp.database.with_conn(|conn|conn.execute("DELETE FROM sms_messages WHERE line_id=?1",[&line]).map_err(|_|STORAGE)).unwrap();
    for id in 2..=130 {receive(&temp,&mut protocol,&owner,&context,id);}
    assert_eq!(context.commit_direct(&temp.database,&first).err(),Some("app_account_inbox_direct_retired"));
    assert_eq!(count(&temp.database,"sms_messages"),129);assert_eq!(count(&temp.database,"app_events"),260);
    let raw:bool=temp.database.with_conn(|conn|conn.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_inbox WHERE id=?1)",[first_commit.id],|row|row.get(0)).map_err(|_|STORAGE)).unwrap();assert!(!raw);
    assert_eq!(count(&temp.database,"app_voice_notification_jobs"),0);
}
#[test]
fn failed_gc_rolls_back_status_watermark_and_detachment_without_erasing_prior_data() {
    let(temp,line,mut protocol,owner,context)=original();for id in 1..=128 {receive(&temp,&mut protocol,&owner,&context,id);}
    temp.database.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_direct_gc BEFORE DELETE ON app_voice_inbox BEGIN SELECT RAISE(ABORT,'fixture-private-error');END;").map_err(|_|STORAGE)).unwrap();
    let message=wire::incoming(&mut protocol,&owner,129,b"durable before ACK");let committed=context.commit_direct(&temp.database,&message).unwrap();complete(&mut protocol,&message);
    assert_eq!(committed.record_reply_with_frontier(&temp.database,"direct_submitted",protocol.direct_replay_frontier().as_ref()),Err(STORAGE));
    let state:(String,i32)=temp.database.with_conn(|conn|conn.query_row("SELECT i.reply_state,f.through_sequence FROM app_voice_inbox i JOIN app_voice_direct_replay_fences f
        ON f.line_id=i.line_id AND f.revision=i.revision AND f.run_nonce=i.run_nonce WHERE i.id=?1",[committed.id],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|_|STORAGE)).unwrap();
    assert_eq!(state,("pending".into(),128));assert_eq!(count(&temp.database,"app_voice_inbox"),129);assert_eq!(count(&temp.database,"sms_messages"),129);
    let detached:i64=temp.database.with_conn(|conn|conn.query_row("SELECT COUNT(*) FROM app_voice_sms_projection WHERE envelope_retained=0 AND line_id=?1",[&line],|row|row.get(0)).map_err(|_|STORAGE)).unwrap();assert_eq!(detached,0);
}
#[test]
fn lower_pending_id_blocks_compaction_then_unknown_and_batch_rows_survive_original_frontier() {
    let(temp,line,mut protocol,owner,context)=original();let first=wire::incoming(&mut protocol,&owner,1,&[0xff,0]);
    let first_commit=context.commit_direct(&temp.database,&first).unwrap();
    let batch=context.commit(&temp.database,"batch",9,500,&message()).unwrap();batch.record_reply(&temp.database,"batch_queued").unwrap();
    for id in 2..=132 {receive(&temp,&mut protocol,&owner,&context,id);}
    assert!(protocol.direct_replay_frontier().is_none());assert_eq!(count(&temp.database,"app_voice_inbox"),133);
    complete(&mut protocol,&first);first_commit.record_reply_with_frontier(&temp.database,"direct_submitted",protocol.direct_replay_frontier().as_ref()).unwrap();
    assert_eq!(count(&temp.database,"app_voice_inbox"),130);assert_eq!(count(&temp.database,"sms_messages"),131);
    assert!(private_message(&temp.database,&line,first_commit.id).is_ok());assert!(private_message(&temp.database,&line,batch.id).is_ok());
    assert_eq!(context.commit_direct(&temp.database,&first).unwrap().id,first_commit.id,"retained unknown data keeps its original identity before the watermark check");
}
#[test]
fn foreign_scope_or_revision_frontier_cannot_mark_or_compact_original_account_rows() {
    let(temp,line,mut protocol,owner,context)=original();let message=wire::incoming(&mut protocol,&owner,1,b"original");let committed=context.commit_direct(&temp.database,&message).unwrap();complete(&mut protocol,&message);
    let(other,_,mut foreign,foreign_owner,foreign_context)=original();receive(&other,&mut foreign,&foreign_owner,&foreign_context,1);
    assert_eq!(committed.record_reply_with_frontier(&temp.database,"direct_submitted",foreign.direct_replay_frontier().as_ref()),Err(SCOPE));
    temp.database.with_conn(|conn|conn.execute("UPDATE app_voice_accounts SET revision=2 WHERE line_id=?1",[&line]).map_err(|_|STORAGE)).unwrap();
    assert_eq!(committed.record_reply_with_frontier(&temp.database,"direct_submitted",protocol.direct_replay_frontier().as_ref()),Err(SCOPE));
    assert_eq!(count(&temp.database,"app_voice_direct_replay_fences"),0);assert_eq!(count(&temp.database,"app_voice_inbox"),1);
}
#[test]
fn no_protocol_frontier_keeps_classified_envelopes_and_does_not_guess_retired_sequence() {
    let(temp,_,mut protocol,owner,context)=original();
    for id in 1..=140 {let message=wire::incoming(&mut protocol,&owner,id,b"retained without proof");let committed=context.commit_direct(&temp.database,&message).unwrap();complete(&mut protocol,&message);committed.record_reply(&temp.database,"direct_submitted").unwrap();}
    assert_eq!(count(&temp.database,"app_voice_inbox"),140);assert_eq!(count(&temp.database,"app_voice_direct_replay_fences"),0);
}
