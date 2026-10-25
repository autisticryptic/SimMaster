//! Actions-only transaction/fence tests; test_quiescence is a data fixture,
//! not evidence of actor/consumer completion (real runtime tests cover that).
use super::*;
use super::super::{account_inbox::InboxScope, account_runtime::{local_nonce,test_quiescence}, account_sms,
    account_test_support::config, native_events::IncomingMessage, rpc_incoming::IncomingBinding,
    rpc_pending::RpcConnectionScope, session::HostingSession, smgw_text::SMGW_RECEIVER, tls_engine::tests::Certificates};
use crate::platform::db::Database;

fn setup()->(Database,String,super::super::session::AttemptOwner,RpcConnectionScope) {
    let cert=Certificates::generate("DNS:fixture.invalid");let db=Database::new(std::path::PathBuf::from(":memory:")).unwrap();
    let line=format!("app-account-{}",local_nonce().unwrap());let imported=config(&cert.root);
    db.with_conn(|conn|account_store::create(conn,&line,&imported,100)).unwrap();
    let mut session=HostingSession::new(line.clone());session.import(imported.credentials,100).unwrap();
    (db,line,session.begin_login(100).unwrap(),RpcConnectionScope::fresh())
}
fn scope(db:&Database,line:&str,owner:&super::super::session::AttemptOwner,physical:&RpcConnectionScope)->(RunToken,InboxScope) {
    let nonce=local_nonce().unwrap();let token=db.with_conn(|conn|register(conn,line,1,&nonce)).unwrap();
    let inbox=InboxScope::original(line,1,&nonce,owner,physical).unwrap().with_receiver(IncomingBinding {
        object_id:b"#fixture-client",domain:b"fixture-domain",receiver_kind:SMGW_RECEIVER}).unwrap();(token,inbox)
}
fn message()->IncomingMessage { IncomingMessage{body:b"fixture-text".to_vec(),from:Some(b"fixture-sender".to_vec()),time:None} }
fn count(db:&Database,table:&str)->i64 {db.with_conn(|conn|conn.query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|row|row.get(0)).map_err(|_|STORAGE)).unwrap()}

