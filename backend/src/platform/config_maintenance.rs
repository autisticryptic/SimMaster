//! Explicit SQLite configuration maintenance.
//!
//! These commands are intentionally separate from startup. SimAdmin never
//! scans or imports legacy JSON automatically; an operator must explicitly
//! request export/import/restore and receives a rollback snapshot path.

use rusqlite::{params, Connection, OpenFlags, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::config::{
    AppConfig, ConfigManager, CONFIG_STORAGE_SCHEMA_VERSION, CURRENT_LINE_CONFIG_VERSION,
};
use crate::connectivity::modems::ims::profile_override::{
    validate_stored_override_document, OVERRIDE_SCHEMA_VERSION,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigExport {
    pub format: u32,
    pub storage_schema_version: u32,
    pub app_config: AppConfig,
    pub overrides: Vec<OverrideExportRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverrideExportRow {
    pub binding_hash: String,
    pub schema_version: u32,
    pub document: serde_json::Value,
    pub updated_at: String,
}

pub fn backup_database(source: &Path, destination: &Path) -> Result<usize, String> {
    ensure_source(source)?;
    ensure_new_destination(destination)?;
    if source == destination {
        return Err("config_backup_source_equals_destination".to_string());
    }
    let source_connection = open_read_only(source)?;
    let rows = validate_connection(&source_connection, source)?;
    let destination_sql = destination.to_string_lossy().to_string();
    source_connection
        .execute("VACUUM main INTO ?1", params![destination_sql])
        .map_err(|error| format!("config_backup_vacuum_failed:{error}"))?;
    sync_file(destination)?;
    set_private_file_mode(destination)?;
    let copied = open_read_only(destination)?;
    let copied_rows = validate_connection(&copied, destination)?;
    if rows != copied_rows {
        return Err("config_backup_row_count_changed".to_string());
    }
    sync_parent(destination)?;
    Ok(rows)
}

pub fn export_json(source: &Path, destination: &Path) -> Result<usize, String> {
    ensure_source(source)?;
    ensure_new_destination(destination)?;
    let connection = open_read_only(source)?;
    validate_connection(&connection, source)?;
    let config_json: String = connection
        .query_row(
            "SELECT config_json FROM app_config WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("config_export_app_config_missing:{error}"))?;
    let app_config: AppConfig = serde_json::from_str(&config_json)
        .map_err(|error| format!("config_export_app_config_invalid:{error}"))?;
    let rows = if table_exists(&connection, "ims_sim_overrides")? {
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
            .map_err(|error| format!("config_export_override_read_failed:{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("config_export_override_row_failed:{error}"))?;
        rows
    } else {
        Vec::new()
    };
    let count = rows.len();
    let export = ConfigExport {
        format: 1,
        storage_schema_version: CONFIG_STORAGE_SCHEMA_VERSION,
        app_config,
        overrides: rows,
    };
    validate_export(&export)?;
    let content = serde_json::to_vec_pretty(&export)
        .map_err(|error| format!("config_export_serialize_failed:{error}"))?;
    atomic_write_private(destination, &content)?;
    Ok(count)
}

/// Import a deliberately exported document. Returns the rollback SQLite
/// snapshot retained before the transaction, or `None` when the target did
/// not exist yet.
pub fn import_json(target: &Path, source: &Path) -> Result<Option<PathBuf>, String> {
    ensure_source(source)?;
    let content = fs::read(source).map_err(|error| format!("config_import_read_failed:{error}"))?;
    let export: ConfigExport = serde_json::from_slice(&content)
        .map_err(|error| format!("config_import_document_invalid:{error}"))?;
    validate_export(&export)?;

    let rollback = if target.exists() {
        let path = sqlite_maintenance_path(target, "import-rollback");
        backup_database(target, &path)?;
        Some(path)
    } else {
        None
    };
    let mut connection = open_writable(target)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("config_import_begin_failed:{error}"))?;
    transaction
        .execute(
            "INSERT INTO app_config (
                singleton, storage_schema_version, line_config_version, config_json, updated_at
             ) VALUES (1, ?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             ON CONFLICT(singleton) DO UPDATE SET
                storage_schema_version=excluded.storage_schema_version,
                line_config_version=excluded.line_config_version,
                config_json=excluded.config_json,
                updated_at=excluded.updated_at",
            params![
                CONFIG_STORAGE_SCHEMA_VERSION,
                export.app_config.line_config_version,
                serde_json::to_string(&export.app_config)
                    .map_err(|error| format!("config_import_serialize_config_failed:{error}"))?,
            ],
        )
        .map_err(|error| format!("config_import_app_config_failed:{error}"))?;
    transaction
        .execute("DELETE FROM ims_sim_overrides", [])
        .map_err(|error| format!("config_import_clear_overrides_failed:{error}"))?;
    for row in &export.overrides {
        transaction
            .execute(
                "INSERT INTO ims_sim_overrides(binding_hash,schema_version,document_json,updated_at)
                 VALUES (?1,?2,?3,?4)",
                params![
                    row.binding_hash,
                    row.schema_version,
                    serde_json::to_string(&row.document)
                        .map_err(|error| format!("config_import_serialize_override_failed:{error}"))?,
                    row.updated_at,
                ],
            )
            .map_err(|error| format!("config_import_override_failed:{error}"))?;
    }
    transaction
        .commit()
        .map_err(|error| format!("config_import_commit_failed:{error}"))?;
    ConfigManager::try_new(target.to_path_buf())
        .map_err(|error| format!("config_import_post_validate_failed:{error}"))?;
    Ok(rollback)
}

/// Restore a SQLite maintenance snapshot into the canonical configuration
/// tables. The current target is snapshotted first and returned to the caller,
/// giving restore the same explicit rollback boundary as JSON import.
pub fn restore_database(target: &Path, source: &Path) -> Result<Option<PathBuf>, String> {
    ensure_source(source)?;
    if target == source {
        return Err("config_restore_source_equals_target".to_string());
    }
    let source_connection = open_read_only(source)?;
    validate_connection(&source_connection, source)?;
    drop(source_connection);

    let rollback = if target.exists() {
        let path = sqlite_maintenance_path(target, "restore-rollback");
        backup_database(target, &path)?;
        Some(path)
    } else {
        None
    };

    let mut target_connection = open_writable(target)?;
    let source_path = source.to_string_lossy().into_owned();
    target_connection
        .execute("ATTACH DATABASE ?1 AS restore_source", params![source_path])
        .map_err(|error| format!("config_restore_attach_failed:{error}"))?;
    let transaction = target_connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("config_restore_begin_failed:{error}"))?;
    transaction
        .execute_batch(
            "DELETE FROM app_config;
             INSERT INTO app_config
                 SELECT * FROM restore_source.app_config;
             DELETE FROM ims_sim_overrides;
             INSERT INTO ims_sim_overrides
                 SELECT * FROM restore_source.ims_sim_overrides;
             DELETE FROM config_schema_journal;
             INSERT INTO config_schema_journal
                 SELECT * FROM restore_source.config_schema_journal;",
        )
        .map_err(|error| format!("config_restore_copy_failed:{error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("config_restore_commit_failed:{error}"))?;
    target_connection
        .execute("DETACH DATABASE restore_source", [])
        .map_err(|error| format!("config_restore_detach_failed:{error}"))?;
    drop(target_connection);
    let validated = open_read_only(target)?;
    validate_connection(&validated, target)?;
    ConfigManager::try_new(target.to_path_buf())
        .map_err(|error| format!("config_restore_post_validate_failed:{error}"))?;
    Ok(rollback)
}

fn validate_export(export: &ConfigExport) -> Result<(), String> {
    if export.format != 1 || export.storage_schema_version != CONFIG_STORAGE_SCHEMA_VERSION {
        return Err("config_import_schema_unsupported".to_string());
    }
    let serialized = serde_json::to_string(&export.app_config)
        .map_err(|error| format!("config_import_app_config_serialize_failed:{error}"))?;
    if export.app_config.line_config_version != CURRENT_LINE_CONFIG_VERSION {
        return Err("config_import_line_schema_unsupported".to_string());
    }
    if serde_json::from_str::<AppConfig>(&serialized).is_err() {
        return Err("config_import_app_config_invalid".to_string());
    }
    for row in &export.overrides {
        if row.schema_version != OVERRIDE_SCHEMA_VERSION
            || row.binding_hash.len() != 64
            || !row
                .binding_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("config_import_override_schema_invalid".to_string());
        }
        if !row.document.is_object() {
            return Err("config_import_override_document_invalid".to_string());
        }
        validate_stored_override_document(&row.binding_hash, row.schema_version, &row.document)
            .map_err(|error| format!("config_import_override_invalid:{}", error.code()))?;
    }
    Ok(())
}

fn ensure_source(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err("config_maintenance_source_symlink_rejected".to_string())
        }
        Ok(metadata) if !metadata.is_file() => {
            Err("config_maintenance_source_not_regular".to_string())
        }
        Ok(_) => Ok(()),
        Err(error) => Err(format!("config_maintenance_source_missing:{error}")),
    }
}

fn ensure_new_destination(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Err("config_maintenance_destination_exists".to_string());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("config_maintenance_destination_parent:{error}"))?;
    }
    Ok(())
}

fn open_read_only(path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("config_maintenance_open_readonly:{error}"))
}

