//! Explicit configuration maintenance: backup, export, import, restore.
//!
//! These commands are deliberately separate from startup. SimAdmin never scans,
//! imports or migrates anything on its own; an operator asks for each action and
//! gets back the paths retained for rollback.
//!
//! # Both halves, always together
//!
//! Configuration lives in two places (see `platform::config_store`): the
//! operator-editable text file, and the config tables inside the application
//! database. Every command here handles both, because a snapshot of one half
//! restores a device to a state it was never in — a file describing line
//! profiles the database does not have, or line profiles whose security policy
//! and DDNS settings are missing.
//!
//! The read-only carrier catalog is excluded on purpose: it is a release
//! artifact, not user data, and it is replaced by upgrades rather than restored.
//!
//! Ordering rule, applied by both `import_json` and `restore`: write the
//! database half first, then the file. If the file write fails afterwards the
//! database is rolled back from the snapshot taken at the start, so the two
//! halves never diverge silently.

use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::config::{
    AutomationConfig, EsimConfig, LineProfileConfig, MainConfig, ModemSlotConfig,
    NotificationConfig, StandaloneSimSlotConfig, CURRENT_LINE_CONFIG_VERSION,
};
use super::config_file::{self, TextFormat};
use super::config_store::{self, StoredConfig, CONFIG_STORE_SCHEMA_VERSION};
use super::db::Database;
use crate::connectivity::modems::ims::profile_override::{
    validate_stored_override_document, OVERRIDE_SCHEMA_VERSION,
};

/// Tables this module owns. A restore replaces exactly these and nothing else:
/// the application database also holds SMS, call history and runtime events,
/// which a configuration restore must not touch.
const CONFIG_TABLES: [&str; 4] = [
    "config_line_profiles",
    "config_modem_slots",
    "config_standalone_sim_slots",
    "config_documents",
];

const EXPORT_FORMAT: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigExport {
    /// Bumped to 2 when configuration split into a file half and a database
    /// half. A format-1 document describes a layout that no longer exists.
    pub format: u32,
    pub config_store_schema_version: u32,
    /// The operator-editable settings.
    pub main_config: MainConfig,
    /// The database-owned settings.
    pub stored: StoredConfigExport,
    pub overrides: Vec<OverrideExportRow>,
}

/// Serializable mirror of [`StoredConfig`].
///
/// `StoredConfig` is a runtime type and deliberately not `Serialize`; keeping the
/// wire shape separate means an export file does not silently change whenever a
/// field is added to the runtime struct.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredConfigExport {
    #[serde(default)]
    pub line_profiles: Vec<LineProfileConfig>,
    #[serde(default)]
    pub modem_slots: Vec<ModemSlotConfig>,
    #[serde(default)]
    pub standalone_sim_slots: Vec<StandaloneSimSlotConfig>,
    #[serde(default)]
    pub notifications: NotificationConfig,
    #[serde(default)]
    pub automation: AutomationConfig,
    #[serde(default)]
    pub esim: EsimConfig,
    #[serde(default)]
    pub last_notified_update_version: Option<String>,
}

impl From<StoredConfig> for StoredConfigExport {
    fn from(stored: StoredConfig) -> Self {
        Self {
            line_profiles: stored.line_profiles,
            modem_slots: stored.modem_slots,
            standalone_sim_slots: stored.standalone_sim_slots,
            notifications: stored.notifications,
            automation: stored.automation,
            esim: stored.esim,
            last_notified_update_version: stored.last_notified_update_version,
        }
    }
}

