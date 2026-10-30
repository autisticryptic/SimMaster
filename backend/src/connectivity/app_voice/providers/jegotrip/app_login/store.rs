//! Small durable login-attempt ledger. No passwords, tokens or request bodies.
//! IDs are retained (bounded admission), so a repeated HTTP submission cannot
//! silently replay a business-changing request after restart.
use super::{AttemptView, Error, stored_code, valid_attempt_id};
use rusqlite::{Connection, OptionalExtension, params};
const STORAGE:&str="app_login_storage_failed";
const MAX_ATTEMPTS:i64=4096;
const PHASES:&[&str]=&["queued","authenticating","reading_bound_number","fetching_credentials","checking_business","enabling_business","confirming_business","creating_account","starting_receiver","completed","failed","interrupted"];
const BUSINESS:&[&str]=&["not_requested","request_uncertain","accepted_unconfirmed","enabled","already_enabled","rejected"];

pub(super) fn initialize(conn:&Connection,owner:&str,now:i64)->Result<(),&'static str> {
    let tx=rusqlite::Transaction::new_unchecked(conn,rusqlite::TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_login_attempts (
        attempt_id TEXT PRIMARY KEY, owner TEXT NOT NULL, phase TEXT NOT NULL,
        business_state TEXT NOT NULL DEFAULT 'not_requested', completed INTEGER NOT NULL DEFAULT 0,
        line_id TEXT, masked_number TEXT, error TEXT, server_code INTEGER,
        created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
    );").map_err(|_|STORAGE)?;
    // Caller holds the original database OS lock. No PID/age guessing and no
    // replay: an exited in-flight request may already have reached the service.
    tx.execute("UPDATE app_voice_login_attempts SET phase='interrupted',completed=1,error='app_login_interrupted',updated_at=?1
        WHERE owner<>?2 AND completed=0",params![now,owner]).map_err(|_|STORAGE)?;
    tx.commit().map_err(|_|STORAGE)
}
fn decode(row:&rusqlite::Row<'_>)->rusqlite::Result<AttemptView> {
    let id:String=row.get(0)?;let phase:String=row.get(1)?;let business:String=row.get(2)?;
    let error:Option<String>=row.get(6)?;let masked:Option<String>=row.get(5)?;
    if !valid_attempt_id(&id) || !PHASES.contains(&phase.as_str()) || !BUSINESS.contains(&business.as_str())
        || masked.as_ref().is_some_and(|s|!s.starts_with("***") || s.len()>7 || !s[3..].bytes().all(|b|b.is_ascii_digit())) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(AttemptView {attempt_id:id,phase,business_state:business,completed:row.get::<_,i64>(3)?!=0,line_id:row.get(4)?,masked_number:masked,
        error:error.as_deref().map(|s|stored_code(s).unwrap_or(STORAGE)),server_code:row.get(7)?,created_at:row.get(8)?,updated_at:row.get(9)?})
}
const COLUMNS:&str="attempt_id,phase,business_state,completed,line_id,masked_number,error,server_code,created_at,updated_at";
pub(super) fn get(conn:&Connection,id:&str)->Result<Option<AttemptView>,&'static str> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM app_voice_login_attempts WHERE attempt_id=?1"),[id],decode).optional().map_err(|_|STORAGE)
}
pub(super) fn list(conn:&Connection)->Result<Vec<AttemptView>,&'static str> {
    let mut stmt=conn.prepare(&format!("SELECT {COLUMNS} FROM app_voice_login_attempts ORDER BY created_at DESC,rowid DESC LIMIT 32")).map_err(|_|STORAGE)?;
    let rows=stmt.query_map([],decode).map_err(|_|STORAGE)?;
    rows.collect::<Result<Vec<_>,_>>().map_err(|_|STORAGE)
}
pub(super) fn insert(conn:&Connection,id:&str,owner:&str,now:i64)->Result<AttemptView,&'static str> {
    let tx=rusqlite::Transaction::new_unchecked(conn,rusqlite::TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
    if get(&tx,id)?.is_some() { return Err("app_login_attempt_exists"); }
    let count:i64=tx.query_row("SELECT COUNT(*) FROM app_voice_login_attempts",[],|r|r.get(0)).map_err(|_|STORAGE)?;
    if count>=MAX_ATTEMPTS { return Err("app_login_attempt_limit"); }
    tx.execute("INSERT INTO app_voice_login_attempts(attempt_id,owner,phase,created_at,updated_at) VALUES(?1,?2,'queued',?3,?3)",params![id,owner,now]).map_err(|_|STORAGE)?;
    let view=get(&tx,id)?.ok_or(STORAGE)?;tx.commit().map_err(|_|STORAGE)?;Ok(view)
}
fn changed(n:usize)->Result<(),&'static str> { if n==1 {Ok(())} else {Err(STORAGE)} }
pub(super) fn step(conn:&Connection,id:&str,owner:&str,phase:&str,business:Option<&str>,now:i64)->Result<(),&'static str> {
    if !PHASES.contains(&phase) || business.is_some_and(|b|!BUSINESS.contains(&b)) { return Err(STORAGE); }
    changed(conn.execute("UPDATE app_voice_login_attempts SET phase=?1,business_state=COALESCE(?2,business_state),updated_at=?3
        WHERE attempt_id=?4 AND owner=?5 AND completed=0",params![phase,business,now,id,owner]).map_err(|_|STORAGE)?)
}
pub(super) fn set_mask(conn:&Connection,id:&str,owner:&str,mask:&str,now:i64)->Result<(),&'static str> {
    changed(conn.execute("UPDATE app_voice_login_attempts SET masked_number=?1,updated_at=?2 WHERE attempt_id=?3 AND owner=?4 AND completed=0",
        params![mask,now,id,owner]).map_err(|_|STORAGE)?)
}
pub(super) fn set_line(conn:&Connection,id:&str,owner:&str,line:&str,now:i64)->Result<(),&'static str> {
    changed(conn.execute("UPDATE app_voice_login_attempts SET line_id=?1,updated_at=?2 WHERE attempt_id=?3 AND owner=?4 AND completed=0",
        params![line,now,id,owner]).map_err(|_|STORAGE)?)
}
pub(super) fn finish(conn:&Connection,id:&str,owner:&str,error:Option<Error>,now:i64)->Result<(),&'static str> {
    let phase=match error {None=>"completed",Some(e) if e.code=="app_login_interrupted"=>"interrupted",_=>"failed"};
    changed(conn.execute("UPDATE app_voice_login_attempts SET phase=?1,completed=1,error=?2,server_code=?3,updated_at=?4
        WHERE attempt_id=?5 AND owner=?6 AND completed=0",params![phase,error.map(|e|e.code),error.and_then(|e|e.server_code),now,id,owner]).map_err(|_|STORAGE)?)
}
