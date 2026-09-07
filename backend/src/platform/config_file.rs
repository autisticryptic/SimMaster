//! Text-file backend for the main program configuration.
//!
//! This module owns every YAML detail in SimAdmin. Callers see four operations:
//! parse a file into a typed value, render a first-time file, apply an update
//! that preserves what the operator wrote, and write it out atomically.
//!
//! # Why two YAML crates
//!
//! Reading and writing have different failure costs, so they use different
//! libraries (see `Cargo.toml` for the full rationale):
//!
//!   - reading goes through `serde-saphyr`, which deserializes `AppConfig`
//!     through its existing serde derives. A parse bug here would mis-provision
//!     a SIM, so this side gets the mature, serde-native parser and no
//!     hand-written conversion layer.
//!   - writing goes through `yaml-edit`, the only crate that can replace a
//!     single value while keeping comments, key order and blank lines. `save()`
//!     needs that because the web UI rewrites settings on every change, and a
//!     serde-based writer would erase an operator's annotations the first time
//!     anyone touched a checkbox.
//!
//! Because `yaml-edit` is young, [`apply_update`] never returns text it has not
//! proven: it re-reads its own output with `serde-saphyr` and compares against
//! the intended value. A mismatch is reported instead of written.
//!
//! # Containment
//!
//! No `yaml_edit` or `serde_saphyr` type appears in this module's public API. If
//! either crate has to be replaced, or the project falls back to whole-document
//! rewrites, only this file changes.

use std::fmt::Debug;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{Map, Value};
use yaml_edit::{Document, YamlFile};

/// Text formats the configuration file may use.
///
/// YAML is the documented default because it takes comments. JSON stays
/// supported because it costs one match arm: an operator who prefers strict
/// JSON gives up comments and gets a canonical rewrite on every save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextFormat {
    Yaml,
    Json,
}

