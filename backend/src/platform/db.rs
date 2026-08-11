//! 数据库模块
//!
//! 使用 SQLite 存储短信历史记录和通话记录

// The legacy unit-test module predates later database domains. Moving the large
// block creates a high-conflict diff without changing runtime organization.
#![allow(clippy::items_after_test_module)]

use chrono::{DateTime, Duration, FixedOffset, NaiveDateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Result, Row};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const BEIJING_UTC_OFFSET_SECONDS: i32 = 8 * 60 * 60;
const SMS_TIMESTAMP_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

fn required_line_id(line_id: &str) -> Result<&str> {
    let line_id = line_id.trim();
    if line_id.is_empty() {
        return Err(rusqlite::Error::InvalidParameterName(
            "line_id must not be empty".to_string(),
        ));
    }
    Ok(line_id)
}

fn required_sms_channel_id(channel_id: &str) -> Result<&str> {
    let channel_id = channel_id.trim();
    if channel_id.is_empty() {
        return Err(rusqlite::Error::InvalidParameterName(
            "channel_id must not be empty".to_string(),
        ));
    }
    Ok(channel_id)
}

/// 短信记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmsMessage {
    pub id: i64,
    pub direction: String,    // "incoming" 或 "outgoing"
    pub phone_number: String, // 发件人或收件人
    pub content: String,      // 短信内容
    pub timestamp: String,    // ISO 8601 格式时间
    pub status: String,       // "pending", "sent", "failed", "received"
    pub pdu: Option<String>,  // 原始 PDU（如果有）
    #[serde(default = "default_sms_transport")]
    pub transport: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_id: Option<String>,
}

fn default_sms_transport() -> String {
    "modem".to_string()
}

/// 通话记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRecord {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_id: Option<String>,
    pub direction: String,        // "incoming" / "outgoing" / "missed"
    pub phone_number: String,     // 电话号码
    pub duration: i64,            // 通话时长（秒）
    pub start_time: String,       // 开始时间 ISO 8601
    pub end_time: Option<String>, // 结束时间 ISO 8601
    pub answered: bool,           // 是否接通
}