impl From<StoredConfigExport> for StoredConfig {
    fn from(export: StoredConfigExport) -> Self {
        Self {
            line_profiles: export.line_profiles,
            modem_slots: export.modem_slots,
            standalone_sim_slots: export.standalone_sim_slots,
            notifications: export.notifications,
            automation: export.automation,
            esim: export.esim,
            last_notified_update_version: export.last_notified_update_version,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverrideExportRow {
    pub binding_hash: String,
    pub schema_version: u32,
    pub document: serde_json::Value,
    pub updated_at: String,
}

/// What a command actually captured, so the operator can check it against the
/// device instead of trusting a bare "done".
#[derive(Debug, Clone, Default)]
pub struct MaintenanceSummary {
    pub line_profiles: usize,
    pub modem_slots: usize,
    pub standalone_sim_slots: usize,
    pub overrides: usize,
}

/// Paths retained before a destructive command, one per half.
#[derive(Debug, Clone, Default)]
pub struct RollbackPaths {
    pub database: Option<PathBuf>,
    pub config_file: Option<PathBuf>,
}

/// Snapshot both halves. `destination` names the database snapshot; the file is
/// saved beside it so a later restore can find its pair without being told.
pub fn backup(
    config_path: &Path,
    database_path: &Path,
    destination: &Path,
) -> Result<MaintenanceSummary, String> {
    ensure_source(database_path)?;
    ensure_new_destination(destination)?;
    if database_path == destination {
        return Err("config_backup_source_equals_destination".to_string());
    }

    let source = open_read_only(database_path)?;
    let summary = summarize(&source)?;
    let destination_sql = destination.to_string_lossy().to_string();
    // VACUUM INTO produces a consistent snapshot of a live database without
    // copying WAL sidecars, which a plain file copy would miss.
    source
        .execute("VACUUM main INTO ?1", params![destination_sql])
        .map_err(|error| format!("config_backup_vacuum_failed:{error}"))?;
    drop(source);
    sync_file(destination)?;
    set_private_file_mode(destination)?;

    let copied = open_read_only(destination)?;
    let copied_summary = summarize(&copied)?;
    drop(copied);
    if summary_counts(&summary) != summary_counts(&copied_summary) {
        return Err("config_backup_row_count_changed".to_string());
    }

    if config_path.exists() {
        let paired = paired_config_path(destination, config_path);
        fs::copy(config_path, &paired)
            .map_err(|error| format!("config_backup_file_copy_failed:{error}"))?;
        set_private_file_mode(&paired)?;
        sync_file(&paired)?;
    }
    sync_parent(destination)?;
    Ok(summary)
}

/// Write both halves into one JSON document.
pub fn export_json(
    config_path: &Path,
    database_path: &Path,
    destination: &Path,
) -> Result<MaintenanceSummary, String> {
    ensure_source(database_path)?;
    ensure_new_destination(destination)?;

    let main_config = read_main_config(config_path)?;
    let connection = open_read_only(database_path)?;
    let stored = read_stored_config(&connection)?;
    let overrides = read_overrides(&connection)?;
    drop(connection);

    let summary = MaintenanceSummary {
        line_profiles: stored.line_profiles.len(),
        modem_slots: stored.modem_slots.len(),
        standalone_sim_slots: stored.standalone_sim_slots.len(),
        overrides: overrides.len(),
    };
    let export = ConfigExport {
        format: EXPORT_FORMAT,
        config_store_schema_version: CONFIG_STORE_SCHEMA_VERSION,
        main_config,
        stored: stored.into(),
        overrides,
    };
    validate_export(&export)?;
    let content = serde_json::to_vec_pretty(&export)
        .map_err(|error| format!("config_export_serialize_failed:{error}"))?;
    atomic_write_private(destination, &content)?;
    Ok(summary)
}

/// Import a deliberately exported document into both halves.
pub fn import_json(
    config_path: &Path,
    database_path: &Path,
    source: &Path,
) -> Result<RollbackPaths, String> {
    ensure_source(source)?;
    let content = fs::read(source).map_err(|error| format!("config_import_read_failed:{error}"))?;
    let export: ConfigExport = serde_json::from_slice(&content)
        .map_err(|error| format!("config_import_document_invalid:{error}"))?;
    validate_export(&export)?;

    let rollback = snapshot_both(config_path, database_path, "import-rollback")?;

    apply_database_half(database_path, &export)?;
    if let Err(error) = write_main_config(config_path, &export.main_config) {
        // The database now holds settings the file does not describe. Put it
        // back so the operator retries from a coherent state.
        if let Some(snapshot) = &rollback.database {
            restore_config_tables(database_path, snapshot).map_err(|rollback_error| {
                format!("config_import_file_failed:{error}:rollback_also_failed:{rollback_error}")
            })?;
        }
        return Err(format!("config_import_file_failed:{error}"));
    }

    validate_loads(config_path, database_path)
        .map_err(|error| format!("config_import_post_validate_failed:{error}"))?;
    Ok(rollback)
}

/// Restore a prior snapshot into both halves.
pub fn restore(
    config_path: &Path,
    database_path: &Path,
    source: &Path,
) -> Result<RollbackPaths, String> {
    ensure_source(source)?;
    if database_path == source {
        return Err("config_restore_source_equals_target".to_string());
    }
    let source_connection = open_read_only(source)?;
    summarize(&source_connection)?;
    drop(source_connection);

    let rollback = snapshot_both(config_path, database_path, "restore-rollback")?;

    restore_config_tables(database_path, source)?;

    // A snapshot taken by `backup` carries its file half beside it.
    let paired = paired_config_path(source, config_path);
    if paired.exists() {
        if let Err(error) = copy_private(&paired, config_path) {
            if let Some(snapshot) = &rollback.database {
                restore_config_tables(database_path, snapshot).map_err(|rollback_error| {
                    format!(
                        "config_restore_file_failed:{error}:rollback_also_failed:{rollback_error}"
                    )
                })?;
            }
            return Err(format!("config_restore_file_failed:{error}"));
        }
    }

    validate_loads(config_path, database_path)
        .map_err(|error| format!("config_restore_post_validate_failed:{error}"))?;
    Ok(rollback)
}

// --- halves -----------------------------------------------------------------

fn read_main_config(config_path: &Path) -> Result<MainConfig, String> {
    if !config_path.exists() {
        // A device that has never been configured exports its defaults, which is
        // a valid thing to hand to a second device.
        return Ok(MainConfig::default());
    }
    let format = TextFormat::from_path(config_path).ok_or_else(|| {
        format!(
            "config_export_unsupported_extension:{}",
            config_path.display()
        )
    })?;
    let content = fs::read_to_string(config_path)
        .map_err(|error| format!("config_export_file_read_failed:{error}"))?;
    let main: MainConfig = config_file::parse(&content, format, config_path)
        .map_err(|error| format!("config_export_file_invalid:{error}"))?;
    if main.config_version != CURRENT_LINE_CONFIG_VERSION {
        return Err(format!(
            "config_export_file_version_unsupported:{}",
            main.config_version
        ));
    }
    Ok(main)
}

fn write_main_config(config_path: &Path, main: &MainConfig) -> Result<(), String> {
    let format = TextFormat::from_path(config_path).ok_or_else(|| {
        format!(
            "config_import_unsupported_extension:{}",
            config_path.display()
        )
    })?;
    config_file::ensure_regular_file(config_path)?;
    let existing = if config_path.exists() {
        fs::read_to_string(config_path)
            .map_err(|error| format!("config_import_file_read_failed:{error}"))?
    } else {
        String::new()
    };
    // An import replaces the settings but keeps the operator's comments on keys
    // that survive, same as an ordinary save.
    let baseline = if existing.trim().is_empty() {
        MainConfig::default()
    } else {
        config_file::parse(&existing, format, config_path).unwrap_or_default()
    };
    let rendered =
        config_file::apply_update(&existing, &baseline, main, format, config_path, &[], &[])?;
    config_file::write_atomically(config_path, &rendered)
}

fn read_stored_config(connection: &Connection) -> Result<StoredConfig, String> {
    let mut stored = StoredConfig::default();
    if table_exists(connection, "config_line_profiles")? {
        stored.line_profiles = read_documents(connection, "config_line_profiles", "line_id")?;
    }
    if table_exists(connection, "config_modem_slots")? {
        stored.modem_slots = read_documents(connection, "config_modem_slots", "slot_key")?;
    }
    if table_exists(connection, "config_standalone_sim_slots")? {
        stored.standalone_sim_slots =
            read_documents(connection, "config_standalone_sim_slots", "slot_id")?;
    }
    if table_exists(connection, "config_documents")? {
        stored.notifications = read_kind(connection, "notifications")?.unwrap_or_default();
        stored.automation = read_kind(connection, "automation")?.unwrap_or_default();
        stored.esim = read_kind(connection, "esim")?.unwrap_or_default();
        stored.last_notified_update_version =
            read_kind::<serde_json::Value>(connection, "version_update_state")?.and_then(|value| {
                value
                    .get("last_notified_version")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            });
    }
    Ok(stored)
}

fn read_documents<T: serde::de::DeserializeOwned>(
    connection: &Connection,
    table: &str,
    key_column: &str,
) -> Result<Vec<T>, String> {
    let sql =
        format!("SELECT {key_column}, document_json FROM {table} ORDER BY position, {key_column}");
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| format!("config_export_prepare_failed:{table}:{error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("config_export_query_failed:{table}:{error}"))?;
    let mut parsed = Vec::new();
    for row in rows {
        let (key, document) =
            row.map_err(|error| format!("config_export_row_failed:{table}:{error}"))?;
        parsed.push(
            serde_json::from_str::<T>(&document)
                .map_err(|error| format!("config_export_invalid:{table}:{key}:{error}"))?,
        );
    }
    Ok(parsed)
}

fn read_kind<T: serde::de::DeserializeOwned>(
    connection: &Connection,
    kind: &str,
) -> Result<Option<T>, String> {
    let row = connection
        .query_row(
            "SELECT document_json FROM config_documents WHERE kind = ?1",
            params![kind],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("config_export_document_read_failed:{kind}:{error}"))?;
    match row {
        Some(document) => serde_json::from_str::<T>(&document)
            .map(Some)
            .map_err(|error| format!("config_export_document_invalid:{kind}:{error}")),
        None => Ok(None),
    }
}

fn read_overrides(connection: &Connection) -> Result<Vec<OverrideExportRow>, String> {
    if !table_exists(connection, "ims_sim_overrides")? {
        return Ok(Vec::new());
    }
    let mut statement = connection
        .prepare(
            "SELECT binding_hash, schema_version, document_json, updated_at
             FROM ims_sim_overrides ORDER BY binding_hash",
        )
        .map_err(|error| format!("config_export_override_query_failed:{error}"))?;
    let rows = statement
        .query_map([], |row| {
            let document: String = row.get(2)?;
            Ok(OverrideExportRow {
                binding_hash: row.get(0)?,
                schema_version: row.get(1)?,
                document: serde_json::from_str(&document).unwrap_or(serde_json::Value::Null),
                updated_at: row.get(3)?,
            })
        })
        .map_err(|error| format!("config_export_override_read_failed:{error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("config_export_override_row_failed:{error}"))
}

/// Replace the database half from `export`, in one transaction.
fn apply_database_half(database_path: &Path, export: &ConfigExport) -> Result<(), String> {
    let database = Arc::new(
        Database::new(database_path.to_path_buf())
            .map_err(|error| format!("config_import_database_open_failed:{error}"))?,
    );
    let stored: StoredConfig = export.stored.clone().into();
    config_store::save(&database, &stored)?;

    // Overrides are replaced wholesale: merging would leave rows the export
    // never mentioned, bound to SIMs the operator thought they had cleared.
    database
        .with_connection(|conn| {
            let transaction = conn.unchecked_transaction()?;
            transaction.execute("DELETE FROM ims_sim_overrides", [])?;
            for row in &export.overrides {
                let document = serde_json::to_string(&row.document).unwrap_or_default();
                transaction.execute(
                    "INSERT INTO ims_sim_overrides
                         (binding_hash, schema_version, document_json, updated_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        row.binding_hash,
                        row.schema_version,
                        document,
                        row.updated_at
                    ],
                )?;
            }
            transaction.commit()
        })
        .map_err(|error| format!("config_import_override_failed:{error}"))
}

/// Copy the config-owned tables from `source` into `target`, leaving every other
/// table in `target` untouched.
fn restore_config_tables(target: &Path, source: &Path) -> Result<(), String> {
    let mut connection = open_writable(target)?;
    let source_sql = source.to_string_lossy().into_owned();
    connection
        .execute("ATTACH DATABASE ?1 AS restore_source", params![source_sql])
        .map_err(|error| format!("config_restore_attach_failed:{error}"))?;

    let outcome = (|| -> Result<(), String> {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("config_restore_begin_failed:{error}"))?;
        for table in CONFIG_TABLES.iter().chain(["ims_sim_overrides"].iter()) {
            let present: bool = transaction
                .query_row(
                    "SELECT COUNT(*) FROM restore_source.sqlite_master
                     WHERE type = 'table' AND name = ?1",
                    params![table],
                    |row| row.get::<_, i64>(0),
                )
                .map(|count| count > 0)
                .map_err(|error| format!("config_restore_probe_failed:{table}:{error}"))?;
            if !present {
                // An older snapshot may predate a table. Clearing the target
                // matches what that snapshot actually described.
                transaction
                    .execute(&format!("DELETE FROM {table}"), [])
                    .map_err(|error| format!("config_restore_clear_failed:{table}:{error}"))?;
                continue;
            }
            transaction
                .execute(&format!("DELETE FROM {table}"), [])
                .map_err(|error| format!("config_restore_clear_failed:{table}:{error}"))?;
            transaction
                .execute(
                    &format!("INSERT INTO {table} SELECT * FROM restore_source.{table}"),
                    [],
                )
                .map_err(|error| format!("config_restore_copy_failed:{table}:{error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("config_restore_commit_failed:{error}"))
    })();

    let _ = connection.execute("DETACH DATABASE restore_source", []);
    outcome
}

fn snapshot_both(
    config_path: &Path,
    database_path: &Path,
    suffix: &str,
) -> Result<RollbackPaths, String> {
    let mut rollback = RollbackPaths::default();
    if database_path.exists() {
        let path = sqlite_maintenance_path(database_path, suffix);
        ensure_new_destination(&path)?;
        let source = open_read_only(database_path)?;
        let destination_sql = path.to_string_lossy().to_string();
        source
            .execute("VACUUM main INTO ?1", params![destination_sql])
            .map_err(|error| format!("config_snapshot_vacuum_failed:{error}"))?;
        drop(source);
        set_private_file_mode(&path)?;
        sync_file(&path)?;
        rollback.database = Some(path);
    }
    if config_path.exists() {
        let path = maintenance_path(config_path, suffix);
        ensure_new_destination(&path)?;
        copy_private(config_path, &path)?;
        rollback.config_file = Some(path);
    }
    Ok(rollback)
}

/// Prove both halves load before reporting success, while the operator still has
/// the rollback snapshot in hand.
fn validate_loads(config_path: &Path, database_path: &Path) -> Result<(), String> {
    let database = Arc::new(
        Database::new(database_path.to_path_buf())
            .map_err(|error| format!("database_open_failed:{error}"))?,
    );
    super::config::ConfigManager::try_new(config_path.to_path_buf(), database)?;
    Ok(())
}

// --- validation -------------------------------------------------------------

fn validate_export(export: &ConfigExport) -> Result<(), String> {
    if export.format != EXPORT_FORMAT {
        return Err(format!(
            "config_export_format_unsupported:{}:expected:{EXPORT_FORMAT}",
            export.format
        ));
    }
    if export.config_store_schema_version != CONFIG_STORE_SCHEMA_VERSION {
        return Err(format!(
            "config_store_schema_unsupported:{}:expected:{CONFIG_STORE_SCHEMA_VERSION}",
            export.config_store_schema_version
        ));
    }
    if export.main_config.config_version != CURRENT_LINE_CONFIG_VERSION {
        return Err(format!(
            "config_export_file_version_unsupported:{}:expected:{CURRENT_LINE_CONFIG_VERSION}",
            export.main_config.config_version
        ));
    }
    for row in &export.overrides {
        if row.binding_hash.len() != 64 || !row.binding_hash.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Err("config_export_override_binding_invalid".to_string());
        }
        if row.schema_version != OVERRIDE_SCHEMA_VERSION {
            return Err(format!(
                "config_export_override_schema_unsupported:{}",
                row.schema_version
            ));
        }
        validate_stored_override_document(&row.binding_hash, row.schema_version, &row.document)
            .map_err(|error| format!("config_export_override_invalid:{error}"))?;
    }
    Ok(())
}

fn summarize(connection: &Connection) -> Result<MaintenanceSummary, String> {
    let count = |table: &str| -> Result<usize, String> {
        if !table_exists(connection, table)? {
            return Ok(0);
        }
        connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|value| value as usize)
            .map_err(|error| format!("config_count_failed:{table}:{error}"))
    };
    Ok(MaintenanceSummary {
        line_profiles: count("config_line_profiles")?,
        modem_slots: count("config_modem_slots")?,
        standalone_sim_slots: count("config_standalone_sim_slots")?,
        overrides: count("ims_sim_overrides")?,
    })
}

fn summary_counts(summary: &MaintenanceSummary) -> (usize, usize, usize, usize) {
    (
        summary.line_profiles,
        summary.modem_slots,
        summary.standalone_sim_slots,
        summary.overrides,
    )
}

// --- file plumbing ----------------------------------------------------------

fn ensure_source(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err("config_maintenance_source_symlink_rejected".to_string())
        }
        Ok(metadata) if !metadata.is_file() => {
            Err("config_maintenance_source_not_regular_file".to_string())
        }
        Ok(_) => Ok(()),
        Err(error) => Err(format!("config_maintenance_source_missing:{error}")),
    }
}

/// Never overwrite an operator's file. They remove it deliberately or choose
/// another name.
fn ensure_new_destination(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Err(format!(
            "config_maintenance_destination_exists:{}",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("config_maintenance_mkdir_failed:{error}"))?;
        }
    }
    Ok(())
}

fn open_read_only(path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("config_maintenance_open_read_only_failed:{error}"))
}

fn open_writable(path: &Path) -> Result<Connection, String> {
    let connection = Connection::open(path)
        .map_err(|error| format!("config_maintenance_open_failed:{error}"))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| format!("config_maintenance_timeout_failed:{error}"))?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|error| format!("config_maintenance_pragma_failed:{error}"))?;
    Ok(connection)
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![table],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .map_err(|error| format!("config_maintenance_table_probe_failed:{table}:{error}"))
}