impl TextFormat {
    /// Classify by extension, or `None` for anything unrecognized so the caller
    /// fails closed rather than guessing how to parse an unknown file.
    pub fn from_path(path: &Path) -> Option<Self> {
        match path
            .extension()
            .and_then(|extension| extension.to_str())?
            .to_ascii_lowercase()
            .as_str()
        {
            "yaml" | "yml" => Some(Self::Yaml),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

/// Deserialize `content` into `T`.
///
/// An all-whitespace file is treated as "no settings written yet" and produces
/// `T::default()`. Without this, a freshly created empty file would fail to
/// parse and hold the service down; with it, an operator can blank the file to
/// reset to defaults.
pub fn parse<T>(content: &str, format: TextFormat, source: &Path) -> Result<T, String>
where
    T: DeserializeOwned + Default,
{
    if content.trim().is_empty() {
        return Ok(T::default());
    }
    match format {
        TextFormat::Json => serde_json::from_str(content)
            .map_err(|error| format!("Failed to parse {}: {error}", source.display())),
        TextFormat::Yaml => serde_saphyr::from_str(content)
            .map_err(|error| format!("Failed to parse {}: {error}", source.display())),
    }
}

/// Render a first-time file for `value`.
///
/// The emitter is hand-written rather than built through `yaml-edit` for two
/// reasons: it keeps serde's field order instead of sorting keys, and it can
/// interleave the documentation comments that make the file worth editing by
/// hand. `annotations` maps a top-level key to the comment lines placed above
/// it; `header` is placed once at the top.
pub fn render_new<T>(
    value: &T,
    format: TextFormat,
    header: &[&str],
    annotations: &[(&str, &[&str])],
) -> Result<String, String>
where
    T: Serialize,
{
    let json = serde_json::to_value(value)
        .map_err(|error| format!("Failed to serialize configuration: {error}"))?;
    match format {
        TextFormat::Json => serde_json::to_string_pretty(&json)
            .map(ensure_trailing_newline)
            .map_err(|error| format!("Failed to serialize configuration: {error}")),
        TextFormat::Yaml => {
            let object = json.as_object().ok_or_else(|| {
                "Configuration must serialize to a mapping at the top level".to_string()
            })?;
            let mut out = String::new();
            for line in header {
                push_comment(&mut out, line, 0);
            }
            if !header.is_empty() {
                out.push('\n');
            }
            for (key, child) in object {
                if let Some((_, comment_lines)) = annotations.iter().find(|(name, _)| name == key) {
                    for line in *comment_lines {
                        push_comment(&mut out, line, 0);
                    }
                }
                emit_entry(&mut out, key, child, 0);
                out.push('\n');
            }
            let rendered = ensure_trailing_newline(out);
            // Never hand back a document the parser cannot read.
            YamlFile::from_str(&rendered)
                .map_err(|error| format!("Generated configuration is not valid YAML: {error}"))?;
            Ok(rendered)
        }
    }
}

/// Apply `desired` onto the document in `content`, changing only what differs.
///
/// `previous` is what the program believes is currently in the file; it decides
/// which keys are rewritten. The authoritative text is `content`, so comments,
/// key order and spacing survive untouched for every key that did not change.
///
/// The result is verified by deserializing it again and comparing to `desired`,
/// so a writer bug surfaces here rather than as a silently damaged file.
pub fn apply_update<T>(
    content: &str,
    previous: &T,
    desired: &T,
    format: TextFormat,
    source: &Path,
    header: &[&str],
    annotations: &[(&str, &[&str])],
) -> Result<String, String>
where
    T: Serialize + DeserializeOwned + Default + PartialEq + Debug,
{
    // JSON carries no comments, so a canonical rewrite loses nothing.
    if format == TextFormat::Json {
        return render_new(desired, format, header, annotations);
    }
    // Nothing to preserve in an empty file; emit the documented layout instead
    // of growing one key at a time.
    if content.trim().is_empty() {
        return render_new(desired, format, header, annotations);
    }

    let previous_json = to_object(previous)?;
    let desired_json = to_object(desired)?;

    let file = YamlFile::from_str(content)
        .map_err(|error| format!("Failed to parse {}: {error}", source.display()))?;
    let document = file
        .document()
        .ok_or_else(|| format!("{} contains no YAML document", source.display()))?;

    merge_into_document(&document, &previous_json, &desired_json)?;
    let rendered = ensure_trailing_newline(file.to_string());

    // Verify with the parser, not the writer.
    let verified: T = parse(&rendered, format, source)?;
    if &verified != desired {
        return Err(format!(
            "Refusing to write {}: the updated document does not read back as intended. \
             This is a configuration-writer bug, not a bad setting.\n  intended: {desired:?}\n  \
             would read back as: {verified:?}",
            source.display()
        ));
    }
    Ok(rendered)
}

fn to_object<T: Serialize>(value: &T) -> Result<Map<String, Value>, String> {
    serde_json::to_value(value)
        .map_err(|error| format!("Failed to serialize configuration: {error}"))?
        .as_object()
        .cloned()
        .ok_or_else(|| "Configuration must serialize to a mapping at the top level".to_string())
}

/// Reconcile the top level of the document.
///
/// `Document` exposes the same mapping operations as a nested `Mapping`, so the
/// top level and every deeper level share [`merge_into_mapping`]'s logic; only
/// the accessors differ.
fn merge_into_document(
    document: &Document,
    previous: &Map<String, Value>,
    desired: &Map<String, Value>,
) -> Result<(), String> {
    for (key, desired_child) in desired {
        let previous_child = previous.get(key);
        if previous_child == Some(desired_child) && document.contains_key(key.as_str()) {
            continue;
        }
        match (desired_child, document.get_mapping(key.as_str())) {
            (Value::Object(desired_object), Some(child)) => {
                let previous_object = previous_child
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                merge_into_mapping(&child, &previous_object, desired_object)?;
            }
            _ => set_value(&SetTarget::Document(document), key, desired_child)?,
        }
    }
    for key in previous.keys() {
        if !desired.contains_key(key) {
            document.remove(key.as_str());
        }
    }
    Ok(())
}

/// Reconcile one nested mapping level.
///
/// Descending into an existing child mapping, instead of replacing it wholesale,
/// is what preserves comments that live *inside* a nested block. A subtree is
/// rebuilt only when the file does not already hold a mapping at that key.
fn merge_into_mapping(
    mapping: &yaml_edit::Mapping,
    previous: &Map<String, Value>,
    desired: &Map<String, Value>,
) -> Result<(), String> {
    for (key, desired_child) in desired {
        let previous_child = previous.get(key);
        if previous_child == Some(desired_child) && mapping.contains_key(key.as_str()) {
            continue;
        }
        match (desired_child, mapping.get_mapping(key.as_str())) {
            (Value::Object(desired_object), Some(child)) => {
                let previous_object = previous_child
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                merge_into_mapping(&child, &previous_object, desired_object)?;
            }
            _ => set_value(&SetTarget::Mapping(mapping), key, desired_child)?,
        }
    }
    for key in previous.keys() {
        if !desired.contains_key(key) {
            mapping.remove(key.as_str());
        }
    }
    Ok(())
}

/// `Document` and `Mapping` both offer `set`, but through separate inherent
/// impls rather than a shared trait, so the two call sites are unified here
/// instead of duplicating the value-kind match.
enum SetTarget<'a> {
    Document(&'a Document),
    Mapping(&'a yaml_edit::Mapping),
}

impl SetTarget<'_> {
    fn set<V: yaml_edit::AsYaml>(&self, key: &str, value: V) {
        match self {
            Self::Document(document) => document.set(key, value),
            Self::Mapping(mapping) => mapping.set(key, value),
        }
    }
}

/// Write one JSON value at `key`.
///
/// Scalars are passed as native Rust types because `yaml-edit`'s `AsYaml` impl
/// for `&str` applies YAML's disambiguating quoting: a string like `"22:00"`,
/// `"true"` or a numeric DDNS access ID comes back as a string instead of a
/// sexagesimal, a boolean or an integer.
///
/// New compound subtrees use parsed flow YAML (JSON is a YAML subset), not a
/// detached builder tree whose nested indentation/key indexes may be invalid.
/// Existing mappings still merge in place above, preserving their comments.
/// Only NEW/replaced compound values use flow style; unrelated operator text
/// is never re-rendered. The complete result is still parsed and verified.
fn set_value(target: &SetTarget<'_>, key: &str, value: &Value) -> Result<(), String> {
    match value {
        Value::Null => target.set(key, Option::<&str>::None),
        Value::Bool(flag) => target.set(key, *flag),
        Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                target.set(key, int);
            } else if let Some(uint) = number.as_u64() {
                target.set(key, uint);
            } else if let Some(float) = number.as_f64() {
                target.set(key, float);
            } else {
                target.set(key, number.to_string().as_str());
            }
        }
        Value::String(text) => target.set(key, text.as_str()),
        Value::Array(_) | Value::Object(_) => {
            let literal = serde_json::to_string(value)
                .map_err(|error| format!("Failed to serialize setting {key}: {error}"))?;
            let file = YamlFile::from_str(&literal)
                .map_err(|error| format!("Failed to parse new setting {key}: {error}"))?;
            let document = file
                .document()
                .ok_or_else(|| format!("New setting {key} has no YAML document"))?;
            // Document::AsYaml is a document wrapper, not an inline value, and
            // ignores the destination indentation. Insert the actual parsed
            // Mapping/Sequence node instead.
            if matches!(value, Value::Object(_)) {
                let mapping = document
                    .as_mapping()
                    .ok_or_else(|| format!("New setting {key} is not a mapping"))?;
                target.set(key, mapping);
            } else {
                let sequence = document
                    .as_sequence()
                    .ok_or_else(|| format!("New setting {key} is not a sequence"))?;
                target.set(key, sequence);
            }
        }
    }
    Ok(())
}

// --- first-time emitter -----------------------------------------------------

fn push_comment(out: &mut String, line: &str, indent: usize) {
    for _ in 0..indent {
        out.push(' ');
    }
    if line.is_empty() {
        out.push_str("#\n");
    } else {
        out.push_str("# ");
        out.push_str(line);
        out.push('\n');
    }
}

fn emit_entry(out: &mut String, key: &str, value: &Value, indent: usize) {
    let pad = " ".repeat(indent);
    match value {
        Value::Object(entries) if entries.is_empty() => {
            out.push_str(&format!("{pad}{key}: {{}}\n"));
        }
        Value::Object(entries) => {
            out.push_str(&format!("{pad}{key}:\n"));
            for (child_key, child) in entries {
                emit_entry(out, child_key, child, indent + 2);
            }
        }
        Value::Array(items) if items.is_empty() => {
            out.push_str(&format!("{pad}{key}: []\n"));
        }
        Value::Array(items) => {
            out.push_str(&format!("{pad}{key}:\n"));
            for item in items {
                emit_sequence_item(out, item, indent + 2);
            }
        }
        scalar => {
            out.push_str(&format!("{pad}{key}: {}\n", emit_scalar(scalar)));
        }
    }
}

fn emit_sequence_item(out: &mut String, value: &Value, indent: usize) {
    let pad = " ".repeat(indent);
    match value {
        Value::Object(entries) if !entries.is_empty() => {
            let mut first = true;
            for (key, child) in entries {
                if first {
                    out.push_str(&format!("{pad}- "));
                    first = false;
                } else {
                    out.push_str(&format!("{pad}  "));
                }
                match child {
                    Value::Object(_) | Value::Array(_) => {
                        out.push_str(&format!("{key}:\n"));
                        emit_entry_body(out, child, indent + 4);
                    }
                    scalar => out.push_str(&format!("{key}: {}\n", emit_scalar(scalar))),
                }
            }
        }
        Value::Array(items) => {
            out.push_str(&format!("{pad}-\n"));
            for item in items {
                emit_sequence_item(out, item, indent + 2);
            }
        }
        scalar => out.push_str(&format!("{pad}- {}\n", emit_scalar(scalar))),
    }
}

fn emit_entry_body(out: &mut String, value: &Value, indent: usize) {
    match value {
        Value::Object(entries) => {
            for (key, child) in entries {
                emit_entry(out, key, child, indent);
            }
        }
        Value::Array(items) => {
            for item in items {
                emit_sequence_item(out, item, indent);
            }
        }
        _ => {}
    }
}

/// Render one scalar, quoting strings that YAML would otherwise reinterpret.
///
/// The set mirrors `yaml-edit`'s own rule so a first-time file and a later
/// surgical rewrite agree: YAML 1.1 booleans (`yes`/`no`/`on`/`off`), `null`,
/// `~`, anything that parses as a number, and anything with leading or
/// structural punctuation.
fn emit_scalar(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => {
            if needs_quoting(text) {
                format!("'{}'", text.replace('\'', "''"))
            } else {
                text.clone()
            }
        }
        other => other.to_string(),
    }
}

