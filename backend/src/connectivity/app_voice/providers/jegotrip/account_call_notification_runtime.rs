//! One owned, fixed-deadline worker per original ringing call. No queue recovery
//! can acquire a new SDK scope; cancellation consumes the real task handle.
use super::*;
use super::super::account_call_notifications::{self as queue,CallNoticeWork,CallNoticeOutcome};
use tokio::task::JoinSet;
struct DispatchGuard{db:Arc<Database>,work:Option<CallNoticeWork>}
impl Drop for DispatchGuard{fn drop(&mut self){if let Some(work)=&self.work{queue::uncertain(&self.db,work);}}}
fn live(call:&Call)->bool{
    if call.cancelled.load(Ordering::SeqCst)||!call.ticket.is_live(){return false;}
    let state=lock(&call.state);state.phase==Phase::Ringing&&!state.accept_queued&&!state.end_queued
}
async fn stopped(call:&Call){loop{
    let changed=call.changed.notified();tokio::pin!(changed);changed.as_mut().enable();
    if !live(call){return;}tokio::select!{biased;_=call.ticket.revoked()=>return,_=changed=>{}}
}}
impl CallService{
    pub(super) fn start_call_notifications(&self,call:&Arc<Call>){
        if !self.settings.call_notifications{return;}
        let attempt=(||{
            if !self.active(call)||!live(call){return Err(SCOPE);}
            let Some(snapshot)=lock(&call.notice_snapshot).take()else{return Ok(());};
            let Some(snapshot)=snapshot?else{return Err("app_account_call_notification_unavailable");};
            let record=history::notification_record(&self.database,&call.history)?;let plan=snapshot.plan(&record)?;
            let empty=plan.is_empty();
            call.ticket.with_media_gate(||queue::enqueue(&self.database,&self.run,&call.history,&record,plan)).ok_or(SCOPE)??;
            if empty{return Ok(());}
            let planner=self.notice_planner.as_ref().ok_or("app_account_call_notification_unavailable")?.clone();
            let db=self.database.clone();let run=self.run.clone();let original=call.clone();
            let mut task=call.notice_task.try_lock().map_err(|_|"app_account_call_notification_operation_pending")?;
            if task.is_some(){return Err("app_account_call_notification_operation_pending");}
            let remaining=self.expiry.map(|expiry|Duration::from_secs(expiry.saturating_sub(super::super::account_runtime::unix_now()))).unwrap_or(queue::WINDOW);
            let deadline=tokio::time::Instant::now()+queue::WINDOW.min(remaining);
            *task=Some(tokio::spawn(async move{worker(db,run,original,planner,deadline).await;}));Ok(())
        })();
        if let Err(code)=attempt{*lock(&call.notice_error)=Some(code);queue::cancel(&self.database,&call.history);}
    }
    pub(super) async fn join_call_notifications(&self,call:&Call)->Result<(),&'static str>{
        let mut slot=call.notice_task.lock().await;
        if let Some(task)=slot.as_mut(){let outcome=task.await;slot.take();if outcome.is_err(){queue::cancel(&self.database,&call.history);return Err("app_account_call_notification_join_failed");}}
        Ok(())
    }
}
async fn worker(db:Arc<Database>,run:Arc<RunToken>,call:Arc<Call>,planner:Arc<dyn super::super::account_notifications::NoticePlanner>,deadline:tokio::time::Instant){
    let mut jobs=JoinSet::new();
    loop{
        if !live(&call)||tokio::time::Instant::now()>=deadline{break;}
        if jobs.is_empty()&&queue::pending(&db,&call.history)==Ok(false){break;}
        while jobs.len()<queue::WORKERS&&live(&call){
            match queue::claim_next(&db,&run,&call.history){
                Ok(Some(work))=>{jobs.spawn(send_one(db.clone(),run.clone(),call.clone(),planner.clone(),work,deadline));}
                Ok(None)=>break,
                Err(code)=>{*lock(&call.notice_error)=Some(code);break;},
            }
        }
        tokio::select!{biased;
            _=stopped(&call)=>break,
            _=tokio::time::sleep_until(deadline)=>break,
            result=jobs.join_next(),if !jobs.is_empty()=>{if result.is_some_and(|r|r.is_err()){*lock(&call.notice_error)=Some("app_account_call_notification_join_failed");}},
            _=tokio::time::sleep(Duration::from_millis(250))=>{},
        }
    }
    // Every job observes the same original call and fixed deadline. Do not
    // abort/detach claims or keep repolling an already-completed Join handle.
    while jobs.join_next().await.is_some(){}
    if live(&call)&&tokio::time::Instant::now()>=deadline{queue::expire(&db,&call.history);}else{queue::cancel(&db,&call.history);}
}
async fn send_one(db:Arc<Database>,run:Arc<RunToken>,call:Arc<Call>,planner:Arc<dyn super::super::account_notifications::NoticePlanner>,work:CallNoticeWork,deadline:tokio::time::Instant){
    let mut guard=DispatchGuard{db,work:Some(work)};let work=guard.work.as_ref().expect("owned call notification");
    if !live(&call)||tokio::time::Instant::now()>=deadline||queue::admitted(&guard.db,&run,&call.history,work)!=Ok(true){return;}
    let outcome=tokio::select!{biased;
        _=stopped(&call)=>return,
        _=tokio::time::sleep_until(deadline)=>return,
        result=async{
            if !live(&call)||tokio::time::Instant::now()>=deadline{return None;}
            Some(planner.deliver_call(work).await)
        }=>result,
    };
    let Some(outcome)=outcome else{return;};
    if !live(&call)||queue::admitted(&guard.db,&run,&call.history,work)!=Ok(true){return;}
    if matches!(outcome,CallNoticeOutcome::Failed|CallNoticeOutcome::Uncertain){*lock(&call.notice_error)=Some("app_account_call_notification_delivery_unconfirmed");}
    if queue::finish(&guard.db,work,outcome).is_ok(){guard.work=None;}
}