/// 短信统计
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SmsStats {
    pub total: i64,
    pub incoming: i64,
    pub outgoing: i64,
    #[serde(default)]
    pub pushed: i64,
    #[serde(default)]
    pub push_attempted: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationLogEntry {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_id: Option<String>,
    pub event_type: String,
    pub status: String,
    pub summary: String,
    pub rule_id: String,
    pub rule_name: String,
    pub channel_id: String,
    pub channel_name: String,
    pub message: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotificationLogsResponse {
    pub logs: Vec<NotificationLogEntry>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationLogEntry {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_id: Option<String>,
    pub task_id: String,
    pub task_name: String,
    pub task_type: String,
    pub status: String,
    pub detail: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AutomationLogsResponse {
    pub logs: Vec<AutomationLogEntry>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotificationStatusCounts {
    pub success: i64,
    pub failed: i64,
    pub quiet_hours: i64,
    pub unmatched: i64,
    pub no_available_channel: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PeriodSmsStats {
    pub incoming: i64,
    pub forwarding: NotificationStatusCounts,
}

pub struct NewNotificationLog<'a> {
    pub line_id: Option<&'a str>,
    pub event_type: &'a str,
    pub status: &'a str,
    pub summary: &'a str,
    pub rule_id: &'a str,
    pub rule_name: &'a str,
    pub channel_id: &'a str,
    pub channel_name: &'a str,
    pub message: &'a str,
}

pub struct NewNotificationQueueItem<'a> {
    pub line_id: Option<&'a str>,
    pub status: &'a str,
    pub event_type: &'a str,
    pub event_label: &'a str,
    pub summary: &'a str,
    pub reason: &'a str,
    pub rule_id: &'a str,
    pub rule_name: &'a str,
    pub channel_id: &'a str,
    pub channel_name: &'a str,
    pub channel_type: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub next_attempt_at: &'a str,
    pub max_attempts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationQueueEntry {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_id: Option<String>,
    pub status: String,
    pub event_type: String,
    pub event_label: String,
    pub summary: String,
    pub reason: String,
    pub channel_id: String,
    pub channel_name: String,
    pub channel_type: String,
    pub rule_id: String,
    pub rule_name: String,
    pub title: String,
    pub body: String,
    pub next_attempt_at: String,
    pub attempt_count: i64,
    pub max_attempts: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotificationQueueResponse {
    pub items: Vec<NotificationQueueEntry>,
    pub total: i64,
}

/// 通话统计
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct CallStats {
    pub total: i64,
    pub incoming: i64,
    pub outgoing: i64,
    pub missed: i64,
    pub total_duration: i64, // 总通话时长（秒）
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmscCacheEntry {
    pub sms_center: String,
    pub source: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnNumberCacheEntry {
    pub phone_numbers: Vec<String>,
    pub source: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EsimProfileCacheEntry {
    pub iccid: String,
    pub name: Option<String>,
    pub provider: Option<String>,
    pub profile_class: Option<String>,
    pub imsi: Option<String>,
    pub msisdn: Option<String>,
    pub smsc: Option<String>,
    pub smdp: Option<String>,
    pub matching_id: Option<String>,
    pub isdp_aid: Option<String>,
    pub mcc: Option<String>,
    pub mnc: Option<String>,
    pub updated_at: String,
}

/// 数据库管理器
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VowifiRuntimeEventEntry {
    pub id: i64,
    pub line_id: Option<String>,
    pub trace_id: Option<String>,
    pub level: String,
    pub phase: String,
    pub profile_id: Option<String>,
    pub event_type: String,
    pub detail_json: String,
    pub created_at: String,
}

pub struct NewVowifiRuntimeEvent<'a> {
    pub line_id: &'a str,
    pub trace_id: Option<&'a str>,
    pub level: &'a str,
    pub phase: &'a str,
    pub profile_id: Option<&'a str>,
    pub event_type: &'a str,
    pub detail_json: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VowifiRuntimeEventsResponse {
    pub events: Vec<VowifiRuntimeEventEntry>,
    pub total: i64,
}

/// Cumulative proxied traffic for one line as stored on disk.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineDataTrafficEntry {
    pub uplink_bytes: u64,
    pub downlink_bytes: u64,
    pub total_connections: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VowifiRuntimeSnapshotEntry {
    pub line_id: Option<String>,
    pub phase: String,
    pub profile_id: Option<String>,
    pub plmn: Option<String>,
    pub identity_ready: bool,
    pub sim_auth_ready: bool,
    pub profile_matched: bool,
    pub epdg_ready: bool,
    pub ike_ready: bool,
    pub child_sa_ready: bool,
    pub esp_ready: bool,
    pub ims_registered: bool,
    pub sms_ready: bool,
    pub degraded_reason: Option<String>,
    pub updated_at: String,
}

pub struct NewVowifiRuntimeSnapshot<'a> {
    pub line_id: &'a str,
    pub phase: &'a str,
    pub profile_id: Option<&'a str>,
    pub plmn: Option<&'a str>,
    pub identity_ready: bool,
    pub sim_auth_ready: bool,
    pub profile_matched: bool,
    pub epdg_ready: bool,
    pub ike_ready: bool,
    pub child_sa_ready: bool,
    pub esp_ready: bool,
    pub ims_registered: bool,
    pub sms_ready: bool,
    pub degraded_reason: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VowifiSmsPartEntry {
    pub message_id: String,
    pub reference: i64,
    pub sequence: i64,
    pub total: i64,
    pub received: bool,
    pub updated_at: String,
}

pub struct NewVowifiSmsPart<'a> {
    pub line_id: &'a str,
    pub message_id: &'a str,
    pub reference: i64,
    pub sequence: i64,
    pub total: i64,
    pub received: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VowifiSmsDeliveryEntry {
    pub message_id: String,
    pub line_id: Option<String>,
    pub trace_id: String,
    pub direction: String,
    pub state: String,
    pub sip_state: String,
    pub rpdu_ack: String,
    pub delivery_reported: bool,
    pub failure_cause: Option<String>,
    pub retry_count: i64,
    pub api_sms_id: Option<i64>,
    pub parts: Vec<VowifiSmsPartEntry>,
    pub created_at: String,
    pub updated_at: String,
}

pub struct NewVowifiSmsDelivery<'a> {
    pub message_id: &'a str,
    pub line_id: &'a str,
    pub trace_id: &'a str,
    pub direction: &'a str,
    pub state: &'a str,
    pub sip_state: &'a str,
    pub rpdu_ack: &'a str,
    pub delivery_reported: bool,
    pub failure_cause: Option<&'a str>,
    pub retry_count: i64,
    pub api_sms_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VowifiSmsDeliveriesResponse {
    pub deliveries: Vec<VowifiSmsDeliveryEntry>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VowifiEsimRestoreEntry {
    pub line_id: Option<String>,
    pub switch_token: Option<String>,
    pub switch_phase: Option<String>,
    pub phase_ms: Option<i64>,
    pub identity_ready: bool,
    pub sim_auth_ready: bool,
    pub degraded_reason: Option<String>,
    pub retry_count: i64,
    pub updated_at: String,
}

pub struct NewVowifiEsimRestore<'a> {
    pub line_id: &'a str,
    pub switch_token: Option<&'a str>,
    pub switch_phase: Option<&'a str>,
    pub phase_ms: Option<i64>,
    pub identity_ready: bool,
    pub sim_auth_ready: bool,
    pub degraded_reason: Option<&'a str>,
    pub retry_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VowifiSoakSampleEntry {
    pub id: i64,
    pub run_id: String,
    pub sample_kind: String,
    pub metric_name: String,
    pub metric_value: i64,
    pub state: String,
    pub created_at: String,
}

pub struct NewVowifiSoakSample<'a> {
    pub line_id: &'a str,
    pub run_id: &'a str,
    pub sample_kind: &'a str,
    pub metric_name: &'a str,
    pub metric_value: i64,
    pub state: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VowifiSoakRunEntry {
    pub run_id: String,
    pub line_id: Option<String>,
    pub scenario_id: String,
    pub profile_id: Option<String>,
    pub plmn: Option<String>,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub duration_seconds: i64,
    pub sample_count: i64,
    pub failure_count: i64,
    pub last_error: Option<String>,
    pub sensitive_values_policy: String,
    pub samples: Vec<VowifiSoakSampleEntry>,
}

pub struct NewVowifiSoakRun<'a> {
    pub run_id: &'a str,
    pub line_id: &'a str,
    pub scenario_id: &'a str,
    pub profile_id: Option<&'a str>,
    pub plmn: Option<&'a str>,
    pub status: &'a str,
    pub duration_seconds: i64,
    pub sample_count: i64,
    pub failure_count: i64,
    pub last_error: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VowifiSoakRunsResponse {
    pub runs: Vec<VowifiSoakRunEntry>,
    pub total: i64,
    pub read_only: bool,
}

pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

fn beijing_offset() -> FixedOffset {
    FixedOffset::east_opt(BEIJING_UTC_OFFSET_SECONDS).expect("valid Beijing UTC offset")
}

pub fn beijing_sms_now_string() -> String {
    Utc::now()
        .with_timezone(&beijing_offset())
        .format(SMS_TIMESTAMP_FORMAT)
        .to_string()
}

pub fn normalize_sms_timestamp_for_display(timestamp: &str) -> Option<String> {
    let timestamp = timestamp.trim();
    if timestamp.is_empty() {
        return None;
    }

    if let Some(parsed) = parse_sms_timestamp_with_offset(timestamp) {
        return Some(parsed);
    }

    for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(timestamp, format) {
            return Some(parsed.format(SMS_TIMESTAMP_FORMAT).to_string());
        }
    }

    None
}

fn parse_sms_timestamp_with_offset(timestamp: &str) -> Option<String> {
    let timestamp = timestamp.replace(' ', "T");

    if let Ok(parsed) = DateTime::parse_from_rfc3339(&timestamp) {
        return Some(
            parsed
                .with_timezone(&beijing_offset())
                .format(SMS_TIMESTAMP_FORMAT)
                .to_string(),
        );
    }

    let offset_start = timestamp
        .char_indices()
        .rev()
        .find_map(|(index, ch)| (index > 10 && matches!(ch, '+' | '-')).then_some(index))?;

    let (datetime, offset) = timestamp.split_at(offset_start);
    let normalized_offset = match offset.len() {
        3 => format!("{offset}:00"),
        5 if !offset.contains(':') => format!("{}:{}", &offset[..3], &offset[3..]),
        _ => offset.to_string(),
    };
    let candidate = format!("{datetime}{normalized_offset}");

    DateTime::parse_from_rfc3339(&candidate).ok().map(|parsed| {
        parsed
            .with_timezone(&beijing_offset())
            .format(SMS_TIMESTAMP_FORMAT)
            .to_string()
    })
}

fn sms_timestamp_for_storage(timestamp: &str) -> String {
    normalize_sms_timestamp_for_display(timestamp).unwrap_or_else(beijing_sms_now_string)
}

fn sms_timestamp_for_display(timestamp: String) -> String {
    normalize_sms_timestamp_for_display(&timestamp).unwrap_or(timestamp)
}

fn notification_log_date_bound(value: &str, suffix: &str) -> String {
    let value = value.trim().replace('/', "-");
    if value.is_empty() {
        String::new()
    } else if value.len() <= 10 {
        format!("{value} {suffix}")
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_database() -> Database {
        Database::new(PathBuf::from(":memory:")).expect("create test database")
    }

    #[test]
    fn call_history_and_stats_are_isolated_by_line() {
        let db = test_database();
        let first = db
            .insert_call(Some("line-a"), "outgoing", "+601112023012", true)
            .expect("insert line-a call");
        db.update_call_end(first, 12, true)
            .expect("finish line-a call");
        db.insert_call(Some("line-b"), "missed", "+60222222222", false)
            .expect("insert line-b call");
        db.insert_call(None, "incoming", "+60333333333", false)
            .expect("insert legacy call");

        assert_eq!(db.get_call_history(10, 0).unwrap().len(), 3);
        let line_a = db.get_call_history_for_line("line-a", 10, 0).unwrap();
        assert_eq!(line_a.len(), 1);
        assert_eq!(line_a[0].line_id.as_deref(), Some("line-a"));
        assert_eq!(line_a[0].phone_number, "+601112023012");

        let line_a_stats = db.get_call_stats_for_line("line-a").unwrap();
        assert_eq!(line_a_stats.total, 1);
        assert_eq!(line_a_stats.outgoing, 1);
        assert_eq!(line_a_stats.total_duration, 12);

        assert_eq!(db.clear_calls_for_line("line-a").unwrap(), 1);
        assert!(db
            .get_call_history_for_line("line-a", 10, 0)
            .unwrap()
            .is_empty());
        assert_eq!(db.get_call_stats().unwrap().total, 2);
    }

    #[test]
    fn automation_logs_persist_and_filter_target_line() {
        let db = test_database();
        db.insert_automation_log(
            Some("line-a"),
            "task-a",
            "Line A task",
            "send_sms",
            "success",
            "done-a",
        )
        .unwrap();
        db.insert_automation_log(
            Some("line-b"),
            "task-b",
            "Line B task",
            "send_sms",
            "failed",
            "done-b",
        )
        .unwrap();
        db.insert_automation_log(
            None,
            "task-device",
            "Device task",
            "reboot_device",
            "success",
            "done-device",
        )
        .unwrap();

        let line_a = db
            .get_automation_logs("", "", "line-a", "", "", "", 10, 0)
            .unwrap();
        assert_eq!(line_a.total, 1);
        assert_eq!(line_a.logs[0].line_id.as_deref(), Some("line-a"));
        let device = db
            .get_automation_logs("reboot_device", "", "", "", "", "", 10, 0)
            .unwrap();
        assert_eq!(device.total, 1);
        assert!(device.logs[0].line_id.is_none());
        assert_eq!(
            db.get_last_log_for_task("task-b")
                .unwrap()
                .unwrap()
                .line_id
                .as_deref(),
            Some("line-b")
        );

        assert_eq!(
            db.clear_automation_logs("", "", "line-a", "", "").unwrap(),
            1
        );
        let remaining = db
            .get_automation_logs("", "", "", "", "", "", 10, 0)
            .unwrap();
        assert_eq!(remaining.total, 2);
        assert!(remaining
            .logs
            .iter()
            .all(|log| log.line_id.as_deref() != Some("line-a")));
    }

    #[test]
    fn notification_history_and_queue_preserve_line_scope() {
        let db = test_database();
        for (line_id, summary) in [
            (Some("line-a"), "line A SMS"),
            (Some("line-b"), "line B automation"),
            (None, "device event"),
        ] {
            db.insert_notification_log(NewNotificationLog {
                line_id,
                event_type: "sms",
                status: "success",
                summary,
                rule_id: "rule",
                rule_name: "Rule",
                channel_id: "channel",
                channel_name: "Channel",
                message: "sent",
            })
            .unwrap();
        }

        let line_a = db
            .get_notification_logs("", "", "line-a", "", "", "", 10, 0)
            .unwrap();
        assert_eq!(line_a.total, 1);
        assert_eq!(line_a.logs[0].line_id.as_deref(), Some("line-a"));
        assert_eq!(
            db.clear_notification_logs("", "", "line-a", "", "")
                .unwrap(),
            1
        );
        assert_eq!(
            db.get_notification_logs("", "", "", "", "", "", 10, 0)
                .unwrap()
                .total,
            2
        );

        for line_id in [Some("line-a"), Some("line-b"), None] {
            db.insert_notification_queue_item(NewNotificationQueueItem {
                line_id,
                status: "pending",
                event_type: "sms",
                event_label: "SMS",
                summary: "queued",
                reason: "rate limit",
                rule_id: "rule",
                rule_name: "Rule",
                channel_id: "channel",
                channel_name: "Channel",
                channel_type: "webhook",
                title: "Title",
                body: "Body",
                next_attempt_at: "2099-01-01 00:00:00",
                max_attempts: 5,
            })
            .unwrap();
        }

        let line_b = db.get_notification_queue("line-b", 10).unwrap();
        assert_eq!(line_b.total, 1);
        assert_eq!(line_b.items[0].line_id.as_deref(), Some("line-b"));
        assert_eq!(db.get_notification_queue("device", 10).unwrap().total, 1);
        assert_eq!(db.get_notification_queue("", 10).unwrap().total, 3);
    }

    #[test]
    fn vowifi_runtime_snapshot_round_trips_without_sensitive_identity() {
        let db = test_database();

        assert!(db
            .get_vowifi_runtime_snapshot_for_line("line-test")
            .expect("read empty snapshot")
            .is_none());

        db.upsert_vowifi_runtime_snapshot(NewVowifiRuntimeSnapshot {
            line_id: "line-test",
            phase: "profile_matched",
            profile_id: Some("gb_ee_23433"),
            plmn: Some("23433"),
            identity_ready: true,
            sim_auth_ready: false,
            profile_matched: true,
            epdg_ready: false,
            ike_ready: false,
            child_sa_ready: false,
            esp_ready: false,
            ims_registered: false,
            sms_ready: false,
            degraded_reason: None,
        })
        .expect("write snapshot");

        let snapshot = db
            .get_vowifi_runtime_snapshot_for_line("line-test")
            .expect("read snapshot")
            .expect("snapshot exists");
        assert_eq!(snapshot.phase, "profile_matched");
        assert_eq!(snapshot.profile_id.as_deref(), Some("gb_ee_23433"));
        assert_eq!(snapshot.plmn.as_deref(), Some("23433"));
        assert!(snapshot.identity_ready);
        assert!(snapshot.profile_matched);
        assert!(!snapshot.sms_ready);
    }

    #[test]
    fn new_vowifi_diagnostic_writes_reject_empty_line_ids() {
        let db = test_database();

        assert!(db
            .insert_vowifi_runtime_event(NewVowifiRuntimeEvent {
                line_id: " ",
                trace_id: None,
                level: "info",
                phase: "starting",
                profile_id: None,
                event_type: "test",
                detail_json: "{}",
            })
            .is_err());
        assert!(db
            .upsert_vowifi_runtime_snapshot(NewVowifiRuntimeSnapshot {
                line_id: "",
                phase: "starting",
                profile_id: None,
                plmn: None,
                identity_ready: false,
                sim_auth_ready: false,
                profile_matched: false,
                epdg_ready: false,
                ike_ready: false,
                child_sa_ready: false,
                esp_ready: false,
                ims_registered: false,
                sms_ready: false,
                degraded_reason: None,
            })
            .is_err());
        assert!(db
            .upsert_vowifi_sms_delivery(NewVowifiSmsDelivery {
                message_id: "msg-empty-line",
                line_id: "",
                trace_id: "trace-empty-line",
                direction: "mo",
                state: "accepted",
                sip_state: "accepted",
                rpdu_ack: "acked",
                delivery_reported: false,
                failure_cause: None,
                retry_count: 0,
                api_sms_id: None,
            })
            .is_err());
        assert!(db
            .upsert_vowifi_sms_part(NewVowifiSmsPart {
                line_id: " ",
                message_id: "msg-empty-line",
                reference: 1,
                sequence: 1,
                total: 1,
                received: true,
            })
            .is_err());
        assert!(db
            .upsert_vowifi_esim_restore(NewVowifiEsimRestore {
                line_id: "",
                switch_token: None,
                switch_phase: None,
                phase_ms: None,
                identity_ready: false,
                sim_auth_ready: false,
                degraded_reason: None,
                retry_count: 0,
            })
            .is_err());
        assert!(db
            .insert_vowifi_soak_sample(NewVowifiSoakSample {
                line_id: "",
                run_id: "run-empty-line",
                sample_kind: "counter",
                metric_name: "attempts",
                metric_value: 1,
                state: "ok",
            })
            .is_err());
        assert!(db
            .upsert_vowifi_soak_run(NewVowifiSoakRun {
                run_id: "run-empty-line",
                line_id: "",
                scenario_id: "test",
                profile_id: None,
                plmn: None,
                status: "running",
                duration_seconds: 0,
                sample_count: 0,
                failure_count: 0,
                last_error: None,
            })
            .is_err());
    }

    #[test]
    fn vowifi_diagnostics_storage_is_isolated_by_line() {
        let db = test_database();

        for (line_id, phase, trace_id, message_id, run_id, retry_count) in [
            ("line-a", "identity_ready", "trace-a", "msg-a", "run-a", 1),
            ("line-b", "sms_ready", "trace-b", "msg-b", "run-b", 2),
        ] {
            db.upsert_vowifi_runtime_snapshot(NewVowifiRuntimeSnapshot {
                line_id,
                phase,
                profile_id: Some("profile-test"),
                plmn: Some("00101"),
                identity_ready: true,
                sim_auth_ready: phase == "sms_ready",
                profile_matched: true,
                epdg_ready: phase == "sms_ready",
                ike_ready: phase == "sms_ready",
                child_sa_ready: phase == "sms_ready",
                esp_ready: phase == "sms_ready",
                ims_registered: phase == "sms_ready",
                sms_ready: phase == "sms_ready",
                degraded_reason: None,
            })
            .expect("write line snapshot");
            db.insert_vowifi_runtime_event(NewVowifiRuntimeEvent {
                line_id,
                trace_id: Some(trace_id),
                level: "info",
                phase,
                profile_id: Some("profile-test"),
                event_type: "line_event",
                detail_json: r#"{"ready":true}"#,
            })
            .expect("write line event");
            db.upsert_vowifi_sms_delivery(NewVowifiSmsDelivery {
                message_id,
                line_id,
                trace_id,
                direction: "mo",
                state: "accepted",
                sip_state: "accepted",
                rpdu_ack: "acked",
                delivery_reported: false,
                failure_cause: None,
                retry_count: 0,
                api_sms_id: None,
            })
            .expect("write line delivery");
            db.upsert_vowifi_esim_restore(NewVowifiEsimRestore {
                line_id,
                switch_token: None,
                switch_phase: Some(phase),
                phase_ms: Some(100),
                identity_ready: true,
                sim_auth_ready: phase == "sms_ready",
                degraded_reason: None,
                retry_count,
            })
            .expect("write line restore");
            db.upsert_vowifi_soak_run(NewVowifiSoakRun {
                run_id,
                line_id,
                scenario_id: "line-isolation",
                profile_id: Some("profile-test"),
                plmn: Some("00101"),
                status: "running",
                duration_seconds: 1,
                sample_count: 0,
                failure_count: 0,
                last_error: None,
            })
            .expect("write line soak run");
        }

        for (line_id, phase, trace_id, message_id, run_id, retry_count) in [
            ("line-a", "identity_ready", "trace-a", "msg-a", "run-a", 1),
            ("line-b", "sms_ready", "trace-b", "msg-b", "run-b", 2),
        ] {
            let snapshot = db
                .get_vowifi_runtime_snapshot_for_line(line_id)
                .expect("read line snapshot")
                .expect("line snapshot exists");
            assert_eq!(snapshot.line_id.as_deref(), Some(line_id));
            assert_eq!(snapshot.phase, phase);

            let events = db
                .get_vowifi_runtime_events_for_line(line_id, 10, 0, None)
                .expect("read line events");
            assert_eq!(events.total, 1);
            assert_eq!(events.events[0].line_id.as_deref(), Some(line_id));
            assert_eq!(events.events[0].trace_id.as_deref(), Some(trace_id));

            let deliveries = db
                .get_vowifi_sms_deliveries_for_line(line_id, 10, 0)
                .expect("read line deliveries");
            assert_eq!(deliveries.total, 1);
            assert_eq!(deliveries.deliveries[0].message_id, message_id);
            assert_eq!(deliveries.deliveries[0].line_id.as_deref(), Some(line_id));

            let restore = db
                .get_vowifi_esim_restore_for_line(line_id)
                .expect("read line restore")
                .expect("line restore exists");
            assert_eq!(restore.line_id.as_deref(), Some(line_id));
            assert_eq!(restore.retry_count, retry_count);

            let runs = db
                .get_vowifi_soak_runs_for_line(line_id, 10, 0)
                .expect("read line soak runs");
            assert_eq!(runs.total, 1);
            assert_eq!(runs.runs[0].run_id, run_id);
            assert_eq!(runs.runs[0].line_id.as_deref(), Some(line_id));
        }
    }

    #[test]
    fn legacy_vowifi_singletons_migrate_to_unscoped_rows() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "simadmin-vowifi-migration-{}-{nonce}.db",
            std::process::id()
        ));
        {
            let conn = Connection::open(&path).expect("create legacy database");
            conn.execute_batch(
                "CREATE TABLE vowifi_runtime_snapshots (
                    singleton_key INTEGER PRIMARY KEY CHECK(singleton_key = 1),
                    phase TEXT NOT NULL,
                    profile_id TEXT,
                    plmn TEXT,
                    identity_ready INTEGER NOT NULL,
                    sim_auth_ready INTEGER NOT NULL,
                    profile_matched INTEGER NOT NULL,
                    epdg_ready INTEGER NOT NULL,
                    ike_ready INTEGER NOT NULL,
                    child_sa_ready INTEGER NOT NULL,
                    esp_ready INTEGER NOT NULL,
                    ims_registered INTEGER NOT NULL,
                    sms_ready INTEGER NOT NULL,
                    degraded_reason TEXT,
                    updated_at TEXT NOT NULL
                 );
                 INSERT INTO vowifi_runtime_snapshots VALUES (
                    1, 'legacy_phase', 'legacy_profile', '00101',
                    1, 0, 1, 0, 0, 0, 0, 0, 0, 'legacy_reason',
                    '2026-08-04 22:00:00'
                 );
                 CREATE TABLE vowifi_esim_restore (
                    singleton_key INTEGER PRIMARY KEY CHECK(singleton_key = 1),
                    switch_token TEXT,
                    switch_phase TEXT,
                    phase_ms INTEGER,
                    identity_ready INTEGER NOT NULL,
                    sim_auth_ready INTEGER NOT NULL,
                    degraded_reason TEXT,
                    retry_count INTEGER NOT NULL DEFAULT 0,
                    updated_at TEXT NOT NULL
                 );
                 INSERT INTO vowifi_esim_restore VALUES (
                    1, 'legacy_token', 'legacy_restore', 250, 1, 0,
                    'legacy_reason', 3, '2026-08-04 22:00:00'
                 );",
            )
            .expect("seed legacy tables");
        }

        let db = Database::new(path.clone()).expect("migrate legacy database");
        let snapshot = db
            .get_vowifi_runtime_snapshot_scoped(None)
            .expect("read migrated snapshot")
            .expect("migrated snapshot exists");
        assert_eq!(snapshot.line_id, None);
        assert_eq!(snapshot.phase, "legacy_phase");
        let restore = db
            .get_vowifi_esim_restore_scoped(None)
            .expect("read migrated restore")
            .expect("migrated restore exists");
        assert_eq!(restore.line_id, None);
        assert_eq!(restore.switch_phase.as_deref(), Some("legacy_restore"));

        {
            let conn = db.conn.lock().unwrap();
            assert!(table_has_column(&conn, "vowifi_runtime_snapshots", "line_id").unwrap());
            assert!(!table_has_column(&conn, "vowifi_runtime_snapshots", "singleton_key").unwrap());
            assert!(table_has_column(&conn, "vowifi_esim_restore", "line_id").unwrap());
            assert!(!table_has_column(&conn, "vowifi_esim_restore", "singleton_key").unwrap());
        }
        drop(db);
        std::fs::remove_file(path).expect("remove migration database");
    }

    #[test]
    fn legacy_vowifi_child_tables_migrate_to_line_scoped_keys() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "simadmin-vowifi-line-key-migration-{}-{nonce}.db",
            std::process::id()
        ));
        {
            let conn = Connection::open(&path).expect("create legacy database");
            conn.execute_batch(
                "CREATE TABLE vowifi_sms_delivery (
                    message_id TEXT PRIMARY KEY,
                    line_id TEXT,
                    trace_id TEXT NOT NULL,
                    direction TEXT NOT NULL,
                    state TEXT NOT NULL,
                    sip_state TEXT NOT NULL,
                    rpdu_ack TEXT NOT NULL,
                    delivery_reported INTEGER NOT NULL,
                    failure_cause TEXT,
                    retry_count INTEGER NOT NULL DEFAULT 0,
                    api_sms_id INTEGER,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                 );
                 INSERT INTO vowifi_sms_delivery VALUES (
                    'shared-message', 'line-a', 'legacy-trace', 'mo', 'accepted',
                    'accepted', 'acked', 1, NULL, 0, NULL,
                    '2026-08-04 22:00:00', '2026-08-04 22:00:00'
                 );
                 CREATE TABLE vowifi_sms_parts (
                    message_id TEXT NOT NULL,
                    reference INTEGER NOT NULL,
                    sequence INTEGER NOT NULL,
                    total INTEGER NOT NULL,
                    received INTEGER NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY(message_id, reference, sequence)
                 );
                 INSERT INTO vowifi_sms_parts VALUES (
                    'shared-message', 7, 1, 1, 1, '2026-08-04 22:00:00'
                 );
                 CREATE TABLE vowifi_soak_runs (
                    run_id TEXT PRIMARY KEY,
                    line_id TEXT,
                    scenario_id TEXT NOT NULL,
                    profile_id TEXT,
                    plmn TEXT,
                    status TEXT NOT NULL,
                    started_at TEXT NOT NULL,
                    finished_at TEXT,
                    duration_seconds INTEGER NOT NULL DEFAULT 0,
                    sample_count INTEGER NOT NULL DEFAULT 0,
                    failure_count INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT
                 );
                 INSERT INTO vowifi_soak_runs VALUES (
                    'shared-run', 'line-a', 'legacy-scenario', NULL, NULL, 'running',
                    '2026-08-04 22:00:00', NULL, 1, 1, 0, NULL
                 );
                 CREATE TABLE vowifi_soak_samples (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    run_id TEXT NOT NULL,
                    sample_kind TEXT NOT NULL,
                    metric_name TEXT NOT NULL,
                    metric_value INTEGER NOT NULL,
                    state TEXT NOT NULL,
                    created_at TEXT NOT NULL
                 );
                 INSERT INTO vowifi_soak_samples (
                    run_id, sample_kind, metric_name, metric_value, state, created_at
                 ) VALUES (
                    'shared-run', 'counter', 'legacy-attempts', 3, 'ok',
                    '2026-08-04 22:00:00'
                 );",
            )
            .expect("seed legacy line-key tables");
        }

        let db = Database::new(path.clone()).expect("migrate legacy line-key tables");
        let delivery = db
            .get_vowifi_sms_delivery_for_line("line-a", "shared-message")
            .expect("read migrated delivery")
            .expect("migrated delivery exists");
        assert_eq!(delivery.trace_id, "legacy-trace");
        assert_eq!(delivery.parts.len(), 1);
        assert_eq!(delivery.parts[0].reference, 7);

        let runs = db
            .get_vowifi_soak_runs_for_line("line-a", 10, 0)
            .expect("read migrated soak run");
        assert_eq!(runs.total, 1);
        assert_eq!(runs.runs[0].scenario_id, "legacy-scenario");
        assert_eq!(runs.runs[0].samples.len(), 1);
        assert_eq!(runs.runs[0].samples[0].metric_name, "legacy-attempts");

        {
            let conn = db.conn.lock().unwrap();
            assert!(
                table_primary_key_is(&conn, "vowifi_sms_delivery", &["line_id", "message_id"])
                    .unwrap()
            );
            assert!(table_primary_key_is(
                &conn,
                "vowifi_sms_parts",
                &["line_id", "message_id", "reference", "sequence"]
            )
            .unwrap());
            assert!(
                table_primary_key_is(&conn, "vowifi_soak_runs", &["line_id", "run_id"]).unwrap()
            );
            assert!(table_has_column(&conn, "vowifi_soak_samples", "line_id").unwrap());
        }

        drop(db);
        std::fs::remove_file(path).expect("remove migration database");
    }

    #[test]
    fn sms_dedup_claim_is_idempotent_across_transports() {
        let db = test_database();
        let fp = "volte-mt:single:+123:2026-07-14 10:00:00:abcdef";

        assert!(db
            .claim_sms_dedup("line-a", fp, "volte_ims")
            .expect("claim line-a"));
        assert!(db
            .sms_dedup_exists("line-a", fp)
            .expect("exists after claim"));
        assert!(!db
            .claim_sms_dedup("line-a", fp, "modem")
            .expect("duplicate on line-a"));
        assert!(db
            .claim_sms_dedup("line-b", fp, "modem")
            .expect("same fingerprint on line-b"));
        assert!(db
            .claim_sms_dedup("line-a", "other-fp", "vowifi_ims")
            .expect("claim other"));
        assert_eq!(db.sms_dedup_count("line-a").expect("line-a count"), 2);
        assert_eq!(db.sms_dedup_count("line-b").expect("line-b count"), 1);
    }

    #[test]
    fn sms_dedup_cleanup_prunes_expired_rows() {
        let db = test_database();
        db.claim_sms_dedup("line-a", "fresh", "modem")
            .expect("claim fresh");

        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO sms_dedup (line_id, fingerprint, transport, created_at)
                 VALUES ('line-a', 'stale', 'modem', '2000-01-01 00:00:00')",
                [],
            )
            .expect("insert stale line-a");
            conn.execute(
                "INSERT INTO sms_dedup (line_id, fingerprint, transport, created_at)
                 VALUES ('line-b', 'stale', 'modem', '2000-01-01 00:00:00')",
                [],
            )
            .expect("insert stale line-b");
        }
        assert_eq!(db.sms_dedup_count("line-a").expect("count before"), 2);

        let deleted = db.cleanup_sms_dedup("line-a", 30).expect("cleanup");
        assert_eq!(deleted, 1, "only line-a's stale row is pruned");
        assert!(db
            .sms_dedup_exists("line-a", "fresh")
            .expect("fresh survives"));
        assert!(!db
            .sms_dedup_exists("line-a", "stale")
            .expect("line-a stale pruned"));
        assert!(db
            .sms_dedup_exists("line-b", "stale")
            .expect("line-b stale survives"));
    }

    #[test]
    fn legacy_sms_dedup_table_migrates_to_line_scoped_unique_key() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "simadmin-sms-dedup-line-migration-{}-{nonce}.db",
            std::process::id()
        ));
        {
            let conn = Connection::open(&path).expect("create legacy database");
            conn.execute_batch(
                "CREATE TABLE sms_dedup (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    fingerprint TEXT NOT NULL UNIQUE,
                    transport TEXT NOT NULL DEFAULT 'modem',
                    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
                 );
                 INSERT INTO sms_dedup (fingerprint, transport, created_at)
                 VALUES ('legacy-fingerprint', 'modem', '2026-08-04 22:00:00');",
            )
            .expect("seed legacy dedup table");
        }

        let db = Database::new(path.clone()).expect("migrate legacy dedup table");
        {
            let conn = db.conn.lock().unwrap();
            assert!(table_has_column(&conn, "sms_dedup", "line_id").unwrap());
            assert!(
                table_has_unique_index(&conn, "sms_dedup", &["line_id", "fingerprint"]).unwrap()
            );
            assert!(!table_has_unique_index(&conn, "sms_dedup", &["fingerprint"]).unwrap());
            let legacy_scope: String = conn
                .query_row(
                    "SELECT line_id FROM sms_dedup WHERE fingerprint = 'legacy-fingerprint'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(legacy_scope, "");
        }
        assert!(db
            .claim_sms_dedup("line-a", "legacy-fingerprint", "modem")
            .expect("legacy compatibility row must not block line-a"));
        assert!(db
            .claim_sms_dedup("line-b", "legacy-fingerprint", "volte_ims")
            .expect("line-a must not block line-b"));

        drop(db);
        std::fs::remove_file(path).expect("remove migration database");
    }

    #[test]
    fn vowifi_sms_delivery_round_trips_with_parts() {
        let db = test_database();

        db.upsert_vowifi_sms_delivery(NewVowifiSmsDelivery {
            message_id: "msg-1",
            line_id: "line-test",
            trace_id: "trace-1",
            direction: "mo",
            state: "accepted",
            sip_state: "accepted",
            rpdu_ack: "acked",
            delivery_reported: false,
            failure_cause: None,
            retry_count: 0,
            api_sms_id: None,
        })
        .expect("write delivery");
        db.upsert_vowifi_sms_part(NewVowifiSmsPart {
            line_id: "line-test",
            message_id: "msg-1",
            reference: 7,
            sequence: 1,
            total: 2,
            received: true,
        })
        .expect("write first part");
        db.upsert_vowifi_sms_part(NewVowifiSmsPart {
            line_id: "line-test",
            message_id: "msg-1",
            reference: 7,
            sequence: 2,
            total: 2,
            received: true,
        })
        .expect("write second part");

        let delivery = db
            .get_vowifi_sms_delivery_for_line("line-test", "msg-1")
            .expect("read delivery")
            .expect("delivery exists");
        assert_eq!(delivery.state, "accepted");
        assert_eq!(delivery.rpdu_ack, "acked");
        assert_eq!(delivery.parts.len(), 2);

        let deliveries = db
            .get_vowifi_sms_deliveries_for_line("line-test", 10, 0)
            .expect("list deliveries");
        assert_eq!(deliveries.total, 1);
        assert_eq!(deliveries.deliveries[0].parts.len(), 2);
    }

    #[test]
    fn vowifi_sms_delivery_ids_and_parts_are_isolated_by_line() {
        let db = test_database();

        for (line_id, trace_id, total, received) in [
            ("line-a", "trace-a", 1, true),
            ("line-b", "trace-b", 2, false),
        ] {
            db.upsert_vowifi_sms_delivery(NewVowifiSmsDelivery {
                message_id: "shared-message",
                line_id,
                trace_id,
                direction: "mobile_terminated",
                state: "received",
                sip_state: "accepted",
                rpdu_ack: "acked",
                delivery_reported: received,
                failure_cause: None,
                retry_count: 0,
                api_sms_id: None,
            })
            .expect("write line delivery");
            db.upsert_vowifi_sms_part(NewVowifiSmsPart {
                line_id,
                message_id: "shared-message",
                reference: 9,
                sequence: 1,
                total,
                received,
            })
            .expect("write line part");
        }

        let line_a = db
            .get_vowifi_sms_delivery_for_line("line-a", "shared-message")
            .expect("read line-a delivery")
            .expect("line-a delivery exists");
        let line_b = db
            .get_vowifi_sms_delivery_for_line("line-b", "shared-message")
            .expect("read line-b delivery")
            .expect("line-b delivery exists");

        assert_eq!(line_a.trace_id, "trace-a");
        assert_eq!(line_a.parts.len(), 1);
        assert_eq!(line_a.parts[0].total, 1);
        assert!(line_a.parts[0].received);
        assert_eq!(line_b.trace_id, "trace-b");
        assert_eq!(line_b.parts.len(), 1);
        assert_eq!(line_b.parts[0].total, 2);
        assert!(!line_b.parts[0].received);
    }

    #[test]
    fn deleting_sms_unlinks_vowifi_delivery_api_reference() {
        let db = test_database();
        let sms_id = db
            .insert_sms(
                "incoming",
                "10086",
                "redacted test body",
                "received",
                Some("marker-1"),
            )
            .expect("insert sms");

        db.upsert_vowifi_sms_delivery(NewVowifiSmsDelivery {
            message_id: "msg-delete",
            line_id: "line-test",
            trace_id: "trace-delete",
            direction: "mobile_terminated",
            state: "received",
            sip_state: "accepted",
            rpdu_ack: "acked",
            delivery_reported: true,
            failure_cause: None,
            retry_count: 0,
            api_sms_id: Some(sms_id),
        })
        .expect("write delivery");

        assert_eq!(
            db.delete_sms_for_channel(sms_id, "unassigned")
                .expect("delete sms"),
            1
        );
        let delivery = db
            .get_vowifi_sms_delivery_for_line("line-test", "msg-delete")
            .expect("read delivery")
            .expect("delivery remains for diagnostics");
        assert_eq!(delivery.api_sms_id, None);
    }

    #[test]
    fn sms_retention_keeps_newest_rows_and_unlinks_delivery_diagnostics() {
        let db = test_database();
        let mut ids = Vec::new();
        for index in 0..5 {
            ids.push(
                db.insert_sms_with_transport_for_line(
                    "incoming",
                    "10086",
                    &format!("message-{index}"),
                    "received",
                    Some(&format!("marker-{index}")),
                    "modem",
                    Some("line-test"),
                )
                .unwrap(),
            );
        }
        let mut other_line_ids = Vec::new();
        for index in 0..4 {
            other_line_ids.push(
                db.insert_sms_with_transport_for_line(
                    "incoming",
                    "10010",
                    &format!("other-message-{index}"),
                    "received",
                    Some(&format!("other-marker-{index}")),
                    "modem",
                    Some("line-other"),
                )
                .unwrap(),
            );
        }
        db.upsert_vowifi_sms_delivery(NewVowifiSmsDelivery {
            message_id: "msg-pruned",
            line_id: "line-test",
            trace_id: "trace-pruned",
            direction: "mobile_terminated",
            state: "received",
            sip_state: "accepted",
            rpdu_ack: "acked",
            delivery_reported: true,
            failure_cause: None,
            retry_count: 0,
            api_sms_id: Some(ids[0]),
        })
        .unwrap();

        assert_eq!(db.prune_sms_messages_for_line("line-test", 3).unwrap(), 2);
        let messages = db.get_sms_messages(10, 0, None).unwrap();
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.line_id.as_deref() == Some("line-test"))
                .map(|message| message.id)
                .collect::<Vec<_>>(),
            vec![ids[4], ids[3], ids[2]]
        );
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.line_id.as_deref() == Some("line-other"))
                .map(|message| message.id)
                .collect::<Vec<_>>(),
            other_line_ids.into_iter().rev().collect::<Vec<_>>()
        );
        assert_eq!(
            db.get_vowifi_sms_delivery_for_line("line-test", "msg-pruned")
                .unwrap()
                .unwrap()
                .api_sms_id,
            None
        );
    }

    #[test]
    fn sms_lists_use_id_as_timestamp_tiebreaker() {
        let db = test_database();

        let first = db
            .insert_sms_at(
                "outgoing",
                "10086",
                "CHECK",
                "2026-06-22 13:13:59",
                "sent",
                None,
            )
            .expect("insert outgoing");
        let second = db
            .insert_sms_at(
                "incoming",
                "10086",
                "reply",
                "2026-06-22 13:13:59",
                "received",
                Some("vowifi-mt:test"),
            )
            .expect("insert incoming");

        let messages = db.get_sms_messages(10, 0, None).expect("list sms");
        assert_eq!(
            messages
                .iter()
                .map(|message| message.id)
                .collect::<Vec<_>>(),
            vec![second, first]
        );

        let conversation = db
            .get_sms_conversation("10086", 10)
            .expect("list conversation");
        assert_eq!(
            conversation
                .iter()
                .map(|message| message.id)
                .collect::<Vec<_>>(),
            vec![second, first]
        );
    }

    #[test]
    fn sms_transport_defaults_to_modem_and_can_mark_vowifi_ims() {
        let db = test_database();

        db.insert_sms("outgoing", "10086", "CHECK", "sent", None)
            .expect("insert modem sms");
        db.insert_sms_with_transport(
            "incoming",
            "10086",
            "reply",
            "received",
            Some("vowifi-mt:test"),
            "vowifi_ims",
        )
        .expect("insert vowifi sms");

        let messages = db.get_sms_messages(10, 0, None).expect("list sms");
        assert!(messages
            .iter()
            .any(|message| message.content == "CHECK" && message.transport == "modem"));
        assert!(messages
            .iter()
            .any(|message| message.content == "reply" && message.transport == "vowifi_ims"));
    }

    #[test]
    fn sms_line_identity_round_trips_without_changing_transport_label() {
        let db = test_database();
        let line_id = "line-0123456789abcdef0123456789abcdef";
        db.insert_sms_with_transport_for_line(
            "outgoing",
            "10086",
            "line-test",
            "sent",
            None,
            "volte_ims",
            Some(line_id),
        )
        .expect("insert line sms");

        let message = db.get_sms_messages(1, 0, None).expect("list sms").remove(0);
        assert_eq!(message.transport, "volte_ims");
        assert_eq!(message.line_id.as_deref(), Some(line_id));
    }

    #[test]
    fn sms_identity_checks_do_not_match_another_line() {
        let db = test_database();
        let line_a = "line-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let line_b = "line-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let timestamp = "2026-08-04 21:30:00";
        let marker = "mmfp:shared-marker";
        let id = db
            .insert_sms_at_with_transport_for_line(
                "incoming",
                "10086",
                "same message",
                timestamp,
                "received",
                Some(marker),
                "modem",
                Some(line_a),
            )
            .expect("insert line-a SMS");

        assert!(db.sms_exists_by_pdu_for_line(line_a, marker).unwrap());
        assert!(!db.sms_exists_by_pdu_for_line(line_b, marker).unwrap());
        assert_eq!(db.sms_id_by_pdu_for_line(line_a, marker).unwrap(), Some(id));
        assert_eq!(db.sms_id_by_pdu_for_line(line_b, marker).unwrap(), None);
        assert!(db
            .incoming_sms_exists_by_timestamp_for_line(line_a, "10086", "same message", timestamp,)
            .unwrap());
        assert!(!db
            .incoming_sms_exists_by_timestamp_for_line(line_b, "10086", "same message", timestamp,)
            .unwrap());

        db.insert_sms_at_with_transport_for_line(
            "incoming",
            "10010",
            "legacy message",
            timestamp,
            "received",
            None,
            "modem",
            Some(line_a),
        )
        .expect("insert legacy line-a SMS");
        assert!(db
            .incoming_sms_exists_by_legacy_content_for_line(line_a, "10010", "legacy message",)
            .unwrap());
        assert!(!db
            .incoming_sms_exists_by_legacy_content_for_line(line_b, "10010", "legacy message",)
            .unwrap());
    }

    #[test]
    fn sms_channel_filters_list_conversation_stats_and_delete() {
        let db = test_database();
        let line_a = "line-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let line_b = "line-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let insert = |phone_number: &str, content: &str, line_id: Option<&str>| {
            db.insert_sms_with_transport_for_line(
                "incoming",
                phone_number,
                content,
                "received",
                None,
                "modem",
                line_id,
            )
            .expect("insert channel sms")
        };
        let line_a_id = insert("10086", "from-a", Some(line_a));
        let line_b_id = insert("10086", "from-b", Some(line_b));
        insert("10010", "conversation-a", Some(line_a));
        insert("10010", "conversation-b", Some(line_b));
        insert("10000", "legacy", None);

        let line_messages = db
            .get_sms_messages_for_channel(10, 0, None, Some(line_a))
            .expect("list line channel");
        assert_eq!(line_messages.len(), 2);
        assert_eq!(db.get_sms_stats_for_channel(Some(line_b)).unwrap().total, 2);
        assert_eq!(
            db.get_sms_conversation_for_channel("10000", 10, Some("unassigned"))
                .unwrap()[0]
                .content,
            "legacy"
        );

        assert_eq!(db.delete_sms_for_channel(line_b_id, line_a).unwrap(), 0);
        assert_eq!(db.get_sms_stats().unwrap().total, 5);
        assert_eq!(
            db.delete_sms_batch_for_channel(&[line_a_id, line_b_id], &[], line_a)
                .unwrap(),
            1
        );
        assert_eq!(db.get_sms_stats_for_channel(Some(line_b)).unwrap().total, 2);
        assert_eq!(
            db.delete_sms_conversation_for_channel("10010", line_a)
                .unwrap(),
            1
        );
        assert_eq!(db.get_sms_stats_for_channel(Some(line_b)).unwrap().total, 2);
        assert_eq!(db.clear_sms_for_channel("unassigned").unwrap(), 1);
        assert_eq!(db.get_sms_stats().unwrap().total, 2);
        assert_eq!(db.clear_sms_for_channel(line_a).unwrap(), 0);
        assert_eq!(db.clear_sms_for_channel(line_b).unwrap(), 2);
        assert_eq!(db.get_sms_stats().unwrap().total, 0);
    }

    #[test]
    fn vowifi_restore_and_events_are_readable_when_empty_or_present() {
        let db = test_database();

        assert!(db
            .get_vowifi_esim_restore_for_line("line-test")
            .expect("empty restore")
            .is_none());
        assert_eq!(
            db.get_vowifi_runtime_events_for_line("line-test", 10, 0, None)
                .expect("empty events")
                .total,
            0
        );

        db.upsert_vowifi_esim_restore(NewVowifiEsimRestore {
            line_id: "line-test",
            switch_token: Some("switch-redacted"),
            switch_phase: Some("retry_scheduled"),
            phase_ms: Some(1500),
            identity_ready: true,
            sim_auth_ready: false,
            degraded_reason: Some("context_canceled"),
            retry_count: 1,
        })
        .expect("write restore");
        db.insert_vowifi_runtime_event(NewVowifiRuntimeEvent {
            line_id: "line-test",
            trace_id: Some("trace-redacted"),
            level: "info",
            phase: "profile_matched",
            profile_id: Some("gb_ee_23433"),
            event_type: "identity_refresh",
            detail_json: r#"{"identity_ready":true}"#,
        })
        .expect("write event");

        let restore = db
            .get_vowifi_esim_restore_for_line("line-test")
            .expect("read restore")
            .expect("restore exists");
        assert_eq!(restore.switch_phase.as_deref(), Some("retry_scheduled"));
        assert_eq!(restore.retry_count, 1);

        let events = db
            .get_vowifi_runtime_events_for_line("line-test", 10, 0, None)
            .expect("read events");
        assert_eq!(events.total, 1);
        assert_eq!(events.events[0].event_type, "identity_refresh");
    }

    #[test]
    fn vowifi_runtime_events_filter_by_trace_and_redact_sensitive_detail() {
        let db = test_database();

        db.insert_vowifi_runtime_event(NewVowifiRuntimeEvent {
            line_id: "line-test",
            trace_id: Some("trace-a"),
            level: "info",
            phase: "identity_ready",
            profile_id: Some("gb_ee_23433"),
            event_type: "identity_refresh",
            detail_json: r#"{"identity_ready":true}"#,
        })
        .expect("write event a");
        db.insert_vowifi_runtime_event(NewVowifiRuntimeEvent {
            line_id: "line-test",
            trace_id: Some("trace-b"),
            level: "warning",
            phase: "sim_auth_gate",
            profile_id: Some("gb_ee_23433"),
            event_type: "sim_auth_retry",
            detail_json: r#"{"imsi":"sample-redacted-value","token":"sample-redacted-value"}"#,
        })
        .expect("write event b");

        let trace_a = db
            .get_vowifi_runtime_events_for_line("line-test", 10, 0, Some("trace-a"))
            .expect("filter trace a");
        assert_eq!(trace_a.total, 1);
        assert_eq!(trace_a.events[0].trace_id.as_deref(), Some("trace-a"));
        assert_eq!(trace_a.events[0].detail_json, r#"{"identity_ready":true}"#);

        let trace_b = db
            .get_vowifi_runtime_events_for_line("line-test", 10, 0, Some("trace-b"))
            .expect("filter trace b");
        assert_eq!(trace_b.total, 1);
        assert!(trace_b.events[0].detail_json.contains("redacted"));
        assert!(!trace_b.events[0]
            .detail_json
            .contains("sample-redacted-value"));
        assert!(!trace_b.events[0].detail_json.contains("secret"));
    }

    #[test]
    fn vowifi_soak_runs_round_trip_with_counter_samples() {
        let db = test_database();

        assert_eq!(
            db.get_vowifi_soak_runs_for_line("line-test", 10, 0)
                .expect("empty soak")
                .total,
            0
        );

        db.upsert_vowifi_soak_run(NewVowifiSoakRun {
            run_id: "soak-1",
            line_id: "line-test",
            scenario_id: "rekey_dpd_nat_t_soak",
            profile_id: Some("gb_ee_23433"),
            plmn: Some("23433"),
            status: "running",
            duration_seconds: 60,
            sample_count: 2,
            failure_count: 0,
            last_error: None,
        })
        .expect("write soak run");
        db.insert_vowifi_soak_sample(NewVowifiSoakSample {
            line_id: "line-test",
            run_id: "soak-1",
            sample_kind: "counter",
            metric_name: "dpd_requests",
            metric_value: 3,
            state: "ok",
        })
        .expect("write sample");

        let runs = db
            .get_vowifi_soak_runs_for_line("line-test", 10, 0)
            .expect("read soak runs");
        assert_eq!(runs.total, 1);
        assert!(runs.read_only);
        assert_eq!(runs.runs[0].scenario_id, "rekey_dpd_nat_t_soak");
        assert_eq!(runs.runs[0].samples.len(), 1);
        assert_eq!(runs.runs[0].samples[0].metric_name, "dpd_requests");

        let json = serde_json::to_string(&runs).expect("serialize soak runs");
        let lower = json.to_ascii_lowercase();
        for forbidden_key in [
            "imsi",
            "iccid",
            "imei",
            "eid",
            "msisdn",
            "phone_number",
            "sms_body",
            "key_material",
            "authorization",
            "password",
            "token",
        ] {
            assert!(!lower.contains(&format!("\"{forbidden_key}\"")));
        }
    }

    #[test]
    fn vowifi_soak_run_ids_and_samples_are_isolated_by_line() {
        let db = test_database();

        for (line_id, scenario_id, metric_value) in
            [("line-a", "scenario-a", 11), ("line-b", "scenario-b", 22)]
        {
            db.upsert_vowifi_soak_run(NewVowifiSoakRun {
                run_id: "shared-run",
                line_id,
                scenario_id,
                profile_id: None,
                plmn: None,
                status: "running",
                duration_seconds: 1,
                sample_count: 1,
                failure_count: 0,
                last_error: None,
            })
            .expect("write line soak run");
            db.insert_vowifi_soak_sample(NewVowifiSoakSample {
                line_id,
                run_id: "shared-run",
                sample_kind: "counter",
                metric_name: "attempts",
                metric_value,
                state: "ok",
            })
            .expect("write line soak sample");
        }

        let line_a = db
            .get_vowifi_soak_runs_for_line("line-a", 10, 0)
            .expect("read line-a runs");
        let line_b = db
            .get_vowifi_soak_runs_for_line("line-b", 10, 0)
            .expect("read line-b runs");

        assert_eq!(line_a.total, 1);
        assert_eq!(line_a.runs[0].scenario_id, "scenario-a");
        assert_eq!(line_a.runs[0].samples.len(), 1);
        assert_eq!(line_a.runs[0].samples[0].metric_value, 11);
        assert_eq!(line_b.total, 1);
        assert_eq!(line_b.runs[0].scenario_id, "scenario-b");
        assert_eq!(line_b.runs[0].samples.len(), 1);
        assert_eq!(line_b.runs[0].samples[0].metric_value, 22);
    }
}

