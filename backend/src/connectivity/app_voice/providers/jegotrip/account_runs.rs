//! Private original-run fences and post-release inbox compaction.
//!
//! This never interprets PID/age/phase/transport ACK as release. Only the
//! account runtime's original controller cleanup PLUS joined consumer produces
//! RunQuiescence. Unknown legacy/orphan runs are retained, not guessed ended.
//! Shared SMS history is independent of its compactable private envelope.
use super::{account_store, account_runtime::RunQuiescence};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::fmt;

const STORAGE: &str = "app_account_run_storage_failed";
const SCOPE: &str = "app_account_run_scope_invalid";
const MAX_RUNS_PER_ACCOUNT: i64 = 256;
const MAX_RUNS_TOTAL: i64 = 4096;

pub(super) struct RunToken { line: String, revision: u64, nonce: String, exclusive_runtime: bool }
impl fmt::Debug for RunToken {
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {f.write_str("RunToken(<redacted>)")}
}
impl RunToken {
    pub(super) fn line_id(&self)->&str {&self.line}
    pub(super) fn revision(&self)->u64 {self.revision}
    pub(super) fn nonce(&self)->&str {&self.nonce}
}
pub fn initialize_schema(conn:&Connection)->Result<(), &'static str> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_inbox_runs(
        line_id TEXT NOT NULL,revision INTEGER NOT NULL,run_nonce TEXT NOT NULL,
        state TEXT NOT NULL CHECK(state IN ('live','released')),
        PRIMARY KEY(line_id,revision,run_nonce));
        CREATE TABLE IF NOT EXISTS app_voice_runtime_leases(
        line_id TEXT PRIMARY KEY NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),
        run_nonce TEXT NOT NULL CHECK(length(run_nonce)=32));").map_err(|_|STORAGE)?;
    // Process-instance tag for OS-lock-proven orphan recovery. NULL = legacy
    // holder from a binary without the claim; never recovered automatically.
    let mut query=conn.prepare("PRAGMA table_info(app_voice_runtime_leases)").map_err(|_|STORAGE)?;
    let columns=query.query_map([],|row|row.get::<_,String>(1)).map_err(|_|STORAGE)?.collect::<Result<Vec<_>,_>>().map_err(|_|STORAGE)?;
    drop(query);
    if !columns.iter().any(|column|column=="holder"){conn.execute("ALTER TABLE app_voice_runtime_leases ADD COLUMN holder TEXT",[]).map_err(|_|STORAGE)?;}
    Ok(())
}
fn valid_nonce(value:&str)->bool {
    value.len()==32 && value.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))
}
/// A durable local account exclusion, not remote principal/device ownership.
/// No age/PID/phase is used to release or take over an unowned original run.
pub(super) fn runtime_held(conn:&Connection,line:&str)->Result<bool,&'static str> {
    let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_voice_runtime_leases')",[],|row|row.get(0)).map_err(|_|STORAGE)?;
    if !exists{return Ok(false);}
    conn.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_runtime_leases WHERE line_id=?1)",[line],|row|row.get(0)).map_err(|_|STORAGE)
}
pub(super) fn ownership_pending(conn:&Connection,line:&str)->Result<bool,&'static str> {
    if runtime_held(conn,line)?{return Ok(true);}
    conn.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_inbox_runs WHERE line_id=?1 AND state='live')",[line],|row|row.get(0)).map_err(|_|STORAGE)
}
pub(super) fn runtime_matches(conn:&Connection,token:&RunToken)->Result<bool,&'static str>{
    if !token.exclusive_runtime{return Ok(false);}
    conn.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_runtime_leases WHERE line_id=?1 AND revision=?2 AND run_nonce=?3)",
        params![token.line,token.revision as i64,token.nonce],|row|row.get(0)).map_err(|_|STORAGE)
}
/// Fence-only fixture registration does not grant production runtime ownership.
#[cfg(test)]
pub(super) fn register(conn:&Connection,line:&str,revision:u64,nonce:&str)->Result<RunToken,&'static str> {
    register_inner(conn,line,revision,nonce,false,None)
}
pub(super) fn register_runtime(conn:&Connection,line:&str,revision:u64,nonce:&str)->Result<RunToken,&'static str> {
    register_inner(conn,line,revision,nonce,true,None)
}
/// Same exclusive registration, tagged with the OS-locked process instance.
pub(super) fn register_runtime_held(conn:&Connection,line:&str,revision:u64,nonce:&str,holder:Option<&str>)->Result<RunToken,&'static str> {
    if holder.is_some_and(|holder|!valid_nonce(holder)){return Err(SCOPE);}
    register_inner(conn,line,revision,nonce,true,holder)
}
fn register_inner(conn:&Connection,line:&str,revision:u64,nonce:&str,exclusive_runtime:bool,holder:Option<&str>)->Result<RunToken,&'static str> {
    if !account_store::valid_line_id(line)||revision==0||revision>i64::MAX as u64||!valid_nonce(nonce){return Err(SCOPE);}
    let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
    let original=account_store::metadata(&tx,line)?.ok_or(SCOPE)?;
    if original.revision!=revision {return Err(SCOPE);}
    if exclusive_runtime && ownership_pending(&tx,line)? {return Err("app_account_stop_before_replace");}
    let (local,total):(i64,i64)=tx.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*) FROM app_voice_inbox_runs",[line],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|_|STORAGE)?;
    if local>=MAX_RUNS_PER_ACCOUNT||total>=MAX_RUNS_TOTAL{return Err("app_account_run_limit");}
    // Never upsert/reactivate a released fence or attach to somebody else's
    // original run just because its local nonce happens to be equal.
    let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_inbox_runs WHERE line_id=?1 AND revision=?2 AND run_nonce=?3)",params![line,revision as i64,nonce],|row|row.get(0)).map_err(|_|STORAGE)?;
    if exists{return Err(SCOPE);}
    tx.execute("INSERT INTO app_voice_inbox_runs(line_id,revision,run_nonce,state) VALUES(?1,?2,?3,'live')",params![line,revision as i64,nonce]).map_err(|_|STORAGE)?;
    if exclusive_runtime {
        tx.execute("INSERT INTO app_voice_runtime_leases(line_id,revision,run_nonce,holder) VALUES(?1,?2,?3,?4)",params![line,revision as i64,nonce,holder]).map_err(|_|STORAGE)?;
    }
    tx.commit().map_err(|_|STORAGE)?;
    Ok(RunToken{line:line.to_owned(),revision,nonce:nonce.to_owned(),exclusive_runtime})
}
pub(super) fn require_live(conn:&Connection,line:&str,revision:u64,nonce:&str)->Result<(), &'static str> {
    let state:Option<String>=conn.query_row("SELECT state FROM app_voice_inbox_runs WHERE line_id=?1 AND revision=?2 AND run_nonce=?3",
        params![line,revision as i64,nonce],|row|row.get(0)).optional().map_err(|_|STORAGE)?;
    if state.as_deref()!=Some("live"){return Err(SCOPE);}
    Ok(())
}