fn open_writable(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("config_maintenance_parent:{error}"))?;
    }
    let connection = Connection::open(path)
        .map_err(|error| format!("config_maintenance_open_writable:{error}"))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| format!("config_maintenance_busy_timeout:{error}"))?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(|error| format!("config_maintenance_wal:{error}"))?;
    connection
        .execute_batch(
            "PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS app_config (
                 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                 storage_schema_version INTEGER NOT NULL,
                 line_config_version INTEGER NOT NULL,
                 config_json TEXT NOT NULL CHECK(json_valid(config_json)),
                 updated_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS ims_sim_overrides (
                 binding_hash TEXT PRIMARY KEY CHECK(length(binding_hash)=64),
                 schema_version INTEGER NOT NULL,
                 document_json TEXT NOT NULL CHECK(json_valid(document_json)),
                 updated_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS config_schema_journal (
                 version INTEGER PRIMARY KEY,
                 applied_at TEXT NOT NULL,
                 note TEXT NOT NULL
             );
             INSERT OR IGNORE INTO config_schema_journal(version, applied_at, note)
                 VALUES (1, strftime('%Y-%m-%dT%H:%M:%fZ','now'), 'initial schema');",
        )
        .map_err(|error| format!("config_maintenance_schema:{error}"))?;
    set_private_file_mode(path)?;
    Ok(connection)
}

