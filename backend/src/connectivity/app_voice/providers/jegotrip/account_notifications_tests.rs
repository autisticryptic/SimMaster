//! Actions-only durable queue/CAS/bounds tests with an in-memory dispatcher.
//! No user destination, SDK target, modem, HTTP peer or real notification runs.
use super::*;
use super::super::{account_inbox::InboxScope,account_test_support::{config,registered_inbox_scope},
    native_events::IncomingMessage,rpc_incoming::IncomingBinding,rpc_pending::RpcConnectionScope,
    session::HostingSession,tls_engine::tests::Certificates};
use std::sync::{Mutex,atomic::{AtomicUsize,Ordering}};

struct FixedPlan {targets:usize}
impl NoticeSnapshot for FixedPlan {
    fn plan(&self,message:&SmsMessage)->Result<NoticePlan,&'static str>{NoticePlan::bounded((0..self.targets).map(|n|NoticeTarget {
        rule_id:format!("rule-{n}"),channel_id:"fixture-channel".into(),fingerprint:"a".repeat(64),title:"fixture-title".into(),body:message.content.clone(),
    }).collect())}
}
fn message()->IncomingMessage{IncomingMessage{body:b"fixture-private-text".to_vec(),from:Some(b"fixture-sender".to_vec()),time:None}}
fn setup(targets:usize)->(Arc<Database>,String,InboxScope){
    let cert=Certificates::generate("DNS:fixture.invalid");let db=Arc::new(Database::new(std::path::PathBuf::from(":memory:")).unwrap());
    let imported=config(&cert.root);let line=format!("app-account-{}",super::super::account_runtime::local_nonce().unwrap());
    db.with_conn(|conn|super::super::account_store::create(conn,&line,&imported,100)).unwrap();
    let mut session=HostingSession::new(line.clone());session.import(imported.credentials,100).unwrap();let owner=session.begin_login(100).unwrap();
    let context=registered_inbox_scope(&db,&line,1,&owner,&RpcConnectionScope::fresh()).with_receiver(IncomingBinding{
        object_id:b"#fixture-client",domain:b"fixture-domain",receiver_kind:b"Sip.Request"}).unwrap()
        .with_notifications(Ok(Some(Box::new(FixedPlan{targets}))));(db,line,context)
}
fn count(db:&Database,table:&str)->i64{db.with_conn(|conn|conn.query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|r|r.get(0)).map_err(|_|STORAGE)).unwrap()}
fn status(db:&Database,line:&str)->Vec<NoticeStatus>{db.with_conn(|conn|metadata(conn,line,128)).unwrap()}
fn sms_status(db:&Database,id:i64)->String{db.with_conn(|conn|conn.query_row("SELECT notification_status FROM sms_messages WHERE id=?1",[id],|r|r.get(0)).map_err(|_|STORAGE)).unwrap()}

