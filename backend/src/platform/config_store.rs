//! Configuration that belongs to the database rather than the text file.
//!
//! # Why these settings are not in the text file
//!
//! `line_registry` rewrites line profiles and the modem/reader slot map every
//! time hardware appears or moves: `reconcile_modem_slots`,
//! `reconcile_line_profiles`, `migrate_line_profile_aliases` and
//! `migrate_standalone_reader_references` all persist on hotplug. Keeping those
//! in the hand-editable file would mean the program rewrote it during normal
//! operation, so operator comments and ordering would be churned by events the
//! operator never triggered. They live here instead, and the text file stays
//! something a person can own.
//!
//! Notification channels, forwarding rules and automation tasks live here for a
//! different reason: they name lines (`sim_channel_ids`, `target.line_id`). Their
//! referents are rows in this database, so keeping the references beside them
//! lets a line's removal be reasoned about in one place.
//!
//! # Shape
//!
//! Per-line and per-slot settings get one row each, keyed by the identity the
//! rest of the code already uses. Rewriting one line therefore does not touch
//! another line's record, which matters when a save races a hotplug.
//!
//! Settings with no per-line identity are stored as one document per kind in
//! `config_documents`. They are read and written whole because that is what
//! `ConfigManager`'s API already promises (`get_notifications` hands back the
//! entire `NotificationConfig`), and splitting them into rows would change that
//! contract without making any caller better off.

use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;

use super::config::{
    AutomationConfig, EsimConfig, LineProfileConfig, ModemSlotConfig, NotificationConfig,
    StandaloneSimSlotConfig,
};
use super::db::Database;

/// Storage schema version for the tables in this module.
///
/// Independent of the text file's `config_version`: the two halves can evolve
/// separately, and an operator editing the file should not be forced to think
/// about database migrations.
pub const CONFIG_STORE_SCHEMA_VERSION: u32 = 1;

const DOC_NOTIFICATIONS: &str = "notifications";
const DOC_AUTOMATION: &str = "automation";
const DOC_ESIM: &str = "esim";
const DOC_UPDATE_STATE: &str = "version_update_state";

/// The database-owned half of the application configuration.
///
/// `ConfigManager` merges this with the file-owned half into the single
/// `AppConfig` its callers already use, so nothing outside these two modules has
/// to know where a given setting is stored.
#[derive(Debug, Clone, Default)]
pub struct StoredConfig {
    pub line_profiles: Vec<LineProfileConfig>,
    pub modem_slots: Vec<ModemSlotConfig>,
    pub standalone_sim_slots: Vec<StandaloneSimSlotConfig>,
    pub notifications: NotificationConfig,
    pub automation: AutomationConfig,
    pub esim: EsimConfig,
    /// Runtime state, not a setting: the last release the user was told about.
    /// It is written by the update checker, so keeping it out of the text file
    /// stops a background poll from rewriting a file nobody edited.
    pub last_notified_update_version: Option<String>,
}

/// Runtime state persisted alongside the update-notification settings.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
struct VersionUpdateState {
    #[serde(default)]
    last_notified_version: Option<String>,
}