fn copy_private(source: &Path, destination: &Path) -> Result<(), String> {
    fs::copy(source, destination)
        .map_err(|error| format!("config_maintenance_copy_failed:{error}"))?;
    set_private_file_mode(destination)?;
    sync_file(destination)?;
    Ok(())
}

fn atomic_write_private(path: &Path, content: &[u8]) -> Result<(), String> {
    let temp_path = path.with_extension("tmp");
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp_path)
        .map_err(|error| format!("config_maintenance_temp_open_failed:{error}"))?;
    file.write_all(content)
        .map_err(|error| format!("config_maintenance_temp_write_failed:{error}"))?;
    file.sync_all()
        .map_err(|error| format!("config_maintenance_temp_sync_failed:{error}"))?;
    drop(file);
    fs::rename(&temp_path, path).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        format!("config_maintenance_rename_failed:{error}")
    })?;
    set_private_file_mode(path)?;
    sync_parent(path)
}

fn sync_file(path: &Path) -> Result<(), String> {
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|error| format!("config_maintenance_sync_open_failed:{error}"))?;
    file.sync_all()
        .map_err(|error| format!("config_maintenance_sync_failed:{error}"))
}

fn sync_parent(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
            let _ = directory.sync_all();
        }
    }
    let _ = path;
    Ok(())
}

fn set_private_file_mode(_path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(_path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("config_maintenance_permissions_failed:{error}"))?;
    }
    Ok(())
}

fn maintenance_path(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    path.with_file_name(format!("{name}.{suffix}-{}", timestamp_tag()))
}

fn sqlite_maintenance_path(path: &Path, suffix: &str) -> PathBuf {
    maintenance_path(path, suffix)
}

/// Where `backup` puts the file half beside a database snapshot, and where
/// `restore` looks for it. Derived from the snapshot name so the pair travels
/// together without the operator having to name both.
fn paired_config_path(database_snapshot: &Path, config_path: &Path) -> PathBuf {
    let snapshot_name = database_snapshot
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("snapshot");
    let config_extension = config_path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("yaml");
    database_snapshot.with_file_name(format!("{snapshot_name}.config.{config_extension}"))
}

fn timestamp_tag() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}
