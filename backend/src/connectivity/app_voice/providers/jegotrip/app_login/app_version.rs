//! Durable, operator-visible App client version for the hosted App login.
//!
//! The App version is not a build-time constant: the official App upgrades
//! without notice and the server refuses the login with a version gate. The
//! resolved value therefore lives in the shared installation state (one row
//! per key, next to the other shared Jegotrip values), is learned from the iOS
//! App Store / official page on first use, and is re-learned when the server
//! reports the gate again. Nothing here guesses a version: a value is only
//! stored after a source produced a validated literal, and the last stored
//! value is kept when a re-check fails.

use super::Error;
use crate::platform::db::Database;
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Arc;

pub(super) const APP_VERSION_KEY:&str="app_version";
/// A stored value older than this is re-checked opportunistically, but is still
/// used when the re-check fails (a stale version beats no version).
pub(super) const STORE_TTL_SECONDS:i64=6*60*60;
/// In-memory reuse of a remote/store answer inside one client instance.
pub(super) const MEMORY_TTL_SECONDS:u64=60;

pub(super) const SOURCE_FILE:&str="file";
pub(super) const SOURCE_STORE:&str="store";
pub(super) const SOURCE_IOS:&str="ios";
pub(super) const SOURCE_PAGE:&str="page";
pub(super) const SOURCE_STATIC:&str="static";
pub(super) const SOURCE_MANUAL:&str="manual";

#[derive(Clone,Debug,PartialEq,Eq)]
pub(super) struct StoredVersion { pub version:String, pub source:String, pub observed_at:i64 }

/// Persistence seam. The production implementation is the installation
/// database; tests use a bounded in-memory map so no remote or file write is
/// needed to prove the resolution order.
pub(super) trait VersionStore: Send+Sync {
    fn load(&self)->Option<StoredVersion>;
    fn save(&self,version:&StoredVersion)->bool;
}

pub(super) fn initialize(conn:&Connection)->Result<(),&'static str> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS jegotrip_shared_state (
        name TEXT PRIMARY KEY,
        value TEXT NOT NULL,
        source TEXT NOT NULL,
        observed_at INTEGER NOT NULL
    );").map_err(|_|"app_login_storage_failed")
}

pub(super) fn load(conn:&Connection)->Option<StoredVersion> {
    let row:Option<(String,String,i64)>=conn.query_row(
        "SELECT value,source,observed_at FROM jegotrip_shared_state WHERE name=?1",
        params![APP_VERSION_KEY],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().ok()?;
    let (version,source,observed_at)=match row { Some(row)=>row, None=>return None };
    if source.len()>16 || observed_at<0 || !super::config::valid_app_version(&version) { return None; }
    Some(StoredVersion { version, source, observed_at })
}

pub(super) fn save(conn:&Connection,stored:&StoredVersion)->Result<(),&'static str> {
    if !super::config::valid_app_version(&stored.version) || stored.source.len()>16 { return Err("app_login_storage_failed"); }
    conn.execute("INSERT INTO jegotrip_shared_state (name,value,source,observed_at) VALUES (?1,?2,?3,?4)
        ON CONFLICT(name) DO UPDATE SET value=excluded.value, source=excluded.source, observed_at=excluded.observed_at",
        params![APP_VERSION_KEY,stored.version,stored.source,stored.observed_at])
        .map(|_|()).map_err(|_|"app_login_storage_failed")
}

/// Production store backed by the installation database. Reads and writes are
/// single short statements under the existing database connection.
pub(super) struct DatabaseVersionStore { database:Arc<Database> }
impl DatabaseVersionStore {
    pub(super) fn new(database:Arc<Database>)->Self { Self { database } }
}
impl VersionStore for DatabaseVersionStore {
    fn load(&self)->Option<StoredVersion> { self.database.with_conn(|conn|Ok::<_,&'static str>(load(conn))).ok().flatten() }
    fn save(&self,version:&StoredVersion)->bool {
        if let Err(code)=self.database.with_conn(|conn|save(conn,version)) { eprintln!("app_login_version_store_failed code={code}"); return false; }
        true
    }
}

/// Public, non-secret view for the operator API. Only the value and its
/// provenance are exposed; no deployment secret is reachable from here.
#[derive(Clone,Debug,serde::Serialize)]
pub(crate) struct AppVersionView {
    pub stored:Option<StoredVersionView>, pub stored_age_seconds:Option<i64>, pub store_ttl_seconds:i64,
    pub configured_file:bool, pub configured_mobile_page:bool, pub configured_ios_lookup:bool, pub configured_static:bool,
    pub store_enabled:bool,
}
#[derive(Clone,Debug,serde::Serialize)]
pub(crate) struct StoredVersionView { pub version:String, pub source:String, pub observed_at:i64 }

pub(super) fn view(stored:Option<StoredVersion>,now:i64,configured_file:bool,configured_mobile_page:bool,configured_ios:bool,configured_static:bool,store_enabled:bool)->AppVersionView {
    let age=stored.as_ref().map(|value|(now-value.observed_at).max(0));
    AppVersionView {
        stored:stored.map(|value|StoredVersionView {version:value.version,source:value.source,observed_at:value.observed_at}),
        stored_age_seconds:age, store_ttl_seconds:STORE_TTL_SECONDS,
        configured_file, configured_mobile_page:configured_mobile_page, configured_ios_lookup:configured_ios,
        configured_static, store_enabled,
    }
}

pub(super) fn manual(version:&str,now:i64)->Result<StoredVersion,Error> {
    if !super::config::valid_app_version(version) { return Err(Error::new("app_login_request_invalid")); }
    Ok(StoredVersion { version:version.to_owned(), source:SOURCE_MANUAL.to_owned(), observed_at:now })
}
