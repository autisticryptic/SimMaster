//! Durable bounded application-SMS notification plans, separate from native ACK.
//! Planning is pure on a captured config; queue admission commits with SMS.
//! Workers never replan an already committed SMS or use a new destination under
//! an old channel id. Remote notification delivery is not exactly-once.
use crate::platform::{db::{Database,SmsMessage},shutdown::ShutdownSignal};
use rusqlite::{params,Connection,OptionalExtension,Transaction};
use std::{fmt,future::Future,pin::Pin,sync::Arc,time::Duration};
use tokio::task::JoinSet;
use zeroize::{Zeroize,Zeroizing};

const STORAGE:&str="app_account_notification_storage_failed";
const MAX_TARGETS:usize=8;
const MAX_PLAN_BYTES:usize=256*1024;
const MAX_JOBS:i64=2048;
const MAX_BYTES:i64=16*1024*1024;
const MAX_ATTEMPTS:i64=5;
const TTL:i64=24*60*60;
const WORKERS:usize=4;
const ATTEMPT_TIMEOUT:Duration=Duration::from_secs(30);

pub(crate) trait NoticePlanner:Send+Sync {fn capture(&self)->Result<Box<dyn NoticeSnapshot>,&'static str>;}
pub(crate) trait NoticeSnapshot:Send+Sync {fn plan(&self,message:&SmsMessage)->Result<NoticePlan,&'static str>;}
pub(crate) struct NoticeTarget {pub rule_id:String,pub channel_id:String,pub fingerprint:String,pub title:String,pub body:String}
impl Drop for NoticeTarget {fn drop(&mut self){self.title.zeroize();self.body.zeroize();}}
impl fmt::Debug for NoticeTarget {fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("NoticeTarget(<redacted>)")}}
pub(crate) struct NoticePlan {targets:Vec<NoticeTarget>}
impl NoticePlan {
    pub(crate) fn bounded(targets:Vec<NoticeTarget>)->Result<Self,&'static str>{
        if targets.len()>MAX_TARGETS{return Err("app_account_notification_plan_too_large");}
        let mut bytes=0usize;
        for (i,target) in targets.iter().enumerate(){
            if [&target.rule_id,&target.channel_id].iter().any(|id|id.is_empty()||id.len()>128||id.chars().any(char::is_control))
                ||target.fingerprint.len()!=64||!target.fingerprint.bytes().all(|b|b.is_ascii_hexdigit())
                ||target.title.len()>8192||target.body.len()>128*1024
                ||targets[..i].iter().any(|prior|prior.rule_id==target.rule_id&&prior.channel_id==target.channel_id){return Err("app_account_notification_plan_invalid");}
            bytes=bytes.checked_add(target.title.len()+target.body.len()).ok_or("app_account_notification_plan_too_large")?;
        }
        if bytes>MAX_PLAN_BYTES{return Err("app_account_notification_plan_too_large");}
        Ok(Self{targets})
    }
    pub(crate) fn is_empty(&self)->bool{self.targets.is_empty()}
}
impl fmt::Debug for NoticePlan {fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("NoticePlan(<redacted>)")}}

/// An immutable already-rendered job. It contains no SDK credentials or guessed
/// modem identity. Its destination fingerprint is checked by the dispatcher.
pub(crate) struct NoticeWork {
    pub id:i64,pub sms_id:i64,pub line_id:String,pub rule_id:String,pub channel_id:String,
    pub fingerprint:String,pub title:Zeroizing<String>,pub body:Zeroizing<String>,
    claim:String,attempt:i64,expires:i64,
}
impl fmt::Debug for NoticeWork {fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("NoticeWork(<redacted>)")}}
#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub(crate) enum NoticeOutcome {Delivered,Deferred(Duration),Failed,TargetChanged}
pub(crate) type NoticeFuture<'a>=Pin<Box<dyn Future<Output=NoticeOutcome>+Send+'a>>;
pub(crate) trait NoticeDispatcher:Send+Sync {fn deliver<'a>(&'a self,work:&'a NoticeWork)->NoticeFuture<'a>;}

