//! Persisted App device identities; never an authentication or hosting grant.
//!
//! Login aliases are realm-scoped hashes, not plaintext login names. Only a
//! successful App login AND its returned bound number attach an identity to a
//! canonical account key. A number's aliases converge on the same identity;
//! conflicting numbers cannot reuse it, including after rotation/deletion.
use super::{config::DeviceIdentityMode, valid_attempt_id, Error};
use super::super::credentials::Secret;
use ring::rand::{SecureRandom, SystemRandom};
use rusqlite::{params, Connection, OptionalExtension};
use std::fmt;

const STORAGE: &str = "app_login_storage_failed";
pub(super) const MAX_IDENTITIES: usize = 64;
const MAX_HISTORY: i64 = 512;
const MAX_ALIASES: i64 = 256;
const SOURCES: &[&str] = &["generated", "manual", "legacy"];

#[derive(Clone)]
pub(super) struct Snapshot { pub id: String, pub device_id: Secret, pub revision: u64 }
impl fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("DeviceIdentitySnapshot(<redacted>)") }
}
struct Identity { snapshot: Snapshot, account_key: Option<Vec<u8>>, source: String, retired: bool, created_at: i64, updated_at: i64 }
#[derive(Clone, serde::Serialize)]
pub(crate) struct DeviceIdentityView {
    pub id: String, pub device_id: String, pub source: String, pub revision: u64,
    pub bound_account_id: Option<String>, pub masked_number: Option<String>,
    pub reserved: bool, pub in_use: bool, pub created_at: i64, pub updated_at: i64,
}
impl fmt::Debug for DeviceIdentityView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("DeviceIdentityView(<redacted>)") }
}
#[derive(serde::Serialize)]
pub(crate) struct DeviceIdentityState {
    pub initialized: bool, pub configured_mode: DeviceIdentityMode,
    pub max_identities: usize, pub identities: Vec<DeviceIdentityView>,
}

pub(super) fn initialize(conn: &Connection) -> Result<(), &'static str> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS jegotrip_device_identities (
        id TEXT PRIMARY KEY CHECK(length(id)=32),
        device_id TEXT NOT NULL COLLATE NOCASE UNIQUE CHECK(length(CAST(device_id AS BLOB)) BETWEEN 8 AND 64),
        account_key BLOB CHECK(account_key IS NULL OR length(account_key)=32),
        source TEXT NOT NULL CHECK(source IN ('generated','manual','legacy')),
        revision INTEGER NOT NULL CHECK(revision>=1), retired INTEGER NOT NULL DEFAULT 0 CHECK(retired IN (0,1)),
        created_at INTEGER NOT NULL CHECK(created_at>=0), updated_at INTEGER NOT NULL CHECK(updated_at>=0)
    );
    CREATE UNIQUE INDEX IF NOT EXISTS jegotrip_device_account_unique ON jegotrip_device_identities(account_key)
        WHERE retired=0 AND account_key IS NOT NULL;
    CREATE TABLE IF NOT EXISTS jegotrip_device_aliases (
        alias_key BLOB PRIMARY KEY CHECK(length(alias_key)=32),
        identity_id TEXT NOT NULL CHECK(length(identity_id)=32) REFERENCES jegotrip_device_identities(id)
    );
    CREATE TABLE IF NOT EXISTS jegotrip_device_identity_history (
        device_id TEXT PRIMARY KEY COLLATE NOCASE CHECK(length(CAST(device_id AS BLOB)) BETWEEN 8 AND 64),
        identity_id TEXT NOT NULL CHECK(length(identity_id)=32) REFERENCES jegotrip_device_identities(id)
    );").map_err(|_| STORAGE)
}

pub(super) fn valid_device_id(value: &str) -> bool {
    (8..=64).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}
