//! On-disk diagnostic log.
//!
//! The web UI shows only the newest handful of activity entries, and it
//! truncates long error strings to keep the panel readable. That is exactly the
//! information needed to reconstruct a field failure after the fact, so this
//! module writes every application event to a plain-text file with the raw,
//! untruncated error chain attached.
//!
//! Three properties matter more than throughput here:
//!
//!   - **Never stall the caller.** Records go through a bounded channel and are
//!     dropped (with a counter) if the writer falls behind. A wedged disk must
//!     not be able to block IMS registration.
//!   - **Attribute every line to a scope.** Each record says whether it is
//!     device-wide or about one line's UE, so a multi-SIM device retrying on
//!     several cards at once stays readable. See [`ExecutionContext`] for why
//!     this is ownership rather than the OS thread.
//!   - **Bound growth by both age and bytes.** Whichever limit trips first wins,
//!     so a registration retry storm cannot fill the device flash and an idle
//!     device still ages history out.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Utc};
use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use crate::platform::config::{ConfigManager, DiagnosticLogConfig, DiagnosticLogSeverity};

/// Beijing time, matching the timestamps the rest of the product displays.
const LOG_UTC_OFFSET_SECONDS: i32 = 8 * 3600;
const FILE_PREFIX: &str = "simadmin-diagnostic-";
const FILE_SUFFIX: &str = ".log";
/// Bounded so a slow disk drops records instead of applying backpressure to
/// callers on the registration path.
const CHANNEL_CAPACITY: usize = 2048;
/// Records drained per file open, to amortise the open/flush cost during bursts.
const BATCH_LIMIT: usize = 256;
/// Retention runs after this many bytes appended, in addition to at rotation.
const CLEANUP_BYTE_INTERVAL: u64 = 1024 * 1024;

/// Which scope a record belongs to.
///
/// This is deliberately *ownership*, not the OS thread that happened to emit the
/// line. Two reasons:
///
///   - Tokio moves tasks between worker threads freely, so neither the thread
///     name nor its id identifies anything stable: one line's task is observed
///     on several workers over its lifetime, and the shared runtime threads
///     serve every line in turn.
///   - The isolated UE is a separate *process* (see `services::ue_worker`) with
///     no handle on this sink, so no line here is ever written from inside it.
///     Its own output goes to tracing/journald.
///
/// So [`Self::UeWorker`] means "this record is about one line's UE", and
/// [`Self::Main`] means "device-wide". Scope is carried in a task-local set by
/// [`with_ue_worker_context`] and captured when the record is built, which is why
/// records must be built on the producing task rather than in a bus subscriber.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionContext {
    /// Device-wide work: request handlers, schedulers, system events.
    Main,
    /// Work owned by a single line's UE.
    UeWorker,
}

impl ExecutionContext {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::UeWorker => "ue_worker",
        }
    }

    /// The scope of the current task, defaulting to [`Self::Main`] outside a
    /// per-line scope.
    pub fn current() -> Self {
        CURRENT_CONTEXT
            .try_with(|value| *value)
            .unwrap_or(Self::Main)
    }
}

tokio::task_local! {
    static CURRENT_CONTEXT: ExecutionContext;
}

/// Run `future` with its records attributed to one line's UE.
///
/// Wrap any body whose records are about a single line — a per-line restore
/// workflow, or the per-line iteration of a shared poller. Task-locals do not
/// cross `tokio::spawn`, so a task spawned inside that logs on its own must be
/// wrapped again.
pub async fn with_ue_worker_context<F>(future: F) -> F::Output
where
    F: std::future::Future,
{
    CURRENT_CONTEXT
        .scope(ExecutionContext::UeWorker, future)
        .await
}

