//! Jegotrip App-account summaries. No passwords, App tokens or SDK-ready cache.
use super::{valid_attempt_id, Error};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use ring::rand::{SecureRandom, SystemRandom};

const STORAGE:&str="app_login_storage_failed";
#[derive(Debug,Clone,Serialize)]
pub struct JegotripAccountView {
    pub account_id:String,
    pub masked_login_name:String,
    pub masked_number:String,
    /// Last successful App authentication/user-info result, not a live grant.
    pub last_authenticated_at:i64,
    pub configuration_revision:u64,
    pub voice_line_id:Option<String>,
}
pub(super) fn initialize(conn:&Connection)->Result<(),&'static str> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS jegotrip_accounts (
        account_id TEXT PRIMARY KEY, identity_key BLOB NOT NULL UNIQUE,
        masked_login_name TEXT NOT NULL, masked_number TEXT NOT NULL,
        last_authenticated_at INTEGER NOT NULL, configuration_revision INTEGER NOT NULL,
        voice_line_id TEXT
    );").map_err(|_|STORAGE)
}
fn valid_mask(mask:&str)->bool {
    mask.starts_with("***") && mask.len()<=7 && mask[3..].bytes().all(|b|b.is_ascii_digit())
}
fn decode(row:&rusqlite::Row<'_>)->rusqlite::Result<JegotripAccountView> {
    let id:String=row.get(0)?;let hint:String=row.get(1)?;let number:String=row.get(2)?;
    let revision:i64=row.get(4)?;
    if !valid_attempt_id(&id) || hint.is_empty() || hint.len()>64 || hint.chars().any(char::is_control)
        || !valid_mask(&number) || revision<0 {return Err(rusqlite::Error::InvalidQuery);}
    Ok(JegotripAccountView {account_id:id,masked_login_name:hint,masked_number:number,
        last_authenticated_at:row.get(3)?,configuration_revision:revision as u64,voice_line_id:row.get(5)?})
}
const COLUMNS:&str="account_id,masked_login_name,masked_number,last_authenticated_at,configuration_revision,voice_line_id";
pub(super) fn list(conn:&Connection)->Result<Vec<JegotripAccountView>,&'static str> {
    let mut stmt=conn.prepare(&format!("SELECT {COLUMNS} FROM jegotrip_accounts ORDER BY last_authenticated_at DESC,rowid DESC LIMIT 64")).map_err(|_|STORAGE)?;
    let rows=stmt.query_map([],decode).map_err(|_|STORAGE)?;
    rows.collect::<Result<Vec<_>,_>>().map_err(|_|STORAGE)
}
pub(super) fn record(conn:&Connection,key:&[u8],hint:&str,mask:&str,revision:u64,now:i64)->Result<String,Error> {
    if key.len()!=32 || hint.is_empty() || hint.len()>64 || hint.chars().any(char::is_control)
        || !valid_mask(mask) || revision>i64::MAX as u64 {return Err(Error::new(STORAGE));}
    let tx=rusqlite::Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|Error::new(STORAGE))?;
    let old:Option<String>=tx.query_row("SELECT account_id FROM jegotrip_accounts WHERE identity_key=?1",[key],|r|r.get(0)).optional().map_err(|_|Error::new(STORAGE))?;
    let id=if let Some(id)=old {
        if !valid_attempt_id(&id) {return Err(Error::new(STORAGE));} id
    } else {
        let count:i64=tx.query_row("SELECT COUNT(*) FROM jegotrip_accounts",[],|r|r.get(0)).map_err(|_|Error::new(STORAGE))?;
        if count>=64 {return Err(Error::new("app_login_account_limit"));}
        let mut bytes=[0u8;16];SystemRandom::new().fill(&mut bytes).map_err(|_|Error::new("app_login_entropy_failed"))?;
        bytes.iter().map(|b|format!("{b:02x}")).collect::<String>()
    };
    tx.execute("INSERT INTO jegotrip_accounts(account_id,identity_key,masked_login_name,masked_number,last_authenticated_at,configuration_revision)
        VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(identity_key) DO UPDATE SET
        masked_login_name=excluded.masked_login_name,masked_number=excluded.masked_number,
        last_authenticated_at=excluded.last_authenticated_at,configuration_revision=excluded.configuration_revision",
        params![id,key,hint,mask,now,revision as i64]).map_err(|_|Error::new(STORAGE))?;
    tx.commit().map_err(|_|Error::new(STORAGE))?;Ok(id)
}
pub(super) fn link_voice_line(conn:&Connection,id:&str,line:&str)->Result<(),&'static str> {
    let changed=conn.execute("UPDATE jegotrip_accounts SET voice_line_id=?1 WHERE account_id=?2",params![line,id]).map_err(|_|STORAGE)?;
    if changed==1 {Ok(())}else{Err(STORAGE)}
}