/// Matches the App's persisted random uppercase UUID shape, not a fabricated IMEI.
pub(super) fn generated() -> Result<String, Error> {
    let mut bytes = [0u8; 16];
    SystemRandom::new().fill(&mut bytes).map_err(|_| Error::new("app_login_entropy_failed"))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40; bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let h: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
    Ok(format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]))
}
fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<Identity> {
    let id: String = row.get(0)?; let device_id: String = row.get(1)?; let key: Option<Vec<u8>> = row.get(2)?;
    let source: String = row.get(3)?; let revision: i64 = row.get(4)?; let retired: i64 = row.get(5)?;
    let created_at: i64 = row.get(6)?; let updated_at: i64 = row.get(7)?; let key_type: String = row.get(8)?;
    if !valid_attempt_id(&id) || !valid_device_id(&device_id) || key.as_ref().is_some_and(|k| k.len()!=32)
        || (key_type!="null" && (key_type!="blob" || key.is_none()))
        || !SOURCES.contains(&source.as_str()) || revision<1 || !matches!(retired,0|1) || created_at<0 || updated_at<0 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(Identity { snapshot: Snapshot { id, device_id: Secret::from_owned(device_id), revision: revision as u64 },
        account_key: key, source, retired: retired==1, created_at, updated_at })
}
// Reject malformed rows instead of filtering them out. These CASE projections
// bound each owned allocation BEFORE row.get copies it into Rust.
const COLUMNS: &str = "CASE WHEN typeof(id)='text' AND length(CAST(id AS BLOB))=32 THEN id END AS id,
    CASE WHEN typeof(device_id)='text' AND length(CAST(device_id AS BLOB)) BETWEEN 8 AND 64 THEN device_id END AS device_id,
    CASE WHEN typeof(account_key)='blob' AND length(account_key)=32 THEN account_key END AS account_key,
    CASE WHEN typeof(source)='text' AND length(CAST(source AS BLOB)) BETWEEN 1 AND 16 THEN source END AS source,
    revision,retired,created_at,updated_at,typeof(account_key)";
fn get(conn: &Connection, id: &str) -> Result<Option<Identity>, Error> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM jegotrip_device_identities WHERE id=?1"), [id], decode)
        .optional().map_err(|_| Error::new(STORAGE))
}
fn active_account(conn: &Connection, key: &[u8]) -> Result<Option<Identity>, Error> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM jegotrip_device_identities WHERE account_key=?1 AND retired=0"), [key], decode)
        .optional().map_err(|_| Error::new(STORAGE))
}
fn count(conn: &Connection, table: &str) -> Result<i64, Error> {
    // Only fixed internal table names are passed, never request input.
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).map_err(|_| Error::new(STORAGE))
}
fn remember_value(conn: &Connection, id: &str, value: &str) -> Result<(), Error> {
    let owner: Option<String> = conn.query_row("SELECT CASE WHEN typeof(identity_id)='text' AND length(CAST(identity_id AS BLOB))=32 THEN identity_id END
        FROM jegotrip_device_identity_history WHERE device_id=?1 COLLATE NOCASE",
        [value], |r| r.get(0)).optional().map_err(|_| Error::new(STORAGE))?;
    if let Some(owner) = owner {
        if owner!=id { return Err(Error::new("app_login_device_identity_conflict")); }
    } else {
        if count(conn,"jegotrip_device_identity_history")?>=MAX_HISTORY { return Err(Error::new("app_login_device_identity_limit")); }
        conn.execute("INSERT INTO jegotrip_device_identity_history(device_id,identity_id) VALUES(?1,?2)", params![value,id])
            .map_err(|_| Error::new(STORAGE))?;
    }
    Ok(())
}
fn create(conn: &Connection, value: &str, source: &str, key: Option<&[u8]>, now: i64) -> Result<Snapshot, Error> {
    if !valid_device_id(value) || !SOURCES.contains(&source) || key.is_some_and(|k| k.len()!=32) || now<0 { return Err(Error::new(STORAGE)); }
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM jegotrip_device_identities WHERE retired=0",[],|r|r.get(0)).map_err(|_| Error::new(STORAGE))?;
    if n>=MAX_IDENTITIES as i64 { return Err(Error::new("app_login_device_identity_limit")); }
    let mut bytes = [0u8;16]; SystemRandom::new().fill(&mut bytes).map_err(|_| Error::new("app_login_entropy_failed"))?;
    let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    // The row exists before its history entry even with SQLite foreign_keys=ON.
    let taken: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM jegotrip_device_identity_history WHERE device_id=?1 COLLATE NOCASE)",
        [value], |r| r.get(0)).map_err(|_| Error::new(STORAGE))?;
    if taken { return Err(Error::new("app_login_device_identity_conflict")); }
    conn.execute("INSERT INTO jegotrip_device_identities(id,device_id,account_key,source,revision,created_at,updated_at)
        VALUES(?1,?2,?3,?4,1,?5,?5)",params![id,value,key,source,now]).map_err(|_| Error::new(STORAGE))?;
    remember_value(conn,&id,value)?;
    Ok(Snapshot { id, device_id: Secret::from_owned(value.to_owned()), revision: 1 })
}
fn alias(conn: &Connection, key: &[u8], id: &str) -> Result<(), Error> {
    let existing: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM jegotrip_device_aliases WHERE alias_key=?1)",[key],|r|r.get(0))
        .map_err(|_| Error::new(STORAGE))?;
    if !existing && count(conn,"jegotrip_device_aliases")?>=MAX_ALIASES { return Err(Error::new("app_login_device_identity_limit")); }
    conn.execute("INSERT INTO jegotrip_device_aliases(alias_key,identity_id) VALUES(?1,?2)
        ON CONFLICT(alias_key) DO UPDATE SET identity_id=excluded.identity_id",params![key,id]).map_err(|_| Error::new(STORAGE))?;
    Ok(())
}