pub fn initialize_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS config_line_profiles (
             line_id TEXT PRIMARY KEY,
             position INTEGER NOT NULL,
             document_json TEXT NOT NULL CHECK (json_valid(document_json)),
             updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS config_modem_slots (
             slot_key TEXT PRIMARY KEY,
             position INTEGER NOT NULL,
             document_json TEXT NOT NULL CHECK (json_valid(document_json)),
             updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS config_standalone_sim_slots (
             slot_id TEXT PRIMARY KEY,
             position INTEGER NOT NULL,
             document_json TEXT NOT NULL CHECK (json_valid(document_json)),
             updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS config_documents (
             kind TEXT PRIMARY KEY,
             schema_version INTEGER NOT NULL,
             document_json TEXT NOT NULL CHECK (json_valid(document_json)),
             updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS config_store_journal (
             version INTEGER PRIMARY KEY,
             applied_at TEXT NOT NULL,
             note TEXT NOT NULL
         );
         INSERT OR IGNORE INTO config_store_journal(version, applied_at, note)
             VALUES (1, strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                     'line/slot settings, event-bound records and per-SIM IMS overrides moved out of the text file');
         -- Per-SIM IMS connection overrides, keyed by the binding hash from
         -- `SimBindingKey` (normalized ICCID, or EID + enabled profile ICCID).
         -- These are per-SIM facts the read-only carrier catalog cannot own, so
         -- they belong beside the other per-line records rather than in a third
         -- file with its own backup lifecycle. `SimOverrideStore` owns the rows;
         -- the schema lives here so one place creates every config table.
         CREATE TABLE IF NOT EXISTS ims_sim_overrides (
             binding_hash TEXT PRIMARY KEY,
             schema_version INTEGER NOT NULL,
             document_json TEXT NOT NULL CHECK (json_valid(document_json)),
             updated_at TEXT NOT NULL
         );",
    )
}

/// Read the database-owned configuration.
///
/// A malformed row fails the whole read. Substituting a default would silently
/// drop a line's real settings — the APN, the path policies, the trunk secret —
/// and route operations to a SIM the operator never configured that way.
pub fn load(database: &Arc<Database>) -> Result<StoredConfig, String> {
    database
        .with_connection(|conn| Ok(read_all(conn)))
        .map_err(|error| format!("config_store_read_failed:{error}"))?
}

fn read_all(conn: &Connection) -> Result<StoredConfig, String> {
    Ok(StoredConfig {
        line_profiles: read_ordered(conn, "config_line_profiles", "line_id")?,
        modem_slots: read_ordered(conn, "config_modem_slots", "slot_key")?,
        standalone_sim_slots: read_ordered(conn, "config_standalone_sim_slots", "slot_id")?,
        notifications: read_document(conn, DOC_NOTIFICATIONS)?.unwrap_or_default(),
        automation: read_document(conn, DOC_AUTOMATION)?.unwrap_or_default(),
        esim: read_document(conn, DOC_ESIM)?.unwrap_or_default(),
        last_notified_update_version: read_document::<VersionUpdateState>(conn, DOC_UPDATE_STATE)?
            .unwrap_or_default()
            .last_notified_version,
    })
}

fn read_ordered<T: DeserializeOwned>(
    conn: &Connection,
    table: &str,
    key_column: &str,
) -> Result<Vec<T>, String> {
    let sql =
        format!("SELECT {key_column}, document_json FROM {table} ORDER BY position, {key_column}");
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| format!("config_store_prepare_failed:{table}:{error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("config_store_query_failed:{table}:{error}"))?;

    let mut parsed = Vec::new();
    for row in rows {
        let (key, document) =
            row.map_err(|error| format!("config_store_row_failed:{table}:{error}"))?;
        parsed.push(
            serde_json::from_str::<T>(&document)
                .map_err(|error| format!("config_store_document_invalid:{table}:{key}:{error}"))?,
        );
    }
    Ok(parsed)
}

fn read_document<T: DeserializeOwned>(conn: &Connection, kind: &str) -> Result<Option<T>, String> {
    let row = conn
        .query_row(
            "SELECT schema_version, document_json FROM config_documents WHERE kind = ?1",
            params![kind],
            |row| Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| format!("config_store_document_read_failed:{kind}:{error}"))?;
    let Some((version, document)) = row else {
        return Ok(None);
    };
    if version != CONFIG_STORE_SCHEMA_VERSION {
        return Err(format!(
            "config_store_document_version_unsupported:{kind}:{version}:expected:{CONFIG_STORE_SCHEMA_VERSION}"
        ));
    }
    serde_json::from_str::<T>(&document)
        .map(Some)
        .map_err(|error| format!("config_store_document_invalid:{kind}:{error}"))
}

/// Persist the database-owned configuration in one transaction.
///
/// Rows whose serialized form is unchanged are left alone so `updated_at` keeps
/// meaning "when this line last actually changed" rather than "when the process
/// last saved anything". Keys absent from `config` are deleted, which is how a
/// retired line or a removed notification channel disappears.
pub fn save(database: &Arc<Database>, config: &StoredConfig) -> Result<(), String> {
    let line_rows = keyed_rows(&config.line_profiles, |profile| profile.line_id.clone())?;
    let modem_rows = keyed_rows(&config.modem_slots, modem_slot_key)?;
    let standalone_rows = keyed_rows(&config.standalone_sim_slots, |slot| slot.id.clone())?;
    let documents = vec![
        (DOC_NOTIFICATIONS, serialize(&config.notifications)?),
        (DOC_AUTOMATION, serialize(&config.automation)?),
        (DOC_ESIM, serialize(&config.esim)?),
        (
            DOC_UPDATE_STATE,
            serialize(&VersionUpdateState {
                last_notified_version: config.last_notified_update_version.clone(),
            })?,
        ),
    ];

    database
        .with_connection(|conn| {
            Ok(write_all(
                conn,
                &line_rows,
                &modem_rows,
                &standalone_rows,
                &documents,
            ))
        })
        .map_err(|error| format!("config_store_write_failed:{error}"))?
}

type KeyedRows = Vec<(String, String)>;

fn write_all(
    conn: &Connection,
    line_rows: &KeyedRows,
    modem_rows: &KeyedRows,
    standalone_rows: &KeyedRows,
    documents: &[(&str, String)],
) -> Result<(), String> {
    // `unchecked_transaction` takes `&Connection`, which is what
    // `Database::with_connection` hands us. Rollback-on-drop is the default, so
    // an early `?` below abandons the whole write rather than leaving the line
    // tables half-updated.
    let mut transaction = conn
        .unchecked_transaction()
        .map_err(|error| format!("config_store_begin_failed:{error}"))?;
    transaction.set_drop_behavior(rusqlite::DropBehavior::Rollback);

    sync_table(&transaction, "config_line_profiles", "line_id", line_rows)?;
    sync_table(&transaction, "config_modem_slots", "slot_key", modem_rows)?;
    sync_table(
        &transaction,
        "config_standalone_sim_slots",
        "slot_id",
        standalone_rows,
    )?;
    for (kind, document) in documents {
        write_document(&transaction, kind, document)?;
    }

    transaction
        .commit()
        .map_err(|error| format!("config_store_commit_failed:{error}"))
}

fn sync_table(
    conn: &Connection,
    table: &str,
    key_column: &str,
    rows: &KeyedRows,
) -> Result<(), String> {
    let existing = {
        let sql = format!("SELECT {key_column}, position, document_json FROM {table}");
        let mut statement = conn
            .prepare(&sql)
            .map_err(|error| format!("config_store_prepare_failed:{table}:{error}"))?;
        let mapped = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    (row.get::<_, i64>(1)?, row.get::<_, String>(2)?),
                ))
            })
            .map_err(|error| format!("config_store_query_failed:{table}:{error}"))?;
        let mut map = BTreeMap::new();
        for row in mapped {
            let (key, value) =
                row.map_err(|error| format!("config_store_row_failed:{table}:{error}"))?;
            map.insert(key, value);
        }
        map
    };

    let now = timestamp();
    for (position, (key, document)) in rows.iter().enumerate() {
        let position = position as i64;
        if let Some((stored_position, stored_document)) = existing.get(key) {
            if *stored_position == position && stored_document == document {
                continue;
            }
        }
        let sql = format!(
            "INSERT INTO {table} ({key_column}, position, document_json, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT({key_column}) DO UPDATE SET
                 position = excluded.position,
                 document_json = excluded.document_json,
                 updated_at = excluded.updated_at"
        );
        conn.execute(&sql, params![key, position, document, now])
            .map_err(|error| format!("config_store_upsert_failed:{table}:{key}:{error}"))?;
    }

    let wanted: Vec<&String> = rows.iter().map(|(key, _)| key).collect();
    for key in existing.keys() {
        if wanted.contains(&key) {
            continue;
        }
        let sql = format!("DELETE FROM {table} WHERE {key_column} = ?1");
        conn.execute(&sql, params![key])
            .map_err(|error| format!("config_store_delete_failed:{table}:{key}:{error}"))?;
    }
    Ok(())
}