pub fn initialize_schema(conn:&Connection)->Result<(),&'static str>{
    conn.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_notification_jobs(
        id INTEGER PRIMARY KEY AUTOINCREMENT,sms_id INTEGER NOT NULL,line_id TEXT NOT NULL,
        rule_id TEXT NOT NULL,channel_id TEXT NOT NULL,fingerprint TEXT NOT NULL,
        title TEXT NOT NULL,body TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN
            ('pending','sending','delivered','failed','cancelled','uncertain','expired')),
        claim TEXT,attempts INTEGER NOT NULL DEFAULT 0,not_before INTEGER NOT NULL,
        expires_at INTEGER NOT NULL,created_at INTEGER NOT NULL,
        UNIQUE(sms_id,rule_id,channel_id));
        CREATE INDEX IF NOT EXISTS idx_app_voice_notification_due ON app_voice_notification_jobs(state,not_before,id);")
        .map_err(|_|STORAGE)
}
fn now()->i64{chrono::Utc::now().timestamp()}
fn is_terminal(state:&str)->bool{matches!(state,"delivered"|"failed"|"cancelled"|"uncertain"|"expired")}
fn aggregate_sms(conn:&Connection,sms:i64,line:&str)->Result<(),&'static str>{
    let (active,delivered,failed):(i64,i64,i64)=conn.query_row("SELECT COALESCE(SUM(state IN ('pending','sending')),0),
        COALESCE(SUM(state='delivered'),0),COALESCE(SUM(state IN ('failed','uncertain','expired','cancelled')),0)
        FROM app_voice_notification_jobs WHERE sms_id=?1 AND line_id=?2",params![sms,line],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_|STORAGE)?;
    let status=if active>0{"pending"}else if delivered>0{"success"}else if failed>0{"failed"}else{"skipped"};
    conn.execute("UPDATE sms_messages SET notification_status=?1 WHERE id=?2 AND line_id=?3 AND transport='app_voice'",params![status,sms,line]).map_err(|_|STORAGE)?;Ok(())
}
fn prune_terminal(conn:&Connection)->Result<(),&'static str>{
    // Keep a bounded metadata audit, never delete a terminal sibling until the
    // entire SMS's status has been finalized. Payload is scrubbed on completion.
    conn.execute("DELETE FROM app_voice_notification_jobs WHERE id NOT IN
        (SELECT id FROM app_voice_notification_jobs WHERE state NOT IN ('pending','sending') ORDER BY id DESC LIMIT 256)
        AND state NOT IN ('pending','sending') AND NOT EXISTS
        (SELECT 1 FROM app_voice_notification_jobs j WHERE j.sms_id=app_voice_notification_jobs.sms_id AND j.state IN ('pending','sending'))",[]).map_err(|_|STORAGE)?;Ok(())
}