/// Called inside the SAME transaction as durable attempt admission. Auto never
/// takes an arbitrary pool entry: only an explicitly selected manual value can.
/// Preserve the first legacy installation value rather than silently rotate it;
/// once used, all other new accounts get independent random values.
pub(super) fn resolve(conn: &Connection, alias_key: &[u8], preferred: Option<&str>, legacy_seed: Option<&str>, now: i64) -> Result<Snapshot, Error> {
    if alias_key.len()!=32 || preferred.is_some_and(|v| !valid_device_id(v)) { return Err(Error::new("app_login_request_invalid")); }
    let old: Option<String> = conn.query_row("SELECT CASE WHEN typeof(identity_id)='text' AND length(CAST(identity_id AS BLOB))=32 THEN identity_id END
        FROM jegotrip_device_aliases WHERE alias_key=?1",[alias_key],|r|r.get(0))
        .optional().map_err(|_| Error::new(STORAGE))?;
    if let Some(old) = old {
        let row = get(conn,&old)?.ok_or(Error::new(STORAGE))?;
        if !row.retired {
            if preferred.is_some_and(|v| !v.eq_ignore_ascii_case(row.snapshot.device_id.expose_to_backend())) {
                return Err(Error::new("app_login_device_identity_conflict"));
            }
            return Ok(row.snapshot);
        }
    }
    let (value,source) = if let Some(value) = preferred { (value.to_owned(),"manual") }
        else if let Some(seed) = legacy_seed.filter(|s| valid_device_id(s)) {
            if count(conn,"jegotrip_device_identities")?==0 { (seed.to_owned(),"legacy") } else { (generated()?,"generated") }
        } else { (generated()?,"generated") };
    let owned: Option<String> = conn.query_row("SELECT CASE WHEN typeof(identity_id)='text' AND length(CAST(identity_id AS BLOB))=32 THEN identity_id END
        FROM jegotrip_device_identity_history WHERE device_id=?1 COLLATE NOCASE",[&value],|r|r.get(0))
        .optional().map_err(|_| Error::new(STORAGE))?;
    let snapshot = if let Some(id) = owned {
        let row = get(conn,&id)?.ok_or(Error::new(STORAGE))?;
        let reserved: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM jegotrip_device_aliases WHERE identity_id=?1)",[&id],|r|r.get(0))
            .map_err(|_| Error::new(STORAGE))?;
        if row.retired || row.account_key.is_some() || reserved || !value.eq_ignore_ascii_case(row.snapshot.device_id.expose_to_backend()) {
            return Err(Error::new("app_login_device_identity_conflict"));
        }
        row.snapshot
    } else { create(conn,&value,source,None,now)? };
    alias(conn,alias_key,&snapshot.id)?;
    Ok(snapshot)
}

/// Successful App user-info is the ONLY authority for the number binding. A
/// different login alias may discover a canonical identity here; the caller
/// must re-authenticate with it BEFORE credentials/trust/business calls.
/// The caller owns a transaction plus a per-device admission lease.
pub(super) fn confirm(conn: &Connection, alias_key: &[u8], used: &Snapshot, account_key: &[u8], now: i64) -> Result<Snapshot, Error> {
    if alias_key.len()!=32 || account_key.len()!=32 { return Err(Error::new(STORAGE)); }
    let row = current(conn,&used.id,used.revision)?;
    if !row.snapshot.device_id.expose_to_backend().eq(used.device_id.expose_to_backend()) { return Err(Error::new(STORAGE)); }
    if row.account_key.as_ref().is_some_and(|key| key.as_slice()!=account_key) { return Err(Error::new("app_login_identity_mismatch")); }
    if let Some(canonical) = active_account(conn,account_key)? {
        if canonical.snapshot.id!=used.id {
            alias(conn,alias_key,&canonical.snapshot.id)?;
            // A provisional identity is never recycled to a different number.
            conn.execute("UPDATE jegotrip_device_identities SET retired=1,revision=revision+1,updated_at=?1 WHERE id=?2",params![now,used.id])
                .map_err(|_| Error::new(STORAGE))?;
        }
        return Ok(canonical.snapshot);
    }
    let revision = used.revision.checked_add(1).filter(|v| *v<=i64::MAX as u64).ok_or(Error::new(STORAGE))?;
    conn.execute("UPDATE jegotrip_device_identities SET account_key=?1,revision=?2,updated_at=?3 WHERE id=?4",
        params![account_key,revision as i64,now,used.id]).map_err(|_| Error::new(STORAGE))?;
    alias(conn,alias_key,&used.id)?;
    Ok(Snapshot { revision, ..used.clone() })
}
fn current(conn: &Connection, id: &str, revision: u64) -> Result<Identity, Error> {
    if !valid_attempt_id(id) || revision==0 || revision>i64::MAX as u64 { return Err(Error::new("app_login_request_invalid")); }
    let row = get(conn,id)?.ok_or(Error::new("app_login_device_identity_not_found"))?;
    if row.retired || row.snapshot.revision!=revision { return Err(Error::new("app_login_device_identity_revision_conflict")); }
    Ok(row)
}
fn in_use(conn: &Connection, account_key: Option<&[u8]>) -> Result<bool, Error> {
    let Some(key) = account_key else { return Ok(false); };
    let account: Option<(String,Option<String>)> = conn.query_row(
        "SELECT CASE WHEN typeof(account_id)='text' AND length(CAST(account_id AS BLOB))=32 THEN account_id END,
         CASE WHEN voice_line_id IS NULL THEN NULL WHEN typeof(voice_line_id)='text' AND length(CAST(voice_line_id AS BLOB))=44 THEN voice_line_id ELSE '' END
         FROM jegotrip_accounts WHERE identity_key=?1",[key],|r|Ok((r.get(0)?,r.get(1)?)))
        .optional().map_err(|_| Error::new(STORAGE))?;
    let Some((id,line)) = account else { return Ok(false); };
    if !valid_attempt_id(&id) {return Err(Error::new(STORAGE));}
    let mut lines:std::collections::BTreeSet<String>=line.into_iter().collect();
    let ledger:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='jegotrip_login_attempts')",[],|r|r.get(0))
        .map_err(|_|Error::new(STORAGE))?;
    if ledger {
        // A later login may replace the summary's latest link. Earlier receiver
        // lines still have original runtime fences, so inspect EVERY linked
        // attempt, not only the most recent voice_line_id.
        let mut query=conn.prepare("SELECT CASE WHEN typeof(line_id)='text' AND length(CAST(line_id AS BLOB))=44 THEN line_id ELSE '' END
            FROM jegotrip_login_attempts WHERE account_id=?1 AND line_id IS NOT NULL LIMIT 4097").map_err(|_|Error::new(STORAGE))?;
        let older=query.query_map([&id],|r|r.get::<_,String>(0)).map_err(|_|Error::new(STORAGE))?
            .collect::<Result<Vec<_>,_>>().map_err(|_|Error::new(STORAGE))?;
        if older.len()>4096 {return Err(Error::new(STORAGE));}lines.extend(older);
    }
    let runs: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_voice_inbox_runs')",[],|r|r.get(0))
        .map_err(|_| Error::new(STORAGE))?;
    for line in lines {
        if !super::super::account_store::valid_line_id(&line) {return Err(Error::new(STORAGE));}
        let held = if runs {super::super::account_runs::ownership_pending(conn,&line)} else {super::super::account_runs::runtime_held(conn,&line)};
        if held.map_err(|_|Error::new(STORAGE))? {return Ok(true);}
    }
    Ok(false)
}
pub(super) fn native_ownership_pending(conn:&Connection,account_key:&[u8])->Result<bool,&'static str> {
    in_use(conn,Some(account_key)).map_err(|e|e.code)
}

/// Admin CAS: no clearing/rebinding existing ownership by omitting account_id.
/// Old values stay reserved to this identity in the history table.
pub(super) fn set(conn: &Connection, id: Option<&str>, expected: u64, key: Option<&[u8]>, requested: Option<&str>, now: i64) -> Result<(), Error> {
    if requested.is_some_and(|v| !valid_device_id(v)) || key.is_some_and(|k| k.len()!=32) || now<0 { return Err(Error::new("app_login_request_invalid")); }
    let tx = rusqlite::Transaction::new_unchecked(conn,rusqlite::TransactionBehavior::Immediate).map_err(|_| Error::new(STORAGE))?;
    let value = requested.map(str::to_owned).map(Ok).unwrap_or_else(generated)?;
    let source = if requested.is_some() { "manual" } else { "generated" };
    if let Some(id) = id {
        let row = current(&tx,id,expected)?;
        if row.account_key.as_ref().is_some_and(|old| key.is_some_and(|new| old.as_slice()!=new)) {
            return Err(Error::new("app_login_device_identity_conflict"));
        }
        let binding = row.account_key.as_deref().or(key);
        if in_use(&tx,binding)? { return Err(Error::new("app_login_device_identity_in_use")); }
        if let Some(key) = binding {
            if active_account(&tx,key)?.is_some_and(|other| other.snapshot.id!=id) { return Err(Error::new("app_login_device_identity_conflict")); }
        }
        let revision = expected.checked_add(1).filter(|v| *v<=i64::MAX as u64).ok_or(Error::new(STORAGE))?;
        remember_value(&tx,id,&value)?;
        tx.execute("UPDATE jegotrip_device_identities SET device_id=?1,account_key=?2,source=?3,revision=?4,updated_at=?5 WHERE id=?6",
            params![value,binding,source,revision as i64,now,id]).map_err(|_| Error::new(STORAGE))?;
    } else {
        if expected!=0 { return Err(Error::new("app_login_device_identity_revision_conflict")); }
        if let Some(key) = key {
            if active_account(&tx,key)?.is_some() { return Err(Error::new("app_login_device_identity_conflict")); }
            if in_use(&tx,Some(key))? { return Err(Error::new("app_login_device_identity_in_use")); }
        }
        create(&tx,&value,source,key,now)?;
    }
    tx.commit().map_err(|_| Error::new(STORAGE))
}
/// Deletion is a tombstone, not permission to lend an old server-bound value to
/// a new account. It does not stop a receiver or close a remote hosting service.
pub(super) fn remove(conn: &Connection, id: &str, expected: u64, now: i64) -> Result<(), Error> {
    if now<0 {return Err(Error::new("app_login_request_invalid"));}
    let tx = rusqlite::Transaction::new_unchecked(conn,rusqlite::TransactionBehavior::Immediate).map_err(|_| Error::new(STORAGE))?;
    let row = current(&tx,id,expected)?;
    if in_use(&tx,row.account_key.as_deref())? { return Err(Error::new("app_login_device_identity_in_use")); }
    let revision = expected.checked_add(1).filter(|v| *v<=i64::MAX as u64).ok_or(Error::new(STORAGE))?;
    tx.execute("UPDATE jegotrip_device_identities SET retired=1,revision=?1,updated_at=?2 WHERE id=?3",params![revision as i64,now,id])
        .map_err(|_| Error::new(STORAGE))?;
    tx.commit().map_err(|_| Error::new(STORAGE))
}
pub(super) fn list(conn: &Connection) -> Result<Vec<DeviceIdentityView>, &'static str> {
    let mut statement = conn.prepare(&format!("SELECT {COLUMNS} FROM jegotrip_device_identities WHERE retired=0 ORDER BY created_at,id LIMIT 65"))
        .map_err(|_| STORAGE)?;
    let rows = statement.query_map([],decode).map_err(|_| STORAGE)?.collect::<Result<Vec<_>,_>>().map_err(|_| STORAGE)?;
    if rows.len()>MAX_IDENTITIES { return Err(STORAGE); }
    let mut views = Vec::with_capacity(rows.len());
    for row in rows {
        let account: Option<(String,String)> = if let Some(key) = &row.account_key {
            conn.query_row("SELECT CASE WHEN typeof(account_id)='text' AND length(CAST(account_id AS BLOB))=32 THEN account_id END,
                CASE WHEN typeof(masked_number)='text' AND length(CAST(masked_number AS BLOB)) BETWEEN 3 AND 7 THEN masked_number END
                FROM jegotrip_accounts WHERE identity_key=?1",[key],|r|Ok((r.get(0)?,r.get(1)?)))
                .optional().map_err(|_| STORAGE)?
        } else { None };
        if account.as_ref().is_some_and(|(id,mask)| !valid_attempt_id(id) || !super::accounts::valid_mask(mask)) { return Err(STORAGE); }
        let reserved: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM jegotrip_device_aliases WHERE identity_id=?1)",[&row.snapshot.id],|r|r.get(0))
            .map_err(|_| STORAGE)?;
        let used = in_use(conn,row.account_key.as_deref()).map_err(|e|e.code)?;
        views.push(DeviceIdentityView { id: row.snapshot.id, device_id: row.snapshot.device_id.expose_to_backend().to_owned(), source: row.source,
            revision: row.snapshot.revision, bound_account_id: account.as_ref().map(|(id,_)|id.clone()), masked_number: account.map(|(_,mask)|mask),
            reserved, in_use: used, created_at: row.created_at, updated_at: row.updated_at });
    }
    Ok(views)
}