fn write_document(conn: &Connection, kind: &str, document: &str) -> Result<(), String> {
    let unchanged = conn
        .query_row(
            "SELECT document_json FROM config_documents
             WHERE kind = ?1 AND schema_version = ?2",
            params![kind, CONFIG_STORE_SCHEMA_VERSION],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("config_store_document_read_failed:{kind}:{error}"))?
        .is_some_and(|stored| stored == document);
    if unchanged {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO config_documents (kind, schema_version, document_json, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(kind) DO UPDATE SET
             schema_version = excluded.schema_version,
             document_json = excluded.document_json,
             updated_at = excluded.updated_at",
        params![kind, CONFIG_STORE_SCHEMA_VERSION, document, timestamp()],
    )
    .map_err(|error| format!("config_store_document_write_failed:{kind}:{error}"))?;
    Ok(())
}

/// Composite identity for a modem slot row.
///
/// A module can be moved between UIM slots on the same physical anchor, so the
/// anchor alone is not unique. This mirrors the `<slot_id>#uim<n>` key the slot
/// reconciler already builds.
fn modem_slot_key(slot: &ModemSlotConfig) -> String {
    let anchor = if slot.slot_id.trim().is_empty() {
        slot.hardware_key.trim()
    } else {
        slot.slot_id.trim()
    };
    format!("{anchor}#uim{}", slot.uim_slot)
}

fn keyed_rows<T: Serialize>(
    items: &[T],
    key_of: impl Fn(&T) -> String,
) -> Result<KeyedRows, String> {
    let mut rows: KeyedRows = Vec::with_capacity(items.len());
    for item in items {
        let key = key_of(item);
        if key.trim().is_empty() || key.trim_start().starts_with('#') {
            return Err("config_store_row_key_empty".to_string());
        }
        if rows.iter().any(|(existing, _)| existing == &key) {
            return Err(format!("config_store_row_key_duplicate:{key}"));
        }
        rows.push((key, serialize(item)?));
    }
    Ok(rows)
}

fn serialize<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|error| format!("config_store_serialize_failed:{error}"))
}

fn timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

// A `clear()` helper used to live here. It had no callers: an import replaces
// rows through `save`, which already deletes keys the incoming document does not
// mention, and overrides are cleared inside `config_maintenance`'s own
// transaction. Reintroduce it only with a caller.
