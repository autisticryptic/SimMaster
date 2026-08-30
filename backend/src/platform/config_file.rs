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
use yaml_edit::{Document, MappingBuilder, SequenceBuilder, YamlFile};

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

    merge_into_document(&document, &previous_json, &desired_json);
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
) {
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
                merge_into_mapping(&child, &previous_object, desired_object);
            }
            _ => set_value(&SetTarget::Document(document), key, desired_child),
        }
    }
    for key in previous.keys() {
        if !desired.contains_key(key) {
            document.remove(key.as_str());
        }
    }
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
) {
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
                merge_into_mapping(&child, &previous_object, desired_object);
            }
            _ => set_value(&SetTarget::Mapping(mapping), key, desired_child),
        }
    }
    for key in previous.keys() {
        if !desired.contains_key(key) {
            mapping.remove(key.as_str());
        }
    }
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

    /// Fetch `key` as a mapping, for recursing into a subtree.
    fn get_mapping(&self, key: &str) -> Option<yaml_edit::Mapping> {
        match self {
            Self::Document(document) => document.get_mapping(key),
            Self::Mapping(mapping) => mapping.get_mapping(key),
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
/// Nested mappings are deliberately *not* built with `MappingBuilder` and
/// inserted as one value. Doing that indents only the first child correctly and
/// leaves the rest at the parent's level, producing a document that no longer
/// parses. Instead the key is created empty and then filled by recursion, so
/// every leaf goes through the single-key `set` path that keeps indentation
/// correct.
fn set_value(target: &SetTarget<'_>, key: &str, value: &Value) {
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
        Value::Array(items) => {
            // A sequence is a single node with no nested keys to indent, so the
            // builder is safe here.
            let mut builder = SequenceBuilder::new();
            for item in items {
                builder = push_sequence_item(builder, item);
            }
            target.set(key, builder.build_document());
        }
        Value::Object(entries) if entries.is_empty() => {
            target.set(key, MappingBuilder::new().build_document());
        }
        Value::Object(entries) => {
            // Seed the block with its first entry so the parser sees a mapping,
            // then recurse for the rest at the correct depth.
            let mut iterator = entries.iter();
            let Some((first_key, first_value)) = iterator.next() else {
                return;
            };
            let mut seed = MappingBuilder::new();
            seed = push_mapping_pair(seed, first_key, first_value);
            target.set(key, seed.build_document());

            let Some(child) = target.get_mapping(key) else {
                // The write did not produce a mapping we can descend into.
                // Leaving the remaining keys out would silently drop settings,
                // so fall back to the flat builder: `apply_update` verifies the
                // result and refuses to write a document that reads back wrong.
                let mut builder = MappingBuilder::new();
                for (child_key, child_value) in entries {
                    builder = push_mapping_pair(builder, child_key, child_value);
                }
                target.set(key, builder.build_document());
                return;
            };
            let child_target = SetTarget::Mapping(&child);
            for (child_key, child_value) in iterator {
                set_value(&child_target, child_key, child_value);
            }
        }
    }
}

fn push_sequence_item(builder: SequenceBuilder, value: &Value) -> SequenceBuilder {
    match value {
        Value::Null => builder.item(Option::<&str>::None),
        Value::Bool(flag) => builder.item(*flag),
        Value::Number(number) => match number.as_i64() {
            Some(int) => builder.item(int),
            None => builder.item(number.to_string().as_str()),
        },
        Value::String(text) => builder.item(text.as_str()),
        Value::Array(items) => {
            let mut nested = SequenceBuilder::new();
            for item in items {
                nested = push_sequence_item(nested, item);
            }
            builder.insert_sequence(nested)
        }
        Value::Object(entries) => {
            let mut nested = MappingBuilder::new();
            for (key, child) in entries {
                nested = push_mapping_pair(nested, key, child);
            }
            builder.insert_mapping(nested)
        }
    }
}

fn push_mapping_pair(builder: MappingBuilder, key: &str, value: &Value) -> MappingBuilder {
    match value {
        Value::Null => builder.pair(key, Option::<&str>::None),
        Value::Bool(flag) => builder.pair(key, *flag),
        Value::Number(number) => match number.as_i64() {
            Some(int) => builder.pair(key, int),
            None => builder.pair(key, number.to_string().as_str()),
        },
        Value::String(text) => builder.pair(key, text.as_str()),
        Value::Array(items) => {
            let mut nested = SequenceBuilder::new();
            for item in items {
                nested = push_sequence_item(nested, item);
            }
            builder.insert_sequence(key, nested)
        }
        Value::Object(entries) => {
            let mut nested = MappingBuilder::new();
            for (child_key, child) in entries {
                nested = push_mapping_pair(nested, child_key, child);
            }
            builder.insert_mapping(key, nested)
        }
    }
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