fn validate_connection(connection: &Connection, path: &Path) -> Result<usize, String> {
    let quick_check: String = connection
        .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
        .map_err(|error| format!("config_maintenance_quick_check:{error}"))?;
    if quick_check != "ok" {
        return Err(format!("config_maintenance_corrupt:{quick_check}"));
    }
    let (storage_version, config_json): (u32, String) = connection
        .query_row(
            "SELECT storage_schema_version, config_json FROM app_config WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| {
            format!(
                "config_maintenance_app_config_missing:{}:{error}",
                path.display()
            )
        })?;
    if storage_version != CONFIG_STORAGE_SCHEMA_VERSION {
        return Err("config_maintenance_storage_schema_unsupported".to_string());
    }
    let journal_version: Option<u32> = connection
        .query_row(
            "SELECT MAX(version) FROM config_schema_journal",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("config_maintenance_schema_journal_invalid:{error}"))?;
    if journal_version != Some(CONFIG_STORAGE_SCHEMA_VERSION) {
        return Err("config_maintenance_schema_journal_unsupported".to_string());
    }
    serde_json::from_str::<AppConfig>(&config_json)
        .map_err(|error| format!("config_maintenance_app_config_invalid:{error}"))?;
    let count = if table_exists(connection, "ims_sim_overrides")? {
        connection
            .query_row("SELECT COUNT(*) FROM ims_sim_overrides", [], |row| {
                row.get(0)
            })
            .map_err(|error| format!("config_maintenance_override_count:{error}"))?
    } else {
        0
    };
    Ok(count)
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            params![table],
            |row| row.get(0),
        )
        .map_err(|error| format!("config_maintenance_schema_query:{error}"))
}