/// One line in the diagnostic log.
#[derive(Debug, Clone)]
pub struct DiagnosticRecord {
    pub timestamp: DateTime<FixedOffset>,
    pub severity: DiagnosticLogSeverity,
    /// Short subsystem tag, e.g. `VoLTE`. Written in brackets so a future split
    /// into per-subsystem files is a routing change rather than a format change.
    pub subsystem: String,
    pub context: ExecutionContext,
    pub line_id: Option<String>,
    /// Event or stage name, e.g. `volte.register.failed`.
    pub event: String,
    pub detail: Option<String>,
    /// The unmodified error string, however long or deeply nested. This is the
    /// field the UI truncates and the reason this log exists.
    pub raw_error: Option<String>,
}

impl DiagnosticRecord {
    pub fn new(
        severity: DiagnosticLogSeverity,
        subsystem: impl Into<String>,
        event: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: Utc::now().with_timezone(&log_offset()),
            severity,
            subsystem: subsystem.into(),
            context: ExecutionContext::current(),
            line_id: None,
            event: event.into(),
            detail: None,
            raw_error: None,
        }
    }

    pub fn with_line(mut self, line_id: impl Into<String>) -> Self {
        self.line_id = Some(line_id.into());
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_raw_error(mut self, raw_error: impl Into<String>) -> Self {
        self.raw_error = Some(raw_error.into());
        self
    }

    /// Render one log line. `redact` masks subscriber identifiers, phone
    /// numbers, message bodies and P-CSCF addresses.
    fn render(&self, redact: bool) -> String {
        let mut line = String::with_capacity(160);
        line.push_str(
            &self
                .timestamp
                .format("%Y-%m-%dT%H:%M:%S%.3f%:z")
                .to_string(),
        );
        line.push_str(&format!(" [{:<5}]", self.severity.as_label()));
        line.push_str(&format!(" [{}]", self.subsystem));
        line.push_str(&format!(" [{}]", self.context.as_label()));
        if let Some(line_id) = &self.line_id {
            line.push_str(&format!(" line={}", sanitize_field(line_id)));
        }
        line.push_str(&format!(" event={}", sanitize_field(&self.event)));
        if let Some(detail) = &self.detail {
            let detail = if redact {
                redact_text(detail)
            } else {
                detail.clone()
            };
            line.push_str(&format!(" detail={}", sanitize_field(&detail)));
        }
        if let Some(raw_error) = &self.raw_error {
            // Kept verbatim apart from newline flattening: the nested
            // `volte_command_failed:...` chain is the whole point of the file.
            let raw_error = if redact {
                redact_text(raw_error)
            } else {
                raw_error.clone()
            };
            line.push_str(&format!(" raw_error={}", sanitize_field(&raw_error)));
        }
        line.push('\n');
        line
    }
}

/// Build a record from an application event.
///
/// Called on the publishing task so [`ExecutionContext::current`] still sees the
/// producer's context. Deriving this downstream (in a bus subscriber) would
/// attribute every line to the subscriber's own task instead, collapsing the
/// main/UE distinction the file exists to preserve.
pub fn record_for_app_event(
    event_type: &str,
    line_id: Option<&str>,
    transport: Option<&str>,
    payload: &Value,
) -> DiagnosticRecord {
    let mut record = DiagnosticRecord::new(
        severity_for_event(event_type, payload),
        subsystem_for_event(event_type, transport),
        event_type,
    )
    .with_detail(payload.to_string());
    if let Some(line_id) = line_id {
        record = record.with_line(line_id);
    }
    if let Some(raw) = extract_raw_error(payload) {
        record = record.with_raw_error(raw);
    }
    record
}

/// Map an event to a bracketed subsystem tag.
///
/// Derived centrally from the event type rather than left to each producer, so a
/// later split into per-subsystem files is a routing change here rather than an
/// edit at every call site.
fn subsystem_for_event(event_type: &str, transport: Option<&str>) -> &'static str {
    match event_type.split('.').next().unwrap_or(event_type) {
        "volte" => "VoLTE",
        "vowifi" => "VoWiFi",
        "trunk" => "Trunk",
        "sms" => "SMS",
        "call" | "calls" => "Call",
        "system" => "System",
        _ => match transport {
            Some("volte_ims") => "VoLTE",
            Some("vowifi") => "VoWiFi",
            Some("trunk") => "Trunk",
            _ => "App",
        },
    }
}