pub(super) fn enqueue(tx:&Transaction<'_>,message:&SmsMessage,plan:NoticePlan)->Result<&'static str,&'static str>{
    let line=message.line_id.as_deref().filter(|id|super::account_store::valid_line_id(id)).ok_or(STORAGE)?;
    if message.id<=0||message.transport!=super::PROVIDER_ID||message.direction!="incoming"||message.pdu.is_some(){return Err(STORAGE);}
    prune_terminal(tx)?;
    let (count,bytes):(i64,i64)=tx.query_row("SELECT COUNT(*),COALESCE(SUM(length(CAST(title AS BLOB))+length(CAST(body AS BLOB))),0) FROM app_voice_notification_jobs",[],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|STORAGE)?;
    let added=plan.targets.iter().map(|target|target.title.len()+target.body.len()).sum::<usize>() as i64;
    if count+plan.targets.len() as i64>MAX_JOBS||bytes.checked_add(added).is_none_or(|sum|sum>MAX_BYTES){return Err("app_account_notification_queue_full");}
    let at=now();let expires=at.checked_add(TTL).ok_or(STORAGE)?;
    for target in &plan.targets {
        tx.execute("INSERT INTO app_voice_notification_jobs(sms_id,line_id,rule_id,channel_id,fingerprint,title,body,state,not_before,expires_at,created_at)
            VALUES(?1,?2,?3,?4,?5,?6,?7,'pending',?8,?9,?8)",params![message.id,line,target.rule_id,target.channel_id,target.fingerprint,target.title,target.body,at,expires]).map_err(|_|STORAGE)?;
    }
    let state=if plan.is_empty(){"skipped"}else{"queued"};
    tx.execute("UPDATE sms_messages SET notification_status=?1 WHERE id=?2 AND line_id=?3",params![if plan.is_empty(){"skipped"}else{"pending"},message.id,line]).map_err(|_|STORAGE)?;
    Ok(state)
}

fn claim_next(database:&Database,at:i64)->Result<Option<NoticeWork>,&'static str>{
    database.with_conn(|conn|{
        let tx=rusqlite::Transaction::new_unchecked(conn,rusqlite::TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
        // Expiry bounds even orphaned sending claims. Do NOT take over a live
        // or unknown prior-process claim and manufacture exactly-once recovery.
        let expired={
            let mut query=tx.prepare("SELECT DISTINCT sms_id,line_id FROM app_voice_notification_jobs WHERE state IN ('pending','sending') AND expires_at<=?1 LIMIT 2048").map_err(|_|STORAGE)?;
            let rows=query.query_map([at],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).map_err(|_|STORAGE)?
                .collect::<Result<Vec<_>,_>>().map_err(|_|STORAGE)?;rows
        };
        tx.execute("UPDATE app_voice_notification_jobs SET state='expired',claim=NULL,title='',body='' WHERE state IN ('pending','sending') AND expires_at<=?1",[at]).map_err(|_|STORAGE)?;
        for (sms,line) in expired {aggregate_sms(&tx,sms,&line)?;}
        prune_terminal(&tx)?;
        let active:i64=tx.query_row("SELECT COUNT(*) FROM app_voice_notification_jobs WHERE state='sending'",[],|r|r.get(0)).map_err(|_|STORAGE)?;
        if active>=WORKERS as i64 {tx.commit().map_err(|_|STORAGE)?;return Ok(None);}
        let row:Option<(i64,i64,String,String,String,String,String,String,i64,i64)>=tx.query_row(
            "SELECT j.id,j.sms_id,j.line_id,j.rule_id,j.channel_id,j.fingerprint,j.title,j.body,j.attempts,j.expires_at
            FROM app_voice_notification_jobs j JOIN sms_messages s ON s.id=j.sms_id AND s.line_id=j.line_id AND s.transport='app_voice'
            WHERE j.state='pending' AND j.not_before<=?1 AND j.expires_at>?1 AND j.attempts<5
                AND length(CAST(j.title AS BLOB))<=8192 AND length(CAST(j.body AS BLOB))<=131072
                AND NOT EXISTS(SELECT 1 FROM app_voice_notification_jobs active WHERE active.state='sending' AND active.channel_id=j.channel_id)
            ORDER BY j.id LIMIT 1",[at],
            |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?))).optional().map_err(|_|STORAGE)?;
        let Some((id,sms_id,line_id,rule_id,channel_id,fingerprint,title,body,attempt,expires))=row else{tx.commit().map_err(|_|STORAGE)?;return Ok(None);};
        let claim=super::account_runtime::local_nonce()?;
        let changed=tx.execute("UPDATE app_voice_notification_jobs SET state='sending',claim=?1,attempts=attempts+1 WHERE id=?2 AND state='pending'",params![claim,id]).map_err(|_|STORAGE)?;
        if changed!=1{return Err(STORAGE);}
        tx.commit().map_err(|_|STORAGE)?;
        Ok(Some(NoticeWork{id,sms_id,line_id,rule_id,channel_id,fingerprint,title:Zeroizing::new(title),body:Zeroizing::new(body),claim,attempt:attempt+1,expires}))
    })
}
fn still_admitted(database:&Database,work:&NoticeWork)->Result<bool,&'static str>{
    database.with_conn(|conn|conn.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_notification_jobs j JOIN sms_messages s ON s.id=j.sms_id
        AND s.line_id=j.line_id AND s.transport='app_voice' WHERE j.id=?1 AND j.claim=?2 AND j.state='sending' AND j.expires_at>?3)",
        params![work.id,work.claim,now()],|r|r.get(0)).map_err(|_|STORAGE))
}
fn finish(database:&Database,work:&NoticeWork,outcome:NoticeOutcome,at:i64)->Result<(),&'static str>{
    let (state,not_before,refund)=match outcome {
        NoticeOutcome::Delivered=>("delivered",at,false),NoticeOutcome::TargetChanged=>("cancelled",at,false),
        NoticeOutcome::Deferred(delay) if at<work.expires=>("pending",at.saturating_add(delay.as_secs().clamp(5,3600) as i64),true),
        NoticeOutcome::Failed if work.attempt<MAX_ATTEMPTS&&at<work.expires=>("pending",at.saturating_add(60),false),
        _=>("failed",at,false),
    };
    database.with_conn(|conn|{
        let tx=rusqlite::Transaction::new_unchecked(conn,rusqlite::TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
        let changed=tx.execute("UPDATE app_voice_notification_jobs SET state=?1,not_before=?2,claim=NULL,
            attempts=attempts-?3,title=CASE WHEN ?4 THEN '' ELSE title END,body=CASE WHEN ?4 THEN '' ELSE body END
            WHERE id=?5 AND claim=?6 AND state='sending'",params![state,not_before,i64::from(refund),is_terminal(state),work.id,work.claim]).map_err(|_|STORAGE)?;
        if changed==1 {aggregate_sms(&tx,work.sms_id,&work.line_id)?;prune_terminal(&tx)?;}
        tx.commit().map_err(|_|STORAGE)
    })
}
fn uncertain(database:&Database,work:&NoticeWork){
    let _=database.with_conn(|conn|{
        let tx=rusqlite::Transaction::new_unchecked(conn,rusqlite::TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
        let changed=tx.execute("UPDATE app_voice_notification_jobs SET state='uncertain',claim=NULL,title='',body='' WHERE id=?1 AND claim=?2 AND state='sending'",params![work.id,work.claim]).map_err(|_|STORAGE)?;
        if changed==1{aggregate_sms(&tx,work.sms_id,&work.line_id)?;prune_terminal(&tx)?;}tx.commit().map_err(|_|STORAGE)
    });
}
struct DispatchGuard {database:Arc<Database>,work:Option<NoticeWork>}
impl Drop for DispatchGuard {fn drop(&mut self){if let Some(work)=&self.work{uncertain(&self.database,work);}}}
async fn send_one(database:Arc<Database>,dispatcher:Arc<dyn NoticeDispatcher>,work:NoticeWork,mut shutdown:ShutdownSignal){
    let mut guard=DispatchGuard{database,work:Some(work)};
    let work=guard.work.as_ref().expect("owned job");
    if shutdown.is_shutting_down()||still_admitted(&guard.database,work)!=Ok(true){return;}
    let budget=Duration::from_secs((work.expires-now()).max(0) as u64).min(ATTEMPT_TIMEOUT);
    let deadline=tokio::time::Instant::now()+budget;
    let outcome=tokio::select!{biased;
        _=shutdown.wait()=>return,
        _=tokio::time::sleep_until(deadline)=>NoticeOutcome::Failed,
        result=dispatcher.deliver(work)=>result,
    };
    if finish(&guard.database,work,outcome,now()).is_ok(){guard.work=None;}
}

pub(crate) struct NotificationWorker {task:Option<tokio::task::JoinHandle<()>>}
impl NotificationWorker {
    pub(crate) fn spawn(database:Arc<Database>,dispatcher:Arc<dyn NoticeDispatcher>,mut shutdown:ShutdownSignal)->Self{
        let task=tokio::spawn(async move{
            let mut running=JoinSet::new();let mut ticker=tokio::time::interval(Duration::from_secs(2));ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop{
                tokio::select!{biased;
                    _=shutdown.wait()=>break,
                    _=running.join_next(),if !running.is_empty()=>{},
                    _=ticker.tick()=>{
                        while running.len()<WORKERS&&!shutdown.is_shutting_down(){
                            match claim_next(&database,now()) {
                                Ok(Some(work))=>{running.spawn(send_one(database.clone(),dispatcher.clone(),work,shutdown.clone()));},
                                _=>break,
                            }
                        }
                    }
                }
            }
            // Every active job observes shutdown; joining retains its claim
            // finalizer. The caller may abort on a hard process deadline.
            while running.join_next().await.is_some(){}
        });Self{task:Some(task)}
    }
    pub(crate) async fn join(&mut self){if let Some(task)=self.task.as_mut(){let _=task.await;}self.task=None;}
}
impl Drop for NotificationWorker {fn drop(&mut self){if let Some(task)=&self.task{task.abort();}}}

#[derive(serde::Serialize)]
pub struct NoticeStatus{pub id:i64,pub sms_id:i64,pub state:&'static str,pub attempts:i64,pub not_before:i64,pub expires_at:i64}
pub(super) fn metadata(conn:&Connection,line:&str,limit:usize)->Result<Vec<NoticeStatus>,&'static str>{
    if !super::account_store::valid_line_id(line){return Err(STORAGE);}
    let mut query=conn.prepare("SELECT id,sms_id,state,attempts,not_before,expires_at FROM app_voice_notification_jobs WHERE line_id=?1 ORDER BY id DESC LIMIT ?2").map_err(|_|STORAGE)?;
    let rows=query.query_map(params![line,limit.clamp(1,128) as i64],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,i64>(5)?))).map_err(|_|STORAGE)?;
    let mut statuses=Vec::new();for row in rows{let(id,sms_id,state,attempts,not_before,expires_at)=row.map_err(|_|STORAGE)?;
        let state=match state.as_str(){"pending"=>"pending","sending"=>"sending","delivered"=>"delivered","failed"=>"failed","cancelled"=>"cancelled","uncertain"=>"uncertain","expired"=>"expired",_=>return Err(STORAGE)};
        statuses.push(NoticeStatus{id,sms_id,state,attempts,not_before,expires_at});
    }Ok(statuses)
}

pub(super) fn cancel_account(conn:&Connection,line:&str)->Result<(),&'static str>{
    let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_voice_notification_jobs')",[],|r|r.get(0)).map_err(|_|STORAGE)?;
    if exists{
        let mut query=conn.prepare("SELECT DISTINCT sms_id FROM app_voice_notification_jobs WHERE line_id=?1 AND state IN ('pending','sending')").map_err(|_|STORAGE)?;
        let ids=query.query_map([line],|r|r.get::<_,i64>(0)).map_err(|_|STORAGE)?.collect::<Result<Vec<_>,_>>().map_err(|_|STORAGE)?;
        drop(query);
        conn.execute("UPDATE app_voice_notification_jobs SET state='cancelled',claim=NULL,title='',body='' WHERE line_id=?1 AND state IN ('pending','sending')",[line]).map_err(|_|STORAGE)?;
        for sms in ids{aggregate_sms(conn,sms,line)?;}prune_terminal(conn)?;
    }
    Ok(())
}

#[cfg(test)]
#[path="account_notifications_tests.rs"]
mod tests;