#[test]
fn plan_admission_sms_and_events_are_one_commit_and_duplicate_does_not_replan(){
    let(db,line,context)=setup(2);context.commit_fixture(&db,1,&message()).unwrap();
    assert_eq!(count(&db,"sms_messages"),1);assert_eq!(count(&db,"app_voice_notification_jobs"),2);assert_eq!(count(&db,"app_events"),2);
    let jobs=status(&db,&line);assert!(jobs.iter().all(|j|j.state=="pending"&&j.attempts==0));assert_eq!(sms_status(&db,jobs[0].sms_id),"pending");
    let context=context.with_notifications(Err("fixture-new-config-unavailable"));context.commit_fixture(&db,1,&message()).unwrap();
    assert_eq!(count(&db,"app_voice_notification_jobs"),2);assert_eq!(count(&db,"sms_messages"),1);
    let body=serde_json::to_string(&jobs).unwrap();assert!(!body.contains("fixture"));assert!(!body.contains("fingerprint"));
}
#[test]
fn empty_plan_is_explicitly_skipped_and_legacy_rows_are_not_retroactively_scheduled(){
    let(db,line,context)=setup(0);context.commit_fixture(&db,1,&message()).unwrap();assert!(status(&db,&line).is_empty());
    let sms=db.get_sms_messages_for_channel(10,0,None,Some(&line)).unwrap()[0].id;assert_eq!(sms_status(&db,sms),"skipped");
    let context=context.with_notifications(Ok(Some(Box::new(FixedPlan{targets:1}))));context.commit_fixture(&db,1,&message()).unwrap();assert_eq!(count(&db,"app_voice_notification_jobs"),0);
}
#[test]
fn queue_insert_failure_or_invalid_fanout_rolls_back_raw_sms_events_and_jobs(){
    let(db,_,context)=setup(1);db.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_notice BEFORE INSERT ON app_voice_notification_jobs BEGIN SELECT RAISE(ABORT,'fixture-private-error');END;").map_err(|_|STORAGE)).unwrap();
    assert_eq!(context.commit_fixture(&db,1,&message()).err(),Some(STORAGE));
    for table in ["app_voice_inbox","sms_messages","app_events","app_voice_notification_jobs"]{assert_eq!(count(&db,table),0);}
    db.with_conn(|conn|conn.execute_batch("DROP TRIGGER fail_notice").map_err(|_|STORAGE)).unwrap();
    let context=context.with_notifications(Ok(Some(Box::new(FixedPlan{targets:9}))));assert_eq!(context.commit_fixture(&db,1,&message()).err(),Some("app_account_notification_plan_too_large"));
    assert_eq!(count(&db,"sms_messages"),0);
}
#[test]
fn queue_count_limit_and_rendered_payload_limit_do_not_drop_prior_obligations(){
    let(db,line,context)=setup(1);
    db.with_conn(|conn|{
        for n in 0..MAX_JOBS {conn.execute("INSERT INTO app_voice_notification_jobs(sms_id,line_id,rule_id,channel_id,fingerprint,title,body,state,not_before,expires_at,created_at)
            VALUES(?1,?2,'r','c',?3,'t','kept','pending',0,?4,0)",params![10_000+n,line,"a".repeat(64),now()+TTL]).map_err(|_|STORAGE)?;}
        Ok(())
    }).unwrap();
    assert_eq!(context.commit_fixture(&db,1,&message()).err(),Some("app_account_notification_queue_full"));assert_eq!(count(&db,"sms_messages"),0);assert_eq!(count(&db,"app_voice_notification_jobs"),MAX_JOBS);
    let target=NoticeTarget{rule_id:"r".into(),channel_id:"c".into(),fingerprint:"a".repeat(64),title:"t".into(),body:"x".repeat(128*1024+1)};
    assert_eq!(NoticePlan::bounded(vec![target]).err(),Some("app_account_notification_plan_invalid"));
}
#[test]
fn generation_cas_prevents_stale_completion_and_accounts_are_not_rebound(){
    let(db,line,context)=setup(1);context.commit_fixture(&db,1,&message()).unwrap();let at=now();let first=claim_next(&db,at).unwrap().unwrap();
    assert_eq!(first.line_id,line);assert_eq!(first.body.as_str(),"fixture-private-text");finish(&db,&first,NoticeOutcome::Failed,at).unwrap();
    let second=claim_next(&db,at+61).unwrap().unwrap();assert_ne!(first.claim,second.claim);assert_eq!(second.attempt,2);
    finish(&db,&first,NoticeOutcome::Delivered,at+62).unwrap();assert_eq!(status(&db,&line)[0].state,"sending");
    finish(&db,&second,NoticeOutcome::Delivered,at+62).unwrap();assert_eq!(status(&db,&line)[0].state,"delivered");assert_eq!(sms_status(&db,second.sms_id),"success");
    let empty:bool=db.with_conn(|conn|conn.query_row("SELECT title='' AND body='' AND claim IS NULL FROM app_voice_notification_jobs",[],|r|r.get(0)).map_err(|_|STORAGE)).unwrap();assert!(empty);
}
#[test]
fn delete_cancels_before_claim_and_stale_inflight_result_cannot_resurrect_text(){
    let(db,line,context)=setup(1);context.commit_fixture(&db,1,&message()).unwrap();let work=claim_next(&db,now()).unwrap().unwrap();
    db.delete_sms_for_channel(work.sms_id,&line).unwrap();assert_eq!(still_admitted(&db,&work),Ok(false));finish(&db,&work,NoticeOutcome::Delivered,now()).unwrap();
    assert_eq!(status(&db,&line)[0].state,"cancelled");assert!(claim_next(&db,now()).unwrap().is_none());assert_eq!(count(&db,"sms_messages"),0);
    context.commit_fixture(&db,1,&message()).unwrap();assert_eq!(count(&db,"sms_messages"),0);assert_eq!(count(&db,"app_voice_notification_jobs"),1);
}
#[test]
fn one_channel_claim_is_serialized_and_cancellation_keeps_other_channels_independent(){
    let(db,line,context)=setup(1);context.commit_fixture(&db,1,&message()).unwrap();context.commit_fixture(&db,2,&message()).unwrap();
    let first=claim_next(&db,now()).unwrap().unwrap();assert!(claim_next(&db,now()).unwrap().is_none());
    finish(&db,&first,NoticeOutcome::Delivered,now()).unwrap();let second=claim_next(&db,now()).unwrap().unwrap();assert_ne!(first.id,second.id);
    db.with_conn(|conn|cancel_account(conn,&line)).unwrap();assert_eq!(still_admitted(&db,&second),Ok(false));
    finish(&db,&second,NoticeOutcome::Delivered,now()).unwrap();assert_eq!(status(&db,&line)[0].state,"cancelled");
}

#[test]
fn deferred_delivery_refunds_attempt_but_failed_sends_have_a_finite_retry_budget(){
    let(db,line,context)=setup(1);context.commit_fixture(&db,1,&message()).unwrap();let at=now();
    let delayed=claim_next(&db,at).unwrap().unwrap();finish(&db,&delayed,NoticeOutcome::Deferred(Duration::from_secs(5)),at).unwrap();assert_eq!(status(&db,&line)[0].attempts,0);
    for n in 0..MAX_ATTEMPTS {let at=at+10+n*61;let work=claim_next(&db,at).unwrap().unwrap();assert_eq!(work.attempt,n+1);finish(&db,&work,NoticeOutcome::Failed,at).unwrap();}
    assert_eq!(status(&db,&line)[0].state,"failed");assert!(claim_next(&db,at+1000).unwrap().is_none());
}
#[test]
fn unknown_crashed_claim_is_not_replayed_or_falsely_marked_delivered(){
    let(db,line,context)=setup(1);context.commit_fixture(&db,1,&message()).unwrap();let at=now();let work=claim_next(&db,at).unwrap().unwrap();drop(work);
    assert!(claim_next(&db,at+60).unwrap().is_none());assert_eq!(status(&db,&line)[0].state,"sending");
    assert!(claim_next(&db,at+TTL+1).unwrap().is_none());assert_eq!(status(&db,&line)[0].state,"expired");
}
#[test]
fn failed_final_status_write_becomes_uncertain_without_replaying_original_work(){
    let(db,line,context)=setup(1);context.commit_fixture(&db,1,&message()).unwrap();let work=claim_next(&db,now()).unwrap().unwrap();
    db.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_delivered BEFORE UPDATE ON app_voice_notification_jobs WHEN NEW.state='delivered' BEGIN SELECT RAISE(ABORT,'fixture-private-error');END;").map_err(|_|STORAGE)).unwrap();
    assert_eq!(finish(&db,&work,NoticeOutcome::Delivered,now()),Err(STORAGE));uncertain(&db,&work);
    assert_eq!(status(&db,&line)[0].state,"uncertain");assert!(claim_next(&db,now()+100).unwrap().is_none());
}

struct Dispatcher {active:AtomicUsize,maximum:AtomicUsize,calls:AtomicUsize,permits:tokio::sync::Semaphore,seen:Mutex<Vec<String>>}
impl Dispatcher {fn new()->Self{Self{active:AtomicUsize::new(0),maximum:AtomicUsize::new(0),calls:AtomicUsize::new(0),permits:tokio::sync::Semaphore::new(0),seen:Mutex::new(Vec::new())}}}
struct Active<'a>(&'a AtomicUsize);impl Drop for Active<'_>{fn drop(&mut self){self.0.fetch_sub(1,Ordering::SeqCst);}}
impl NoticeDispatcher for Dispatcher {
    fn deliver<'a>(&'a self,work:&'a NoticeWork)->NoticeFuture<'a>{Box::pin(async move{
        let active=self.active.fetch_add(1,Ordering::SeqCst)+1;let _active=Active(&self.active);self.maximum.fetch_max(active,Ordering::SeqCst);self.calls.fetch_add(1,Ordering::SeqCst);
        self.seen.lock().unwrap().push(work.line_id.clone());self.permits.acquire().await.unwrap().forget();NoticeOutcome::Delivered
    })}
}
#[tokio::test]
async fn two_workers_share_four_claim_slots_and_durable_pending_survives_no_broadcast(){
    let(db,line,context)=setup(1);for sequence in 1..=8{context.commit_fixture(&db,sequence,&message()).unwrap();}
    db.with_conn(|conn|conn.execute("UPDATE app_voice_notification_jobs SET channel_id='fixture-channel-'||(sms_id%4)",[]).map(|_|()).map_err(|_|STORAGE)).unwrap();
    let dispatcher=Arc::new(Dispatcher::new());let(control,signal)=crate::platform::shutdown::channel();
    let mut first=NotificationWorker::spawn(db.clone(),dispatcher.clone(),signal.clone());let mut second=NotificationWorker::spawn(db.clone(),dispatcher.clone(),signal);
    tokio::time::timeout(Duration::from_secs(5),async{while dispatcher.calls.load(Ordering::SeqCst)<4{tokio::task::yield_now().await;}}).await.unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;assert_eq!(dispatcher.calls.load(Ordering::SeqCst),4);
    dispatcher.permits.add_permits(8);
    tokio::time::timeout(Duration::from_secs(8),async{while status(&db,&line).iter().any(|j|j.state!="delivered"){tokio::time::sleep(Duration::from_millis(5)).await;}}).await.unwrap();
    control.trigger();first.join().await;second.join().await;assert!(dispatcher.maximum.load(Ordering::SeqCst)<=4);assert_eq!(dispatcher.calls.load(Ordering::SeqCst),8);
    assert!(dispatcher.seen.lock().unwrap().iter().all(|original|original==&line));
}
#[tokio::test]
async fn shutdown_joins_inflight_dispatch_and_marks_uncertainty_instead_of_success(){
    let(db,line,context)=setup(1);context.commit_fixture(&db,1,&message()).unwrap();let dispatcher=Arc::new(Dispatcher::new());let(control,signal)=crate::platform::shutdown::channel();
    let mut worker=NotificationWorker::spawn(db.clone(),dispatcher.clone(),signal);
    tokio::time::timeout(Duration::from_secs(5),async{while dispatcher.calls.load(Ordering::SeqCst)==0{tokio::task::yield_now().await;}}).await.unwrap();
    control.trigger();tokio::time::timeout(Duration::from_secs(2),worker.join()).await.unwrap();assert_eq!(dispatcher.active.load(Ordering::SeqCst),0);
    assert_eq!(status(&db,&line)[0].state,"uncertain");assert!(claim_next(&db,now()+60).unwrap().is_none());
}
#[tokio::test]
async fn already_shutdown_worker_never_claims_and_next_worker_can_resume_pending(){
    let(db,line,context)=setup(1);context.commit_fixture(&db,1,&message()).unwrap();let dispatcher=Arc::new(Dispatcher::new());
    let(control,signal)=crate::platform::shutdown::channel();control.trigger();let mut worker=NotificationWorker::spawn(db.clone(),dispatcher.clone(),signal);worker.join().await;
    assert_eq!(dispatcher.calls.load(Ordering::SeqCst),0);assert_eq!(status(&db,&line)[0].state,"pending");
    dispatcher.permits.add_permits(1);let(control,signal)=crate::platform::shutdown::channel();let mut worker=NotificationWorker::spawn(db.clone(),dispatcher.clone(),signal);
    tokio::time::timeout(Duration::from_secs(5),async{while status(&db,&line)[0].state!="delivered"{tokio::task::yield_now().await;}}).await.unwrap();control.trigger();worker.join().await;assert_eq!(dispatcher.calls.load(Ordering::SeqCst),1);
}