/// The caller cannot create this proof from a closed scope or saved phase.
/// The durable fence is committed WITH compaction: a late writer either
/// committed first, or sees no live run. It cannot resurrect reclaimed rows.
pub(super) fn release_and_compact(conn:&Connection,proof:&RunQuiescence)->Result<usize,&'static str> {
    let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
    let count=release_in(&tx,proof.token())?;tx.commit().map_err(|_|STORAGE)?;Ok(count)
}
fn release_in(tx:&Transaction<'_>,token:&RunToken)->Result<usize,&'static str> {
    let state:Option<String>=tx.query_row("SELECT state FROM app_voice_inbox_runs WHERE line_id=?1 AND revision=?2 AND run_nonce=?3",
        params![token.line,token.revision as i64,token.nonce],|row|row.get(0)).optional().map_err(|_|STORAGE)?;
    let leased=if token.exclusive_runtime {
        let held:Option<(i64,String)>=tx.query_row("SELECT revision,run_nonce FROM app_voice_runtime_leases WHERE line_id=?1",[&token.line],
            |row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|_|STORAGE)?;
        if held.as_ref().is_some_and(|(revision,nonce)|*revision!=token.revision as i64||nonce!=&token.nonce){return Err(SCOPE);}
        if held.is_some() && state.is_none() || held.is_none() && state.as_deref()==Some("live"){return Err(SCOPE);}
        held.is_some()
    }else{false};
    // A prior successful empty-run compaction may already have removed its
    // fence. The still-owned proof permits an idempotent repeat, never rebind.
    // The same owning Join proof can retire this run's direct replay watermark;
    // no other nonce/revision or pending/unknown run is implicitly swept.
    super::batch_retention::delete_run(&tx,&token.line,token.revision,&token.nonce)?;
    tx.execute("DELETE FROM app_voice_direct_replay_fences WHERE line_id=?1 AND revision=?2 AND run_nonce=?3",
        params![token.line,token.revision as i64,token.nonce]).map_err(|_|STORAGE)?;
    if state.is_none() {return Ok(0);}
    if !matches!(state.as_deref(),Some("live"|"released")){return Err(SCOPE);}
    tx.execute("UPDATE app_voice_inbox_runs SET state='released' WHERE line_id=?1 AND revision=?2 AND run_nonce=?3",
        params![token.line,token.revision as i64,token.nonce]).map_err(|_|STORAGE)?;
    // Mark surviving SMS metadata as detached before deleting the raw row;
    // the original local inbox id remains provenance, not a live foreign key.
    tx.execute("UPDATE app_voice_sms_projection SET envelope_retained=0 WHERE line_id=?1 AND revision=?2 AND inbox_id IN
        (SELECT id FROM app_voice_inbox WHERE line_id=?1 AND revision=?2 AND run_nonce=?3)",
        params![token.line,token.revision as i64,token.nonce]).map_err(|_|STORAGE)?;
    let count=tx.execute("DELETE FROM app_voice_inbox WHERE line_id=?1 AND revision=?2 AND run_nonce=?3
        AND EXISTS(SELECT 1 FROM app_voice_sms_projection p WHERE p.inbox_id=app_voice_inbox.id AND p.line_id=app_voice_inbox.line_id
        AND p.revision=app_voice_inbox.revision AND p.envelope_retained=0)",params![token.line,token.revision as i64,token.nonce]).map_err(|_|STORAGE)?;
    tx.execute("DELETE FROM app_voice_sms_projection WHERE line_id=?1 AND revision=?2 AND envelope_retained=0 AND deleted=1",
        params![token.line,token.revision as i64]).map_err(|_|STORAGE)?;
    // Released runs with unknown raw observations retain their write fence.
    // With no raw rows left, absence itself fails require_live forever.
    tx.execute("DELETE FROM app_voice_inbox_runs WHERE line_id=?1 AND revision=?2 AND run_nonce=?3 AND state='released'
        AND NOT EXISTS(SELECT 1 FROM app_voice_inbox WHERE line_id=?1 AND revision=?2 AND run_nonce=?3)",
        params![token.line,token.revision as i64,token.nonce]).map_err(|_|STORAGE)?;
    if leased {
        let removed=tx.execute("DELETE FROM app_voice_runtime_leases WHERE line_id=?1 AND revision=?2 AND run_nonce=?3",
            params![token.line,token.revision as i64,token.nonce]).map_err(|_|STORAGE)?;
        if removed!=1{return Err(SCOPE);}
    }
    Ok(count)
}

/// Leases tagged by ANOTHER process instance. Only meaningful to a caller that
/// holds the exclusive OS lock over this database, which every tagging process
/// held while it lived: the lock itself is the proof that holder has exited.
pub(super) fn orphaned_leases(conn:&Connection,me:&str)->Result<Vec<(String,u64,String,String)>,&'static str> {
    let mut query=conn.prepare("SELECT line_id,revision,run_nonce,holder FROM app_voice_runtime_leases WHERE holder IS NOT NULL AND holder!=?1 ORDER BY line_id")
        .map_err(|_|STORAGE)?;
    let rows=query.query_map([me],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)? as u64,row.get::<_,String>(2)?,row.get::<_,String>(3)?)))
        .map_err(|_|STORAGE)?.collect::<Result<Vec<_>,_>>().map_err(|_|STORAGE)?;
    Ok(rows)
}
/// Release one dead-process run: finish its still-open app calls (duration
/// unknown -> 0, recovered flag) and apply the ordinary release/compaction.
/// Unknown raw rows, SMS history and notification audit are retained.
pub(super) fn release_orphan(tx:&Transaction<'_>,lease:&(String,u64,String,String),now:&str)
    ->Result<Vec<crate::platform::db::AppEventEntry>,&'static str> {
    let (line,revision,nonce,holder)=lease;
    let still:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_runtime_leases WHERE line_id=?1 AND revision=?2 AND run_nonce=?3 AND holder=?4)",
        params![line,*revision as i64,nonce,holder],|row|row.get(0)).map_err(|_|STORAGE)?;
    if !still{return Ok(Vec::new());}
    let mut events=Vec::new();
    let open={
        let mut query=tx.prepare("SELECT id,answered FROM call_history WHERE line_id=?1 AND app_call_token IS NOT NULL AND end_time IS NULL").map_err(|_|STORAGE)?;
        let rows=query.query_map([line],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?))).map_err(|_|STORAGE)?
            .collect::<Result<Vec<_>,_>>().map_err(|_|STORAGE)?;rows
    };
    for (id,answered) in open {
        tx.execute("UPDATE call_history SET end_time=?2 WHERE id=?1 AND end_time IS NULL",params![id,now]).map_err(|_|STORAGE)?;
        let payload=serde_json::json!({"call_id":id,"duration":serde_json::Value::Null,"answered":answered!=0,"transport":"app_voice","recovered":true}).to_string();
        events.push(crate::platform::db::insert_app_event_for_conn(tx,"call.ended",Some(line.as_str()),Some("app_voice"),&payload,now).map_err(|_|STORAGE)?);
    }
    let token=RunToken{line:line.clone(),revision:*revision,nonce:nonce.clone(),exclusive_runtime:true};
    release_in(tx,&token)?;
    let payload=serde_json::json!({"recovered_run":true,"reason":"previous_process_exited"}).to_string();
    events.push(crate::platform::db::insert_app_event_for_conn(tx,"app_voice.runtime_recovered",Some(line.as_str()),Some("app_voice"),&payload,now).map_err(|_|STORAGE)?);
    Ok(events)
}

/// Explicit account deletion owns its history-removal decision. Do not erase
/// surviving shared SMS, or sweep another line/unknown user data to free space.
pub(super) fn delete_account_state(conn:&Connection,line:&str)->Result<(), &'static str> {
    let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_voice_inbox_runs')",[],|row|row.get(0)).map_err(|_|STORAGE)?;
    if exists {
        super::batch_retention::delete_account(conn,line)?;
        conn.execute("DELETE FROM app_voice_direct_replay_fences WHERE line_id=?1",[line]).map_err(|_|STORAGE)?;
        conn.execute("DELETE FROM app_voice_inbox_runs WHERE line_id=?1",[line]).map_err(|_|STORAGE)?;
    }
    Ok(())
}

#[cfg(test)]
#[path="account_runs_tests.rs"]
mod tests;