fn vowifi_runtime_event_from_row(row: &Row<'_>) -> Result<VowifiRuntimeEventEntry> {
    let detail_json: String = row.get(7)?;
    Ok(VowifiRuntimeEventEntry {
        id: row.get(0)?,
        line_id: row.get(1)?,
        trace_id: row.get(2)?,
        level: row.get(3)?,
        phase: row.get(4)?,
        profile_id: row.get(5)?,
        event_type: row.get(6)?,
        detail_json: redact_vowifi_event_detail(&detail_json),
        created_at: row.get(8)?,
    })
}

fn redact_vowifi_event_detail(detail_json: &str) -> String {
    const SENSITIVE_MARKERS: &[&str] = &[
        "imsi",
        "iccid",
        "imei",
        "eid",
        "msisdn",
        "phone_number",
        "token",
        "password",
        "authorization",
        "aka",
        "key_material",
        "spi",
        "sms_body",
        "content",
    ];

    let lower = detail_json.to_ascii_lowercase();
    if SENSITIVE_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
    {
        r#"{"redacted":true,"policy":"vowifi_event_detail_sensitive_marker"}"#.to_string()
    } else {
        detail_json.to_string()
    }
}

fn vowifi_soak_run_from_row(row: &Row<'_>) -> Result<VowifiSoakRunEntry> {
    Ok(VowifiSoakRunEntry {
        run_id: row.get(0)?,
        line_id: row.get(1)?,
        scenario_id: row.get(2)?,
        profile_id: row.get(3)?,
        plmn: row.get(4)?,
        status: row.get(5)?,
        started_at: row.get(6)?,
        finished_at: row.get(7)?,
        duration_seconds: row.get(8)?,
        sample_count: row.get(9)?,
        failure_count: row.get(10)?,
        last_error: row.get(11)?,
        sensitive_values_policy: "counters_and_state_names_only_no_payload_or_secret_values"
            .to_string(),
        samples: Vec::new(),
    })
}

