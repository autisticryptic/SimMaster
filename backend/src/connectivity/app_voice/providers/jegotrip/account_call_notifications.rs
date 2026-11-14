//! Original-call notification jobs, not SMS rows, invitation ACKs or media grants.
//! Only a live call's owned worker dispatches them; orphan work is never replayed.
use super::{account_call_history::HistoryTicket,account_notifications::{NoticePlan,NoticeTarget},account_runs::{self,RunToken},account_store};
use crate::platform::db::{Database,CallRecord};
use rusqlite::{Connection,Transaction,TransactionBehavior,OptionalExtension,params};
use std::{fmt,future::Future,pin::Pin,time::Duration};
use zeroize::Zeroizing;
const STORAGE:&str="app_account_call_notification_storage_failed";
const MAX_JOBS:i64=512;const MAX_BYTES:i64=4*1024*1024;
pub(super) const WORKERS:usize=4;
pub(super) const WINDOW:Duration=Duration::from_secs(30);
pub(crate) trait CallNoticeSnapshot:Send+Sync{fn plan(&self,record:&CallRecord)->Result<NoticePlan,&'static str>;}
pub(crate) enum CallNoticeOutcome{Delivered,Deferred(Duration),Failed,TargetChanged,Uncertain}
pub(crate) type CallNoticeFuture<'a>=Pin<Box<dyn Future<Output=CallNoticeOutcome>+Send+'a>>;
pub(crate) struct CallNoticeWork{
    pub id:i64,pub call_history_id:i64,pub line_id:String,pub rule_id:String,pub channel_id:String,pub fingerprint:String,
    pub title:Zeroizing<String>,pub body:Zeroizing<String>,claim:String,call_token:String,expires:i64,
}
impl fmt::Debug for CallNoticeWork{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("CallNoticeWork(<redacted>)")}}
#[derive(serde::Serialize)]
pub struct CallNoticeStatus{pub id:i64,pub call_history_id:i64,pub state:&'static str,pub attempts:i64,pub expires_at:i64}
fn now()->i64{chrono::Utc::now().timestamp().max(0)}
fn table(conn:&Connection,name:&str)->Result<bool,&'static str>{conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",[name],|r|r.get(0)).map_err(|_|STORAGE)}
pub fn initialize_schema(conn:&Connection)->Result<(),&'static str>{
    conn.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_call_notification_jobs(
        id INTEGER PRIMARY KEY AUTOINCREMENT,call_history_id INTEGER NOT NULL,line_id TEXT NOT NULL,call_token TEXT NOT NULL,
        run_nonce TEXT NOT NULL,revision INTEGER NOT NULL,rule_id TEXT NOT NULL,channel_id TEXT NOT NULL,fingerprint TEXT NOT NULL,
        title TEXT NOT NULL,body TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN ('pending','sending','delivered','failed','cancelled','uncertain','expired')),
        claim TEXT,attempts INTEGER NOT NULL DEFAULT 0,not_before INTEGER NOT NULL,expires_at INTEGER NOT NULL,created_at INTEGER NOT NULL,
        UNIQUE(call_token,rule_id,channel_id));
        CREATE INDEX IF NOT EXISTS idx_app_call_notice_due ON app_voice_call_notification_jobs(state,not_before,id);").map_err(|_|STORAGE)
}
pub(super) fn sending_count(conn:&Connection)->Result<i64,&'static str>{if !table(conn,"app_voice_call_notification_jobs")?{return Ok(0);}conn.query_row("SELECT COUNT(*) FROM app_voice_call_notification_jobs WHERE state='sending'",[],|r|r.get(0)).map_err(|_|STORAGE)}
pub(super) fn channel_sending(conn:&Connection,channel:&str)->Result<bool,&'static str>{if !table(conn,"app_voice_call_notification_jobs")?{return Ok(false);}conn.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_call_notification_jobs WHERE state='sending' AND channel_id=?1)",[channel],|r|r.get(0)).map_err(|_|STORAGE)}
fn sms_sending(conn:&Connection,channel:Option<&str>)->Result<i64,&'static str>{
    if !table(conn,"app_voice_notification_jobs")?{return Ok(0);}
    conn.query_row("SELECT COUNT(*) FROM app_voice_notification_jobs WHERE state='sending' AND (?1 IS NULL OR channel_id=?1)",[channel],|r|r.get(0)).map_err(|_|STORAGE)
}
fn prune(conn:&Connection)->Result<(),&'static str>{conn.execute("DELETE FROM app_voice_call_notification_jobs WHERE state NOT IN ('pending','sending') AND id NOT IN (SELECT id FROM app_voice_call_notification_jobs WHERE state NOT IN ('pending','sending') ORDER BY id DESC LIMIT 256)",[]).map_err(|_|STORAGE)?;Ok(())}
fn original(conn:&Connection,run:&RunToken,ticket:&HistoryTicket)->Result<bool,&'static str>{
    if !account_runs::runtime_matches(conn,run).map_err(|_|STORAGE)?{return Ok(false);}
    if account_store::metadata(conn,run.line_id()).map_err(|_|STORAGE)?.is_none_or(|m|m.revision!=run.revision()){return Ok(false);}
    if account_runs::require_live(conn,run.line_id(),run.revision(),run.nonce()).is_err(){return Ok(false);}
    conn.query_row("SELECT EXISTS(SELECT 1 FROM call_history WHERE id=?1 AND line_id=?2 AND app_call_token=?3 AND direction='incoming' AND answered=0 AND end_time IS NULL)",params![ticket.id(),run.line_id(),ticket.token()],|r|r.get(0)).map_err(|_|STORAGE)
}
pub(super) fn enqueue(db:&Database,run:&RunToken,ticket:&HistoryTicket,record:&CallRecord,plan:NoticePlan)->Result<(),&'static str>{
    if record.id!=ticket.id()||record.line_id.as_deref()!=Some(run.line_id())||record.direction!="incoming"||record.answered||record.end_time.is_some(){return Err(STORAGE);}
    db.with_conn(|conn|{
        let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
        if !original(&tx,run,ticket)?{return Err("app_account_call_scope_invalid");}prune(&tx)?;
        let targets=plan.targets();let(count,bytes):(i64,i64)=tx.query_row("SELECT COUNT(*),COALESCE(SUM(length(CAST(title AS BLOB))+length(CAST(body AS BLOB))),0) FROM app_voice_call_notification_jobs",[],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|STORAGE)?;
        let added=targets.iter().map(|t|t.title.len()+t.body.len()).sum::<usize>() as i64;
        if count+targets.len() as i64>MAX_JOBS||bytes.checked_add(added).is_none_or(|n|n>MAX_BYTES){return Err("app_account_call_notification_queue_full");}
        let at=now();let expires=at.checked_add(WINDOW.as_secs() as i64).ok_or(STORAGE)?;
        for NoticeTarget{rule_id,channel_id,fingerprint,title,body} in targets{
            tx.execute("INSERT INTO app_voice_call_notification_jobs(call_history_id,line_id,call_token,run_nonce,revision,rule_id,channel_id,fingerprint,title,body,state,not_before,expires_at,created_at)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'pending',?11,?12,?11) ON CONFLICT(call_token,rule_id,channel_id) DO NOTHING",
                params![ticket.id(),run.line_id(),ticket.token(),run.nonce(),run.revision() as i64,rule_id,channel_id,fingerprint,title,body,at,expires]).map_err(|_|STORAGE)?;
        }tx.commit().map_err(|_|STORAGE)
    })
}
pub(super) fn claim_next(db:&Database,run:&RunToken,ticket:&HistoryTicket)->Result<Option<CallNoticeWork>,&'static str>{
    db.with_conn(|conn|{
        let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;let at=now();
        if !original(&tx,run,ticket)?{return Ok(None);}
        if sending_count(&tx)?+sms_sending(&tx,None)?>=WORKERS as i64{return Ok(None);}
        let row:Option<(i64,String,String,String,String,String)>=tx.query_row("SELECT id,rule_id,channel_id,fingerprint,title,body FROM app_voice_call_notification_jobs
            WHERE call_history_id=?1 AND line_id=?2 AND call_token=?3 AND run_nonce=?4 AND revision=?5 AND state='pending' AND not_before<=?6 AND expires_at>?6 AND created_at<=?6 AND attempts<5
                AND typeof(rule_id)='text' AND length(CAST(rule_id AS BLOB)) BETWEEN 1 AND 128 AND typeof(channel_id)='text' AND length(CAST(channel_id AS BLOB)) BETWEEN 1 AND 128
                AND typeof(fingerprint)='text' AND length(CAST(fingerprint AS BLOB))=64 AND typeof(title)='text' AND typeof(body)='text'
                AND length(CAST(title AS BLOB))<=8192 AND length(CAST(body AS BLOB))<=131072 ORDER BY id LIMIT 1",
            params![ticket.id(),run.line_id(),ticket.token(),run.nonce(),run.revision() as i64,at],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional().map_err(|_|STORAGE)?;
        let Some((id,rule_id,channel_id,fingerprint,title,body))=row else{return Ok(None);};
        if channel_sending(&tx,&channel_id)?||sms_sending(&tx,Some(&channel_id))?>0{return Ok(None);}
        let claim=super::account_runtime::local_nonce()?;
        let changed=tx.execute("UPDATE app_voice_call_notification_jobs SET state='sending',claim=?1,attempts=attempts+1 WHERE id=?2 AND state='pending'",params![claim,id]).map_err(|_|STORAGE)?;if changed!=1{return Err(STORAGE);}
        let expires=tx.query_row("SELECT expires_at FROM app_voice_call_notification_jobs WHERE id=?1",[id],|r|r.get(0)).map_err(|_|STORAGE)?;
        tx.commit().map_err(|_|STORAGE)?;
        Ok(Some(CallNoticeWork{id,call_history_id:ticket.id(),line_id:run.line_id().to_owned(),rule_id,channel_id,fingerprint,title:Zeroizing::new(title),body:Zeroizing::new(body),claim,call_token:ticket.token().to_owned(),expires}))
    })
}
pub(super) fn admitted(db:&Database,run:&RunToken,ticket:&HistoryTicket,work:&CallNoticeWork)->Result<bool,&'static str>{
    db.with_conn(|conn|{if !original(conn,run,ticket)?{return Ok(false);}conn.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_call_notification_jobs WHERE id=?1 AND claim=?2 AND call_token=?3 AND state='sending' AND expires_at>?4)",params![work.id,work.claim,work.call_token,now()],|r|r.get(0)).map_err(|_|STORAGE)})
}
pub(super) fn finish(db:&Database,work:&CallNoticeWork,outcome:CallNoticeOutcome)->Result<(),&'static str>{
    let at=now();let(state,due,refund)=match outcome{
        CallNoticeOutcome::Delivered=>("delivered",at,false),CallNoticeOutcome::Failed=>("failed",at,false),CallNoticeOutcome::TargetChanged=>("cancelled",at,false),
        CallNoticeOutcome::Deferred(delay) if at<work.expires=>("pending",at.saturating_add(delay.as_secs().clamp(1,30) as i64),true),_ =>("uncertain",at,false),
    };
    db.with_conn(|conn|{conn.execute("UPDATE app_voice_call_notification_jobs SET state=?1,not_before=?2,claim=NULL,attempts=attempts-?3,
        title=CASE WHEN ?1='pending' THEN title ELSE '' END,body=CASE WHEN ?1='pending' THEN body ELSE '' END WHERE id=?4 AND claim=?5 AND call_token=?6 AND state='sending'",
        params![state,due,i64::from(refund),work.id,work.claim,work.call_token]).map_err(|_|STORAGE)?;Ok(())})
}
pub(super) fn uncertain(db:&Database,work:&CallNoticeWork){let _=db.with_conn(|c|{c.execute("UPDATE app_voice_call_notification_jobs SET state='uncertain',claim=NULL,title='',body='' WHERE id=?1 AND claim=?2 AND call_token=?3 AND state='sending'",params![work.id,work.claim,work.call_token]).map_err(|_|STORAGE)?;Ok(())});}
pub(super) fn cancel(db:&Database,ticket:&HistoryTicket){let _=db.with_conn(|c|cancel_for_conn(c,ticket.id(),ticket.line(),ticket.token()));}
fn cancel_for_conn(conn:&Connection,id:i64,line:&str,token:&str)->Result<(),&'static str>{
    conn.execute("UPDATE app_voice_call_notification_jobs SET state=CASE WHEN state='sending' THEN 'uncertain' ELSE 'cancelled' END,claim=NULL,title='',body='' WHERE call_history_id=?1 AND line_id=?2 AND call_token=?3 AND state IN ('pending','sending')",params![id,line,token]).map_err(|_|STORAGE)?;Ok(())
}
pub(super) fn cancel_account(conn:&Connection,line:&str)->Result<(),&'static str>{if !table(conn,"app_voice_call_notification_jobs")?{return Ok(());}conn.execute("UPDATE app_voice_call_notification_jobs SET state=CASE WHEN state='sending' THEN 'uncertain' ELSE 'cancelled' END,claim=NULL,title='',body='' WHERE line_id=?1 AND state IN ('pending','sending')",[line]).map_err(|_|STORAGE)?;Ok(())}
pub(super) fn recover(conn:&Connection)->Result<(),&'static str>{
    if !table(conn,"app_voice_call_notification_jobs")?{return Ok(());}
    conn.execute("UPDATE app_voice_call_notification_jobs SET state=CASE WHEN state='sending' THEN 'uncertain' ELSE 'cancelled' END,claim=NULL,title='',body='' WHERE state IN ('pending','sending')",[]).map_err(|_|STORAGE)?;Ok(())
}
pub(super) fn pending(db:&Database,ticket:&HistoryTicket)->Result<bool,&'static str>{
    db.with_conn(|c|c.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_call_notification_jobs WHERE call_history_id=?1 AND line_id=?2 AND call_token=?3 AND state IN ('pending','sending'))",params![ticket.id(),ticket.line(),ticket.token()],|r|r.get(0)).map_err(|_|STORAGE))
}
pub(super) fn expire(db:&Database,ticket:&HistoryTicket){let _=db.with_conn(|c|{c.execute("UPDATE app_voice_call_notification_jobs SET state=CASE WHEN state='sending' THEN 'uncertain' ELSE 'expired' END,claim=NULL,title='',body='' WHERE call_history_id=?1 AND line_id=?2 AND call_token=?3 AND state IN ('pending','sending')",params![ticket.id(),ticket.line(),ticket.token()]).map_err(|_|STORAGE)?;Ok(())});}
pub(super) fn metadata(conn:&Connection,line:&str,limit:usize)->Result<Vec<CallNoticeStatus>,&'static str>{
    if !account_store::valid_line_id(line){return Err(STORAGE);}
    let mut query=conn.prepare("SELECT id,call_history_id,CASE WHEN typeof(state)='text' AND length(CAST(state AS BLOB))<=9 THEN state ELSE '' END,attempts,expires_at FROM app_voice_call_notification_jobs WHERE line_id=?1 ORDER BY id DESC LIMIT ?2").map_err(|_|STORAGE)?;
    let rows=query.query_map(params![line,limit.clamp(1,128) as i64],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?))).map_err(|_|STORAGE)?;
    let mut out=Vec::new();for row in rows{let(id,call_history_id,state,attempts,expires_at)=row.map_err(|_|STORAGE)?;let state=match state.as_str(){"pending"=>"pending","sending"=>"sending","delivered"=>"delivered","failed"=>"failed","cancelled"=>"cancelled","uncertain"=>"uncertain","expired"=>"expired",_=>return Err(STORAGE)};out.push(CallNoticeStatus{id,call_history_id,state,attempts,expires_at});}Ok(out)
}
