//! Persistence for five-code subscriptions.
//!
//! One master key row plus one sealed profile per line. The key never leaves
//! this table in cleartext; rotation re-seals every record in one transaction.
use rusqlite::{params, Connection, OptionalExtension};

use super::store::{self, FiveCodes, MasterKey};

const FAILED: &str = "ims_virtual_sim_storage_failed";

fn storage<E>(_: E) -> &'static str { FAILED }

fn now() -> String { chrono::Utc::now().to_rfc3339() }

/// Reads the master key, creating it on first use.
pub fn load_or_create_master_key(conn: &Connection) -> Result<MasterKey, &'static str> {
    let existing: Option<Vec<u8>> = conn
        .query_row("SELECT key FROM virtual_sim_key WHERE id = 1", [], |row| row.get(0))
        .optional()
        .map_err(storage)?;
    if let Some(bytes) = existing {
        return MasterKey::from_bytes(&bytes);
    }
    let key = MasterKey::generate()?;
    conn.execute(
        "INSERT INTO virtual_sim_key (id, key, created_at) VALUES (1, ?1, ?2)",
        params![key.as_bytes(), now()],
    ).map_err(storage)?;
    Ok(key)
}

/// `(line_id, profile_id)` for every stored subscription.
pub fn list_profiles(conn: &Connection) -> Result<Vec<(String, String)>, &'static str> {
    let mut statement = conn
        .prepare("SELECT line_id, profile_id FROM virtual_sim_profile ORDER BY line_id")
        .map_err(storage)?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(storage)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(storage)
}

fn sealed(conn: &Connection, line_id: &str) -> Result<Option<(String, Vec<u8>)>, &'static str> {
    conn.query_row(
        "SELECT profile_id, sealed FROM virtual_sim_profile WHERE line_id = ?1",
        params![line_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(storage)
}

/// Stores one subscription, replacing any previous record for the same line.
pub fn store_profile(conn: &Connection, line_id: &str, profile_id: &str, codes: &FiveCodes)
    -> Result<(), &'static str> {
    let key = load_or_create_master_key(conn)?;
    let blob = store::seal(&key, profile_id, codes)?;
    let at = now();
    conn.execute(
        "INSERT INTO virtual_sim_profile (line_id, profile_id, sealed, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(line_id) DO UPDATE SET profile_id = ?2, sealed = ?3, updated_at = ?4",
        params![line_id, profile_id, blob, at],
    ).map_err(storage)?;
    Ok(())
}

pub fn open_profile(conn: &Connection, line_id: &str) -> Result<Option<FiveCodes>, &'static str> {
    let Some((profile_id, blob)) = sealed(conn, line_id)? else { return Ok(None) };
    let key = load_or_create_master_key(conn)?;
    store::open(&key, &profile_id, &blob).map(Some)
}

pub fn delete_profile(conn: &Connection, line_id: &str) -> Result<bool, &'static str> {
    conn.execute("DELETE FROM virtual_sim_profile WHERE line_id = ?1", params![line_id])
        .map(|rows| rows > 0).map_err(storage)
}

/// New master key, every record re-sealed. Records that cannot be opened are
/// left untouched and reported, never silently dropped.
pub fn rotate_master_key(conn: &Connection) -> Result<(usize, usize), &'static str> {
    let old = load_or_create_master_key(conn)?;
    let mut rows = Vec::new();
    {
        let mut statement = conn
            .prepare("SELECT line_id, profile_id, sealed FROM virtual_sim_profile")
            .map_err(storage)?;
        let mut query = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Vec<u8>>(2)?))
        }).map_err(storage)?;
        for row in query.by_ref() { rows.push(row.map_err(storage)?); }
    }
    let mut opened = Vec::with_capacity(rows.len());
    let mut skipped = 0usize;
    for (line_id, profile_id, blob) in rows {
        match store::open(&old, &profile_id, &blob) {
            Ok(codes) => opened.push((line_id, profile_id, codes)),
            Err(_) => skipped += 1,
        }
    }
    let key = MasterKey::generate()?;
    conn.execute_batch("BEGIN IMMEDIATE").map_err(storage)?;
    let result = (|| -> Result<(), &'static str> {
        conn.execute(
            "UPDATE virtual_sim_key SET key = ?1, created_at = ?2 WHERE id = 1",
            params![key.as_bytes(), now()],
        ).map_err(storage)?;
        for (line_id, profile_id, codes) in &opened {
            let blob = store::seal(&key, profile_id, codes)?;
            conn.execute(
                "UPDATE virtual_sim_profile SET sealed = ?1, updated_at = ?2 WHERE line_id = ?3",
                params![blob, now(), line_id],
            ).map_err(storage)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => conn.execute_batch("COMMIT").map_err(storage)?,
        Err(error) => { let _ = conn.execute_batch("ROLLBACK"); return Err(error); }
    }
    Ok((opened.len(), skipped))
}

/// Installs every stored subscription into the in-process registry. Lines whose
/// record cannot be opened stay in the database and are reported.
pub fn install_all(conn: &Connection) -> Result<(usize, usize), &'static str> {
    let profiles = list_profiles(conn)?;
    let (mut installed, mut failed) = (0usize, 0usize);
    for (line_id, _) in profiles {
        match open_profile(conn, &line_id)? {
            Some(codes) => {
                let profile_id = sealed(conn, &line_id)?.map(|(id, _)| id).unwrap_or_default();
                super::remove_for_line(&line_id);
                super::install_for_line(&line_id, super::VirtualSim::new(&profile_id, codes))?;
                installed += 1;
            }
            None => failed += 1,
        }
    }
    Ok((installed, failed))
}