fn vowifi_soak_sample_from_row(row: &Row<'_>) -> Result<VowifiSoakSampleEntry> {
    Ok(VowifiSoakSampleEntry {
        id: row.get(0)?,
        run_id: row.get(1)?,
        sample_kind: row.get(2)?,
        metric_name: row.get(3)?,
        metric_value: row.get(4)?,
        state: row.get(5)?,
        created_at: row.get(6)?,
    })
}

fn notification_log_start_bound(value: &str) -> String {
    notification_log_date_bound(value, "00:00:00")
}

fn notification_log_end_bound(value: &str) -> String {
    notification_log_date_bound(value, "23:59:59")
}

fn sms_message_from_row(row: &Row<'_>) -> Result<SmsMessage> {
    let timestamp: String = row.get(4)?;
    Ok(SmsMessage {
        id: row.get(0)?,
        direction: row.get(1)?,
        phone_number: row.get(2)?,
        content: row.get(3)?,
        timestamp: sms_timestamp_for_display(timestamp),
        status: row.get(5)?,
        pdu: row.get(6)?,
        transport: row.get(7)?,
        line_id: row.get(8)?,
    })
}

fn notification_queue_entry_from_row(row: &Row<'_>) -> Result<NotificationQueueEntry> {
    Ok(NotificationQueueEntry {
        id: row.get(0)?,
        line_id: row.get(1)?,
        status: row.get(2)?,
        event_type: row.get(3)?,
        event_label: row.get(4)?,
        summary: row.get(5)?,
        reason: row.get(6)?,
        channel_id: row.get(7)?,
        channel_name: row.get(8)?,
        channel_type: row.get(9)?,
        rule_id: row.get(10)?,
        rule_name: row.get(11)?,
        title: row.get(12)?,
        body: row.get(13)?,
        next_attempt_at: row.get(14)?,
        attempt_count: row.get(15)?,
        max_attempts: row.get(16)?,
        created_at: row.get(17)?,
        updated_at: row.get(18)?,
    })
}

fn call_record_from_row(row: &Row<'_>) -> Result<CallRecord> {
    Ok(CallRecord {
        id: row.get(0)?,
        line_id: row.get(1)?,
        direction: row.get(2)?,
        phone_number: row.get(3)?,
        duration: row.get(4)?,
        start_time: row.get(5)?,
        end_time: row.get(6)?,
        answered: row.get::<_, i32>(7)? != 0,
    })
}

fn normalize_existing_sms_timestamps(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("SELECT id, timestamp FROM sms_messages")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut updates = Vec::new();
    for row in rows {
        let (id, timestamp) = row?;
        if let Some(normalized) = normalize_sms_timestamp_for_display(&timestamp) {
            if normalized != timestamp {
                updates.push((id, normalized));
            }
        }
    }
    drop(stmt);

    for (id, timestamp) in updates {
        conn.execute(
            "UPDATE sms_messages SET timestamp = ?1 WHERE id = ?2",
            params![timestamp, id],
        )?;
    }

    Ok(())
}

fn non_empty_option(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn normalized_sms_transport(value: &str) -> &'static str {
    match value.trim() {
        "vowifi_ims" => "vowifi_ims",
        "volte_ims" => "volte_ims",
        _ => "modem",
    }
}

fn table_has_column(conn: &Connection, table_name: &str, column_name: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table_name})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;

    for row in rows {
        if row? == column_name {
            return Ok(true);
        }
    }

    Ok(false)
}

fn table_primary_key_is(
    conn: &Connection,
    table_name: &str,
    expected_columns: &[&str],
) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table_name})"))?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
    })?;
    let mut columns = Vec::new();
    for row in rows {
        let (name, position) = row?;
        if position > 0 {
            columns.push((position, name));
        }
    }
    columns.sort_by_key(|(position, _)| *position);

    Ok(columns
        .iter()
        .map(|(_, name)| name.as_str())
        .eq(expected_columns.iter().copied()))
}

fn table_has_unique_index(
    conn: &Connection,
    table_name: &str,
    expected_columns: &[&str],
) -> Result<bool> {
    let indexes = {
        let mut stmt = conn.prepare(&format!("PRAGMA index_list({table_name})"))?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)? != 0))
        })?;
        rows.collect::<Result<Vec<_>>>()?
    };

    for (index_name, unique) in indexes {
        if !unique {
            continue;
        }
        let escaped_name = index_name.replace('"', "\"\"");
        let mut stmt = conn.prepare(&format!("PRAGMA index_info(\"{escaped_name}\")"))?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(2))?;
        let columns = rows.collect::<Result<Vec<_>>>()?;
        if columns
            .iter()
            .map(String::as_str)
            .eq(expected_columns.iter().copied())
        {
            return Ok(true);
        }
    }

    Ok(false)
}

