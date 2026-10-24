//! Machine-local private configuration for independent app accounts only.
//! No fake SIM/trunk rows, exported credentials or persisted readiness state.
use super::account_config::AccountConfig;
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use serde::Serialize;
use std::fmt;
use zeroize::Zeroizing;

pub const MAX_ACCOUNTS: i64 = 64;
const PROVIDER: &str = "app-voice";
const STORAGE: &str = "app_account_storage_failed";
const CONFIGURATION: &str = "app_account_configuration_invalid";
const CONFLICT: &str = "app_account_revision_conflict";
const NOT_FOUND: &str = "app_account_not_found";
const MAX_PRIVATE_BYTES: i64 = 256 * 1024;
const METADATA_COLUMNS: &str = "line_id, provider_id, revision, created_at, updated_at";

pub fn initialize_schema(conn: &Connection) -> Result<(), &'static str> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_voice_accounts (
            line_id TEXT PRIMARY KEY NOT NULL,
            provider_id TEXT NOT NULL CHECK (provider_id = 'app-voice'),
            revision INTEGER NOT NULL CHECK (revision > 0),
            private_config BLOB NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );"
    ).map_err(|_| STORAGE)
}

pub fn valid_line_id(id: &str) -> bool {
    id.strip_prefix("app-account-").is_some_and(|suffix| suffix.len() == 32
        && suffix.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}
fn check_id(id: &str) -> Result<(), &'static str> {
    if valid_line_id(id) { Ok(()) } else { Err("app_account_line_invalid") }
}
fn check_revision(revision: u64) -> Result<i64, &'static str> {
    let revision = i64::try_from(revision).map_err(|_| CONFLICT)?;
    if revision == 0 { return Err(CONFLICT); }
    Ok(revision)
}
fn timestamp(now: u64) -> Result<String, &'static str> {
    let seconds = i64::try_from(now).map_err(|_| STORAGE)?;
    DateTime::<Utc>::from_timestamp(seconds, 0)
        .map(|time| time.to_rfc3339_opts(SecondsFormat::Secs, true)).ok_or(STORAGE)
}

/// Metadata is intentionally free of identities, endpoints, previews and counts.
/// Listing never decodes a private credential blob or invents an expiry value.
#[derive(Clone, Debug, Serialize)]
pub struct AccountMetadata {
    pub line_id: String,
    pub provider_id: String,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}
pub struct StoredAccount { pub metadata: AccountMetadata, pub config: AccountConfig }
impl fmt::Debug for StoredAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("StoredAccount(<redacted>)") }
}

fn read_metadata(row: &Row<'_>) -> rusqlite::Result<AccountMetadata> {
    let revision: i64 = row.get(2)?;
    Ok(AccountMetadata {
        line_id: row.get(0)?, provider_id: row.get(1)?, revision: revision as u64,
        created_at: row.get(3)?, updated_at: row.get(4)?,
    })
}
fn check_metadata(metadata: &AccountMetadata) -> Result<(), &'static str> {
    if !valid_line_id(&metadata.line_id) || metadata.provider_id != PROVIDER
        || metadata.revision == 0 || metadata.revision > i64::MAX as u64 {
        return Err(STORAGE);
    }
    Ok(())
}
pub fn list(conn: &Connection) -> Result<Vec<AccountMetadata>, &'static str> {
    // One extra row detects over-cap corruption without an unbounded scan/parse.
    let mut statement = conn.prepare(&format!(
        "SELECT {METADATA_COLUMNS} FROM app_voice_accounts ORDER BY line_id LIMIT 65"
    )).map_err(|_| STORAGE)?;
    let rows = statement.query_map([], read_metadata).map_err(|_| STORAGE)?;
    let mut result = Vec::new();
    for row in rows {
        let row = row.map_err(|_| STORAGE)?;
        check_metadata(&row)?;
        if result.len() == MAX_ACCOUNTS as usize { return Err("app_account_limit"); }
        result.push(row);
    }
    Ok(result)
}
pub fn metadata(conn: &Connection, id: &str) -> Result<Option<AccountMetadata>, &'static str> {
    check_id(id)?;
    let result = conn.query_row(&format!(
        "SELECT {METADATA_COLUMNS} FROM app_voice_accounts WHERE line_id = ?1"
    ), [id], read_metadata).optional().map_err(|_| STORAGE)?;
    if let Some(row) = &result { check_metadata(row)?; }
    Ok(result)
}
pub fn load(conn: &Connection, id: &str) -> Result<Option<StoredAccount>, &'static str> {
    check_id(id)?;
    // Do not allocate an oversized corrupt blob. The application-owned copy is
    // zeroizing, but SQLite's own page/cache memory is not a secure-memory store.
    let row = conn.query_row(&format!(
        "SELECT {METADATA_COLUMNS}, CASE WHEN typeof(private_config) = 'blob'
            AND length(private_config) BETWEEN 1 AND {MAX_PRIVATE_BYTES}
            THEN private_config ELSE NULL END FROM app_voice_accounts WHERE line_id = ?1"
    ), [id], |row| {
        let metadata = read_metadata(row)?;
        let blob = row.get::<_, Option<Vec<u8>>>(5)?.map(Zeroizing::new);
        Ok((metadata, blob))
    }).optional().map_err(|_| STORAGE)?;
    let Some((metadata, blob)) = row else { return Ok(None); };
    check_metadata(&metadata)?;
    let blob = blob.ok_or(CONFIGURATION)?;
    let config = AccountConfig::decode_private(&blob).map_err(|_| CONFIGURATION)?;
    Ok(Some(StoredAccount { metadata, config }))
}