/// Grade an event from its name and payload.
///
/// Events carry no level of their own, so failure is inferred from the event name
/// and from an error field actually holding a string — a `last_error` of null
/// means the most recent attempt succeeded.
fn severity_for_event(event_type: &str, payload: &Value) -> DiagnosticLogSeverity {
    if event_type.contains("failed")
        || event_type.contains("error")
        || event_type.contains("exhausted")
    {
        return DiagnosticLogSeverity::Error;
    }
    if extract_raw_error(payload).is_some() {
        return DiagnosticLogSeverity::Warn;
    }
    DiagnosticLogSeverity::Info
}

/// Pull the untruncated error string out of an event payload.
///
/// Searched recursively because attempt records and status snapshots both carry
/// their error a level down (`{"attempt":{"error":…}}`) — and that nested string
/// is exactly what the UI shortens.
fn extract_raw_error(payload: &Value) -> Option<String> {
    const ERROR_KEYS: [&str; 4] = ["last_error", "error", "failure", "degraded_reason"];

    match payload {
        Value::Object(map) => {
            for key in ERROR_KEYS {
                if let Some(Value::String(text)) = map.get(key) {
                    if !text.trim().is_empty() {
                        return Some(text.clone());
                    }
                }
            }
            map.values().find_map(extract_raw_error)
        }
        Value::Array(items) => items.iter().find_map(extract_raw_error),
        _ => None,
    }
}

fn log_offset() -> FixedOffset {
    FixedOffset::east_opt(LOG_UTC_OFFSET_SECONDS).expect("valid log UTC offset")
}

/// Flatten newlines and control characters so one record is always one line.
fn sanitize_field(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\n' => '⏎',
            '\r' => ' ',
            '\t' => ' ',
            ch if ch.is_control() => ' ',
            ch => ch,
        })
        .collect()
}

/// JSON keys whose values are subscriber-identifying.
const SENSITIVE_KEYS: &[&str] = &[
    "imsi",
    "impi",
    "impu",
    "msisdn",
    "iccid",
    "imei",
    "eid",
    "phone_number",
    "phone",
    "number",
    "caller",
    "callee",
    "from",
    "to",
    "content",
    "text",
    "body",
    "message",
    "sms_content",
    "pcscf",
    "p_cscf",
    "pcscf_address",
    "password",
    "secret",
    "token",
    "key",
];

fn is_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    SENSITIVE_KEYS
        .iter()
        .any(|candidate| key == *candidate || key.ends_with(&format!("_{candidate}")))
}

/// Mask a value, keeping enough of it to correlate records.
///
/// Short values are replaced wholesale; longer ones keep the last two characters
/// so the same subscriber can still be followed across lines.
fn mask_value(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 4 {
        return "***".to_string();
    }
    let tail: String = chars[chars.len() - 2..].iter().collect();
    format!("***{tail}")
}

/// Redact a detail string.
///
/// JSON payloads are walked by key, which is precise. Anything else falls back
/// to masking long digit runs — deliberately blunt, because over-masking a
/// diagnostic string is cheaper than leaking an IMSI to whoever downloads it.
fn redact_text(value: &str) -> String {
    if let Ok(mut parsed) = serde_json::from_str::<Value>(value) {
        redact_json(&mut parsed);
        return parsed.to_string();
    }
    redact_digit_runs(value)
}

fn redact_json(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if is_sensitive_key(key) {
                    let masked = match &*child {
                        Value::String(text) => Value::String(mask_value(text)),
                        Value::Null => Value::Null,
                        other => Value::String(mask_value(&other.to_string())),
                    };
                    *child = masked;
                } else {
                    redact_json(child);
                }
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                redact_json(item);
            }
        }
        Value::String(text) => {
            *text = redact_digit_runs(text);
        }
        _ => {}
    }
}