fn needs_quoting(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    const RESERVED: [&str; 8] = ["true", "false", "yes", "no", "on", "off", "null", "~"];
    if RESERVED.iter().any(|word| value.eq_ignore_ascii_case(word)) {
        return true;
    }
    if value.parse::<i64>().is_ok() || value.parse::<f64>().is_ok() {
        return true;
    }
    if value.starts_with([
        '-', '?', '[', ']', '{', '}', ',', '>', '<', '&', '*', '!', '|', '\'', '"', '%', '@', '`',
        '#', ' ',
    ]) {
        return true;
    }
    if value.ends_with(' ') || value.contains('\n') || value.contains(": ") || value.contains(" #")
    {
        return true;
    }
    value.contains(':') && value.ends_with(':')
}

fn ensure_trailing_newline(mut text: String) -> String {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn adding_a_missing_root_with_a_nested_first_child_keeps_indentation() {
        let source = "# operator comment\nconfig_version: 5\nproxy:\n  enabled: true\n";
        // Typed configuration has already filled absent defaults in memory.
        let previous = json!({
            "config_version": 5,
            "proxy": {"enabled": true},
            "device_network": {"ddns": {
                "access_id": "", "enabled": false,
                "ipv4": {"fallbacks": ["first", "second"], "source": "interface"}
            }}
        });
        let mut desired = previous.clone();
        desired["proxy"]["enabled"] = json!(false);
        let rendered = apply_update(
            source,
            &previous,
            &desired,
            TextFormat::Yaml,
            Path::new("fixture.yaml"),
            &[],
            &[],
        )
        .unwrap();
        let parsed: Value = parse(&rendered, TextFormat::Yaml, Path::new("fixture.yaml")).unwrap();
        assert_eq!(parsed, desired);
        assert!(rendered.contains("# operator comment"));
        assert!(!rendered.lines().any(|line| line.starts_with("access_id:")));
    }

    #[test]
    fn adding_nested_subtrees_preserves_existing_mapping_comments() {
        let source =
            "# owned by operator\ndevice_network:\n  # keep enabled comment\n  enabled: true\n";
        let previous = json!({"device_network": {"enabled": true}});
        let desired = json!({"device_network": {
            "enabled": true,
            "ddns": {"ipv4": {"access": {"id": "00123", "secret": ""}}, "provider": "test"}
        }});
        let rendered = apply_update(
            source,
            &previous,
            &desired,
            TextFormat::Yaml,
            Path::new("fixture.yaml"),
            &[],
            &[],
        )
        .unwrap();
        let parsed: Value = parse(&rendered, TextFormat::Yaml, Path::new("fixture.yaml")).unwrap();
        assert_eq!(parsed, desired);
        assert!(rendered.contains("# owned by operator"));
        assert!(rendered.contains("# keep enabled comment"));
        // The inserted flow subtree remains editable on the next save.
        let mut changed = desired.clone();
        changed["device_network"]["ddns"]["ipv4"]["access"]["id"] = json!("00456");
        let updated = apply_update(
            &rendered,
            &desired,
            &changed,
            TextFormat::Yaml,
            Path::new("fixture.yaml"),
            &[],
            &[],
        )
        .unwrap();
        let parsed: Value = parse(&updated, TextFormat::Yaml, Path::new("fixture.yaml")).unwrap();
        assert_eq!(parsed, changed);
        assert!(updated.contains("# keep enabled comment"));
    }
}