fn migrate_sms_dedup_for_line_scope(conn: &Connection) -> Result<()> {
    let has_line_id = table_has_column(conn, "sms_dedup", "line_id")?;
    if has_line_id
        && table_has_unique_index(conn, "sms_dedup", &["line_id", "fingerprint"])?
        && !table_has_unique_index(conn, "sms_dedup", &["fingerprint"])?
    {
        return Ok(());
    }

    conn.execute_batch("BEGIN IMMEDIATE")?;
    let migration = (|| -> Result<()> {
        conn.execute("ALTER TABLE sms_dedup RENAME TO sms_dedup_legacy", [])?;
        conn.execute_batch(
            "CREATE TABLE sms_dedup (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                line_id TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                transport TEXT NOT NULL DEFAULT 'modem',
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(line_id, fingerprint)
            )",
        )?;
        let line_id_expression = if has_line_id {
            "COALESCE(NULLIF(TRIM(line_id), ''), '')"
        } else {
            "''"
        };
        conn.execute(
            &format!(
                "INSERT OR IGNORE INTO sms_dedup (
                    id, line_id, fingerprint, transport, created_at
                 )
                 SELECT id, {line_id_expression}, fingerprint, transport, created_at
                 FROM sms_dedup_legacy"
            ),
            [],
        )?;
        conn.execute("DROP TABLE sms_dedup_legacy", [])?;
        Ok(())
    })();

    match migration {
        Ok(()) => conn.execute_batch("COMMIT"),
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn migrate_vowifi_sms_tables_for_line_scope(conn: &Connection) -> Result<()> {
    if table_primary_key_is(conn, "vowifi_sms_delivery", &["line_id", "message_id"])?
        && table_primary_key_is(
            conn,
            "vowifi_sms_parts",
            &["line_id", "message_id", "reference", "sequence"],
        )?
    {
        return Ok(());
    }

    conn.execute_batch(
        "BEGIN IMMEDIATE;
         ALTER TABLE vowifi_sms_parts RENAME TO vowifi_sms_parts_legacy;
         ALTER TABLE vowifi_sms_delivery RENAME TO vowifi_sms_delivery_legacy;
         CREATE TABLE vowifi_sms_delivery (
            line_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            trace_id TEXT NOT NULL,
            direction TEXT NOT NULL,
            state TEXT NOT NULL,
            sip_state TEXT NOT NULL,
            rpdu_ack TEXT NOT NULL,
            delivery_reported INTEGER NOT NULL,
            failure_cause TEXT,
            retry_count INTEGER NOT NULL DEFAULT 0,
            api_sms_id INTEGER,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY(line_id, message_id),
            FOREIGN KEY(api_sms_id) REFERENCES sms_messages(id) ON DELETE SET NULL
         );
         INSERT INTO vowifi_sms_delivery (
            line_id, message_id, trace_id, direction, state, sip_state, rpdu_ack,
            delivery_reported, failure_cause, retry_count, api_sms_id, created_at, updated_at
         )
         SELECT COALESCE(NULLIF(TRIM(line_id), ''), ''), message_id, trace_id, direction,
            state, sip_state, rpdu_ack, delivery_reported, failure_cause, retry_count,
            api_sms_id, created_at, updated_at
         FROM vowifi_sms_delivery_legacy;
         CREATE TABLE vowifi_sms_parts (
            line_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            reference INTEGER NOT NULL,
            sequence INTEGER NOT NULL,
            total INTEGER NOT NULL,
            received INTEGER NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY(line_id, message_id, reference, sequence),
            FOREIGN KEY(line_id, message_id)
                REFERENCES vowifi_sms_delivery(line_id, message_id)
         );
         INSERT INTO vowifi_sms_parts (
            line_id, message_id, reference, sequence, total, received, updated_at
         )
         SELECT COALESCE(NULLIF(TRIM(delivery.line_id), ''), ''), part.message_id,
            part.reference, part.sequence, part.total, part.received, part.updated_at
         FROM vowifi_sms_parts_legacy AS part
         JOIN vowifi_sms_delivery_legacy AS delivery
           ON delivery.message_id = part.message_id;
         DROP TABLE vowifi_sms_parts_legacy;
         DROP TABLE vowifi_sms_delivery_legacy;
         COMMIT;",
    )?;

    Ok(())
}

fn migrate_vowifi_soak_tables_for_line_scope(conn: &Connection) -> Result<()> {
    if table_primary_key_is(conn, "vowifi_soak_runs", &["line_id", "run_id"])?
        && table_has_column(conn, "vowifi_soak_samples", "line_id")?
    {
        return Ok(());
    }

    conn.execute_batch(
        "BEGIN IMMEDIATE;
         ALTER TABLE vowifi_soak_samples RENAME TO vowifi_soak_samples_legacy;
         ALTER TABLE vowifi_soak_runs RENAME TO vowifi_soak_runs_legacy;
         CREATE TABLE vowifi_soak_runs (
            line_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            scenario_id TEXT NOT NULL,
            profile_id TEXT,
            plmn TEXT,
            status TEXT NOT NULL,
            started_at TEXT NOT NULL,
            finished_at TEXT,
            duration_seconds INTEGER NOT NULL DEFAULT 0,
            sample_count INTEGER NOT NULL DEFAULT 0,
            failure_count INTEGER NOT NULL DEFAULT 0,
            last_error TEXT,
            PRIMARY KEY(line_id, run_id)
         );
         INSERT INTO vowifi_soak_runs (
            line_id, run_id, scenario_id, profile_id, plmn, status, started_at, finished_at,
            duration_seconds, sample_count, failure_count, last_error
         )
         SELECT COALESCE(NULLIF(TRIM(line_id), ''), ''), run_id, scenario_id, profile_id,
            plmn, status, started_at, finished_at, duration_seconds, sample_count,
            failure_count, last_error
         FROM vowifi_soak_runs_legacy;
         CREATE TABLE vowifi_soak_samples (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            line_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            sample_kind TEXT NOT NULL,
            metric_name TEXT NOT NULL,
            metric_value INTEGER NOT NULL,
            state TEXT NOT NULL,
            created_at TEXT NOT NULL,
            FOREIGN KEY(line_id, run_id) REFERENCES vowifi_soak_runs(line_id, run_id)
         );
         INSERT INTO vowifi_soak_samples (
            id, line_id, run_id, sample_kind, metric_name, metric_value, state, created_at
         )
         SELECT sample.id, COALESCE(NULLIF(TRIM(run.line_id), ''), ''), sample.run_id,
            sample.sample_kind, sample.metric_name, sample.metric_value, sample.state,
            sample.created_at
         FROM vowifi_soak_samples_legacy AS sample
         JOIN vowifi_soak_runs_legacy AS run ON run.run_id = sample.run_id;
         DROP TABLE vowifi_soak_samples_legacy;
         DROP TABLE vowifi_soak_runs_legacy;
         COMMIT;",
    )?;

    Ok(())
}

impl Database {
    /// 创建或打开数据库
    pub fn new(db_path: PathBuf) -> Result<Self> {
        let conn = Connection::open(db_path)?;

        // 创建短信表（如果不存在）
        conn.execute(
            "CREATE TABLE IF NOT EXISTS sms_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                direction TEXT NOT NULL,
                phone_number TEXT NOT NULL,
                content TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                status TEXT NOT NULL,
                notification_status TEXT NOT NULL DEFAULT 'pending',
                pdu TEXT,
                transport TEXT NOT NULL DEFAULT 'modem',
                line_id TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;

        if !table_has_column(&conn, "sms_messages", "notification_status")? {
            conn.execute(
                "ALTER TABLE sms_messages
                 ADD COLUMN notification_status TEXT NOT NULL DEFAULT 'pending'",
                [],
            )?;
        }
        if !table_has_column(&conn, "sms_messages", "transport")? {
            conn.execute(
                "ALTER TABLE sms_messages
                 ADD COLUMN transport TEXT NOT NULL DEFAULT 'modem'",
                [],
            )?;
        }
        if !table_has_column(&conn, "sms_messages", "line_id")? {
            conn.execute("ALTER TABLE sms_messages ADD COLUMN line_id TEXT", [])?;
        }

        // 创建短信索引
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_sms_timestamp ON sms_messages(timestamp DESC)",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_sms_phone ON sms_messages(phone_number)",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_sms_notification_status ON sms_messages(notification_status)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_sms_transport ON sms_messages(transport)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_sms_line_id ON sms_messages(line_id)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_sms_line_pdu ON sms_messages(line_id, pdu)",
            [],
        )?;
        normalize_existing_sms_timestamps(&conn)?;

        // Cross-transport SMS dedup fingerprints are isolated by line. Legacy
        // rows are retained under the empty compatibility scope because their
        // originating line cannot be reconstructed safely.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS sms_dedup (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                line_id TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                transport TEXT NOT NULL DEFAULT 'modem',
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(line_id, fingerprint)
            )",
            [],
        )?;
        migrate_sms_dedup_for_line_scope(&conn)?;
        conn.execute("DROP INDEX IF EXISTS idx_sms_dedup_created_at", [])?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_sms_dedup_line_created_at
             ON sms_dedup(line_id, created_at)",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS notification_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                line_id TEXT,
                event_type TEXT NOT NULL,
                status TEXT NOT NULL,
                summary TEXT NOT NULL,
                rule_id TEXT NOT NULL,
                rule_name TEXT NOT NULL,
                channel_id TEXT NOT NULL,
                channel_name TEXT NOT NULL,
                message TEXT NOT NULL,
                created_at TEXT NOT NULL
            )",
            [],
        )?;
        if !table_has_column(&conn, "notification_logs", "line_id")? {
            conn.execute("ALTER TABLE notification_logs ADD COLUMN line_id TEXT", [])?;
        }
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_notification_logs_created_at ON notification_logs(created_at DESC)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_notification_logs_type_status ON notification_logs(event_type, status)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_notification_logs_line_created_at
             ON notification_logs(line_id, created_at DESC)",
            [],
        )?;

        // Notification retries retain the originating line after rendering.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS notification_queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                line_id TEXT,
                status TEXT NOT NULL,
                event_type TEXT NOT NULL,
                event_label TEXT NOT NULL,
                summary TEXT NOT NULL,
                reason TEXT NOT NULL DEFAULT '',
                rule_id TEXT NOT NULL DEFAULT '',
                rule_name TEXT NOT NULL DEFAULT '',
                channel_id TEXT NOT NULL,
                channel_name TEXT NOT NULL,
                channel_type TEXT NOT NULL,
                title TEXT NOT NULL DEFAULT '',
                body TEXT NOT NULL DEFAULT '',
                next_attempt_at TEXT NOT NULL,
                attempt_count INTEGER NOT NULL DEFAULT 0,
                max_attempts INTEGER NOT NULL DEFAULT 5,
                last_error TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                expires_at TEXT NOT NULL DEFAULT ''
            )",
            [],
        )?;
        if !table_has_column(&conn, "notification_queue", "line_id")? {
            conn.execute("ALTER TABLE notification_queue ADD COLUMN line_id TEXT", [])?;
        }
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_notification_queue_status_next_attempt
             ON notification_queue(status, next_attempt_at, id)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_notification_queue_channel_status
             ON notification_queue(channel_id, status, next_attempt_at)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_notification_queue_line_status
             ON notification_queue(line_id, status, next_attempt_at)",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS call_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                line_id TEXT,
                direction TEXT NOT NULL,
                phone_number TEXT NOT NULL,
                duration INTEGER DEFAULT 0,
                start_time TEXT NOT NULL,
                end_time TEXT,
                answered INTEGER DEFAULT 0,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;

        if !table_has_column(&conn, "call_history", "line_id")? {
            conn.execute("ALTER TABLE call_history ADD COLUMN line_id TEXT", [])?;
        }

        // 创建通话记录索引
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_call_start_time ON call_history(start_time DESC)",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_call_phone ON call_history(phone_number)",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_call_line_start_time
             ON call_history(line_id, start_time DESC)",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS smsc_cache (
                identity_key TEXT PRIMARY KEY,
                iccid TEXT,
                imsi TEXT,
                operator_id TEXT,
                sms_center TEXT NOT NULL,
                source TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS own_number_cache (
                identity_key TEXT PRIMARY KEY,
                iccid TEXT,
                imsi TEXT,
                operator_id TEXT,
                phone_numbers TEXT NOT NULL,
                source TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS esim_profile_cache (
                iccid TEXT PRIMARY KEY,
                name TEXT,
                provider TEXT,
                profile_class TEXT,
                imsi TEXT,
                msisdn TEXT,
                smsc TEXT,
                smdp TEXT,
                isdp_aid TEXT,
                mcc TEXT,
                mnc TEXT,
                updated_at TEXT NOT NULL
            )",
            [],
        )?;

        if !table_has_column(&conn, "esim_profile_cache", "matching_id")? {
            conn.execute(
                "ALTER TABLE esim_profile_cache ADD COLUMN matching_id TEXT",
                [],
            )?;
        }

        conn.execute(
            "CREATE TABLE IF NOT EXISTS auth_config (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS auth_sessions (
                session_hash TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_auth_sessions_expires_at ON auth_sessions(expires_at)",
            [],
        )?;

        // 创建自动化运行日志表
        conn.execute(
            "CREATE TABLE IF NOT EXISTS automation_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                line_id TEXT,
                task_id TEXT NOT NULL,
                task_name TEXT NOT NULL,
                task_type TEXT NOT NULL,
                status TEXT NOT NULL,
                detail TEXT NOT NULL,
                created_at TEXT NOT NULL
            )",
            [],
        )?;
        if !table_has_column(&conn, "automation_logs", "line_id")? {
            conn.execute("ALTER TABLE automation_logs ADD COLUMN line_id TEXT", [])?;
        }

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_automation_logs_created_at ON automation_logs(created_at DESC)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_automation_logs_line_created_at
             ON automation_logs(line_id, created_at DESC)",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS vowifi_runtime_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                line_id TEXT,
                trace_id TEXT,
                level TEXT NOT NULL,
                phase TEXT NOT NULL,
                profile_id TEXT,
                event_type TEXT NOT NULL,
                detail_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            )",
            [],
        )?;
        if !table_has_column(&conn, "vowifi_runtime_events", "line_id")? {
            conn.execute(
                "ALTER TABLE vowifi_runtime_events ADD COLUMN line_id TEXT",
                [],
            )?;
        }
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_vowifi_runtime_events_created_at
             ON vowifi_runtime_events(created_at DESC, id DESC)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_vowifi_runtime_events_trace
             ON vowifi_runtime_events(trace_id, id DESC)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_vowifi_runtime_events_line_created
             ON vowifi_runtime_events(line_id, created_at DESC, id DESC)",
            [],
        )?;

        if table_has_column(&conn, "vowifi_runtime_snapshots", "singleton_key")?
            && !table_has_column(&conn, "vowifi_runtime_snapshots", "line_id")?
        {
            conn.execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE vowifi_runtime_snapshots
                   RENAME TO vowifi_runtime_snapshots_legacy;
                 CREATE TABLE vowifi_runtime_snapshots (
                    line_id TEXT PRIMARY KEY,
                    phase TEXT NOT NULL,
                    profile_id TEXT,
                    plmn TEXT,
                    identity_ready INTEGER NOT NULL,
                    sim_auth_ready INTEGER NOT NULL,
                    profile_matched INTEGER NOT NULL,
                    epdg_ready INTEGER NOT NULL,
                    ike_ready INTEGER NOT NULL,
                    child_sa_ready INTEGER NOT NULL,
                    esp_ready INTEGER NOT NULL,
                    ims_registered INTEGER NOT NULL,
                    sms_ready INTEGER NOT NULL,
                    degraded_reason TEXT,
                    updated_at TEXT NOT NULL
                 );
                 INSERT INTO vowifi_runtime_snapshots (
                    line_id, phase, profile_id, plmn,
                    identity_ready, sim_auth_ready, profile_matched,
                    epdg_ready, ike_ready, child_sa_ready, esp_ready,
                    ims_registered, sms_ready, degraded_reason, updated_at
                 )
                 SELECT '', phase, profile_id, plmn,
                    identity_ready, sim_auth_ready, profile_matched,
                    epdg_ready, ike_ready, child_sa_ready, esp_ready,
                    ims_registered, sms_ready, degraded_reason, updated_at
                 FROM vowifi_runtime_snapshots_legacy
                 WHERE singleton_key = 1;
                 DROP TABLE vowifi_runtime_snapshots_legacy;
                 COMMIT;",
            )?;
        }
        conn.execute(
            "CREATE TABLE IF NOT EXISTS vowifi_runtime_snapshots (
                line_id TEXT PRIMARY KEY,
                phase TEXT NOT NULL,
                profile_id TEXT,
                plmn TEXT,
                identity_ready INTEGER NOT NULL,
                sim_auth_ready INTEGER NOT NULL,
                profile_matched INTEGER NOT NULL,
                epdg_ready INTEGER NOT NULL,
                ike_ready INTEGER NOT NULL,
                child_sa_ready INTEGER NOT NULL,
                esp_ready INTEGER NOT NULL,
                ims_registered INTEGER NOT NULL,
                sms_ready INTEGER NOT NULL,
                degraded_reason TEXT,
                updated_at TEXT NOT NULL
            )",
            [],
        )?;

        // Cumulative proxied traffic per line, so the web page can answer "has
        // this SIM used any data" across restarts. Keyed by line_id because
        // every SIM has its own proxy and its own quota.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS line_data_traffic (
                line_id TEXT PRIMARY KEY,
                uplink_bytes INTEGER NOT NULL DEFAULT 0,
                downlink_bytes INTEGER NOT NULL DEFAULT 0,
                total_connections INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS vowifi_sms_delivery (
                line_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                trace_id TEXT NOT NULL,
                direction TEXT NOT NULL,
                state TEXT NOT NULL,
                sip_state TEXT NOT NULL,
                rpdu_ack TEXT NOT NULL,
                delivery_reported INTEGER NOT NULL,
                failure_cause TEXT,
                retry_count INTEGER NOT NULL DEFAULT 0,
                api_sms_id INTEGER,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(line_id, message_id),
                FOREIGN KEY(api_sms_id) REFERENCES sms_messages(id) ON DELETE SET NULL
            )",
            [],
        )?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS vowifi_sms_parts (
                line_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                reference INTEGER NOT NULL,
                sequence INTEGER NOT NULL,
                total INTEGER NOT NULL,
                received INTEGER NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(line_id, message_id, reference, sequence),
                FOREIGN KEY(line_id, message_id)
                    REFERENCES vowifi_sms_delivery(line_id, message_id)
            )",
            [],
        )?;
        if !table_has_column(&conn, "vowifi_sms_delivery", "line_id")? {
            conn.execute(
                "ALTER TABLE vowifi_sms_delivery ADD COLUMN line_id TEXT",
                [],
            )?;
        }
        migrate_vowifi_sms_tables_for_line_scope(&conn)?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_vowifi_sms_delivery_trace
             ON vowifi_sms_delivery(trace_id)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_vowifi_sms_delivery_updated
             ON vowifi_sms_delivery(updated_at DESC)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_vowifi_sms_delivery_line_updated
             ON vowifi_sms_delivery(line_id, updated_at DESC)",
            [],
        )?;

        if table_has_column(&conn, "vowifi_esim_restore", "singleton_key")?
            && !table_has_column(&conn, "vowifi_esim_restore", "line_id")?
        {
            conn.execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE vowifi_esim_restore RENAME TO vowifi_esim_restore_legacy;
                 CREATE TABLE vowifi_esim_restore (
                    line_id TEXT PRIMARY KEY,
                    switch_token TEXT,
                    switch_phase TEXT,
                    phase_ms INTEGER,
                    identity_ready INTEGER NOT NULL,
                    sim_auth_ready INTEGER NOT NULL,
                    degraded_reason TEXT,
                    retry_count INTEGER NOT NULL DEFAULT 0,
                    updated_at TEXT NOT NULL
                 );
                 INSERT INTO vowifi_esim_restore (
                    line_id, switch_token, switch_phase, phase_ms,
                    identity_ready, sim_auth_ready, degraded_reason, retry_count, updated_at
                 )
                 SELECT '', switch_token, switch_phase, phase_ms,
                    identity_ready, sim_auth_ready, degraded_reason, retry_count, updated_at
                 FROM vowifi_esim_restore_legacy
                 WHERE singleton_key = 1;
                 DROP TABLE vowifi_esim_restore_legacy;
                 COMMIT;",
            )?;
        }
        conn.execute(
            "CREATE TABLE IF NOT EXISTS vowifi_esim_restore (
                line_id TEXT PRIMARY KEY,
                switch_token TEXT,
                switch_phase TEXT,
                phase_ms INTEGER,
                identity_ready INTEGER NOT NULL,
                sim_auth_ready INTEGER NOT NULL,
                degraded_reason TEXT,
                retry_count INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS vowifi_soak_runs (
                line_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                scenario_id TEXT NOT NULL,
                profile_id TEXT,
                plmn TEXT,
                status TEXT NOT NULL,
                started_at TEXT NOT NULL,
                finished_at TEXT,
                duration_seconds INTEGER NOT NULL DEFAULT 0,
                sample_count INTEGER NOT NULL DEFAULT 0,
                failure_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                PRIMARY KEY(line_id, run_id)
            )",
            [],
        )?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS vowifi_soak_samples (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                line_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                sample_kind TEXT NOT NULL,
                metric_name TEXT NOT NULL,
                metric_value INTEGER NOT NULL,
                state TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY(line_id, run_id) REFERENCES vowifi_soak_runs(line_id, run_id)
            )",
            [],
        )?;
        if !table_has_column(&conn, "vowifi_soak_runs", "line_id")? {
            conn.execute("ALTER TABLE vowifi_soak_runs ADD COLUMN line_id TEXT", [])?;
        }
        migrate_vowifi_soak_tables_for_line_scope(&conn)?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_vowifi_soak_runs_started
             ON vowifi_soak_runs(started_at DESC, run_id DESC)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_vowifi_soak_runs_scenario
             ON vowifi_soak_runs(scenario_id, started_at DESC)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_vowifi_soak_runs_line_started
             ON vowifi_soak_runs(line_id, started_at DESC, run_id DESC)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_vowifi_soak_samples_run
             ON vowifi_soak_samples(line_id, run_id, id DESC)",
            [],
        )?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    // ==================== 认证相关方法 ====================

    pub fn auth_is_configured(&self) -> Result<bool> {
        Ok(self.get_auth_config_value("admin_password_hash")?.is_some())
    }

    pub fn get_auth_config_value(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM auth_config WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
    }

    pub fn set_auth_config_value(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().timestamp();
        conn.execute(
            "INSERT INTO auth_config (key, value, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET
                value = excluded.value,
                updated_at = excluded.updated_at",
            params![key, value, now],
        )?;
        Ok(())
    }

    pub fn replace_admin_password_hash(&self, password_hash: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let now = Utc::now().timestamp();
        tx.execute(
            "INSERT INTO auth_config (key, value, updated_at)
             VALUES ('admin_password_hash', ?1, ?2)
             ON CONFLICT(key) DO UPDATE SET
                value = excluded.value,
                updated_at = excluded.updated_at",
            params![password_hash, now],
        )?;
        tx.execute("DELETE FROM auth_sessions", [])?;
        tx.commit()?;
        Ok(())
    }

    pub fn clear_admin_auth(&self) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM auth_config WHERE key = 'admin_password_hash'",
            [],
        )?;
        tx.execute("DELETE FROM auth_sessions", [])?;
        tx.commit()?;
        Ok(())
    }

    pub fn insert_auth_session(&self, session_hash: &str, ttl_seconds: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().timestamp();
        conn.execute(
            "INSERT INTO auth_sessions (session_hash, created_at, expires_at)
             VALUES (?1, ?2, ?3)",
            params![session_hash, now, now + ttl_seconds],
        )?;
        conn.execute(
            "DELETE FROM auth_sessions WHERE expires_at <= ?1",
            params![now],
        )?;
        Ok(())
    }

    pub fn auth_session_valid(&self, session_hash: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().timestamp();
        conn.execute(
            "DELETE FROM auth_sessions WHERE expires_at <= ?1",
            params![now],
        )?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM auth_sessions
             WHERE session_hash = ?1 AND expires_at > ?2",
            params![session_hash, now],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn delete_auth_session(&self, session_hash: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM auth_sessions WHERE session_hash = ?1",
            params![session_hash],
        )?;
        Ok(())
    }

    pub fn refresh_auth_session(&self, session_hash: &str, ttl_seconds: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().timestamp();
        conn.execute(
            "UPDATE auth_sessions SET expires_at = ?1 WHERE session_hash = ?2",
            params![now + ttl_seconds, session_hash],
        )?;
        Ok(())
    }

    // ==================== 短信相关方法 ====================

    /// 插入新短信
    pub fn insert_vowifi_runtime_event(&self, event: NewVowifiRuntimeEvent<'_>) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let created_at = beijing_sms_now_string();
        let line_id = required_line_id(event.line_id)?;
        conn.execute(
            "INSERT INTO vowifi_runtime_events (
                line_id, trace_id, level, phase, profile_id, event_type, detail_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                line_id,
                event.trace_id,
                event.level,
                event.phase,
                event.profile_id,
                event.event_type,
                event.detail_json,
                created_at,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_vowifi_runtime_events_for_line(
        &self,
        line_id: &str,
        limit: i64,
        offset: i64,
        trace_id: Option<&str>,
    ) -> Result<VowifiRuntimeEventsResponse> {
        self.get_vowifi_runtime_events_scoped(limit, offset, Some(line_id), trace_id)
    }

    fn get_vowifi_runtime_events_scoped(
        &self,
        limit: i64,
        offset: i64,
        line_id: Option<&str>,
        trace_id: Option<&str>,
    ) -> Result<VowifiRuntimeEventsResponse> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 200);
        let offset = offset.max(0);
        let line_id = line_id.map(str::trim).filter(|value| !value.is_empty());
        let trace_id = trace_id.map(str::trim).filter(|value| !value.is_empty());

        let total = conn.query_row(
            "SELECT COUNT(*) FROM vowifi_runtime_events
             WHERE (?1 IS NULL OR line_id = ?1)
               AND (?2 IS NULL OR trace_id = ?2)",
            params![line_id, trace_id],
            |row| row.get(0),
        )?;

        let mut events = Vec::new();
        let mut stmt = conn.prepare(
            "SELECT id, line_id, trace_id, level, phase, profile_id, event_type,
                    detail_json, created_at
             FROM vowifi_runtime_events
             WHERE (?1 IS NULL OR line_id = ?1)
               AND (?2 IS NULL OR trace_id = ?2)
             ORDER BY id DESC
             LIMIT ?3 OFFSET ?4",
        )?;
        let rows = stmt.query_map(
            params![line_id, trace_id, limit, offset],
            vowifi_runtime_event_from_row,
        )?;
        for row in rows {
            events.push(row?);
        }

        Ok(VowifiRuntimeEventsResponse { events, total })
    }

    /// Cumulative proxied traffic for one line. Absent rows read as zero, which
    /// is what a line that has never carried traffic should report.
    pub fn get_line_data_traffic(&self, line_id: &str) -> Result<LineDataTrafficEntry> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT uplink_bytes, downlink_bytes, total_connections
             FROM line_data_traffic WHERE line_id = ?1",
            params![line_id],
            |row| {
                Ok(LineDataTrafficEntry {
                    uplink_bytes: row.get::<_, i64>(0)?.max(0) as u64,
                    downlink_bytes: row.get::<_, i64>(1)?.max(0) as u64,
                    total_connections: row.get::<_, i64>(2)?.max(0) as u64,
                })
            },
        )
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(LineDataTrafficEntry::default()),
            other => Err(other),
        })
    }

    /// Store one line's cumulative traffic. Callers pass absolute totals (the
    /// persisted baseline plus what this process has carried), so a crash
    /// between flushes loses only the traffic since the last flush rather than
    /// double-counting it.
    pub fn set_line_data_traffic(
        &self,
        line_id: &str,
        traffic: &LineDataTrafficEntry,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO line_data_traffic (
                line_id, uplink_bytes, downlink_bytes, total_connections, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(line_id) DO UPDATE SET
                uplink_bytes = excluded.uplink_bytes,
                downlink_bytes = excluded.downlink_bytes,
                total_connections = excluded.total_connections,
                updated_at = excluded.updated_at",
            params![
                line_id,
                traffic.uplink_bytes as i64,
                traffic.downlink_bytes as i64,
                traffic.total_connections as i64,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn clear_line_data_traffic(&self, line_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM line_data_traffic WHERE line_id = ?1",
            params![line_id],
        )?;
        Ok(())
    }

    pub fn upsert_vowifi_runtime_snapshot(
        &self,
        snapshot: NewVowifiRuntimeSnapshot<'_>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let updated_at = beijing_sms_now_string();
        let line_id = required_line_id(snapshot.line_id)?;
        conn.execute(
            "INSERT INTO vowifi_runtime_snapshots (
                line_id, phase, profile_id, plmn,
                identity_ready, sim_auth_ready, profile_matched,
                epdg_ready, ike_ready, child_sa_ready, esp_ready,
                ims_registered, sms_ready, degraded_reason, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(line_id) DO UPDATE SET
                phase = excluded.phase,
                profile_id = excluded.profile_id,
                plmn = excluded.plmn,
                identity_ready = excluded.identity_ready,
                sim_auth_ready = excluded.sim_auth_ready,
                profile_matched = excluded.profile_matched,
                epdg_ready = excluded.epdg_ready,
                ike_ready = excluded.ike_ready,
                child_sa_ready = excluded.child_sa_ready,
                esp_ready = excluded.esp_ready,
                ims_registered = excluded.ims_registered,
                sms_ready = excluded.sms_ready,
                degraded_reason = excluded.degraded_reason,
                updated_at = excluded.updated_at",
            params![
                line_id,
                snapshot.phase,
                snapshot.profile_id,
                snapshot.plmn,
                snapshot.identity_ready,
                snapshot.sim_auth_ready,
                snapshot.profile_matched,
                snapshot.epdg_ready,
                snapshot.ike_ready,
                snapshot.child_sa_ready,
                snapshot.esp_ready,
                snapshot.ims_registered,
                snapshot.sms_ready,
                snapshot.degraded_reason,
                updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_vowifi_runtime_snapshot_for_line(
        &self,
        line_id: &str,
    ) -> Result<Option<VowifiRuntimeSnapshotEntry>> {
        self.get_vowifi_runtime_snapshot_scoped(Some(line_id))
    }

    fn get_vowifi_runtime_snapshot_scoped(
        &self,
        line_id: Option<&str>,
    ) -> Result<Option<VowifiRuntimeSnapshotEntry>> {
        let conn = self.conn.lock().unwrap();
        let line_id = line_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("");
        conn.query_row(
            "SELECT NULLIF(line_id, ''), phase, profile_id, plmn,
                    identity_ready, sim_auth_ready, profile_matched,
                    epdg_ready, ike_ready, child_sa_ready, esp_ready,
                    ims_registered, sms_ready, degraded_reason, updated_at
             FROM vowifi_runtime_snapshots
             WHERE line_id = ?1",
            params![line_id],
            |row| {
                Ok(VowifiRuntimeSnapshotEntry {
                    line_id: row.get(0)?,
                    phase: row.get(1)?,
                    profile_id: row.get(2)?,
                    plmn: row.get(3)?,
                    identity_ready: row.get::<_, i64>(4)? != 0,
                    sim_auth_ready: row.get::<_, i64>(5)? != 0,
                    profile_matched: row.get::<_, i64>(6)? != 0,
                    epdg_ready: row.get::<_, i64>(7)? != 0,
                    ike_ready: row.get::<_, i64>(8)? != 0,
                    child_sa_ready: row.get::<_, i64>(9)? != 0,
                    esp_ready: row.get::<_, i64>(10)? != 0,
                    ims_registered: row.get::<_, i64>(11)? != 0,
                    sms_ready: row.get::<_, i64>(12)? != 0,
                    degraded_reason: row.get(13)?,
                    updated_at: row.get(14)?,
                })
            },
        )
        .optional()
    }

    pub fn upsert_vowifi_sms_delivery(&self, delivery: NewVowifiSmsDelivery<'_>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = beijing_sms_now_string();
        let line_id = required_line_id(delivery.line_id)?;
        conn.execute(
            "INSERT INTO vowifi_sms_delivery (
                line_id, message_id, trace_id, direction, state, sip_state, rpdu_ack,
                delivery_reported, failure_cause, retry_count, api_sms_id,
                created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)
             ON CONFLICT(line_id, message_id) DO UPDATE SET
                trace_id = excluded.trace_id,
                direction = excluded.direction,
                state = excluded.state,
                sip_state = excluded.sip_state,
                rpdu_ack = excluded.rpdu_ack,
                delivery_reported = excluded.delivery_reported,
                failure_cause = excluded.failure_cause,
                retry_count = excluded.retry_count,
                api_sms_id = excluded.api_sms_id,
                updated_at = excluded.updated_at",
            params![
                line_id,
                delivery.message_id,
                delivery.trace_id,
                delivery.direction,
                delivery.state,
                delivery.sip_state,
                delivery.rpdu_ack,
                delivery.delivery_reported,
                delivery.failure_cause,
                delivery.retry_count,
                delivery.api_sms_id,
                now,
            ],
        )?;
        Ok(())
    }

    pub fn upsert_vowifi_sms_part(&self, part: NewVowifiSmsPart<'_>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let updated_at = beijing_sms_now_string();
        let line_id = required_line_id(part.line_id)?;
        conn.execute(
            "INSERT INTO vowifi_sms_parts (
                line_id, message_id, reference, sequence, total, received, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(line_id, message_id, reference, sequence) DO UPDATE SET
                total = excluded.total,
                received = excluded.received,
                updated_at = excluded.updated_at",
            params![
                line_id,
                part.message_id,
                part.reference,
                part.sequence,
                part.total,
                part.received,
                updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_vowifi_sms_deliveries_for_line(
        &self,
        line_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<VowifiSmsDeliveriesResponse> {
        self.get_vowifi_sms_deliveries_scoped(limit, offset, Some(line_id))
    }

    fn get_vowifi_sms_deliveries_scoped(
        &self,
        limit: i64,
        offset: i64,
        line_id: Option<&str>,
    ) -> Result<VowifiSmsDeliveriesResponse> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 200);
        let offset = offset.max(0);
        let line_id = line_id.map(str::trim).filter(|value| !value.is_empty());

        let total = conn.query_row(
            "SELECT COUNT(*) FROM vowifi_sms_delivery
             WHERE (?1 IS NULL OR line_id = ?1)",
            params![line_id],
            |row| row.get(0),
        )?;
        let mut stmt = conn.prepare(
            "SELECT message_id, NULLIF(line_id, ''), trace_id, direction, state, sip_state, rpdu_ack,
                    delivery_reported, failure_cause, retry_count, api_sms_id,
                    created_at, updated_at
             FROM vowifi_sms_delivery
             WHERE (?1 IS NULL OR line_id = ?1)
             ORDER BY updated_at DESC, message_id DESC
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![line_id, limit, offset], |row| {
            Ok(VowifiSmsDeliveryEntry {
                message_id: row.get(0)?,
                line_id: row.get(1)?,
                trace_id: row.get(2)?,
                direction: row.get(3)?,
                state: row.get(4)?,
                sip_state: row.get(5)?,
                rpdu_ack: row.get(6)?,
                delivery_reported: row.get::<_, i64>(7)? != 0,
                failure_cause: row.get(8)?,
                retry_count: row.get(9)?,
                api_sms_id: row.get(10)?,
                parts: Vec::new(),
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            })
        })?;

        let mut deliveries = Vec::new();
        for row in rows {
            deliveries.push(row?);
        }
        drop(stmt);

        for delivery in &mut deliveries {
            delivery.parts = Self::vowifi_sms_parts_for_conn(
                &conn,
                delivery.line_id.as_deref().unwrap_or(""),
                &delivery.message_id,
            )?;
        }

        Ok(VowifiSmsDeliveriesResponse { deliveries, total })
    }

    pub fn get_vowifi_sms_delivery_for_line(
        &self,
        line_id: &str,
        message_id: &str,
    ) -> Result<Option<VowifiSmsDeliveryEntry>> {
        self.get_vowifi_sms_delivery_scoped(message_id, Some(line_id))
    }

    fn get_vowifi_sms_delivery_scoped(
        &self,
        message_id: &str,
        line_id: Option<&str>,
    ) -> Result<Option<VowifiSmsDeliveryEntry>> {
        let conn = self.conn.lock().unwrap();
        let line_id = line_id.map(str::trim).filter(|value| !value.is_empty());
        let mut delivery = conn
            .query_row(
                "SELECT message_id, NULLIF(line_id, ''), trace_id, direction, state, sip_state, rpdu_ack,
                        delivery_reported, failure_cause, retry_count, api_sms_id,
                        created_at, updated_at
                 FROM vowifi_sms_delivery
                 WHERE message_id = ?1
                   AND (?2 IS NULL OR line_id = ?2)",
                params![message_id, line_id],
                |row| {
                    Ok(VowifiSmsDeliveryEntry {
                        message_id: row.get(0)?,
                        line_id: row.get(1)?,
                        trace_id: row.get(2)?,
                        direction: row.get(3)?,
                        state: row.get(4)?,
                        sip_state: row.get(5)?,
                        rpdu_ack: row.get(6)?,
                        delivery_reported: row.get::<_, i64>(7)? != 0,
                        failure_cause: row.get(8)?,
                        retry_count: row.get(9)?,
                        api_sms_id: row.get(10)?,
                        parts: Vec::new(),
                        created_at: row.get(11)?,
                        updated_at: row.get(12)?,
                    })
                },
            )
            .optional()?;

        if let Some(delivery) = delivery.as_mut() {
            delivery.parts = Self::vowifi_sms_parts_for_conn(
                &conn,
                delivery.line_id.as_deref().unwrap_or(""),
                &delivery.message_id,
            )?;
        }

        Ok(delivery)
    }

    fn vowifi_sms_parts_for_conn(
        conn: &Connection,
        line_id: &str,
        message_id: &str,
    ) -> Result<Vec<VowifiSmsPartEntry>> {
        let mut stmt = conn.prepare(
            "SELECT message_id, reference, sequence, total, received, updated_at
             FROM vowifi_sms_parts
             WHERE line_id = ?1 AND message_id = ?2
             ORDER BY reference ASC, sequence ASC",
        )?;
        let rows = stmt.query_map(params![line_id, message_id], |row| {
            Ok(VowifiSmsPartEntry {
                message_id: row.get(0)?,
                reference: row.get(1)?,
                sequence: row.get(2)?,
                total: row.get(3)?,
                received: row.get::<_, i64>(4)? != 0,
                updated_at: row.get(5)?,
            })
        })?;

        let mut parts = Vec::new();
        for row in rows {
            parts.push(row?);
        }
        Ok(parts)
    }

    pub fn upsert_vowifi_esim_restore(&self, restore: NewVowifiEsimRestore<'_>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let updated_at = beijing_sms_now_string();
        let line_id = required_line_id(restore.line_id)?;
        conn.execute(
            "INSERT INTO vowifi_esim_restore (
                line_id, switch_token, switch_phase, phase_ms,
                identity_ready, sim_auth_ready, degraded_reason, retry_count, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(line_id) DO UPDATE SET
                switch_token = excluded.switch_token,
                switch_phase = excluded.switch_phase,
                phase_ms = excluded.phase_ms,
                identity_ready = excluded.identity_ready,
                sim_auth_ready = excluded.sim_auth_ready,
                degraded_reason = excluded.degraded_reason,
                retry_count = excluded.retry_count,
                updated_at = excluded.updated_at",
            params![
                line_id,
                restore.switch_token,
                restore.switch_phase,
                restore.phase_ms,
                restore.identity_ready,
                restore.sim_auth_ready,
                restore.degraded_reason,
                restore.retry_count,
                updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_vowifi_esim_restore_for_line(
        &self,
        line_id: &str,
    ) -> Result<Option<VowifiEsimRestoreEntry>> {
        self.get_vowifi_esim_restore_scoped(Some(line_id))
    }

    fn get_vowifi_esim_restore_scoped(
        &self,
        line_id: Option<&str>,
    ) -> Result<Option<VowifiEsimRestoreEntry>> {
        let conn = self.conn.lock().unwrap();
        let line_id = line_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("");
        conn.query_row(
            "SELECT NULLIF(line_id, ''), switch_token, switch_phase, phase_ms,
                    identity_ready, sim_auth_ready, degraded_reason, retry_count, updated_at
             FROM vowifi_esim_restore
             WHERE line_id = ?1",
            params![line_id],
            |row| {
                Ok(VowifiEsimRestoreEntry {
                    line_id: row.get(0)?,
                    switch_token: row.get(1)?,
                    switch_phase: row.get(2)?,
                    phase_ms: row.get(3)?,
                    identity_ready: row.get::<_, i64>(4)? != 0,
                    sim_auth_ready: row.get::<_, i64>(5)? != 0,
                    degraded_reason: row.get(6)?,
                    retry_count: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            },
        )
        .optional()
    }

    pub fn upsert_vowifi_soak_run(&self, run: NewVowifiSoakRun<'_>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = beijing_sms_now_string();
        let line_id = required_line_id(run.line_id)?;
        let finished_at =
            matches!(run.status, "passed" | "failed" | "aborted").then_some(now.as_str());
        conn.execute(
            "INSERT INTO vowifi_soak_runs (
                line_id, run_id, scenario_id, profile_id, plmn, status, started_at, finished_at,
                duration_seconds, sample_count, failure_count, last_error
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(line_id, run_id) DO UPDATE SET
                scenario_id = excluded.scenario_id,
                profile_id = excluded.profile_id,
                plmn = excluded.plmn,
                status = excluded.status,
                finished_at = excluded.finished_at,
                duration_seconds = excluded.duration_seconds,
                sample_count = excluded.sample_count,
                failure_count = excluded.failure_count,
                last_error = excluded.last_error",
            params![
                line_id,
                run.run_id,
                run.scenario_id,
                run.profile_id,
                run.plmn,
                run.status,
                now,
                finished_at,
                run.duration_seconds,
                run.sample_count,
                run.failure_count,
                run.last_error,
            ],
        )?;
        Ok(())
    }

    pub fn insert_vowifi_soak_sample(&self, sample: NewVowifiSoakSample<'_>) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let created_at = beijing_sms_now_string();
        let line_id = required_line_id(sample.line_id)?;
        conn.execute(
            "INSERT INTO vowifi_soak_samples (
                line_id, run_id, sample_kind, metric_name, metric_value, state, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                line_id,
                sample.run_id,
                sample.sample_kind,
                sample.metric_name,
                sample.metric_value,
                sample.state,
                created_at,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_vowifi_soak_runs_for_line(
        &self,
        line_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<VowifiSoakRunsResponse> {
        self.get_vowifi_soak_runs_scoped(limit, offset, Some(line_id))
    }

    fn get_vowifi_soak_runs_scoped(
        &self,
        limit: i64,
        offset: i64,
        line_id: Option<&str>,
    ) -> Result<VowifiSoakRunsResponse> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 100);
        let offset = offset.max(0);
        let line_id = line_id.map(str::trim).filter(|value| !value.is_empty());

        let total = conn.query_row(
            "SELECT COUNT(*) FROM vowifi_soak_runs
             WHERE (?1 IS NULL OR line_id = ?1)",
            params![line_id],
            |row| row.get(0),
        )?;
        let mut stmt = conn.prepare(
            "SELECT run_id, NULLIF(line_id, ''), scenario_id, profile_id, plmn, status, started_at, finished_at,
                    duration_seconds, sample_count, failure_count, last_error
             FROM vowifi_soak_runs
             WHERE (?1 IS NULL OR line_id = ?1)
             ORDER BY started_at DESC, run_id DESC
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![line_id, limit, offset], vowifi_soak_run_from_row)?;

        let mut runs = Vec::new();
        for row in rows {
            runs.push(row?);
        }
        drop(stmt);

        for run in &mut runs {
            run.samples = Self::vowifi_soak_samples_for_conn(
                &conn,
                run.line_id.as_deref().unwrap_or(""),
                &run.run_id,
            )?;
        }

        Ok(VowifiSoakRunsResponse {
            runs,
            total,
            read_only: true,
        })
    }

    fn vowifi_soak_samples_for_conn(
        conn: &Connection,
        line_id: &str,
        run_id: &str,
    ) -> Result<Vec<VowifiSoakSampleEntry>> {
        let mut stmt = conn.prepare(
            "SELECT id, run_id, sample_kind, metric_name, metric_value, state, created_at
             FROM vowifi_soak_samples
             WHERE line_id = ?1 AND run_id = ?2
             ORDER BY id DESC
             LIMIT 50",
        )?;
        let rows = stmt.query_map(params![line_id, run_id], vowifi_soak_sample_from_row)?;

        let mut samples = Vec::new();
        for row in rows {
            samples.push(row?);
        }
        Ok(samples)
    }

    pub fn insert_sms(
        &self,
        direction: &str,
        phone_number: &str,
        content: &str,
        status: &str,
        pdu: Option<&str>,
    ) -> Result<i64> {
        let timestamp = beijing_sms_now_string();
        self.insert_sms_at(direction, phone_number, content, &timestamp, status, pdu)
    }

    pub fn insert_sms_with_transport(
        &self,
        direction: &str,
        phone_number: &str,
        content: &str,
        status: &str,
        pdu: Option<&str>,
        transport: &str,
    ) -> Result<i64> {
        let timestamp = beijing_sms_now_string();
        self.insert_sms_at_with_transport_for_line(
            direction,
            phone_number,
            content,
            &timestamp,
            status,
            pdu,
            transport,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_sms_with_transport_for_line(
        &self,
        direction: &str,
        phone_number: &str,
        content: &str,
        status: &str,
        pdu: Option<&str>,
        transport: &str,
        line_id: Option<&str>,
    ) -> Result<i64> {
        let timestamp = beijing_sms_now_string();
        self.insert_sms_at_with_transport_for_line(
            direction,
            phone_number,
            content,
            &timestamp,
            status,
            pdu,
            transport,
            line_id,
        )
    }

    pub fn insert_sms_at(
        &self,
        direction: &str,
        phone_number: &str,
        content: &str,
        timestamp: &str,
        status: &str,
        pdu: Option<&str>,
    ) -> Result<i64> {
        self.insert_sms_at_with_transport_for_line(
            direction,
            phone_number,
            content,
            timestamp,
            status,
            pdu,
            "modem",
            None,
        )
    }

    // SQL boundary: parameters deliberately mirror the persisted SMS columns.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_sms_at_with_transport(
        &self,
        direction: &str,
        phone_number: &str,
        content: &str,
        timestamp: &str,
        status: &str,
        pdu: Option<&str>,
        transport: &str,
    ) -> Result<i64> {
        self.insert_sms_at_with_transport_for_line(
            direction,
            phone_number,
            content,
            timestamp,
            status,
            pdu,
            transport,
            None,
        )
    }

    // SQL boundary: parameters deliberately mirror the persisted SMS columns.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_sms_at_with_transport_for_line(
        &self,
        direction: &str,
        phone_number: &str,
        content: &str,
        timestamp: &str,
        status: &str,
        pdu: Option<&str>,
        transport: &str,
        line_id: Option<&str>,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let timestamp = sms_timestamp_for_storage(timestamp);
        let transport = normalized_sms_transport(transport);
        conn.execute(
            "INSERT INTO sms_messages (direction, phone_number, content, timestamp, status, pdu, transport, line_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                direction,
                phone_number,
                content,
                timestamp,
                status,
                pdu,
                transport,
                line_id.filter(|value| !value.trim().is_empty())
            ],
        )?;

        Ok(conn.last_insert_rowid())
    }

    /// Check whether an SMS marker has already been stored on one line.
    pub fn sms_exists_by_pdu_for_line(&self, line_id: &str, pdu: &str) -> Result<bool> {
        let line_id = required_line_id(line_id)?;
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sms_messages WHERE line_id = ?1 AND pdu = ?2",
            params![line_id, pdu],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn sms_id_by_pdu_for_line(&self, line_id: &str, pdu: &str) -> Result<Option<i64>> {
        let line_id = required_line_id(line_id)?;
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id FROM sms_messages
             WHERE line_id = ?1 AND pdu = ?2
             ORDER BY id DESC LIMIT 1",
            params![line_id, pdu],
            |row| row.get(0),
        )
        .optional()
    }

    // ================= Phase C: cross-transport SMS dedup =================
    //
    // The orchestrator can receive the same MT SMS on more than one leg (e.g.
    // an IMS leg and the CS listener both surface it). To make sure a message
    // lands in `sms_messages` exactly once regardless of which transport won
    // the race, we record a content fingerprint in a dedicated `sms_dedup`
    // table and consult it *before* inserting. This table is bounded: rows are
    // pruned by `cleanup_sms_dedup` on a retention window so the DB does not
    // grow without limit over long uptimes.

    /// Atomically claim a line-local dedup fingerprint. Returns `true` if this is the
    /// first time we've seen `fingerprint` (caller should proceed to store the
    /// SMS), or `false` if it was already claimed (caller should drop it as a
    /// duplicate). `transport` is recorded for observability/debugging only.
    ///
    /// The claim is a single `INSERT OR IGNORE` on `(line_id, fingerprint)`, so
    /// it is race-free across access legs without coupling separate SIM lines.
    pub fn claim_sms_dedup(
        &self,
        line_id: &str,
        fingerprint: &str,
        transport: &str,
    ) -> Result<bool> {
        let line_id = required_line_id(line_id)?;
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "INSERT OR IGNORE INTO sms_dedup (line_id, fingerprint, transport, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![line_id, fingerprint, transport, beijing_sms_now_string()],
        )?;
        Ok(changed > 0)
    }

    /// Whether a dedup fingerprint has already been claimed (non-mutating).
    pub fn sms_dedup_exists(&self, line_id: &str, fingerprint: &str) -> Result<bool> {
        let line_id = required_line_id(line_id)?;
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sms_dedup WHERE line_id = ?1 AND fingerprint = ?2",
            params![line_id, fingerprint],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Prune dedup fingerprints older than `retention_days`. Returns the number
    /// of rows deleted. A retention of 0 is treated as "keep nothing older than
    /// now" (still bounded). Intended to be called from a daily task.
    pub fn cleanup_sms_dedup(&self, line_id: &str, retention_days: u32) -> Result<usize> {
        let line_id = required_line_id(line_id)?;
        let conn = self.conn.lock().unwrap();
        let cutoff = Utc::now()
            .with_timezone(&beijing_offset())
            .checked_sub_signed(Duration::days(i64::from(retention_days)))
            .unwrap_or_else(|| Utc::now().with_timezone(&beijing_offset()))
            .format(SMS_TIMESTAMP_FORMAT)
            .to_string();
        let deleted = conn.execute(
            "DELETE FROM sms_dedup WHERE line_id = ?1 AND created_at < ?2",
            params![line_id, cutoff],
        )?;
        Ok(deleted)
    }

    /// Keep only the newest `max_messages` user-visible SMS rows. Diagnostic
    /// delivery rows are retained, but their API foreign-key-like reference is
    /// cleared before the corresponding SMS row is removed.
    pub fn prune_sms_messages_for_line(&self, line_id: &str, max_messages: u32) -> Result<usize> {
        let line_id = required_line_id(line_id)?;
        let max_messages = i64::from(max_messages.max(1));
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE vowifi_sms_delivery
             SET api_sms_id = NULL
             WHERE api_sms_id IN (
                 SELECT id FROM sms_messages
                 WHERE line_id = ?1
                 ORDER BY id DESC LIMIT -1 OFFSET ?2
             )",
            params![line_id, max_messages],
        )?;
        let deleted = tx.execute(
            "DELETE FROM sms_messages
             WHERE id IN (
                 SELECT id FROM sms_messages
                 WHERE line_id = ?1
                 ORDER BY id DESC LIMIT -1 OFFSET ?2
             )",
            params![line_id, max_messages],
        )?;
        tx.commit()?;
        Ok(deleted)
    }

    /// Number of live dedup fingerprints for one line (for status/metrics).
    pub fn sms_dedup_count(&self, line_id: &str) -> Result<i64> {
        let line_id = required_line_id(line_id)?;
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM sms_dedup WHERE line_id = ?1",
            params![line_id],
            |row| row.get(0),
        )
    }

    pub fn incoming_sms_exists_by_timestamp_for_line(
        &self,
        line_id: &str,
        phone_number: &str,
        content: &str,
        timestamp: &str,
    ) -> Result<bool> {
        let line_id = required_line_id(line_id)?;
        let conn = self.conn.lock().unwrap();
        let normalized_timestamp = normalize_sms_timestamp_for_display(timestamp)
            .unwrap_or_else(|| timestamp.trim().to_string());
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sms_messages
             WHERE line_id = ?1
               AND direction = 'incoming'
               AND phone_number = ?2
               AND content = ?3
               AND (timestamp = ?4 OR timestamp = ?5)",
            params![
                line_id,
                phone_number,
                content,
                timestamp,
                normalized_timestamp
            ],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn incoming_sms_exists_by_legacy_content_for_line(
        &self,
        line_id: &str,
        phone_number: &str,
        content: &str,
    ) -> Result<bool> {
        let line_id = required_line_id(line_id)?;
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sms_messages
             WHERE line_id = ?1
               AND direction = 'incoming'
               AND phone_number = ?2
               AND content = ?3
               AND (pdu IS NULL OR pdu NOT LIKE 'mmfp:%')",
            params![line_id, phone_number, content],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// 获取所有短信（分页）
    pub fn get_sms_messages(
        &self,
        limit: i64,
        offset: i64,
        direction: Option<&str>,
    ) -> Result<Vec<SmsMessage>> {
        self.get_sms_messages_for_channel(limit, offset, direction, None)
    }

    pub fn get_sms_messages_for_channel(
        &self,
        limit: i64,
        offset: i64,
        direction: Option<&str>,
        channel_id: Option<&str>,
    ) -> Result<Vec<SmsMessage>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, direction, phone_number, content, timestamp, status, pdu, transport, line_id
             FROM sms_messages
             WHERE (?1 IS NULL OR direction = ?1)
               AND (?2 IS NULL OR (?2 = 'unassigned' AND (line_id IS NULL OR TRIM(line_id) = '')) OR line_id = ?2)
             ORDER BY timestamp DESC, id DESC
             LIMIT ?3 OFFSET ?4",
        )?;
        let messages = stmt.query_map(
            params![direction, channel_id, limit, offset],
            sms_message_from_row,
        )?;
        let mut result = Vec::new();
        for message in messages {
            result.push(message?);
        }
        Ok(result)
    }

    /// 获取与特定号码的对话历史
    pub fn get_sms_conversation(&self, phone_number: &str, limit: i64) -> Result<Vec<SmsMessage>> {
        self.get_sms_conversation_for_channel(phone_number, limit, None)
    }

    pub fn get_sms_conversation_for_channel(
        &self,
        phone_number: &str,
        limit: i64,
        channel_id: Option<&str>,
    ) -> Result<Vec<SmsMessage>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, direction, phone_number, content, timestamp, status, pdu, transport, line_id
             FROM sms_messages
             WHERE phone_number = ?1
               AND (?2 IS NULL OR (?2 = 'unassigned' AND (line_id IS NULL OR TRIM(line_id) = '')) OR line_id = ?2)
             ORDER BY timestamp DESC, id DESC
             LIMIT ?3",
        )?;

        let messages = stmt.query_map(
            params![phone_number, channel_id, limit],
            sms_message_from_row,
        )?;

        let mut result = Vec::new();
        for message in messages {
            result.push(message?);
        }

        Ok(result)
    }

    /// 更新短信通知转发状态："pending", "success", "failed", "skipped"
    pub fn update_sms_notification_status(&self, id: i64, status: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sms_messages SET notification_status = ?1 WHERE id = ?2",
            params![status, id],
        )
    }

    /// 获取短信统计
    pub fn insert_notification_log(&self, log: NewNotificationLog<'_>) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO notification_logs (
                line_id, event_type, status, summary, rule_id, rule_name,
                channel_id, channel_name, message, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                log.line_id,
                log.event_type,
                log.status,
                log.summary,
                log.rule_id,
                log.rule_name,
                log.channel_id,
                log.channel_name,
                log.message,
                beijing_sms_now_string(),
            ],
        )
    }

    pub fn insert_notification_queue_item(
        &self,
        item: NewNotificationQueueItem<'_>,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let now = beijing_sms_now_string();
        conn.execute(
            "INSERT INTO notification_queue (
                line_id, status, event_type, event_label, summary, reason,
                rule_id, rule_name, channel_id, channel_name, channel_type,
                title, body, next_attempt_at, max_attempts, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16)",
            params![
                item.line_id,
                item.status,
                item.event_type,
                item.event_label,
                item.summary,
                item.reason,
                item.rule_id,
                item.rule_name,
                item.channel_id,
                item.channel_name,
                item.channel_type,
                item.title,
                item.body,
                item.next_attempt_at,
                item.max_attempts,
                now,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn notification_channel_success_count_since(
        &self,
        channel_id: &str,
        since: &str,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM notification_logs
             WHERE status = 'success'
               AND channel_id = ?1
               AND created_at >= ?2",
            params![channel_id, since],
            |row| row.get(0),
        )
    }

    // Query boundary: explicit filters keep optional/empty-string SQL semantics visible.
    #[allow(clippy::too_many_arguments)]
    pub fn get_notification_logs(
        &self,
        event_type: &str,
        status: &str,
        line_id: &str,
        query: &str,
        start_date: &str,
        end_date: &str,
        limit: i64,
        offset: i64,
    ) -> Result<NotificationLogsResponse> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 200);
        let offset = offset.max(0);
        let event_type = event_type.trim();
        let status = status.trim();
        let line_id = line_id.trim();
        let query = query.trim();
        let start_at = notification_log_start_bound(start_date);
        let end_at = notification_log_end_bound(end_date);

        let total = conn.query_row(
            "SELECT COUNT(*) FROM notification_logs
             WHERE (?1 = '' OR event_type = ?1)
               AND (?2 = '' OR status = ?2)
               AND (?3 = '' OR (?3 = 'device' AND line_id IS NULL) OR line_id = ?3)
               AND (
                    ?4 = ''
                    OR summary LIKE '%' || ?4 || '%'
                    OR rule_name LIKE '%' || ?4 || '%'
                    OR channel_name LIKE '%' || ?4 || '%'
                    OR message LIKE '%' || ?4 || '%'
               )
               AND (?5 = '' OR created_at >= ?5)
               AND (?6 = '' OR created_at <= ?6)",
            params![event_type, status, line_id, query, start_at, end_at],
            |row| row.get(0),
        )?;

        let mut stmt = conn.prepare(
            "SELECT id, line_id, event_type, status, summary, rule_id, rule_name,
                    channel_id, channel_name, message, created_at
             FROM notification_logs
             WHERE (?1 = '' OR event_type = ?1)
               AND (?2 = '' OR status = ?2)
               AND (?3 = '' OR (?3 = 'device' AND line_id IS NULL) OR line_id = ?3)
               AND (
                    ?4 = ''
                    OR summary LIKE '%' || ?4 || '%'
                    OR rule_name LIKE '%' || ?4 || '%'
                    OR channel_name LIKE '%' || ?4 || '%'
                    OR message LIKE '%' || ?4 || '%'
               )
               AND (?5 = '' OR created_at >= ?5)
               AND (?6 = '' OR created_at <= ?6)
             ORDER BY id DESC
             LIMIT ?7 OFFSET ?8",
        )?;

        let rows = stmt.query_map(
            params![event_type, status, line_id, query, start_at, end_at, limit, offset],
            |row| {
                Ok(NotificationLogEntry {
                    id: row.get(0)?,
                    line_id: row.get(1)?,
                    event_type: row.get(2)?,
                    status: row.get(3)?,
                    summary: row.get(4)?,
                    rule_id: row.get(5)?,
                    rule_name: row.get(6)?,
                    channel_id: row.get(7)?,
                    channel_name: row.get(8)?,
                    message: row.get(9)?,
                    created_at: row.get(10)?,
                })
            },
        )?;

        let mut logs = Vec::new();
        for row in rows {
            logs.push(row?);
        }

        Ok(NotificationLogsResponse { logs, total })
    }

    pub fn clear_notification_logs(
        &self,
        event_type: &str,
        status: &str,
        line_id: &str,
        start_date: &str,
        end_date: &str,
    ) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let event_type = event_type.trim();
        let status = status.trim();
        let line_id = line_id.trim();
        let start_at = notification_log_start_bound(start_date);
        let end_at = notification_log_end_bound(end_date);
        conn.execute(
            "DELETE FROM notification_logs
             WHERE (?1 = '' OR event_type = ?1)
               AND (?2 = '' OR status = ?2)
               AND (?3 = '' OR (?3 = 'device' AND line_id IS NULL) OR line_id = ?3)
               AND (?4 = '' OR created_at >= ?4)
               AND (?5 = '' OR created_at <= ?5)",
            params![event_type, status, line_id, start_at, end_at],
        )
    }

    pub fn get_notification_queue(
        &self,
        line_id: &str,
        limit: i64,
    ) -> Result<NotificationQueueResponse> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 500);
        let line_id = line_id.trim();

        let total = conn.query_row(
            "SELECT COUNT(*) FROM notification_queue
             WHERE status IN ('pending', 'scheduled', 'retrying', 'sending', 'failed')
               AND (?1 = '' OR (?1 = 'device' AND line_id IS NULL) OR line_id = ?1)",
            params![line_id],
            |row| row.get(0),
        )?;

        let mut stmt = conn.prepare(
            "SELECT id, line_id, status, event_type, event_label, summary,
                    COALESCE(NULLIF(last_error, ''), reason) AS display_reason,
                    channel_id, channel_name, channel_type, rule_id, rule_name,
                    title, body, next_attempt_at,
                    attempt_count, max_attempts, created_at, updated_at
             FROM notification_queue
             WHERE status IN ('pending', 'scheduled', 'retrying', 'sending', 'failed')
               AND (?1 = '' OR (?1 = 'device' AND line_id IS NULL) OR line_id = ?1)
             ORDER BY next_attempt_at ASC, id ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![line_id, limit], notification_queue_entry_from_row)?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }

        Ok(NotificationQueueResponse { items, total })
    }

    pub fn retry_notification_queue_item(&self, id: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let now = beijing_sms_now_string();
        conn.execute(
            "UPDATE notification_queue
             SET status = 'pending',
                 attempt_count = 0,
                 next_attempt_at = ?1,
                 last_error = '',
                 updated_at = ?1
             WHERE id = ?2
               AND status IN ('pending', 'scheduled', 'retrying', 'sending', 'failed')",
            params![now, id],
        )
    }

    pub fn delete_notification_queue_item(&self, id: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE notification_queue
             SET status = 'cancelled',
                 updated_at = ?1
             WHERE id = ?2
               AND status IN ('pending', 'scheduled', 'retrying', 'sending', 'failed')",
            params![beijing_sms_now_string(), id],
        )
    }

    pub fn retry_all_notification_queue_items(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let now = beijing_sms_now_string();
        conn.execute(
            "UPDATE notification_queue
             SET status = 'pending',
                 attempt_count = 0,
                 next_attempt_at = ?1,
                 last_error = '',
                 updated_at = ?1
             WHERE status IN ('pending', 'scheduled', 'retrying', 'sending', 'failed')",
            params![now],
        )
    }

    pub fn clear_active_notification_queue(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE notification_queue
             SET status = 'cancelled',
                 updated_at = ?1
             WHERE status IN ('pending', 'scheduled', 'retrying', 'sending', 'failed')",
            params![beijing_sms_now_string()],
        )
    }

    pub fn get_due_notification_queue_items(
        &self,
        limit: i64,
    ) -> Result<Vec<NotificationQueueEntry>> {
        let conn = self.conn.lock().unwrap();
        let now = beijing_sms_now_string();
        let limit = limit.clamp(1, 100);
        let mut stmt = conn.prepare(
            "SELECT id, line_id, status, event_type, event_label, summary,
                    COALESCE(NULLIF(last_error, ''), reason) AS display_reason,
                    channel_id, channel_name, channel_type, rule_id, rule_name,
                    title, body, next_attempt_at,
                    attempt_count, max_attempts, created_at, updated_at
             FROM notification_queue
             WHERE status IN ('pending', 'scheduled', 'retrying')
               AND next_attempt_at <= ?1
             ORDER BY next_attempt_at ASC, id ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![now, limit], notification_queue_entry_from_row)?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(items)
    }

    pub fn mark_notification_queue_sending(&self, id: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE notification_queue
             SET status = 'sending',
                 updated_at = ?1
             WHERE id = ?2
               AND status IN ('pending', 'scheduled', 'retrying')",
            params![beijing_sms_now_string(), id],
        )
    }

    pub fn mark_notification_queue_sent(&self, id: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE notification_queue
             SET status = 'sent',
                 last_error = '',
                 updated_at = ?1
             WHERE id = ?2",
            params![beijing_sms_now_string(), id],
        )
    }

    pub fn mark_notification_queue_retry(
        &self,
        id: i64,
        last_error: &str,
        next_attempt_at: &str,
    ) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let now = beijing_sms_now_string();
        conn.execute(
            "UPDATE notification_queue
             SET status = 'retrying',
                 attempt_count = attempt_count + 1,
                 last_error = ?1,
                 next_attempt_at = ?2,
                 updated_at = ?3
             WHERE id = ?4",
            params![last_error, next_attempt_at, now, id],
        )
    }

    pub fn mark_notification_queue_scheduled(
        &self,
        id: i64,
        reason: &str,
        next_attempt_at: &str,
    ) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let now = beijing_sms_now_string();
        conn.execute(
            "UPDATE notification_queue
             SET status = 'scheduled',
                 reason = ?1,
                 next_attempt_at = ?2,
                 updated_at = ?3
             WHERE id = ?4",
            params![reason, next_attempt_at, now, id],
        )
    }

    pub fn mark_notification_queue_failed(&self, id: i64, last_error: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE notification_queue
             SET status = 'failed',
                 attempt_count = attempt_count + 1,
                 last_error = ?1,
                 updated_at = ?2
             WHERE id = ?3",
            params![last_error, beijing_sms_now_string(), id],
        )
    }

    pub fn cleanup_notification_logs(
        &self,
        retention_days: Option<u32>,
        max_entries: Option<u32>,
    ) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let mut deleted = 0usize;

        if let Some(days) = retention_days.filter(|days| *days > 0) {
            let cutoff = Utc::now()
                .with_timezone(&beijing_offset())
                .checked_sub_signed(Duration::days(i64::from(days)))
                .unwrap_or_else(|| Utc::now().with_timezone(&beijing_offset()))
                .format(SMS_TIMESTAMP_FORMAT)
                .to_string();
            deleted += conn.execute(
                "DELETE FROM notification_logs WHERE created_at < ?1",
                params![cutoff],
            )?;
        }

        if let Some(max_entries) = max_entries.filter(|max_entries| *max_entries > 0) {
            deleted += conn.execute(
                "DELETE FROM notification_logs
                 WHERE id NOT IN (
                    SELECT id FROM notification_logs
                    ORDER BY id DESC
                    LIMIT ?1
                 )",
                params![i64::from(max_entries)],
            )?;
        }

        Ok(deleted)
    }

    pub fn notification_status_counts(
        &self,
        event_type: &str,
        since: Option<&str>,
    ) -> Result<NotificationStatusCounts> {
        let conn = self.conn.lock().unwrap();
        let mut counts = NotificationStatusCounts::default();
        let since = since.unwrap_or("").trim();
        let mut stmt = conn.prepare(
            "SELECT status, COUNT(*)
             FROM notification_logs
             WHERE event_type = ?1
               AND (?2 = '' OR created_at >= ?2)
             GROUP BY status",
        )?;
        let rows = stmt.query_map(params![event_type, since], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (status, count) = row?;
            match status.as_str() {
                "success" => counts.success = count,
                "failed" => counts.failed = count,
                "quiet_hours" => counts.quiet_hours = count,
                "unmatched" => counts.unmatched = count,
                "no_available_channel" => counts.no_available_channel = count,
                _ => {}
            }
        }
        Ok(counts)
    }

    pub fn period_sms_stats(&self, since: Option<&str>) -> Result<PeriodSmsStats> {
        let conn = self.conn.lock().unwrap();
        let since = since.unwrap_or("").trim();
        let incoming: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sms_messages
             WHERE direction = 'incoming'
               AND status = 'received'
               AND (?1 = '' OR timestamp >= ?1)",
            params![since],
            |row| row.get(0),
        )?;
        drop(conn);
        let forwarding = self.notification_status_counts("sms", Some(since))?;
        Ok(PeriodSmsStats {
            incoming,
            forwarding,
        })
    }

    pub fn get_sms_stats(&self) -> Result<SmsStats> {
        self.get_sms_stats_for_channel(None)
    }

    pub fn get_sms_stats_for_channel(&self, channel_id: Option<&str>) -> Result<SmsStats> {
        let conn = self.conn.lock().unwrap();

        let channel_filter =
            "(?1 IS NULL OR (?1 = 'unassigned' AND (line_id IS NULL OR TRIM(line_id) = '')) OR line_id = ?1)";
        let total: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM sms_messages WHERE {channel_filter}"),
            params![channel_id],
            |row| row.get(0),
        )?;

        let incoming: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM sms_messages
             WHERE direction = 'incoming' AND status = 'received' AND {channel_filter}"
            ),
            params![channel_id],
            |row| row.get(0),
        )?;

        let outgoing: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM sms_messages WHERE direction = 'outgoing' AND {channel_filter}"),
            params![channel_id],
            |row| row.get(0),
        )?;

        let pushed: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM sms_messages
             WHERE direction = 'incoming'
               AND status = 'received'
               AND notification_status = 'success' AND {channel_filter}"
            ),
            params![channel_id],
            |row| row.get(0),
        )?;

        let push_attempted: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM sms_messages
             WHERE direction = 'incoming'
               AND status = 'received'
               AND notification_status IN ('success', 'failed') AND {channel_filter}"
            ),
            params![channel_id],
            |row| row.get(0),
        )?;

        Ok(SmsStats {
            total,
            incoming,
            outgoing,
            pushed,
            push_attempted,
        })
    }

    pub fn has_unassigned_sms(&self) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sms_messages WHERE line_id IS NULL OR TRIM(line_id) = ''",
            [],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Delete every SMS belonging to one explicit channel.
    pub fn clear_sms_for_channel(&self, channel_id: &str) -> Result<usize> {
        let channel_id = required_sms_channel_id(channel_id)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE vowifi_sms_delivery
             SET api_sms_id = NULL
             WHERE api_sms_id IN (
                 SELECT id FROM sms_messages
                 WHERE (?1 = 'unassigned' AND (line_id IS NULL OR TRIM(line_id) = ''))
                    OR line_id = ?1
             )",
            params![channel_id],
        )?;
        let deleted = tx.execute(
            "DELETE FROM sms_messages
             WHERE (?1 = 'unassigned' AND (line_id IS NULL OR TRIM(line_id) = ''))
                OR line_id = ?1",
            params![channel_id],
        )?;
        tx.commit()?;
        Ok(deleted)
    }

    /// Delete one SMS only when it belongs to the requested channel.
    pub fn delete_sms_for_channel(&self, id: i64, channel_id: &str) -> Result<usize> {
        let channel_id = required_sms_channel_id(channel_id)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE vowifi_sms_delivery
             SET api_sms_id = NULL
             WHERE api_sms_id IN (
                 SELECT id FROM sms_messages
                 WHERE id = ?1
                   AND ((?2 = 'unassigned' AND (line_id IS NULL OR TRIM(line_id) = '')) OR line_id = ?2)
             )",
            params![id, channel_id],
        )?;
        let deleted = tx.execute(
            "DELETE FROM sms_messages
             WHERE id = ?1
               AND ((?2 = 'unassigned' AND (line_id IS NULL OR TRIM(line_id) = '')) OR line_id = ?2)",
            params![id, channel_id],
        )?;
        tx.commit()?;
        Ok(deleted)
    }

    pub fn delete_sms_conversation_for_channel(
        &self,
        phone_number: &str,
        channel_id: &str,
    ) -> Result<usize> {
        let channel_id = required_sms_channel_id(channel_id)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE vowifi_sms_delivery
             SET api_sms_id = NULL
             WHERE api_sms_id IN (
                 SELECT id FROM sms_messages WHERE phone_number = ?1
                   AND ((?2 = 'unassigned' AND (line_id IS NULL OR TRIM(line_id) = '')) OR line_id = ?2)
             )",
            params![phone_number, channel_id],
        )?;
        let deleted = tx.execute(
            "DELETE FROM sms_messages WHERE phone_number = ?1
               AND ((?2 = 'unassigned' AND (line_id IS NULL OR TRIM(line_id) = '')) OR line_id = ?2)",
            params![phone_number, channel_id],
        )?;
        tx.commit()?;
        Ok(deleted)
    }

    pub fn delete_sms_batch_for_channel(
        &self,
        ids: &[i64],
        phone_numbers: &[String],
        channel_id: &str,
    ) -> Result<usize> {
        let channel_id = required_sms_channel_id(channel_id)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut deleted = 0usize;

        for phone_number in phone_numbers {
            tx.execute(
                "UPDATE vowifi_sms_delivery
                 SET api_sms_id = NULL
                 WHERE api_sms_id IN (
                     SELECT id FROM sms_messages WHERE phone_number = ?1
                   AND ((?2 = 'unassigned' AND (line_id IS NULL OR TRIM(line_id) = '')) OR line_id = ?2)
                 )",
                params![phone_number, channel_id],
            )?;
            deleted += tx.execute(
                "DELETE FROM sms_messages WHERE phone_number = ?1
                   AND ((?2 = 'unassigned' AND (line_id IS NULL OR TRIM(line_id) = '')) OR line_id = ?2)",
                params![phone_number, channel_id],
            )?;
        }

        for id in ids {
            tx.execute(
                "UPDATE vowifi_sms_delivery
                 SET api_sms_id = NULL
                 WHERE api_sms_id IN (
                     SELECT id FROM sms_messages
                     WHERE id = ?1
                       AND ((?2 = 'unassigned' AND (line_id IS NULL OR TRIM(line_id) = '')) OR line_id = ?2)
                 )",
                params![id, channel_id],
            )?;
            deleted += tx.execute(
                "DELETE FROM sms_messages
                 WHERE id = ?1
                   AND ((?2 = 'unassigned' AND (line_id IS NULL OR TRIM(line_id) = '')) OR line_id = ?2)",
                params![id, channel_id],
            )?;
        }

        tx.commit()?;
        Ok(deleted)
    }

    pub fn vacuum(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("VACUUM")?;
        Ok(())
    }

    // ==================== SMSC cache ====================

    pub fn upsert_smsc_cache(
        &self,
        identity_key: &str,
        iccid: &str,
        imsi: &str,
        operator_id: &str,
        sms_center: &str,
        source: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let updated_at = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO smsc_cache (
                identity_key, iccid, imsi, operator_id, sms_center, source, updated_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(identity_key) DO UPDATE SET
                iccid = excluded.iccid,
                imsi = excluded.imsi,
                operator_id = excluded.operator_id,
                sms_center = excluded.sms_center,
                source = excluded.source,
                updated_at = excluded.updated_at",
            params![
                identity_key,
                iccid,
                imsi,
                operator_id,
                sms_center,
                source,
                updated_at
            ],
        )?;
        Ok(())
    }

    pub fn get_smsc_cache(&self, identity_keys: &[String]) -> Result<Option<SmscCacheEntry>> {
        let conn = self.conn.lock().unwrap();
        for key in identity_keys {
            let entry = conn
                .query_row(
                    "SELECT sms_center, source, updated_at
                     FROM smsc_cache
                     WHERE identity_key = ?1",
                    params![key],
                    |row| {
                        Ok(SmscCacheEntry {
                            sms_center: row.get(0)?,
                            source: row.get(1)?,
                            updated_at: row.get(2)?,
                        })
                    },
                )
                .optional()?;
            if entry.is_some() {
                return Ok(entry);
            }
        }
        Ok(None)
    }

    // ==================== Own number cache ====================

    pub fn upsert_own_number_cache(
        &self,
        identity_key: &str,
        iccid: &str,
        imsi: &str,
        operator_id: &str,
        phone_numbers: &[String],
        source: &str,
    ) -> Result<()> {
        if phone_numbers.is_empty() {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap();
        let updated_at = Utc::now().to_rfc3339();
        let phone_numbers = phone_numbers.join("\n");
        conn.execute(
            "INSERT INTO own_number_cache (
                identity_key, iccid, imsi, operator_id, phone_numbers, source, updated_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(identity_key) DO UPDATE SET
                iccid = excluded.iccid,
                imsi = excluded.imsi,
                operator_id = excluded.operator_id,
                phone_numbers = excluded.phone_numbers,
                source = excluded.source,
                updated_at = excluded.updated_at",
            params![
                identity_key,
                iccid,
                imsi,
                operator_id,
                phone_numbers,
                source,
                updated_at
            ],
        )?;
        Ok(())
    }

    pub fn get_own_number_cache(
        &self,
        identity_keys: &[String],
    ) -> Result<Option<OwnNumberCacheEntry>> {
        let conn = self.conn.lock().unwrap();
        for key in identity_keys {
            let entry = conn
                .query_row(
                    "SELECT phone_numbers, source, updated_at
                     FROM own_number_cache
                     WHERE identity_key = ?1",
                    params![key],
                    |row| {
                        let phone_numbers: String = row.get(0)?;
                        Ok(OwnNumberCacheEntry {
                            phone_numbers: phone_numbers
                                .lines()
                                .map(str::trim)
                                .filter(|line| !line.is_empty())
                                .map(ToString::to_string)
                                .collect(),
                            source: row.get(1)?,
                            updated_at: row.get(2)?,
                        })
                    },
                )
                .optional()?;
            if entry.is_some() {
                return Ok(entry);
            }
        }
        Ok(None)
    }

    // ==================== eSIM Profile cache ====================

    pub fn upsert_esim_profile_cache(&self, entry: &EsimProfileCacheEntry) -> Result<()> {
        let iccid = crate::platform::utils::normalize_iccid(&entry.iccid);
        if iccid.is_empty() {
            return Ok(());
        }

        let has_profile_data = [
            entry.name.as_deref(),
            entry.provider.as_deref(),
            entry.profile_class.as_deref(),
            entry.imsi.as_deref(),
            entry.msisdn.as_deref(),
            entry.smsc.as_deref(),
            entry.smdp.as_deref(),
            entry.matching_id.as_deref(),
            entry.isdp_aid.as_deref(),
            entry.mcc.as_deref(),
            entry.mnc.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|value| !value.trim().is_empty());

        if !has_profile_data {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap();
        let updated_at = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO esim_profile_cache (
                iccid, name, provider, profile_class, imsi, msisdn, smsc, smdp,
                matching_id, isdp_aid, mcc, mnc, updated_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(iccid) DO UPDATE SET
                name = COALESCE(excluded.name, esim_profile_cache.name),
                provider = COALESCE(excluded.provider, esim_profile_cache.provider),
                profile_class = COALESCE(excluded.profile_class, esim_profile_cache.profile_class),
                imsi = COALESCE(excluded.imsi, esim_profile_cache.imsi),
                msisdn = COALESCE(excluded.msisdn, esim_profile_cache.msisdn),
                smsc = COALESCE(excluded.smsc, esim_profile_cache.smsc),
                smdp = COALESCE(excluded.smdp, esim_profile_cache.smdp),
                matching_id = COALESCE(excluded.matching_id, esim_profile_cache.matching_id),
                isdp_aid = COALESCE(excluded.isdp_aid, esim_profile_cache.isdp_aid),
                mcc = COALESCE(excluded.mcc, esim_profile_cache.mcc),
                mnc = COALESCE(excluded.mnc, esim_profile_cache.mnc),
                updated_at = excluded.updated_at",
            params![
                &iccid,
                non_empty_option(entry.name.as_deref()),
                non_empty_option(entry.provider.as_deref()),
                non_empty_option(entry.profile_class.as_deref()),
                non_empty_option(entry.imsi.as_deref()),
                non_empty_option(entry.msisdn.as_deref()),
                non_empty_option(entry.smsc.as_deref()),
                non_empty_option(entry.smdp.as_deref()),
                non_empty_option(entry.matching_id.as_deref()),
                non_empty_option(entry.isdp_aid.as_deref()),
                non_empty_option(entry.mcc.as_deref()),
                non_empty_option(entry.mnc.as_deref()),
                updated_at
            ],
        )?;
        Ok(())
    }

    pub fn get_esim_profile_cache(&self, iccid: &str) -> Result<Option<EsimProfileCacheEntry>> {
        let iccid = crate::platform::utils::normalize_iccid(iccid);
        if iccid.is_empty() {
            return Ok(None);
        }

        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT iccid, name, provider, profile_class, imsi, msisdn, smsc, smdp,
                    matching_id, isdp_aid, mcc, mnc, updated_at
             FROM esim_profile_cache
             WHERE iccid = ?1",
            params![&iccid],
            |row| {
                Ok(EsimProfileCacheEntry {
                    iccid: row.get(0)?,
                    name: row.get(1)?,
                    provider: row.get(2)?,
                    profile_class: row.get(3)?,
                    imsi: row.get(4)?,
                    msisdn: row.get(5)?,
                    smsc: row.get(6)?,
                    smdp: row.get(7)?,
                    matching_id: row.get(8)?,
                    isdp_aid: row.get(9)?,
                    mcc: row.get(10)?,
                    mnc: row.get(11)?,
                    updated_at: row.get(12)?,
                })
            },
        )
        .optional()
    }

    pub fn list_esim_profile_cache(&self) -> Result<Vec<EsimProfileCacheEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT iccid, name, provider, profile_class, imsi, msisdn, smsc, smdp,
                    matching_id, isdp_aid, mcc, mnc, updated_at
             FROM esim_profile_cache
             ORDER BY updated_at DESC, iccid ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(EsimProfileCacheEntry {
                iccid: row.get(0)?,
                name: row.get(1)?,
                provider: row.get(2)?,
                profile_class: row.get(3)?,
                imsi: row.get(4)?,
                msisdn: row.get(5)?,
                smsc: row.get(6)?,
                smdp: row.get(7)?,
                matching_id: row.get(8)?,
                isdp_aid: row.get(9)?,
                mcc: row.get(10)?,
                mnc: row.get(11)?,
                updated_at: row.get(12)?,
            })
        })?;

        rows.collect()
    }

    pub fn delete_esim_profile_cache(&self, iccid: &str) -> Result<()> {
        let iccid = crate::platform::utils::normalize_iccid(iccid);
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM esim_profile_cache WHERE iccid = ?1",
            params![&iccid],
        )?;
        Ok(())
    }

    // ==================== 通话记录相关方法 ====================

    /// 插入新通话记录
    pub fn insert_call(
        &self,
        line_id: Option<&str>,
        direction: &str,
        phone_number: &str,
        answered: bool,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let start_time = Utc::now().to_rfc3339();
        let line_id = non_empty_option(line_id);

        conn.execute(
            "INSERT INTO call_history (line_id, direction, phone_number, duration, start_time, answered)
             VALUES (?1, ?2, ?3, 0, ?4, ?5)",
            params![line_id, direction, phone_number, start_time, answered as i32],
        )?;

        Ok(conn.last_insert_rowid())
    }

    /// 更新通话记录（通话结束时调用）
    pub fn update_call_end(&self, id: i64, duration: i64, answered: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let end_time = Utc::now().to_rfc3339();

        conn.execute(
            "UPDATE call_history SET duration = ?1, end_time = ?2, answered = ?3 WHERE id = ?4",
            params![duration, end_time, answered as i32, id],
        )?;
        Ok(())
    }

    /// 标记通话为未接来电
    pub fn mark_call_missed(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let end_time = Utc::now().to_rfc3339();

        conn.execute(
            "UPDATE call_history SET direction = 'missed', end_time = ?1, answered = 0 WHERE id = ?2",
            params![end_time, id],
        )?;
        Ok(())
    }

    pub fn get_call_by_id(&self, id: i64) -> Result<Option<CallRecord>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, line_id, direction, phone_number, duration, start_time, end_time, answered
             FROM call_history WHERE id = ?1",
            params![id],
            call_record_from_row,
        )
        .optional()
    }

    /// 获取通话记录（分页）
    pub fn get_call_history(&self, limit: i64, offset: i64) -> Result<Vec<CallRecord>> {
        self.get_call_history_filtered(None, limit, offset)
    }

    pub fn get_call_history_for_line(
        &self,
        line_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<CallRecord>> {
        self.get_call_history_filtered(Some(line_id), limit, offset)
    }

    fn get_call_history_filtered(
        &self,
        line_id: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<CallRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, line_id, direction, phone_number, duration, start_time, end_time, answered
             FROM call_history
             WHERE (?3 IS NULL OR line_id = ?3)
             ORDER BY start_time DESC
             LIMIT ?1 OFFSET ?2",
        )?;

        let records = stmt.query_map(params![limit, offset, line_id], call_record_from_row)?;

        let mut result = Vec::new();
        for record in records {
            result.push(record?);
        }

        Ok(result)
    }

    /// 获取通话统计
    pub fn get_call_stats(&self) -> Result<CallStats> {
        self.get_call_stats_filtered(None)
    }

    pub fn get_call_stats_for_line(&self, line_id: &str) -> Result<CallStats> {
        self.get_call_stats_filtered(Some(line_id))
    }

    fn get_call_stats_filtered(&self, line_id: Option<&str>) -> Result<CallStats> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(CASE WHEN direction = 'incoming' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN direction = 'outgoing' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN direction = 'missed' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN answered = 1 THEN duration ELSE 0 END), 0)
             FROM call_history
             WHERE (?1 IS NULL OR line_id = ?1)",
            params![line_id],
            |row| {
                Ok(CallStats {
                    total: row.get(0)?,
                    incoming: row.get(1)?,
                    outgoing: row.get(2)?,
                    missed: row.get(3)?,
                    total_duration: row.get(4)?,
                })
            },
        )
    }

    pub fn delete_call_for_line(&self, line_id: &str, id: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM call_history WHERE id = ?1 AND line_id = ?2",
            params![id, line_id],
        )
    }

    pub fn clear_calls_for_line(&self, line_id: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM call_history WHERE line_id = ?1",
            params![line_id],
        )
    }

    // ==================== 自动化运行日志相关方法 ====================

    /// 插入新自动化执行日志
    pub fn insert_automation_log(
        &self,
        line_id: Option<&str>,
        task_id: &str,
        task_name: &str,
        task_type: &str,
        status: &str,
        detail: &str,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let created_at = beijing_sms_now_string();
        let line_id = non_empty_option(line_id);
        conn.execute(
            "INSERT INTO automation_logs (
                line_id, task_id, task_name, task_type, status, detail, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![line_id, task_id, task_name, task_type, status, detail, created_at],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// 获取自动化执行日志（分页与过滤）
    #[allow(clippy::too_many_arguments)]
    pub fn get_automation_logs(
        &self,
        task_type: &str,
        status: &str,
        line_id: &str,
        query: &str,
        start_date: &str,
        end_date: &str,
        limit: i64,
        offset: i64,
    ) -> Result<AutomationLogsResponse> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 200);
        let offset = offset.max(0);
        let task_type = task_type.trim();
        let status = status.trim();
        let line_id = line_id.trim();
        let query = query.trim();

        let start_at = notification_log_start_bound(start_date);
        let end_at = notification_log_end_bound(end_date);

        let total = conn.query_row(
            "SELECT COUNT(*) FROM automation_logs
             WHERE (?1 = '' OR task_type = ?1)
               AND (?2 = '' OR status = ?2)
               AND (?3 = '' OR line_id = ?3)
               AND (
                    ?4 = ''
                    OR task_name LIKE '%' || ?4 || '%'
                    OR detail LIKE '%' || ?4 || '%'
                    OR line_id LIKE '%' || ?4 || '%'
               )
               AND (?5 = '' OR created_at >= ?5)
               AND (?6 = '' OR created_at <= ?6)",
            params![task_type, status, line_id, query, start_at, end_at],
            |row| row.get(0),
        )?;

        let mut stmt = conn.prepare(
            "SELECT id, line_id, task_id, task_name, task_type, status, detail, created_at
             FROM automation_logs
             WHERE (?1 = '' OR task_type = ?1)
               AND (?2 = '' OR status = ?2)
               AND (?3 = '' OR line_id = ?3)
               AND (
                    ?4 = ''
                    OR task_name LIKE '%' || ?4 || '%'
                    OR detail LIKE '%' || ?4 || '%'
                    OR line_id LIKE '%' || ?4 || '%'
               )
               AND (?5 = '' OR created_at >= ?5)
               AND (?6 = '' OR created_at <= ?6)
             ORDER BY created_at DESC
             LIMIT ?7 OFFSET ?8",
        )?;

        let rows = stmt.query_map(
            params![task_type, status, line_id, query, start_at, end_at, limit, offset],
            |row| {
                let mut detail: String = row.get(6)?;
                if detail == "执行成功 (0)" || detail.starts_with("执行成功 (0)") {
                    detail = "执行成功".to_string();
                }
                Ok(AutomationLogEntry {
                    id: row.get(0)?,
                    line_id: row.get(1)?,
                    task_id: row.get(2)?,
                    task_name: row.get(3)?,
                    task_type: row.get(4)?,
                    status: row.get(5)?,
                    detail,
                    created_at: row.get(7)?,
                })
            },
        )?;

        let mut logs = Vec::new();
        for row in rows {
            logs.push(row?);
        }

        Ok(AutomationLogsResponse { logs, total })
    }

    /// 获取特定任务的最后一次运行日志
    pub fn get_last_log_for_task(&self, task_id: &str) -> Result<Option<AutomationLogEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, line_id, task_id, task_name, task_type, status, detail, created_at
             FROM automation_logs
             WHERE task_id = ?1
             ORDER BY created_at DESC
             LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![task_id], |row| {
            let mut detail: String = row.get(6)?;
            if detail == "执行成功 (0)" || detail.starts_with("执行成功 (0)") {
                detail = "执行成功".to_string();
            }
            Ok(AutomationLogEntry {
                id: row.get(0)?,
                line_id: row.get(1)?,
                task_id: row.get(2)?,
                task_name: row.get(3)?,
                task_type: row.get(4)?,
                status: row.get(5)?,
                detail,
                created_at: row.get(7)?,
            })
        })?;
        if let Some(row) = rows.next() {
            Ok(Some(row?))
        } else {
            Ok(None)
        }
    }

    /// 清理过滤的日志
    pub fn clear_automation_logs(
        &self,
        task_type: &str,
        status: &str,
        line_id: &str,
        start_date: &str,
        end_date: &str,
    ) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let task_type = task_type.trim();
        let status = status.trim();
        let line_id = line_id.trim();

        let start_at = notification_log_start_bound(start_date);
        let end_at = notification_log_end_bound(end_date);

        conn.execute(
            "DELETE FROM automation_logs
             WHERE (?1 = '' OR task_type = ?1)
               AND (?2 = '' OR status = ?2)
               AND (?3 = '' OR line_id = ?3)
               AND (?4 = '' OR created_at >= ?4)
               AND (?5 = '' OR created_at <= ?5)",
            params![task_type, status, line_id, start_at, end_at],
        )
    }

    /// 自动保留策略清理
    pub fn cleanup_automation_logs(
        &self,
        retention_days: Option<u32>,
        max_entries: Option<u32>,
    ) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let mut deleted = 0usize;

        if let Some(days) = retention_days.filter(|days| *days > 0) {
            let cutoff = Utc::now()
                .with_timezone(&beijing_offset())
                .checked_sub_signed(Duration::days(i64::from(days)))
                .unwrap_or_else(|| Utc::now().with_timezone(&beijing_offset()))
                .format(SMS_TIMESTAMP_FORMAT)
                .to_string();
            deleted += conn.execute(
                "DELETE FROM automation_logs WHERE created_at < ?1",
                params![cutoff],
            )?;
        }

        if let Some(max_entries) = max_entries.filter(|max_entries| *max_entries > 0) {
            deleted += conn.execute(
                "DELETE FROM automation_logs
                 WHERE id NOT IN (
                    SELECT id FROM automation_logs
                    ORDER BY id DESC
                    LIMIT ?1
                 )",
                params![i64::from(max_entries)],
            )?;
        }

        Ok(deleted)
    }
}