/// Mask digit runs of 7 or more, the length at which a run stops being a code or
/// a counter and starts being a subscriber number, IMSI, ICCID or IMEI.
fn redact_digit_runs(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut run = String::new();
    for ch in value.chars() {
        if ch.is_ascii_digit() {
            run.push(ch);
            continue;
        }
        flush_digit_run(&mut run, &mut out);
        out.push(ch);
    }
    flush_digit_run(&mut run, &mut out);
    out
}

fn flush_digit_run(run: &mut String, out: &mut String) {
    if run.is_empty() {
        return;
    }
    if run.chars().count() >= 7 {
        out.push_str(&mask_value(run));
    } else {
        out.push_str(run);
    }
    run.clear();
}

/// Cheap clonable handle for producers.
///
/// `record` never blocks and never fails upward: if the writer is behind, the
/// record is counted as dropped and discarded. Losing diagnostics is strictly
/// better than delaying the work being diagnosed.
#[derive(Clone)]
pub struct DiagnosticLogSink {
    sender: mpsc::Sender<DiagnosticRecord>,
    dropped: Arc<AtomicU64>,
}

impl DiagnosticLogSink {
    pub fn record(&self, record: DiagnosticRecord) {
        if self.sender.try_send(record).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// Create the log directory, restricted to its owner.
///
/// The file can hold subscriber identifiers whenever redaction is turned off, so
/// it must not inherit a world-readable mode from the process umask. Permissions
/// are re-asserted on an existing directory too: an install that predates this
/// hardening would otherwise stay at 0755 forever.
fn ensure_log_directory(directory: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o750);
        if let Err(error) = std::fs::set_permissions(directory, permissions) {
            // Not fatal: a directory owned by another user still accepts writes
            // if the mode already allows them, and losing the log entirely would
            // be worse than logging to a laxer directory.
            tracing::warn!(%error, "diagnostic log: could not restrict log directory permissions");
        }
    }
    Ok(())
}

/// Resolve the log directory.
///
/// `/var/log/simadmin` is the Linux convention and the production location. A
/// development host without it (or without permission to create it) falls back
/// beside the executable, so running locally never fails on log setup.
pub fn resolve_log_directory(config: &DiagnosticLogConfig) -> PathBuf {
    if let Some(directory) = config
        .directory
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(directory);
    }

    let system_path = PathBuf::from("/var/log/simadmin");
    if system_path.is_dir() || ensure_log_directory(&system_path).is_ok() {
        return system_path;
    }

    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("logs")
}

fn file_name_for(date: NaiveDate) -> String {
    format!(
        "{FILE_PREFIX}{:04}-{:02}-{:02}{FILE_SUFFIX}",
        date.year(),
        date.month(),
        date.day()
    )
}

/// Parse the date out of a log file name, so retention never has to trust
/// filesystem mtimes (which a restore or a clock jump can rewrite).
fn date_from_file_name(name: &str) -> Option<NaiveDate> {
    let stem = name.strip_prefix(FILE_PREFIX)?.strip_suffix(FILE_SUFFIX)?;
    NaiveDate::parse_from_str(stem, "%Y-%m-%d").ok()
}

/// One rotated log file on disk.
#[derive(Debug, Clone)]
pub struct LogFileInfo {
    pub name: String,
    pub path: PathBuf,
    pub date: NaiveDate,
    pub size_bytes: u64,
}

/// List log files, oldest first.
pub fn list_log_files(directory: &Path) -> Vec<LogFileInfo> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut files: Vec<LogFileInfo> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let date = date_from_file_name(&name)?;
            let size_bytes = entry.metadata().ok().map(|meta| meta.len()).unwrap_or(0);
            Some(LogFileInfo {
                name,
                path: entry.path(),
                date,
                size_bytes,
            })
        })
        .collect();
    files.sort_by(|a, b| a.date.cmp(&b.date));
    files
}

/// Aggregate state for the settings UI.
#[derive(Debug, Clone, Serialize, Default)]
pub struct DiagnosticLogStatus {
    pub directory: String,
    pub file_count: usize,
    pub total_bytes: u64,
    pub earliest_date: Option<String>,
    pub latest_date: Option<String>,
    pub dropped_records: u64,
}