// --- durability -------------------------------------------------------------

/// Write `content` to `path` atomically, keeping one `.bak` generation.
///
/// The file holds DDNS credentials and proxy passwords, so it is created `0600`
/// and the mode is re-asserted on every write: an operator's editor may have
/// recreated it world-readable.
pub fn write_atomically(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Failed to create config directory: {error}"))?;
        }
    }

    let temp_path = temporary_sibling(path);
    let backup_path = backup_path_for(path);

    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut temp_file = options
        .open(&temp_path)
        .map_err(|error| format!("Failed to open temporary config file: {error}"))?;
    temp_file
        .write_all(content.as_bytes())
        .map_err(|error| format!("Failed to write temporary config file: {error}"))?;
    temp_file
        .sync_all()
        .map_err(|error| format!("Failed to sync temporary config file: {error}"))?;
    drop(temp_file);

    if path.exists() {
        fs::copy(path, &backup_path)
            .map_err(|error| format!("Failed to back up config file: {error}"))?;
    }
    if let Err(rename_error) = fs::rename(&temp_path, path) {
        // Windows refuses to rename onto a file another handle has open; the
        // test suite runs there, so fall back to copy + unlink.
        if cfg!(windows) && path.exists() {
            fs::copy(&temp_path, path)
                .map_err(|error| format!("Failed to replace config file: {error}"))?;
            fs::remove_file(&temp_path)
                .map_err(|error| format!("Failed to remove temporary config file: {error}"))?;
        } else {
            let _ = fs::remove_file(&temp_path);
            return Err(format!(
                "Failed to atomically replace config file: {rename_error}"
            ));
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("Failed to set config file permissions: {error}"))?;
        if let Some(parent) = path.parent() {
            if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
                let _ = directory.sync_all();
            }
        }
    }
    Ok(())
}

/// Path of the backup written before each save, so a caller can restore it.
pub fn backup_path_for(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    path.with_file_name(format!("{name}.bak"))
}

fn temporary_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    path.with_file_name(format!(".{name}.tmp"))
}

/// Reject a path the program must not write through.
///
/// A symlinked configuration file would let whoever owns the link redirect a
/// `0600` write, and the credentials in it, to a file they control.
pub fn ensure_regular_file(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(format!("Refusing symlink config file {}", path.display()))
        }
        Ok(metadata) if !metadata.is_file() => Err(format!(
            "Config path is not a regular file: {}",
            path.display()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Failed to inspect config file {}: {error}",
            path.display()
        )),
    }
}