#[test]
fn registration_is_original_revision_bounded_and_cannot_reactivate_existing_nonce() {
    let (db,line,_,_)=setup();let nonce=local_nonce().unwrap();let token=db.with_conn(|conn|register(conn,&line,1,&nonce)).unwrap();
    assert_eq!(token.line_id(),line);assert_eq!(token.revision(),1);assert_eq!(token.nonce(),nonce);
    assert_eq!(format!("{token:?}"),"RunToken(<redacted>)");assert_eq!(db.with_conn(|conn|register(conn,&line,1,&nonce)).err(),Some(SCOPE));
    assert_eq!(db.with_conn(|conn|register(conn,&line,2,&local_nonce().unwrap())).err(),Some(SCOPE));
    for bad in ["", "bad", "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF"] {assert_eq!(db.with_conn(|conn|register(conn,&line,1,bad)).err(),Some(SCOPE));}
    for _ in 1..MAX_RUNS_PER_ACCOUNT {db.with_conn(|conn|register(conn,&line,1,&local_nonce().unwrap())).unwrap();}
    assert_eq!(db.with_conn(|conn|register(conn,&line,1,&local_nonce().unwrap())).err(),Some("app_account_run_limit"));
}
#[test]
fn joined_run_can_release_more_than_128_lifetime_messages_without_erasing_shared_sms() {
    let (db,line,owner,physical)=setup();
    for n in 0..140 {
        let (token,inbox)=scope(&db,&line,&owner,&physical);inbox.commit_fixture(&db,1,&message()).unwrap();
        let proof=test_quiescence(token);assert_eq!(db.with_conn(|conn|release_and_compact(conn,&proof)).unwrap(),1);
        assert_eq!(count(&db,"app_voice_inbox"),0);assert_eq!(count(&db,"app_voice_inbox_runs"),0);
        assert_eq!(count(&db,"sms_messages"),n+1);
    }
    assert_eq!(db.with_conn(|conn|account_sms::list(conn,&line,128)).unwrap().len(),128);
    assert_eq!(count(&db,"app_voice_sms_projection"),140);
}
#[test]
fn closed_scope_or_reply_marker_alone_never_creates_a_release_fence() {
    let (db,line,owner,physical)=setup();let (token,inbox)=scope(&db,&line,&owner,&physical);let id=inbox.commit_fixture(&db,1,&message()).unwrap();
    db.with_conn(|conn|conn.execute("UPDATE app_voice_inbox SET reply_state='direct_submitted' WHERE id=?1",[id]).map(|_|()).map_err(|_|STORAGE)).unwrap();
    physical.close();assert!(db.with_conn(|conn|require_live(conn,&line,1,token.nonce())).is_ok());
    assert_eq!(count(&db,"app_voice_inbox"),1);assert_eq!(count(&db,"app_voice_sms_projection"),1);
}
#[test]
fn released_fence_prevents_even_a_still_borrowed_fixture_scope_from_reinserting_gc_rows() {
    let (db,line,owner,physical)=setup();let (token,inbox)=scope(&db,&line,&owner,&physical);inbox.commit_fixture(&db,1,&message()).unwrap();
    let proof=test_quiescence(token);db.with_conn(|conn|release_and_compact(conn,&proof)).unwrap();
    // A real release closes this scope; leaving the fixture open exercises the
    // extra durable writer fence, not a promise that the fake scope is real IO.
    assert!(physical.is_open());assert_eq!(inbox.commit_fixture(&db,1,&message()).err(),Some(SCOPE));
    assert_eq!(inbox.commit_fixture(&db,2,&message()).err(),Some(SCOPE));assert_eq!(count(&db,"sms_messages"),1);assert_eq!(count(&db,"app_voice_inbox"),0);
    assert_eq!(db.with_conn(|conn|release_and_compact(conn,&proof)).unwrap(),0);
}
#[test]
fn unknown_raw_is_retained_with_its_released_fence_and_not_projected_by_gc() {
    let (db,line,owner,physical)=setup();let (token,inbox)=scope(&db,&line,&owner,&physical);
    let mut unknown=message();unknown.body=vec![0xff,0,3];inbox.commit_fixture(&db,1,&unknown).unwrap();inbox.commit_fixture(&db,2,&message()).unwrap();
    let proof=test_quiescence(token);assert_eq!(db.with_conn(|conn|release_and_compact(conn,&proof)).unwrap(),1);
    assert_eq!(count(&db,"app_voice_inbox"),1);assert_eq!(count(&db,"app_voice_inbox_runs"),1);assert_eq!(count(&db,"sms_messages"),1);
    let bytes:Vec<u8>=db.with_conn(|conn|conn.query_row("SELECT body FROM app_voice_inbox WHERE line_id=?1",[&line],|row|row.get(0)).map_err(|_|STORAGE)).unwrap();assert_eq!(bytes,[0xff,0,3]);
    assert_eq!(inbox.commit_fixture(&db,1,&unknown).err(),Some(SCOPE));
    assert_eq!(db.with_conn(|conn|register(conn,&line,1,proof.token().nonce())).err(),Some(SCOPE));
}
#[test]
fn deleting_sms_before_and_after_release_does_not_accumulate_detached_tombstones() {
    let (db,line,owner,physical)=setup();let (token,inbox)=scope(&db,&line,&owner,&physical);inbox.commit_fixture(&db,1,&message()).unwrap();
    let sms=db.with_conn(|conn|account_sms::list(conn,&line,10)).unwrap()[0].message.id;db.delete_sms_for_channel(sms,&line).unwrap();
    assert_eq!(count(&db,"app_voice_inbox"),1);assert_eq!(count(&db,"app_voice_sms_projection"),1);
    inbox.commit_fixture(&db,1,&message()).unwrap();assert_eq!(count(&db,"sms_messages"),0);
    db.with_conn(|conn|release_and_compact(conn,&test_quiescence(token))).unwrap();assert_eq!(count(&db,"app_voice_inbox"),0);assert_eq!(count(&db,"app_voice_sms_projection"),0);
    let (token,inbox)=scope(&db,&line,&owner,&physical);inbox.commit_fixture(&db,1,&message()).unwrap();
    db.with_conn(|conn|release_and_compact(conn,&test_quiescence(token))).unwrap();let sms=db.with_conn(|conn|account_sms::list(conn,&line,10)).unwrap()[0].message.id;
    db.delete_sms_for_channel(sms,&line).unwrap();assert_eq!(count(&db,"app_voice_sms_projection"),0);
}
#[test]
fn compaction_failure_rolls_back_the_fence_and_all_envelope_mutations() {
    let (db,line,owner,physical)=setup();let (token,inbox)=scope(&db,&line,&owner,&physical);inbox.commit_fixture(&db,1,&message()).unwrap();
    db.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_compact BEFORE DELETE ON app_voice_inbox BEGIN SELECT RAISE(ABORT,'fixture-private-error');END;").map_err(|_|STORAGE)).unwrap();
    let proof=test_quiescence(token);assert_eq!(db.with_conn(|conn|release_and_compact(conn,&proof)).err(),Some(STORAGE));
    assert!(db.with_conn(|conn|require_live(conn,&line,1,proof.token().nonce())).is_ok());assert_eq!(count(&db,"app_voice_inbox"),1);assert_eq!(count(&db,"sms_messages"),1);
    let retained:bool=db.with_conn(|conn|conn.query_row("SELECT envelope_retained=1 FROM app_voice_sms_projection",[],|row|row.get(0)).map_err(|_|STORAGE)).unwrap();assert!(retained);
    db.with_conn(|conn|conn.execute_batch("DROP TRIGGER fail_compact").map_err(|_|STORAGE)).unwrap();assert_eq!(db.with_conn(|conn|release_and_compact(conn,&proof)).unwrap(),1);
}
#[test]
fn one_run_release_never_reclaims_another_live_run_on_the_same_account() {
    let (db,line,owner,physical)=setup();let (a,first)=scope(&db,&line,&owner,&physical);let (b,second)=scope(&db,&line,&owner,&physical);
    first.commit_fixture(&db,1,&message()).unwrap();second.commit_fixture(&db,1,&message()).unwrap();
    db.with_conn(|conn|release_and_compact(conn,&test_quiescence(a))).unwrap();assert_eq!(count(&db,"app_voice_inbox"),1);assert_eq!(count(&db,"sms_messages"),2);
    assert!(db.with_conn(|conn|require_live(conn,&line,1,b.nonce())).is_ok());second.commit_fixture(&db,1,&message()).unwrap();assert_eq!(count(&db,"sms_messages"),2);
}
#[test]
fn no_durable_run_registration_means_no_inbox_commit_or_synthetic_recovery() {
    let (db,line,owner,physical)=setup();let nonce=local_nonce().unwrap();let inbox=InboxScope::original(&line,1,&nonce,&owner,&physical).unwrap()
        .with_receiver(IncomingBinding{object_id:b"object",domain:b"domain",receiver_kind:SMGW_RECEIVER}).unwrap();
    assert_eq!(inbox.commit_fixture(&db,1,&message()).err(),Some(SCOPE));assert_eq!(count(&db,"app_voice_inbox"),0);assert_eq!(count(&db,"sms_messages"),0);
    db.with_conn(initialize_schema).unwrap();assert_eq!(db.with_conn(|conn|require_live(conn,&line,1,&nonce)),Err(SCOPE));
}
#[test]
fn completed_local_replies_leave_pending_budget_but_all_live_identity_bounds_remain() {
    let (db,line,owner,physical)=setup();let (_token,inbox)=scope(&db,&line,&owner,&physical);
    for sequence in 1..=512 {
        let id=inbox.commit_fixture(&db,sequence,&message()).unwrap();
        db.with_conn(|conn|conn.execute("UPDATE app_voice_inbox SET reply_state='direct_submitted' WHERE id=?1",[id]).map(|_|()).map_err(|_|STORAGE)).unwrap();
    }
    assert_eq!(inbox.commit_fixture(&db,513,&message()).err(),Some("app_account_inbox_full"));assert_eq!(count(&db,"app_voice_inbox"),512);
    assert!(inbox.commit_fixture(&db,1,&message()).is_ok());
}
#[test]
fn runtime_registration_is_exclusive_but_an_unrelated_account_is_not_blocked(){
    let(db,line,_,_)=setup();let nonce=local_nonce().unwrap();let token=db.with_conn(|conn|register_runtime(conn,&line,1,&nonce)).unwrap();
    assert!(db.with_conn(|conn|runtime_held(conn,&line)).unwrap());assert!(db.with_conn(|conn|runtime_matches(conn,&token)).unwrap());
    assert_eq!(db.with_conn(|conn|register_runtime(conn,&line,1,&local_nonce().unwrap())).err(),Some("app_account_stop_before_replace"));
    assert_eq!(count(&db,"app_voice_runtime_leases"),1);assert_eq!(count(&db,"app_voice_inbox_runs"),1);
    let cert=Certificates::generate("DNS:fixture.invalid");let other=format!("app-account-{}",local_nonce().unwrap());
    db.with_conn(|conn|account_store::create(conn,&other,&config(&cert.root),100)).unwrap();
    let other_token=db.with_conn(|conn|register_runtime(conn,&other,1,&local_nonce().unwrap())).unwrap();
    assert_eq!(count(&db,"app_voice_runtime_leases"),2);
    db.with_conn(|conn|release_and_compact(conn,&test_quiescence(token))).unwrap();
    assert!(!db.with_conn(|conn|runtime_held(conn,&line)).unwrap());assert!(db.with_conn(|conn|runtime_held(conn,&other)).unwrap());
    db.with_conn(|conn|release_and_compact(conn,&test_quiescence(other_token))).unwrap();
}
#[test]
fn legacy_live_run_without_local_release_proof_blocks_new_runtime_without_pid_or_age_takeover(){
    let(db,line,_,_)=setup();let nonce=local_nonce().unwrap();db.with_conn(|conn|register(conn,&line,1,&nonce)).unwrap();
    assert!(!db.with_conn(|conn|runtime_held(conn,&line)).unwrap());assert!(db.with_conn(|conn|ownership_pending(conn,&line)).unwrap());
    assert_eq!(db.with_conn(|conn|register_runtime(conn,&line,1,&local_nonce().unwrap())).err(),Some("app_account_stop_before_replace"));
    assert_eq!(count(&db,"app_voice_runtime_leases"),0);assert!(db.with_conn(|conn|require_live(conn,&line,1,&nonce)).is_ok());
}
#[test]
fn failed_exclusive_admission_rolls_back_run_fence_and_does_not_leave_an_empty_lease(){
    let(db,line,_,_)=setup();let nonce=local_nonce().unwrap();
    db.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_runtime_claim BEFORE INSERT ON app_voice_runtime_leases BEGIN SELECT RAISE(ABORT,'fixture-private');END;").map_err(|_|STORAGE)).unwrap();
    assert_eq!(db.with_conn(|conn|register_runtime(conn,&line,1,&nonce)).err(),Some(STORAGE));
    assert_eq!(count(&db,"app_voice_runtime_leases"),0);assert_eq!(count(&db,"app_voice_inbox_runs"),0);
    db.with_conn(|conn|conn.execute_batch("DROP TRIGGER fail_runtime_claim").map_err(|_|STORAGE)).unwrap();
    let token=db.with_conn(|conn|register_runtime(conn,&line,1,&nonce)).unwrap();db.with_conn(|conn|release_and_compact(conn,&test_quiescence(token))).unwrap();
}
#[test]
fn failed_runtime_release_rolls_back_original_fence_and_retains_exclusion_until_retry(){
    let(db,line,_,_)=setup();let nonce=local_nonce().unwrap();let token=db.with_conn(|conn|register_runtime(conn,&line,1,&nonce)).unwrap();
    db.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_runtime_release BEFORE DELETE ON app_voice_runtime_leases BEGIN SELECT RAISE(ABORT,'fixture-private');END;").map_err(|_|STORAGE)).unwrap();
    let proof=test_quiescence(token);assert_eq!(db.with_conn(|conn|release_and_compact(conn,&proof)).err(),Some(STORAGE));
    assert!(db.with_conn(|conn|runtime_held(conn,&line)).unwrap());assert!(db.with_conn(|conn|require_live(conn,&line,1,&nonce)).is_ok());
    assert_eq!(db.with_conn(|conn|register_runtime(conn,&line,1,&local_nonce().unwrap())).err(),Some("app_account_stop_before_replace"));
    db.with_conn(|conn|conn.execute_batch("DROP TRIGGER fail_runtime_release").map_err(|_|STORAGE)).unwrap();
    db.with_conn(|conn|release_and_compact(conn,&proof)).unwrap();assert!(!db.with_conn(|conn|runtime_held(conn,&line)).unwrap());
}
#[test]
fn stale_joined_proof_cannot_release_a_new_run_with_the_same_line_and_revision(){
    let(db,line,_,_)=setup();let old=db.with_conn(|conn|register_runtime(conn,&line,1,&local_nonce().unwrap())).unwrap();
    let proof=test_quiescence(old);db.with_conn(|conn|release_and_compact(conn,&proof)).unwrap();
    let next=db.with_conn(|conn|register_runtime(conn,&line,1,&local_nonce().unwrap())).unwrap();
    assert_eq!(db.with_conn(|conn|release_and_compact(conn,&proof)).err(),Some(SCOPE));
    assert!(db.with_conn(|conn|runtime_matches(conn,&next)).unwrap());assert!(db.with_conn(|conn|require_live(conn,&line,1,next.nonce())).is_ok());
    db.with_conn(|conn|release_and_compact(conn,&test_quiescence(next))).unwrap();
}
#[test]
fn independent_sqlite_connections_atomically_admit_only_one_original_runtime(){
    use super::super::account_test_support::TempDatabase;
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let line=format!("app-account-{}",local_nonce().unwrap());
    temp.database.with_conn(|conn|account_store::create(conn,&line,&config(&cert.root),100)).unwrap();
    let barrier=std::sync::Arc::new(std::sync::Barrier::new(2));
    let results=std::thread::scope(|scope|{
        let jobs=(0..2).map(|_|{let barrier=barrier.clone();let path=temp.path.clone();let line=line.clone();scope.spawn(move||{
            let conn=Connection::open(path).unwrap();conn.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
            let nonce=local_nonce().unwrap();barrier.wait();register_runtime(&conn,&line,1,&nonce)
        })}).collect::<Vec<_>>();jobs.into_iter().map(|job|job.join().unwrap()).collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|result|result.is_ok()).count(),1);
    assert_eq!(results.iter().filter(|result|result.as_ref().err()==Some(&"app_account_stop_before_replace")).count(),1);
    let token=results.into_iter().find_map(Result::ok).unwrap();
    temp.database.with_conn(|conn|release_and_compact(conn,&test_quiescence(token))).unwrap();
}