/// Delete files that exceed either retention bound, oldest first.
///
/// Age is applied before size so a device that is simply idle keeps a full
/// window of history, while a burst-heavy device sheds its oldest days.
fn enforce_retention(directory: &Path, config: &DiagnosticLogConfig, today: NaiveDate) {
    let mut files: VecDeque<LogFileInfo> = list_log_files(directory).into();

    let cutoff = today - chrono::Duration::days(i64::from(config.retention_days));
    while let Some(file) = files.front() {
        if file.date >= cutoff {
            break;
        }
        if let Err(error) = std::fs::remove_file(&file.path) {
            tracing::warn!(file = %file.name, %error, "diagnostic log: failed to delete aged file");
            break;
        }
        files.pop_front();
    }

    let max_bytes = config.max_total_bytes();
    let mut total: u64 = files.iter().map(|file| file.size_bytes).sum();
    // The newest file is the one being appended to; never delete it, or the
    // writer's open handle would keep writing to an unlinked inode.
    while total > max_bytes && files.len() > 1 {
        let Some(file) = files.pop_front() else { break };
        if let Err(error) = std::fs::remove_file(&file.path) {
            tracing::warn!(file = %file.name, %error, "diagnostic log: failed to delete oversized file");
            break;
        }
        total = total.saturating_sub(file.size_bytes);
    }
}

/// Start the writer task and return the producer handle.
pub fn spawn_diagnostic_logger(config_manager: Arc<ConfigManager>) -> Arc<DiagnosticLogSink> {
    let (sender, mut receiver) = mpsc::channel::<DiagnosticRecord>(CHANNEL_CAPACITY);
    let dropped = Arc::new(AtomicU64::new(0));
    let sink = Arc::new(DiagnosticLogSink {
        sender,
        dropped: Arc::clone(&dropped),
    });

    tokio::spawn(async move {
        let mut open_date: Option<NaiveDate> = None;
        let mut file: Option<tokio::fs::File> = None;
        let mut bytes_since_cleanup: u64 = 0;

        while let Some(first) = receiver.recv().await {
            let config = config_manager.get_diagnostic_log();
            let mut batch = Vec::with_capacity(BATCH_LIMIT);
            batch.push(first);
            while batch.len() < BATCH_LIMIT {
                match receiver.try_recv() {
                    Ok(record) => batch.push(record),
                    Err(_) => break,
                }
            }

            if !config.enabled {
                // Drop the handle so a disabled log does not pin a file, and
                // clear the date so re-enabling reopens cleanly.
                file = None;
                open_date = None;
                continue;
            }

            let directory = resolve_log_directory(&config);
            let today = Utc::now().with_timezone(&log_offset()).date_naive();

            if open_date != Some(today) {
                if ensure_log_directory(&directory).is_err() {
                    continue;
                }
                match tokio::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(directory.join(file_name_for(today)))
                    .await
                {
                    Ok(handle) => {
                        file = Some(handle);
                        open_date = Some(today);
                        enforce_retention(&directory, &config, today);
                        bytes_since_cleanup = 0;
                    }
                    Err(error) => {
                        tracing::warn!(%error, "diagnostic log: failed to open log file");
                        file = None;
                        open_date = None;
                        continue;
                    }
                }
            }

            let Some(handle) = file.as_mut() else {
                continue;
            };

            let mut buffer = String::new();
            for record in &batch {
                if record.severity < config.min_severity {
                    continue;
                }
                buffer.push_str(&record.render(config.redact_sensitive));
            }
            if buffer.is_empty() {
                continue;
            }

            if let Err(error) = handle.write_all(buffer.as_bytes()).await {
                tracing::warn!(%error, "diagnostic log: write failed");
                file = None;
                open_date = None;
                continue;
            }
            let _ = handle.flush().await;

            bytes_since_cleanup += buffer.len() as u64;
            if bytes_since_cleanup >= CLEANUP_BYTE_INTERVAL {
                enforce_retention(&directory, &config, today);
                bytes_since_cleanup = 0;
            }
        }
    });

    sink
}

