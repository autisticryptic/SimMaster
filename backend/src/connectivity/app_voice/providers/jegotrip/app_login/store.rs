//! Jegotrip durable attempts. Never password/token/request-body storage.
//! Legacy IDs and uncertain business results survive the provider-table rename.
use super::{AttemptView, Error, LoginIntent, stored_code, valid_attempt_id};
use rusqlite::{Connection, OptionalExtension, params, TransactionBehavior};
const STORAGE:&str="app_login_storage_failed";
const MAX_ATTEMPTS:i64=4096;
const PHASES:&[&str]=&["queued","authenticating","reading_bound_number","saving_app_account","fetching_credentials","checking_business","enabling_business","confirming_business","creating_account","starting_receiver","completed","failed","interrupted"];
const BUSINESS:&[&str]=&["not_requested","request_uncertain","accepted_unconfirmed","enabled","already_enabled","rejected"];
const LEGACY_COLUMNS:&str="attempt_id,owner,phase,business_state,completed,line_id,masked_number,error,server_code,created_at,updated_at";
fn exists(conn:&Connection,name:&str)->Result<bool,&'static str> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",[name],|r|r.get(0)).map_err(|_|STORAGE)
}
fn columns(conn:&Connection)->Result<Vec<String>,&'static str> {
    let mut stmt=conn.prepare("PRAGMA table_info(jegotrip_login_attempts)").map_err(|_|STORAGE)?;
    let rows=stmt.query_map([],|r|r.get(1)).map_err(|_|STORAGE)?;
    rows.collect::<Result<Vec<_>,_>>().map_err(|_|STORAGE)
}
pub(super) fn initialize(conn:&Connection,owner:&str,now:i64)->Result<(),&'static str> {
    let tx=rusqlite::Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
    let old=exists(&tx,"app_voice_login_attempts")?;
    let new=exists(&tx,"jegotrip_login_attempts")?;
    if old && new {
        let old_count:i64=tx.query_row("SELECT COUNT(*) FROM app_voice_login_attempts",[],|r|r.get(0)).map_err(|_|STORAGE)?;
        let new_count:i64=tx.query_row("SELECT COUNT(*) FROM jegotrip_login_attempts",[],|r|r.get(0)).map_err(|_|STORAGE)?;
        if old_count==0 {
            tx.execute_batch("DROP TABLE app_voice_login_attempts;").map_err(|_|STORAGE)?;
        } else if new_count==0 {
            tx.execute_batch("DROP TABLE jegotrip_login_attempts; ALTER TABLE app_voice_login_attempts RENAME TO jegotrip_login_attempts;").map_err(|_|STORAGE)?;
        } else {
            // Ambiguous dual writers/partial manual migrations are not a reason
            // to discard IDs or an uncertain request. Leave both tables intact.
            return Err("app_login_migration_conflict");
        }
    } else if old {
        tx.execute_batch("ALTER TABLE app_voice_login_attempts RENAME TO jegotrip_login_attempts;").map_err(|_|STORAGE)?;
    }
    tx.execute_batch("CREATE TABLE IF NOT EXISTS jegotrip_login_attempts (
        attempt_id TEXT PRIMARY KEY, owner TEXT NOT NULL, phase TEXT NOT NULL,
        business_state TEXT NOT NULL DEFAULT 'not_requested', completed INTEGER NOT NULL DEFAULT 0,
        line_id TEXT, masked_number TEXT, error TEXT, server_code INTEGER,
        created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
        intent TEXT NOT NULL DEFAULT 'enable_hosting', account_id TEXT
    );").map_err(|_|STORAGE)?;
    let cols=columns(&tx)?;
    if !LEGACY_COLUMNS.split(',').all(|name|cols.iter().any(|s|s==name)) {return Err(STORAGE);}
    if !cols.iter().any(|s|s=="intent") {
        tx.execute_batch("ALTER TABLE jegotrip_login_attempts ADD COLUMN intent TEXT NOT NULL DEFAULT 'enable_hosting';").map_err(|_|STORAGE)?;
    }
    if !cols.iter().any(|s|s=="account_id") {
        tx.execute_batch("ALTER TABLE jegotrip_login_attempts ADD COLUMN account_id TEXT;").map_err(|_|STORAGE)?;
    }
    super::settings::initialize(&tx)?;
    super::accounts::initialize(&tx)?;
    // Caller holds the original database OS lock; no PID/age guessing, replay
    // or schema-level manipulation of SIM/IMS/history/application run tables.
    tx.execute("UPDATE jegotrip_login_attempts SET phase='interrupted',completed=1,error='app_login_interrupted',updated_at=?1
        WHERE owner<>?2 AND completed=0",params![now,owner]).map_err(|_|STORAGE)?;
    tx.commit().map_err(|_|STORAGE)
}
fn decode(row:&rusqlite::Row<'_>)->rusqlite::Result<AttemptView> {
    let id:String=row.get(0)?;let phase:String=row.get(1)?;let business:String=row.get(2)?;
    let error:Option<String>=row.get(6)?;let masked:Option<String>=row.get(5)?;
    let intent:String=row.get(10)?;let account_id:Option<String>=row.get(11)?;
    let intent=LoginIntent::parse(&intent).ok_or(rusqlite::Error::InvalidQuery)?;
    if !valid_attempt_id(&id) || !PHASES.contains(&phase.as_str()) || !BUSINESS.contains(&business.as_str())
        || account_id.as_ref().is_some_and(|s|!valid_attempt_id(s))
        || masked.as_ref().is_some_and(|s|!s.starts_with("***") || s.len()>7 || !s[3..].bytes().all(|b|b.is_ascii_digit())) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(AttemptView {attempt_id:id,phase,business_state:business,completed:row.get::<_,i64>(3)?!=0,line_id:row.get(4)?,masked_number:masked,
        error:error.as_deref().map(|s|stored_code(s).unwrap_or(STORAGE)),server_code:row.get(7)?,created_at:row.get(8)?,updated_at:row.get(9)?,intent,account_id})
}
const COLUMNS:&str="attempt_id,phase,business_state,completed,line_id,masked_number,error,server_code,created_at,updated_at,intent,account_id";
pub(super) fn get(conn:&Connection,id:&str)->Result<Option<AttemptView>,&'static str> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM jegotrip_login_attempts WHERE attempt_id=?1"),[id],decode).optional().map_err(|_|STORAGE)
}
pub(super) fn list(conn:&Connection)->Result<Vec<AttemptView>,&'static str> {
    let mut stmt=conn.prepare(&format!("SELECT {COLUMNS} FROM jegotrip_login_attempts ORDER BY created_at DESC,rowid DESC LIMIT 32")).map_err(|_|STORAGE)?;
    let rows=stmt.query_map([],decode).map_err(|_|STORAGE)?;
    rows.collect::<Result<Vec<_>,_>>().map_err(|_|STORAGE)
}
#[cfg(test)]
pub(super) fn insert(conn:&Connection,id:&str,owner:&str,now:i64)->Result<AttemptView,&'static str> {
    insert_with_intent(conn,id,owner,LoginIntent::EnableHosting,now)
}
pub(super) fn insert_with_intent(conn:&Connection,id:&str,owner:&str,intent:LoginIntent,now:i64)->Result<AttemptView,&'static str> {
    let tx=rusqlite::Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
    if get(&tx,id)?.is_some() {return Err("app_login_attempt_exists");}
    let count:i64=tx.query_row("SELECT COUNT(*) FROM jegotrip_login_attempts",[],|r|r.get(0)).map_err(|_|STORAGE)?;
    if count>=MAX_ATTEMPTS {return Err("app_login_attempt_limit");}
    tx.execute("INSERT INTO jegotrip_login_attempts(attempt_id,owner,phase,intent,created_at,updated_at) VALUES(?1,?2,'queued',?3,?4,?4)",
        params![id,owner,intent.as_str(),now]).map_err(|_|STORAGE)?;
    let view=get(&tx,id)?.ok_or(STORAGE)?;tx.commit().map_err(|_|STORAGE)?;Ok(view)
}
fn changed(n:usize)->Result<(),&'static str> {if n==1 {Ok(())}else{Err(STORAGE)}}
pub(super) fn step(conn:&Connection,id:&str,owner:&str,phase:&str,business:Option<&str>,now:i64)->Result<(),&'static str> {
    if !PHASES.contains(&phase) || business.is_some_and(|b|!BUSINESS.contains(&b)) {return Err(STORAGE);}
    changed(conn.execute("UPDATE jegotrip_login_attempts SET phase=?1,business_state=COALESCE(?2,business_state),updated_at=?3
        WHERE attempt_id=?4 AND owner=?5 AND completed=0",params![phase,business,now,id,owner]).map_err(|_|STORAGE)?)
}
pub(super) fn set_mask(conn:&Connection,id:&str,owner:&str,mask:&str,now:i64)->Result<(),&'static str> {
    changed(conn.execute("UPDATE jegotrip_login_attempts SET masked_number=?1,updated_at=?2 WHERE attempt_id=?3 AND owner=?4 AND completed=0",
        params![mask,now,id,owner]).map_err(|_|STORAGE)?)
}
pub(super) fn set_account(conn:&Connection,id:&str,owner:&str,account:&str,now:i64)->Result<(),&'static str> {
    changed(conn.execute("UPDATE jegotrip_login_attempts SET account_id=?1,updated_at=?2 WHERE attempt_id=?3 AND owner=?4 AND completed=0",
        params![account,now,id,owner]).map_err(|_|STORAGE)?)
}
pub(super) fn set_line(conn:&Connection,id:&str,owner:&str,line:&str,now:i64)->Result<(),&'static str> {
    let tx=rusqlite::Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
    let account:Option<String>=tx.query_row("SELECT account_id FROM jegotrip_login_attempts WHERE attempt_id=?1 AND owner=?2 AND completed=0",
        params![id,owner],|r|r.get(0)).map_err(|_|STORAGE)?;
    changed(tx.execute("UPDATE jegotrip_login_attempts SET line_id=?1,updated_at=?2 WHERE attempt_id=?3 AND owner=?4 AND completed=0",
        params![line,now,id,owner]).map_err(|_|STORAGE)?)?;
    if let Some(account)=account {super::accounts::link_voice_line(&tx,&account,line)?;}
    tx.commit().map_err(|_|STORAGE)
}
pub(super) fn finish(conn:&Connection,id:&str,owner:&str,error:Option<Error>,now:i64)->Result<(),&'static str> {
    let phase=match error {None=>"completed",Some(e)if e.code=="app_login_interrupted"=>"interrupted",_=>"failed"};
    changed(conn.execute("UPDATE jegotrip_login_attempts SET phase=?1,completed=1,error=?2,server_code=?3,updated_at=?4
        WHERE attempt_id=?5 AND owner=?6 AND completed=0",params![phase,error.map(|e|e.code),error.and_then(|e|e.server_code),now,id,owner]).map_err(|_|STORAGE)?)
}
