//! Jegotrip-only private installation configuration. No secret-bearing GET DTO.
use super::{config::DatabaseConfiguration, Error};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use zeroize::Zeroizing;

const STORAGE:&str="app_login_storage_failed";
pub(super) enum StoredConfiguration {
    Missing,
    Database { revision:u64, configuration:Option<DatabaseConfiguration> },
}

pub(super) fn initialize(conn:&Connection)->Result<(),&'static str> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS jegotrip_login_settings (
        id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL CHECK(revision>=1),
        configuration BLOB, updated_at INTEGER NOT NULL
    );").map_err(|_|STORAGE)
}

pub(super) fn load(conn:&Connection)->Result<StoredConfiguration,&'static str> {
    let row:Option<(i64,String,Option<Vec<u8>>)>=conn.query_row(
        "SELECT revision,typeof(configuration),CASE WHEN typeof(configuration)='blob'
         AND length(configuration) BETWEEN 1 AND 262144 THEN configuration ELSE NULL END
         FROM jegotrip_login_settings WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))
        .optional().map_err(|_|STORAGE)?;
    let Some((revision,kind,bytes))=row else{return Ok(StoredConfiguration::Missing);};
    let revision=u64::try_from(revision).ok().filter(|n|*n>=1).ok_or(STORAGE)?;
    if kind=="null" { return Ok(StoredConfiguration::Database {revision,configuration:None}); }
    if kind!="blob" { return Err("app_login_deployment_invalid"); }
    let bytes=Zeroizing::new(bytes.ok_or("app_login_deployment_invalid")?);
    let configuration=DatabaseConfiguration::decode_private(&bytes).map_err(|e|e.code)?;
    Ok(StoredConfiguration::Database {revision,configuration:Some(configuration)})
}

/// CAS is based on a monotonically retained row, including an explicit clear.
/// Clearing never restores the historical private-file fallback automatically.
pub(super) fn save(conn:&Connection,expected:u64,configuration:Option<&DatabaseConfiguration>,now:i64)->Result<u64,Error> {
    let bytes=configuration.map(DatabaseConfiguration::encode_private).transpose()?;
    let tx=rusqlite::Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|Error::new(STORAGE))?;
    let revision:Option<i64>=tx.query_row("SELECT revision FROM jegotrip_login_settings WHERE id=1",[],|r|r.get(0))
        .optional().map_err(|_|Error::new(STORAGE))?;
    let current=revision.map(|n|u64::try_from(n).ok().filter(|n|*n>=1)).unwrap_or(Some(0)).ok_or(Error::new(STORAGE))?;
    if current!=expected {return Err(Error::new("app_login_configuration_revision_conflict"));}
    let next=current.checked_add(1).filter(|n|*n<=i64::MAX as u64).ok_or(Error::new(STORAGE))?;
    tx.execute("INSERT INTO jegotrip_login_settings(id,revision,configuration,updated_at) VALUES(1,?1,?2,?3)
        ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,configuration=excluded.configuration,updated_at=excluded.updated_at",
        params![next as i64,bytes.as_ref().map(|v|v.as_slice()),now]).map_err(|_|Error::new(STORAGE))?;
    tx.commit().map_err(|_|Error::new(STORAGE))?;Ok(next)
}
