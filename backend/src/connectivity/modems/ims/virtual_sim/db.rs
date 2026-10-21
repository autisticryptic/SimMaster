//! Persistence for five-code subscriptions.
//!
//! One record per line, stored as the plain `store` record format. No master
//! key is kept and nothing is decrypted on read.
use rusqlite::{params, Connection, OptionalExtension};

use super::store::FiveCodes;

const FAILED: &str = "ims_virtual_sim_storage_failed";

fn storage<E>(_: E) -> &'static str { FAILED }

fn now() -> String { chrono::Utc::now().to_rfc3339() }

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

fn record(conn: &Connection, line_id: &str) -> Result<Option<(String, Vec<u8>)>, &'static str> {
    conn.query_row(
        "SELECT profile_id, sealed FROM virtual_sim_profile WHERE line_id = ?1",
        params![line_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(storage)
}

/// Stores one subscription, replacing any previous record for the same line.
pub fn store_profile(conn: &Connection, line_id: &str, profile_id: &str, codes: &FiveCodes)
    -> Result<(), &'static str> {
    let blob = codes.encode();
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
    let Some((_profile_id, blob)) = record(conn, line_id)? else { return Ok(None) };
    FiveCodes::decode(&blob).map(Some)
}

pub fn delete_profile(conn: &Connection, line_id: &str) -> Result<bool, &'static str> {
    conn.execute("DELETE FROM virtual_sim_profile WHERE line_id = ?1", params![line_id])
        .map(|rows| rows > 0).map_err(storage)
}

/// Installs every stored subscription into the in-process registry.
pub fn install_all(conn: &Connection) -> Result<(usize, usize), &'static str> {
    let profiles = list_profiles(conn)?;
    let (mut installed, mut failed) = (0usize, 0usize);
    for (line_id, profile_id) in profiles {
        match open_profile(conn, &line_id)? {
            Some(codes) => {
                super::remove_for_line(&line_id);
                super::install_for_line(&line_id, super::VirtualSim::new(&profile_id, codes))?;
                installed += 1;
            }
            None => failed += 1,
        }
    }
    Ok((installed, failed))
}
