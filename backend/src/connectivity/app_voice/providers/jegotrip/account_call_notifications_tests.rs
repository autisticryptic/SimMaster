//! Actions-only original SDK/CallService integration, synthetic channel fixture.
use super::*;
use super::super::super::{account_notifications::{NoticePlanner,NoticeSnapshot,NoticePlan,NoticeTarget},account_call_notifications::{CallNoticeSnapshot,CallNoticeWork,CallNoticeOutcome,CallNoticeFuture}};
use std::sync::atomic::AtomicUsize;
struct Planner{mode:u8,entered:AtomicUsize,active:AtomicUsize}
struct Snapshot;
impl NoticeSnapshot for Snapshot{fn plan(&self,_:&crate::platform::db::SmsMessage)->Result<NoticePlan,&'static str>{NoticePlan::bounded(Vec::new())}}
impl CallNoticeSnapshot for Snapshot{fn plan(&self,r:&crate::platform::db::CallRecord)->Result<NoticePlan,&'static str>{
    assert_eq!(r.direction,"incoming");assert_eq!(r.phone_number,"fixture-caller");assert!(!r.answered);assert!(r.line_id.is_some());
    NoticePlan::bounded(vec![NoticeTarget{rule_id:"fixture-rule".into(),channel_id:"fixture-channel".into(),fingerprint:"ab".repeat(32),title:"fixture-call-title".into(),body:"fixture-call-body".into()}])
}}
struct Active<'a>(&'a AtomicUsize);impl Drop for Active<'_>{fn drop(&mut self){self.0.fetch_sub(1,Ordering::SeqCst);}}
impl NoticePlanner for Planner{
    fn capture(&self)->Result<Box<dyn NoticeSnapshot>,&'static str>{Ok(Box::new(Snapshot))}
    fn capture_calls(&self)->Result<Option<Box<dyn CallNoticeSnapshot>>,&'static str>{Ok(Some(Box::new(Snapshot)))}
    fn deliver_call<'a>(&'a self,w:&'a CallNoticeWork)->CallNoticeFuture<'a>{Box::pin(async move{
        assert_eq!(w.title.as_str(),"fixture-call-title");assert_eq!(w.body.as_str(),"fixture-call-body");self.entered.fetch_add(1,Ordering::SeqCst);self.active.fetch_add(1,Ordering::SeqCst);let _active=Active(&self.active);
        match self.mode{0=>CallNoticeOutcome::Delivered,1=>std::future::pending().await,2=>CallNoticeOutcome::Uncertain,_=>CallNoticeOutcome::Deferred(Duration::from_secs(5))}
    })}
}
async fn status(manager:&AccountManager,line:&str,wanted:&str){bounded(async{loop{if manager.call_notification_status(line,16).unwrap().iter().any(|v|v.state==wanted){return;}tokio::task::yield_now().await;}}).await;}
async fn scenario(enabled:bool,mode:u8){
    let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let sdk=Certificates::generate("DNS:fixture.invalid");let media=Certificates::generate("IP:127.0.0.1");
    let planner=Arc::new(Planner{mode,entered:AtomicUsize::new(0),active:AtomicUsize::new(0)});
    let manager=Arc::new(AccountManager::with_factory_and_notifications(temp.database.clone(),factory.clone(),planner.clone()));
    let mut cfg=call_config(&sdk.root,&media.root);cfg.call_media.as_mut().unwrap().call_notifications=enabled;let account=manager.create(cfg,100).await.unwrap().account;
    let(mut peer,_,dropped)=sdk_peer(&sdk,&factory);let(go,invite)=oneshot::channel();let(seen,checking)=oneshot::channel();let(allow,confirmation)=oneshot::channel();let(done,stop)=oneshot::channel();
    let sdk_task=tokio::spawn(async move{bounded(async move{
        peer.authenticate().await;peer.swap(Some(4),true).await;invite.await.unwrap();notice(&mut peer,CallNoticeKind::Invited,101,&sdp("127.0.0.1:5006".parse().unwrap()),b"fixture-domain").await;receipt(&mut peer,101,true).await;
        let(id,_)=operation(&mut peer,CallOperation::Inform).await;seen.send(()).unwrap();confirmation.await.unwrap();reply_operation(&mut peer,id,CallOperation::Inform,true,None).await;
        let(id,_)=operation(&mut peer,CallOperation::Release).await;reply_operation(&mut peer,id,CallOperation::Release,true,None).await;stop.await.unwrap();finish_sdk(peer).await;
    }).await});
    bounded(manager.start(&account.line_id,account.revision,100)).await.unwrap();ready(&manager,&account.line_id).await;go.send(()).unwrap();bounded(checking).await.unwrap();
    // The original invitation receipt and a queued call history are NOT the
    // matching Inform result, nor permission to send the external alert early.
    assert_eq!(planner.entered.load(Ordering::SeqCst),0);assert!(manager.call_notification_status(&account.line_id,16).unwrap().is_empty());allow.send(()).unwrap();let call=first_call(&manager,&account.line_id).await;
    assert!(!call.media_active);assert!(!temp.database.get_call_by_id(call.history_id).unwrap().unwrap().answered);
    if enabled{match mode{0=>status(&manager,&account.line_id,"delivered").await,1=>status(&manager,&account.line_id,"sending").await,_=>status(&manager,&account.line_id,"uncertain").await};}
    else{tokio::time::sleep(Duration::from_millis(20)).await;assert_eq!(planner.entered.load(Ordering::SeqCst),0);assert!(manager.call_notification_status(&account.line_id,16).unwrap().is_empty());}
    if enabled&&mode==1{bounded(async{while planner.entered.load(Ordering::SeqCst)==0{tokio::task::yield_now().await;}}).await;}
    let ended=bounded(manager.hangup_call(&account.line_id,account.revision,&call.id,100)).await.unwrap();assert_eq!(ended.phase,"ended");assert!(!ended.media_active);
    if enabled&&mode==1{status(&manager,&account.line_id,"uncertain").await;assert_eq!(planner.active.load(Ordering::SeqCst),0);}
    if enabled{assert_eq!(planner.entered.load(Ordering::SeqCst),1);let raw=serde_json::to_string(&manager.call_notification_status(&account.line_id,16).unwrap()).unwrap();for secret in ["fixture","token","channel","claim","body"]{assert!(!raw.contains(secret));}}
    done.send(()).unwrap();bounded(manager.stop(&account.line_id,account.revision,100)).await.unwrap();bounded(sdk_task).await.unwrap();assert!(dropped.load(Ordering::SeqCst));assert_eq!(planner.active.load(Ordering::SeqCst),0);assert!(!temp.database.with_conn(|c|account_runs::ownership_pending(c,&account.line_id)).unwrap());
}
#[tokio::test]
async fn call_notice_requires_explicit_installation_flag_and_default_never_sends(){scenario(false,0).await;}
#[tokio::test]
async fn call_notice_original_sdk_inform_precedes_channel_send_and_delivery_never_grants_media(){scenario(true,0).await;}
#[tokio::test]
async fn call_notice_unknown_channel_result_is_not_replayed_and_keeps_original_call_cleanup(){scenario(true,2).await;}
#[tokio::test]
async fn call_notice_hangup_cancels_inflight_channel_future_and_joins_real_owned_worker(){scenario(true,1).await;}
#[tokio::test]
async fn call_notice_rejected_inform_never_sends_even_after_positive_invitation_receipt(){
    let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let sdk=Certificates::generate("DNS:fixture.invalid");let media=Certificates::generate("IP:127.0.0.1");let planner=Arc::new(Planner{mode:0,entered:AtomicUsize::new(0),active:AtomicUsize::new(0)});
    let manager=AccountManager::with_factory_and_notifications(temp.database.clone(),factory.clone(),planner.clone());let mut cfg=call_config(&sdk.root,&media.root);cfg.call_media.as_mut().unwrap().call_notifications=true;let account=manager.create(cfg,100).await.unwrap().account;
    let(mut peer,_,dropped)=sdk_peer(&sdk,&factory);let(go,invite)=oneshot::channel();let(done,stop)=oneshot::channel();let(seen,checked)=oneshot::channel();
    let task=tokio::spawn(async move{bounded(async move{peer.authenticate().await;peer.swap(Some(4),true).await;invite.await.unwrap();notice(&mut peer,CallNoticeKind::Invited,101,&sdp("127.0.0.1:5006".parse().unwrap()),b"fixture-domain").await;receipt(&mut peer,101,true).await;let(id,_)=operation(&mut peer,CallOperation::Inform).await;reply_operation(&mut peer,id,CallOperation::Inform,false,None).await;let(id,_)=operation(&mut peer,CallOperation::Release).await;reply_operation(&mut peer,id,CallOperation::Release,true,None).await;seen.send(()).unwrap();stop.await.unwrap();finish_sdk(peer).await;}).await});
    bounded(manager.start(&account.line_id,account.revision,100)).await.unwrap();ready(&manager,&account.line_id).await;go.send(()).unwrap();bounded(checked).await.unwrap();assert_eq!(planner.entered.load(Ordering::SeqCst),0);assert!(manager.call_notification_status(&account.line_id,16).unwrap().is_empty());done.send(()).unwrap();bounded(manager.stop(&account.line_id,account.revision,100)).await.unwrap();bounded(task).await.unwrap();assert!(dropped.load(Ordering::SeqCst));
}
#[test]
fn call_notice_installation_flag_is_false_and_omitted_in_original_private_config_bytes(){
    let sdk=Certificates::generate("DNS:fixture.invalid");let media=Certificates::generate("IP:127.0.0.1");let cfg=call_config(&sdk.root,&media.root);assert!(!cfg.call_media.as_ref().unwrap().call_notifications);
    let bytes=cfg.encode_private().unwrap();let value:serde_json::Value=serde_json::from_slice(&bytes).unwrap();assert!(value["config"]["call_media"].get("call_notifications").is_none());
}