fn atomic_write_private(path: &Path, content: &[u8]) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| format!("config_export_parent:{error}"))?;
    let temp = maintenance_path(path, "tmp");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(|error| format!("config_export_temp:{error}"))?;
    file.write_all(content)
        .map_err(|error| format!("config_export_write:{error}"))?;
    file.sync_all()
        .map_err(|error| format!("config_export_sync:{error}"))?;
    drop(file);
    set_private_file_mode(&temp)?;
    fs::rename(&temp, path).map_err(|error| format!("config_export_rename:{error}"))?;
    sync_parent(path)
}

fn sync_file(path: &Path) -> Result<(), String> {
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|error| format!("config_maintenance_sync_open:{error}"))?;
    file.sync_all()
        .map_err(|error| format!("config_maintenance_sync:{error}"))
}

fn sync_parent(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
            directory
                .sync_all()
                .map_err(|error| format!("config_maintenance_sync_parent:{error}"))?;
        }
    }
    Ok(())
}

fn set_private_file_mode(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("config_maintenance_permissions:{error}"))?;
    }
    Ok(())
}

fn maintenance_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(format!(".{suffix}.{}.tmp", std::process::id()));
    PathBuf::from(value)
}

fn sqlite_maintenance_path(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    path.with_file_name(format!("{name}.{suffix}.{}.sqlite3", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::modems::ims::profile_override::{
        SimBindingKey, SimOverride, SimOverrideStore,
    };

    fn test_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "simadmin-config-maintenance-{name}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn source_database(path: &Path, ttl: i64) {
        let manager = ConfigManager::try_new(path.to_path_buf()).unwrap();
        let mut security = manager.get_security();
        security.session_ttl_seconds = ttl;
        manager.set_security(security).unwrap();
        let key = SimBindingKey::resolve(Some("89014103211118510720"), None).unwrap();
        let mut override_ = SimOverride::default();
        override_.ims_common.voicemail_number = Some("+18005551234".to_string());
        SimOverrideStore::sqlite(path.to_path_buf())
            .save(&key, &override_)
            .unwrap();
    }

    #[test]
    fn online_backup_contains_app_config_and_sim_overrides() {
        let dir = test_dir("backup");
        let source = dir.join("config.sqlite3");
        let backup = dir.join("backup.sqlite3");
        source_database(&source, 7_200);
        assert_eq!(backup_database(&source, &backup).unwrap(), 1);
        assert_eq!(
            ConfigManager::try_new(backup.clone())
                .unwrap()
                .get_security()
                .session_ttl_seconds,
            7_200
        );
        let connection = open_read_only(&backup).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM ims_sim_overrides", [], |row| row
                    .get::<_, usize>(0))
                .unwrap(),
            1
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn explicit_export_import_replaces_atomically_and_retains_rollback() {
        let dir = test_dir("import");
        let source = dir.join("source.sqlite3");
        let target = dir.join("target.sqlite3");
        let export = dir.join("config-export.json");
        source_database(&source, 7_200);
        source_database(&target, 9_000);
        assert_eq!(export_json(&source, &export).unwrap(), 1);
        let rollback = import_json(&target, &export).unwrap().unwrap();
        assert!(rollback.exists());
        assert_eq!(
            ConfigManager::try_new(target)
                .unwrap()
                .get_security()
                .session_ttl_seconds,
            7_200
        );
        assert_eq!(
            ConfigManager::try_new(rollback)
                .unwrap()
                .get_security()
                .session_ttl_seconds,
            9_000
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_import_does_not_modify_target() {
        let dir = test_dir("invalid-import");
        let target = dir.join("target.sqlite3");
        let invalid = dir.join("invalid.json");
        source_database(&target, 8_000);
        fs::write(&invalid, b"{not-json").unwrap();
        assert!(import_json(&target, &invalid).is_err());
        assert_eq!(
            ConfigManager::try_new(target)
                .unwrap()
                .get_security()
                .session_ttl_seconds,
            8_000
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn destination_is_never_overwritten() {
        let dir = test_dir("existing-destination");
        let source = dir.join("source.sqlite3");
        let destination = dir.join("existing.sqlite3");
        source_database(&source, 7_200);
        fs::write(&destination, b"sentinel").unwrap();
        assert_eq!(
            backup_database(&source, &destination).unwrap_err(),
            "config_maintenance_destination_exists"
        );
        assert_eq!(fs::read(&destination).unwrap(), b"sentinel");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn forged_override_binding_hash_is_rejected_before_target_write() {
        let dir = test_dir("forged-binding");
        let source = dir.join("source.sqlite3");
        let target = dir.join("target.sqlite3");
        let export = dir.join("config-export.json");
        source_database(&source, 7_200);
        source_database(&target, 9_000);
        export_json(&source, &export).unwrap();
        let mut document: serde_json::Value =
            serde_json::from_slice(&fs::read(&export).unwrap()).unwrap();
        document["overrides"][0]["binding_hash"] = serde_json::Value::String("0".repeat(64));
        fs::write(&export, serde_json::to_vec(&document).unwrap()).unwrap();

        let error = import_json(&target, &export).unwrap_err();
        assert!(error.contains("sim_override_binding_hash_mismatch"));
        assert_eq!(
            ConfigManager::try_new(target)
                .unwrap()
                .get_security()
                .session_ttl_seconds,
            9_000
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupted_database_and_future_schema_are_rejected() {
        let dir = test_dir("corruption-version");
        let corrupt = dir.join("corrupt.sqlite3");
        fs::write(&corrupt, b"not a sqlite database").unwrap();
        let corrupt_backup = dir.join("corrupt-backup.sqlite3");
        assert!(backup_database(&corrupt, &corrupt_backup).is_err());
        assert!(!corrupt_backup.exists());

        let future = dir.join("future.sqlite3");
        source_database(&future, 7_200);
        let connection = Connection::open(&future).unwrap();
        connection
            .execute(
                "UPDATE app_config SET storage_schema_version = 999 WHERE singleton = 1",
                [],
            )
            .unwrap();
        drop(connection);
        let future_backup = dir.join("future-backup.sqlite3");
        assert_eq!(
            backup_database(&future, &future_backup).unwrap_err(),
            "config_maintenance_storage_schema_unsupported"
        );
        assert!(!future_backup.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn online_backup_takes_a_consistent_snapshot_during_writes() {
        let dir = test_dir("concurrent-backup");
        let source = dir.join("source.sqlite3");
        source_database(&source, 7_200);
        let writer_path = source.clone();
        let writer = std::thread::spawn(move || {
            let manager = ConfigManager::try_new(writer_path).unwrap();
            for value in 7_201..7_221 {
                let mut security = manager.get_security();
                security.session_ttl_seconds = value;
                manager.set_security(security).unwrap();
            }
        });
        let backup = dir.join("backup.sqlite3");
        backup_database(&source, &backup).unwrap();
        writer.join().unwrap();

        let ttl = ConfigManager::try_new(backup)
            .unwrap()
            .get_security()
            .session_ttl_seconds;
        assert!((7_200..=7_220).contains(&ttl));
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn read_only_source_can_be_backed_up_without_wal_copying() {
        use std::os::unix::fs::PermissionsExt;

        let dir = test_dir("readonly-source");
        let source = dir.join("source.sqlite3");
        source_database(&source, 7_200);
        fs::set_permissions(&source, fs::Permissions::from_mode(0o400)).unwrap();
        let backup = dir.join("backup.sqlite3");
        backup_database(&source, &backup).unwrap();
        assert_eq!(
            ConfigManager::try_new(backup)
                .unwrap()
                .get_security()
                .session_ttl_seconds,
            7_200
        );
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sqlite_restore_replaces_config_and_retains_previous_snapshot() {
        let dir = test_dir("restore");
        let source = dir.join("source.sqlite3");
        let target = dir.join("target.sqlite3");
        source_database(&source, 7_200);
        source_database(&target, 9_000);

        let rollback = restore_database(&target, &source).unwrap().unwrap();
        assert_eq!(
            ConfigManager::try_new(target)
                .unwrap()
                .get_security()
                .session_ttl_seconds,
            7_200
        );
        assert_eq!(
            ConfigManager::try_new(rollback)
                .unwrap()
                .get_security()
                .session_ttl_seconds,
            9_000
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