fn immediate(conn: &Connection) -> Result<Transaction<'_>, &'static str> {
    // &Connection is intentional. IMMEDIATE acquires the write reservation
    // before reading capacity/revision, including across separate connections.
    Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(|_| STORAGE)
}
fn current_revision(conn: &Connection, id: &str) -> Result<Option<i64>, &'static str> {
    let revision = conn.query_row(
        "SELECT revision FROM app_voice_accounts WHERE line_id = ?1", [id], |row| row.get::<_, i64>(0)
    ).optional().map_err(|_| STORAGE)?;
    if revision.is_some_and(|value| value <= 0) { return Err(STORAGE); }
    Ok(revision)
}
fn matched_revision(conn: &Connection, id: &str, expected: i64) -> Result<(), &'static str> {
    let actual = current_revision(conn, id)?.ok_or(NOT_FOUND)?;
    if actual != expected { return Err(CONFLICT); }
    Ok(())
}

pub fn create(conn: &Connection, id: &str, config: &AccountConfig, now: u64)
    -> Result<AccountMetadata, &'static str> {
    check_id(id)?;
    config.validate(now).map_err(|_| CONFIGURATION)?;
    let private = config.encode_private().map_err(|_| CONFIGURATION)?;
    let time = timestamp(now)?;
    let tx = immediate(conn)?;
    if current_revision(&tx, id)?.is_some() { return Err(CONFLICT); }
    let count: i64 = tx.query_row("SELECT count(*) FROM app_voice_accounts", [], |row| row.get(0))
        .map_err(|_| STORAGE)?;
    if count >= MAX_ACCOUNTS { return Err("app_account_limit"); }
    tx.execute(
        "INSERT INTO app_voice_accounts (line_id, provider_id, revision, private_config, created_at, updated_at)
            VALUES (?1, 'app-voice', 1, ?2, ?3, ?3)",
        params![id, private.as_slice(), &time],
    ).map_err(|_| STORAGE)?;
    let metadata = AccountMetadata { line_id: id.to_owned(), provider_id: PROVIDER.to_owned(), revision: 1,
        created_at: time.clone(), updated_at: time };
    tx.commit().map_err(|_| STORAGE)?;
    Ok(metadata)
}

pub fn replace(conn: &Connection, id: &str, expected_revision: u64, config: &AccountConfig, now: u64)
    -> Result<AccountMetadata, &'static str> {
    check_id(id)?;
    let expected = check_revision(expected_revision)?;
    let tx = immediate(conn)?;
    matched_revision(&tx, id, expected)?;
    let revision = expected.checked_add(1).ok_or(CONFLICT)?;
    config.validate(now).map_err(|_| CONFIGURATION)?;
    let private = config.encode_private().map_err(|_| CONFIGURATION)?;
    let time = timestamp(now)?;
    let changed = tx.execute(
        "UPDATE app_voice_accounts SET revision = ?1, private_config = ?2, updated_at = ?3
            WHERE line_id = ?4 AND revision = ?5",
        params![revision, private.as_slice(), time, id, expected],
    ).map_err(|_| STORAGE)?;
    if changed != 1 { return Err(CONFLICT); }
    let metadata = metadata(&tx, id)?.ok_or(STORAGE)?;
    tx.commit().map_err(|_| STORAGE)?;
    Ok(metadata)
}

pub fn delete(conn: &Connection, id: &str, expected_revision: u64) -> Result<bool, &'static str> {
    check_id(id)?;
    let expected = check_revision(expected_revision)?;
    let tx = immediate(conn)?;
    matched_revision(&tx, id, expected)?;
    let changed = tx.execute("DELETE FROM app_voice_accounts WHERE line_id = ?1 AND revision = ?2",
        params![id, expected]).map_err(|_| STORAGE)?;
    if changed != 1 { return Err(CONFLICT); }
    tx.commit().map_err(|_| STORAGE)?;
    Ok(true)
}

#[cfg(test)]
#[path = "account_store_tests.rs"]
mod tests;