/// Read the on-disk state for the settings UI.
pub fn read_status(config: &DiagnosticLogConfig, dropped_records: u64) -> DiagnosticLogStatus {
    let directory = resolve_log_directory(config);
    let files = list_log_files(&directory);
    DiagnosticLogStatus {
        directory: directory.to_string_lossy().to_string(),
        file_count: files.len(),
        total_bytes: files.iter().map(|file| file.size_bytes).sum(),
        earliest_date: files.first().map(|file| file.date.to_string()),
        latest_date: files.last().map(|file| file.date.to_string()),
        dropped_records,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_sensitive_json_keys_and_leaves_diagnostics_readable() {
        let payload = r#"{"imsi":"460010123456789","stage":"register","sip_status":403}"#;
        let redacted = redact_text(payload);
        assert!(!redacted.contains("460010123456789"));
        assert!(
            redacted.contains("register"),
            "stage must survive: {redacted}"
        );
        assert!(
            redacted.contains("403"),
            "SIP status must survive: {redacted}"
        );
    }

    #[test]
    fn keeps_short_numbers_but_masks_subscriber_length_runs() {
        // Exit codes and SIP statuses are what make a raw error useful; only
        // runs long enough to identify a subscriber are masked.
        let masked = redact_digit_runs("volte_command_failed:mmcli:1:403:460010123456789");
        assert!(masked.contains(":1:"));
        assert!(masked.contains("403"));
        assert!(!masked.contains("460010123456789"));
    }

    #[test]
    fn record_renders_one_line_with_context_and_raw_error() {
        let record = DiagnosticRecord::new(
            DiagnosticLogSeverity::Error,
            "VoLTE",
            "bearer.connect.failed",
        )
        .with_line("79139C")
        .with_raw_error("volte_command_failed:mmcli:1\nerror: couldn't find modem");
        let rendered = record.render(false);
        assert_eq!(
            rendered.matches('\n').count(),
            1,
            "must be exactly one line"
        );
        assert!(rendered.contains("[ERROR]"));
        assert!(rendered.contains("[main]"));
        assert!(rendered.contains("line=79139C"));
        assert!(
            rendered.contains("couldn't find modem"),
            "raw error must survive verbatim"
        );
    }

    #[test]
    fn file_name_round_trips_through_date_parser() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 27).unwrap();
        assert_eq!(date_from_file_name(&file_name_for(date)), Some(date));
        assert_eq!(date_from_file_name("unrelated.log"), None);
    }

    #[tokio::test]
    async fn ue_worker_context_is_scoped_to_the_task_body() {
        assert_eq!(ExecutionContext::current(), ExecutionContext::Main);
        with_ue_worker_context(async {
            assert_eq!(ExecutionContext::current(), ExecutionContext::UeWorker);
        })
        .await;
        assert_eq!(ExecutionContext::current(), ExecutionContext::Main);
    }

    #[tokio::test]
    async fn app_event_records_capture_the_scope_they_were_built_in() {
        // Regression guard: building records in a bus subscriber instead of on
        // the producing task collapsed every line to `main`, which made the
        // scope column useless on a multi-SIM device.
        let payload = serde_json::json!({ "attempt": { "error": "volte_bearer_failed" } });

        let device_wide =
            record_for_app_event("system.service_started", None, Some("system"), &payload);
        assert_eq!(device_wide.context, ExecutionContext::Main);

        let per_line = with_ue_worker_context(async {
            record_for_app_event(
                "volte.connection_attempt",
                Some("79139C"),
                Some("volte_ims"),
                &payload,
            )
        })
        .await;
        assert_eq!(per_line.context, ExecutionContext::UeWorker);
        assert_eq!(per_line.subsystem, "VoLTE");
        // An error present anywhere in the payload lifts the record above Info.
        assert_eq!(per_line.severity, DiagnosticLogSeverity::Warn);
        assert_eq!(per_line.raw_error.as_deref(), Some("volte_bearer_failed"));
        assert!(per_line.render(true).contains("[ue_worker]"));
    }
}
