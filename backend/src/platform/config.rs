//! 配置管理模块
//!
//! User configuration with an in-memory typed view and durable persistence.
//!
//! Production stores the canonical `AppConfig` document in SQLite. JSON is
//! retained only as an internal test backend and is never imported at startup.

use rusqlite::{params, Connection as SqliteConnection, OptionalExtension, TransactionBehavior};
use serde::{de::Error as DeError, Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::connectivity::core::ims_access::ImsAccessPreference;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use tracing::info;

/// Webhook 配置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebhookConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub url: String,
    #[serde(default = "default_true")]
    pub forward_sms: bool,
    #[serde(default = "default_true")]
    pub forward_calls: bool,
    #[serde(default = "default_true")]
    pub forward_ddns: bool,
    #[serde(default = "default_true")]
    pub forward_updates: bool,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub secret: String, // 可选的签名密钥
    #[serde(default = "default_sms_template")]
    pub sms_template: String, // 短信 payload 模板
    #[serde(default = "default_call_template")]
    pub call_template: String, // 通话 payload 模板
    #[serde(default = "default_ddns_template")]
    pub ddns_template: String, // DDNS payload 模板
    #[serde(default = "default_update_template")]
    pub update_template: String, // 版本更新 payload 模板
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageChannelConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub forward_sms: bool,
    #[serde(default = "default_true")]
    pub forward_calls: bool,
    #[serde(default = "default_true")]
    pub forward_ddns: bool,
    #[serde(default = "default_true")]
    pub forward_updates: bool,
    #[serde(default = "default_plain_sms_template")]
    pub sms_template: String,
    #[serde(default = "default_plain_call_template")]
    pub call_template: String,
    #[serde(default = "default_plain_ddns_template")]
    pub ddns_template: String,
    #[serde(default = "default_plain_update_template")]
    pub update_template: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BarkConfig {
    #[serde(flatten)]
    pub common: MessageChannelConfig,
    #[serde(default = "default_bark_server_url")]
    pub server_url: String,
    #[serde(default)]
    pub device_key: String,
    #[serde(default = "default_sms_title_template")]
    pub title_template: String,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub sound: String,
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub click_url: String,
    #[serde(default)]
    pub copy: String,
    #[serde(default)]
    pub auto_copy: bool,
    #[serde(default = "default_true")]
    pub save_history: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushPlusConfig {
    #[serde(flatten)]
    pub common: MessageChannelConfig,
    #[serde(default)]
    pub token: String,
    #[serde(default = "default_sms_title_template")]
    pub title_template: String,
    #[serde(default)]
    pub topic: String,
    #[serde(default = "default_pushplus_template")]
    pub template: String,
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub option: String,
    #[serde(default)]
    pub callback_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WecomAppConfig {
    #[serde(flatten)]
    pub common: MessageChannelConfig,
    #[serde(default)]
    pub corp_id: String,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub secret: String,
    #[serde(default = "default_wecom_to_user")]
    pub to_user: String,
    #[serde(default)]
    pub to_party: String,
    #[serde(default)]
    pub to_tag: String,
    #[serde(default)]
    pub safe: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WecomRobotConfig {
    #[serde(flatten)]
    pub common: MessageChannelConfig,
    #[serde(default)]
    pub webhook_url: String,
    #[serde(default)]
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DingtalkRobotConfig {
    #[serde(flatten)]
    pub common: MessageChannelConfig,
    #[serde(default)]
    pub webhook_url: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub secret: String,
    #[serde(default)]
    pub at_mobiles: String,
    #[serde(default)]
    pub at_all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DingtalkAppConfig {
    #[serde(flatten)]
    pub common: MessageChannelConfig,
    #[serde(default)]
    pub app_key: String,
    #[serde(default)]
    pub app_secret: String,
    #[serde(default)]
    pub robot_code: String,
    #[serde(default)]
    pub open_conversation_id: String,
    #[serde(default = "default_dingtalk_msg_key")]
    pub msg_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FeishuRobotConfig {
    #[serde(flatten)]
    pub common: MessageChannelConfig,
    #[serde(default)]
    pub webhook_url: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    #[serde(flatten)]
    pub common: MessageChannelConfig,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub chat_id: String,
    #[serde(default)]
    pub parse_mode: String,
    #[serde(default)]
    pub disable_web_page_preview: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LegacyNotificationConfig {
    #[serde(default)]
    pub webhook: WebhookConfig,
    #[serde(default)]
    pub bark: BarkConfig,
    #[serde(default)]
    pub pushplus: PushPlusConfig,
    #[serde(default)]
    pub wecom_app: WecomAppConfig,
    #[serde(default)]
    pub wecom_robot: WecomRobotConfig,
    #[serde(default)]
    pub dingtalk_robot: DingtalkRobotConfig,
    #[serde(default)]
    pub dingtalk_app: DingtalkAppConfig,
    #[serde(default)]
    pub feishu_robot: FeishuRobotConfig,
    #[serde(default)]
    pub telegram: TelegramConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationChannel {
    Webhook,
    Bark,
    #[serde(rename = "pushplus", alias = "push_plus")]
    PushPlus,
    WecomApp,
    WecomRobot,
    DingtalkRobot,
    DingtalkApp,
    FeishuRobot,
    Telegram,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationEventType {
    Sms,
    Call,
    Ddns,
    VersionUpdate,
    SystemEvent,
    DeviceStatus,
    Automation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatcherOperator {
    Always,
    Contains,
    NotContains,
    Equals,
    Regex,
}

fn default_matcher_operator() -> MatcherOperator {
    MatcherOperator::Always
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleMatcher {
    #[serde(default)]
    pub field: String,
    #[serde(default = "default_matcher_operator")]
    pub operator: MatcherOperator,
    #[serde(default)]
    pub value: String,
}

impl Default for RuleMatcher {
    fn default() -> Self {
        Self {
            field: "summary".to_string(),
            operator: MatcherOperator::Always,
            value: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuietHoursSchedule {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub weekdays: Vec<u8>,
    #[serde(default = "default_quiet_start")]
    pub start: String,
    #[serde(default = "default_quiet_end")]
    pub end: String,
}

fn default_quiet_start() -> String {
    "22:00".to_string()
}

fn default_quiet_end() -> String {
    "08:00".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DeviceStatusSchedule {
    #[serde(default = "default_device_status_schedule_mode")]
    pub mode: String,
    #[serde(default = "default_device_status_interval_minutes")]
    pub interval_minutes: u32,
    #[serde(default = "default_device_status_weekdays")]
    pub weekdays: Vec<u8>,
    #[serde(default = "default_device_status_times")]
    pub times: Vec<String>,
}

impl Default for DeviceStatusSchedule {
    fn default() -> Self {
        Self {
            mode: default_device_status_schedule_mode(),
            interval_minutes: default_device_status_interval_minutes(),
            weekdays: default_device_status_weekdays(),
            times: default_device_status_times(),
        }
    }
}

fn default_device_status_schedule_mode() -> String {
    "fixed".to_string()
}

fn default_device_status_interval_minutes() -> u32 {
    24 * 60
}

fn default_device_status_weekdays() -> Vec<u8> {
    vec![1, 2, 3, 4, 5, 6, 7]
}

fn default_device_status_times() -> Vec<String> {
    vec!["09:00".to_string()]
}

fn default_device_status_sms_period() -> String {
    "last_24h".to_string()
}

pub fn default_device_status_items() -> Vec<String> {
    [
        "device_power",
        "device_model",
        "system_version",
        "uptime",
        "sim_present",
        "sim_operator",
        "cellular_registration",
        "cellular_operator",
        "cellular_technology",
        "signal_strength",
        "data_connection",
        "airplane_mode",
        "roaming",
        "ipv4_connectivity",
        "ipv6_connectivity",
        "default_route",
        "default_ip",
        "wlan_enabled",
        "wlan_connected",
        "wlan_ssid",
        "key_interfaces",
        "cellular_traffic",
        "cpu_usage",
        "memory_usage",
        "root_disk",
        "top_temperatures",
        "service_version",
        "ddns_status",
        "ota_status",
        "forwarding_channels",
        "forwarding_rules",
        "sms_forwarding_stats",
    ]
    .into_iter()
    .map(ToString::to_string)
    .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationRule {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: NotificationEventType,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub matcher: RuleMatcher,
    #[serde(default)]
    pub channel_ids: Vec<String>,
    /// Empty means every SIM source. Values are stable modem line IDs,
    /// `reader:<slot_id>`, or `unassigned` for legacy rows.
    #[serde(default)]
    pub sim_channel_ids: Vec<String>,
    #[serde(default)]
    pub event_codes: Vec<String>,
    #[serde(default)]
    pub template: String,
    #[serde(default)]
    pub quiet_hours: Vec<QuietHoursSchedule>,
    #[serde(default = "default_ddns_failure_threshold")]
    pub ddns_failure_threshold: u32,
    #[serde(default = "default_device_status_items")]
    pub device_status_items: Vec<String>,
    #[serde(default)]
    pub device_status_schedule: DeviceStatusSchedule,
    #[serde(default = "default_device_status_sms_period")]
    pub device_status_sms_period: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationChannelInstance {
    pub id: String,
    #[serde(rename = "type")]
    pub channel_type: NotificationChannel,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub rate_limit: NotificationRateLimitConfig,
    #[serde(default)]
    pub config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationRateLimitConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_notification_rate_limit_max_messages")]
    pub max_messages: u32,
    #[serde(default = "default_notification_rate_limit_window_seconds")]
    pub window_seconds: u32,
}

impl Default for NotificationRateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_messages: default_notification_rate_limit_max_messages(),
            window_seconds: default_notification_rate_limit_window_seconds(),
        }
    }
}

fn default_notification_rate_limit_max_messages() -> u32 {
    20
}

fn default_notification_rate_limit_window_seconds() -> u32 {
    60
}

fn default_ddns_failure_threshold() -> u32 {
    1
}

fn default_notification_log_retention_days() -> u32 {
    90
}

fn default_notification_log_max_entries() -> u32 {
    10_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationLogCleanupConfig {
    #[serde(default = "default_true")]
    pub retention_days_enabled: bool,
    #[serde(default = "default_notification_log_retention_days")]
    pub retention_days: u32,
    #[serde(default = "default_true")]
    pub max_entries_enabled: bool,
    #[serde(default = "default_notification_log_max_entries")]
    pub max_entries: u32,
}

impl Default for NotificationLogCleanupConfig {
    fn default() -> Self {
        Self {
            retention_days_enabled: true,
            retention_days: default_notification_log_retention_days(),
            max_entries_enabled: true,
            max_entries: default_notification_log_max_entries(),
        }
    }
}

fn default_notification_version() -> u8 {
    2
}

#[derive(Debug, Clone, Serialize)]
pub struct NotificationConfig {
    #[serde(default = "default_notification_version")]
    pub version: u8,
    #[serde(default)]
    pub channels: Vec<NotificationChannelInstance>,
    #[serde(default)]
    pub rules: Vec<NotificationRule>,
    #[serde(default)]
    pub log_cleanup: NotificationLogCleanupConfig,
}

#[derive(Deserialize)]
struct NotificationConfigV2 {
    #[serde(default = "default_notification_version", rename = "version")]
    _version: u8,
    #[serde(default)]
    channels: Vec<NotificationChannelInstance>,
    #[serde(default)]
    rules: Vec<NotificationRule>,
    #[serde(default)]
    log_cleanup: NotificationLogCleanupConfig,
}

impl<'de> Deserialize<'de> for NotificationConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let is_v2 = value.get("channels").is_some() || value.get("rules").is_some();
        if is_v2 {
            let parsed: NotificationConfigV2 =
                serde_json::from_value(value).map_err(D::Error::custom)?;
            return Ok(Self {
                version: 2,
                channels: parsed.channels,
                rules: parsed.rules,
                log_cleanup: parsed.log_cleanup,
            });
        }

        let legacy: LegacyNotificationConfig =
            serde_json::from_value(value).map_err(D::Error::custom)?;
        Ok(Self::from_legacy(legacy))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub struct DeviceNetworkConfig {
    #[serde(default)]
    pub ddns: DdnsConfig,
}

/// Per-UE isolation configuration (Linux network namespaces).
///
/// This is the master switch for the multi-UE architecture documented in
/// `multi_ue_ims_volte_vowifi_architecture.md`. When enabled, every line gets
/// its own UE Context and Linux network namespace so identical IPs, P-CSCF
/// addresses and route state can never leak between SIMs. The data planes
/// (VoLTE bearer netdev, VoWiFi TUN, per-UE proxy) are migrated into the
/// namespace incrementally behind this switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UeIsolationConfig {
    /// Master switch. Defaults to false: behaviour is exactly the current
    /// host-namespace routing until the migration is complete.
    #[serde(default)]
    pub enabled: bool,
    /// Prefix for per-UE network namespace names.
    #[serde(default = "default_ue_namespace_prefix")]
    pub namespace_prefix: String,
    /// Prefix for the host side of each UE egress veth pair.
    #[serde(default = "default_ue_host_veth_prefix")]
    pub host_veth_prefix: String,
    /// Prefix for the UE side of each UE egress veth pair.
    #[serde(default = "default_ue_veth_prefix")]
    pub ue_veth_prefix: String,
    /// MTU used for the egress veth pairs.
    #[serde(default = "default_ue_veth_mtu")]
    pub veth_mtu: u32,
    /// Stage 2b gate: move the VoWiFi TUN device into the UE namespace after
    /// creation. Defaults to false because the host-side SIP/RTP sockets still
    /// bind by device name and cannot follow the TUN into another namespace
    /// yet; enable only after the VoWiFi sockets are migrated into the worker.
    #[serde(default)]
    pub vowifi_tun_in_namespace: bool,
    /// Stage 3 gate: place 3GPP IMS (LTE today, NR later) bearer networking,
    /// SIP/XFRM and RTP sockets in the per-line worker namespace. The hardware
    /// bearer stays device-owned and the host path remains the safe fallback.
    #[serde(default)]
    pub three_gpp_ims_sockets_in_worker: bool,
    /// Stage 4 gate: run per-line proxy outbound sockets through its worker.
    #[serde(default)]
    pub data_proxy_in_worker: bool,
    /// Stage 4 gate for trunk media sockets. Signalling and dialog ownership
    /// are already line-scoped; this controls only operator RTP socket creation.
    /// Depends on `three_gpp_ims_sockets_in_worker`; read it through
    /// [`UeIsolationConfig::effective_trunk_sockets_in_worker`].
    #[serde(default)]
    pub trunk_sockets_in_worker: bool,
}

impl UeIsolationConfig {
    /// Whether operator RTP sockets may actually be created inside the worker.
    ///
    /// Trunk media can only follow a bearer that already lives in the UE
    /// namespace. Enabling this gate alone would advertise a worker that cannot
    /// see the bearer interface, so the RTP socket would either fail to bind or
    /// silently egress through an ambiguous host route — a half-migrated state
    /// that reads as "enabled" while traffic still leaves via the host.
    pub fn effective_trunk_sockets_in_worker(&self) -> bool {
        self.trunk_sockets_in_worker && self.three_gpp_ims_sockets_in_worker
    }

    /// True when the trunk gate is set but suppressed by its missing
    /// dependency. Callers use this to explain the ignored setting.
    pub fn trunk_sockets_gate_suppressed(&self) -> bool {
        self.trunk_sockets_in_worker && !self.three_gpp_ims_sockets_in_worker
    }
}

impl Default for UeIsolationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            namespace_prefix: default_ue_namespace_prefix(),
            host_veth_prefix: default_ue_host_veth_prefix(),
            ue_veth_prefix: default_ue_veth_prefix(),
            veth_mtu: default_ue_veth_mtu(),
            vowifi_tun_in_namespace: false,
            three_gpp_ims_sockets_in_worker: false,
            data_proxy_in_worker: false,
            trunk_sockets_in_worker: false,
        }
    }
}

fn default_ue_namespace_prefix() -> String {
    crate::platform::netns::DEFAULT_NAMESPACE_PREFIX.to_string()
}

fn default_ue_host_veth_prefix() -> String {
    crate::platform::netns::DEFAULT_HOST_VETH_PREFIX.to_string()
}

fn default_ue_veth_prefix() -> String {
    crate::platform::netns::DEFAULT_UE_VETH_PREFIX.to_string()
}

fn default_ue_veth_mtu() -> u32 {
    crate::platform::netns::DEFAULT_VETH_MTU
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct VersionUpdateNotificationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub proxy_prefix: String,
    #[serde(default)]
    pub last_notified_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GithubDownloadProxyConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_github_download_proxy_prefix")]
    pub proxy_prefix: String,
}

fn default_github_download_proxy_prefix() -> String {
    "https://gh-proxy.com/".to_string()
}

impl Default for GithubDownloadProxyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            proxy_prefix: default_github_download_proxy_prefix(),
        }
    }
}

/// On-disk diagnostic log settings.
///
/// The web UI keeps only the newest handful of activity entries; anything older
/// exists solely in this file, which is the sole record when a field failure has
/// to be reconstructed after the fact. Retention is enforced by whichever bound
/// trips first — age or total bytes — so a burst of registration retries cannot
/// fill the device flash and an idle device still ages its history out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DiagnosticLogConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Files older than this are deleted by the cleanup pass.
    #[serde(default = "default_diagnostic_log_retention_days")]
    pub retention_days: u32,
    /// Combined ceiling across all rotated files, in mebibytes.
    #[serde(default = "default_diagnostic_log_max_total_mb")]
    pub max_total_mb: u32,
    /// Lowest severity written to disk.
    #[serde(default)]
    pub min_severity: DiagnosticLogSeverity,
    /// Mask subscriber identifiers (IMSI/IMPI/IMPU), phone numbers, SMS bodies
    /// and P-CSCF addresses. On by default: the download endpoint hands the file
    /// to anyone who can log in, and the recovery path for the common failures
    /// (SIP status codes, error chains, stage transitions) does not need PII.
    #[serde(default = "default_true")]
    pub redact_sensitive: bool,
    /// Directory override. Empty means the platform default.
    #[serde(default)]
    pub directory: Option<String>,
}

/// Severity ladder for on-disk diagnostic lines.
///
/// Deliberately ordered so `PartialOrd` expresses "at least as severe as", which
/// is how the writer applies `min_severity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticLogSeverity {
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl DiagnosticLogSeverity {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

fn default_diagnostic_log_retention_days() -> u32 {
    7
}

fn default_diagnostic_log_max_total_mb() -> u32 {
    50
}

impl Default for DiagnosticLogConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_days: default_diagnostic_log_retention_days(),
            max_total_mb: default_diagnostic_log_max_total_mb(),
            min_severity: DiagnosticLogSeverity::default(),
            redact_sensitive: true,
            directory: None,
        }
    }
}

impl DiagnosticLogConfig {
    /// Reject values that would disable retention entirely or overflow the byte
    /// math, so a bad API payload cannot turn the log into an unbounded writer.
    pub fn validate(&self) -> Result<(), String> {
        if self.retention_days == 0 || self.retention_days > 365 {
            return Err("日志保留天数需在 1-365 之间".to_string());
        }
        if self.max_total_mb == 0 || self.max_total_mb > 4096 {
            return Err("日志体积上限需在 1-4096 MB 之间".to_string());
        }
        Ok(())
    }

    pub fn max_total_bytes(&self) -> u64 {
        u64::from(self.max_total_mb) * 1024 * 1024
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SecurityConfig {
    #[serde(default = "default_true")]
    pub password_protection_enabled: bool,
    #[serde(default = "default_password_min_length")]
    pub password_min_length: u8,
    #[serde(default = "default_true")]
    pub password_require_letters: bool,
    #[serde(default = "default_true")]
    pub password_require_digits: bool,
    #[serde(default = "default_true")]
    pub password_require_symbols: bool,
    #[serde(default = "default_session_ttl_seconds")]
    pub session_ttl_seconds: i64,
    #[serde(default = "default_idle_timeout_seconds")]
    pub idle_timeout_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DdnsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_ddns_provider")]
    pub provider: String,
    #[serde(default)]
    pub access_id: String,
    #[serde(default)]
    pub access_secret: String,
    #[serde(default = "default_ddns_interval_seconds")]
    pub interval_seconds: u64,
    #[serde(default = "default_ddns_ttl")]
    pub ttl: u32,
    #[serde(default)]
    pub ipv4: DdnsIpConfig,
    #[serde(default = "default_ddns_ipv6_config")]
    pub ipv6: DdnsIpConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DdnsIpConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_ddns_get_type")]
    pub get_type: String,
    #[serde(default)]
    pub interface_name: String,
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub domains: Vec<String>,
}

fn default_true() -> bool {
    true
}

/// 默认短信模板
fn default_sms_template() -> String {
    r#"{
  "msg_type": "text",
  "content": {
    "text": "📱 短信通知\n号码: {{phone_number}}\n内容: {{content}}\n时间: {{timestamp}}\n路径: {{transport}}\n来源: {{own_number}}"
  }
}"#
    .to_string()
}

/// 默认通话模板
fn default_call_template() -> String {
    r#"{
  "msg_type": "text",
  "content": {
    "text": "📞 来电通知\n号码: {{phone_number}}\n类型: {{direction}}\n时间: {{start_time}}\n时长: {{duration}}秒\n已接听: {{answered}}"
  }
}"#.to_string()
}

fn default_ddns_template() -> String {
    r#"{
  "msg_type": "text",
  "content": {
    "text": "SimAdmin DDNS 通知\n域名: {{domains}}\nIP类型: {{ip_type}}\n新IP: {{new_ip}}\n旧IP: {{old_ip}}\n服务商: {{provider}}\n记录类型: {{record_type}}\n状态: {{status}}\n消息: {{message}}\n更新时间: {{timestamp}}"
  }
}"#
    .to_string()
}

fn default_update_template() -> String {
    r#"{
  "msg_type": "text",
  "content": {
    "text": "🚀 SimAdmin 发现新版本\n固件包: {{asset_name}}\n版本号: {{version}}\nCommit: {{commit}}\n时间: {{time}}\n来源: {{own_number}}\n\n请前往 OTA 在线更新模块检测版本，一键下载并升级。"
  }
}"#
    .to_string()
}

fn default_plain_sms_template() -> String {
    "📱 短信通知\n号码: {{发送方号码}}\n内容: {{短信内容}}\n时间: {{时间}}\n路径: {{短信途径}}\n来源: {{本机号码}}"
        .to_string()
}

fn default_plain_call_template() -> String {
    "📞 来电通知\n号码: {{phone_number}}\n类型: {{direction}}\n时间: {{start_time}}\n时长: {{duration}}秒\n已接听: {{answered}}".to_string()
}

fn default_plain_ddns_template() -> String {
    "SimAdmin DDNS 通知\n域名: {{域名}}\nIP类型: {{IP类型}}\n新IP: {{新IP}}\n旧IP: {{旧IP}}\n服务商: {{服务商}}\n记录类型: {{记录类型}}\n状态: {{状态}}\n消息: {{消息}}\n更新时间: {{更新时间}}".to_string()
}

fn default_plain_update_template() -> String {
    "🚀 SimAdmin 发现新版本\n固件包: {{固件包}}\n版本号: {{版本号}}\nCommit: {{Commit}}\n时间: {{时间}}\n来源: {{本机号码}}\n\n请前往 OTA 在线更新模块检测版本，一键下载并升级。".to_string()
}

fn default_sms_title_template() -> String {
    "SimAdmin 短信通知".to_string()
}

fn default_bark_server_url() -> String {
    "https://api.day.app".to_string()
}

fn default_pushplus_template() -> String {
    "txt".to_string()
}

fn default_wecom_to_user() -> String {
    "@all".to_string()
}

fn default_dingtalk_msg_key() -> String {
    "sampleText".to_string()
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            forward_sms: true,
            forward_calls: true,
            forward_ddns: true,
            forward_updates: true,
            headers: HashMap::new(),
            secret: String::new(),
            sms_template: default_sms_template(),
            call_template: default_call_template(),
            ddns_template: default_ddns_template(),
            update_template: default_update_template(),
        }
    }
}

impl Default for MessageChannelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            forward_sms: true,
            forward_calls: true,
            forward_ddns: true,
            forward_updates: true,
            sms_template: default_plain_sms_template(),
            call_template: default_plain_call_template(),
            ddns_template: default_plain_ddns_template(),
            update_template: default_plain_update_template(),
        }
    }
}

impl Default for BarkConfig {
    fn default() -> Self {
        Self {
            common: MessageChannelConfig::default(),
            server_url: default_bark_server_url(),
            device_key: String::new(),
            title_template: default_sms_title_template(),
            group: String::new(),
            sound: String::new(),
            level: String::new(),
            icon: String::new(),
            click_url: String::new(),
            copy: String::new(),
            auto_copy: false,
            save_history: true,
        }
    }
}

impl Default for PushPlusConfig {
    fn default() -> Self {
        Self {
            common: MessageChannelConfig::default(),
            token: String::new(),
            title_template: default_sms_title_template(),
            topic: String::new(),
            template: default_pushplus_template(),
            channel: String::new(),
            option: String::new(),
            callback_url: String::new(),
        }
    }
}

impl Default for WecomAppConfig {
    fn default() -> Self {
        Self {
            common: MessageChannelConfig::default(),
            corp_id: String::new(),
            agent_id: String::new(),
            secret: String::new(),
            to_user: default_wecom_to_user(),
            to_party: String::new(),
            to_tag: String::new(),
            safe: false,
        }
    }
}

impl Default for DingtalkAppConfig {
    fn default() -> Self {
        Self {
            common: MessageChannelConfig::default(),
            app_key: String::new(),
            app_secret: String::new(),
            robot_code: String::new(),
            open_conversation_id: String::new(),
            msg_key: default_dingtalk_msg_key(),
        }
    }
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            common: MessageChannelConfig::default(),
            bot_token: String::new(),
            chat_id: String::new(),
            parse_mode: String::new(),
            disable_web_page_preview: true,
        }
    }
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            version: 2,
            channels: Vec::new(),
            rules: Vec::new(),
            log_cleanup: NotificationLogCleanupConfig::default(),
        }
    }
}

struct LegacyChannelMigration {
    id: String,
    channel_type: NotificationChannel,
    name: String,
    enabled: bool,
    config: Value,
    forward_sms: bool,
    forward_calls: bool,
    forward_ddns: bool,
    forward_updates: bool,
    sms_template: String,
    call_template: String,
    ddns_template: String,
    update_template: String,
}

impl NotificationConfig {
    pub fn from_legacy(legacy: LegacyNotificationConfig) -> Self {
        let migrations = legacy_channel_migrations(&legacy);
        let channels = migrations
            .iter()
            .map(|item| NotificationChannelInstance {
                id: item.id.clone(),
                channel_type: item.channel_type,
                name: item.name.clone(),
                enabled: item.enabled,
                rate_limit: NotificationRateLimitConfig::default(),
                config: item.config.clone(),
            })
            .collect::<Vec<_>>();

        let mut rules = Vec::new();
        push_legacy_rule(
            &mut rules,
            NotificationEventType::Sms,
            "默认短信转发",
            "legacy-sms",
            &migrations,
        );
        push_legacy_rule(
            &mut rules,
            NotificationEventType::Call,
            "默认通话转发",
            "legacy-call",
            &migrations,
        );
        push_legacy_rule(
            &mut rules,
            NotificationEventType::Ddns,
            "默认 DDNS 转发",
            "legacy-ddns",
            &migrations,
        );
        push_legacy_rule(
            &mut rules,
            NotificationEventType::VersionUpdate,
            "默认版本更新转发",
            "legacy-version-update",
            &migrations,
        );

        Self {
            version: 2,
            channels,
            rules,
            log_cleanup: NotificationLogCleanupConfig::default(),
        }
    }
}

fn channel_label(channel: NotificationChannel) -> &'static str {
    match channel {
        NotificationChannel::Webhook => "Webhook",
        NotificationChannel::Bark => "Bark",
        NotificationChannel::PushPlus => "PushPlus",
        NotificationChannel::WecomApp => "企业微信应用消息",
        NotificationChannel::WecomRobot => "企业微信群机器人",
        NotificationChannel::DingtalkRobot => "钉钉群自定义机器人",
        NotificationChannel::DingtalkApp => "钉钉企业内机器人",
        NotificationChannel::FeishuRobot => "飞书机器人",
        NotificationChannel::Telegram => "Telegram 机器人",
    }
}

fn config_value<T: Serialize>(config: &T) -> Value {
    let mut value = serde_json::to_value(config).unwrap_or(Value::Object(Default::default()));
    strip_legacy_channel_fields(&mut value);
    value
}

fn strip_legacy_channel_fields(value: &mut Value) -> bool {
    const LEGACY_FIELDS: [&str; 9] = [
        "enabled",
        "forward_sms",
        "forward_calls",
        "forward_ddns",
        "forward_updates",
        "sms_template",
        "call_template",
        "ddns_template",
        "update_template",
    ];

    let Some(object) = value.as_object_mut() else {
        return false;
    };
    let mut changed = false;
    for field in LEGACY_FIELDS {
        changed |= object.remove(field).is_some();
    }
    if let Some(common) = object.get_mut("common").and_then(Value::as_object_mut) {
        for field in LEGACY_FIELDS {
            changed |= common.remove(field).is_some();
        }
        if common.is_empty() {
            object.remove("common");
        }
    }
    changed
}

fn strip_legacy_notification_channel_fields(config: &mut NotificationConfig) -> bool {
    config.channels.iter_mut().fold(false, |changed, channel| {
        strip_legacy_channel_fields(&mut channel.config) || changed
    })
}

fn legacy_channel_migrations(legacy: &LegacyNotificationConfig) -> Vec<LegacyChannelMigration> {
    let mut channels = Vec::new();

    if legacy.webhook.enabled || !legacy.webhook.url.trim().is_empty() {
        channels.push(LegacyChannelMigration {
            id: "webhook-1".to_string(),
            channel_type: NotificationChannel::Webhook,
            name: channel_label(NotificationChannel::Webhook).to_string(),
            enabled: legacy.webhook.enabled,
            config: config_value(&legacy.webhook),
            forward_sms: legacy.webhook.forward_sms,
            forward_calls: legacy.webhook.forward_calls,
            forward_ddns: legacy.webhook.forward_ddns,
            forward_updates: legacy.webhook.forward_updates,
            sms_template: webhook_text_template(
                &legacy.webhook.sms_template,
                &default_rule_template(NotificationEventType::Sms),
            ),
            call_template: webhook_text_template(
                &legacy.webhook.call_template,
                &default_rule_template(NotificationEventType::Call),
            ),
            ddns_template: webhook_text_template(
                &legacy.webhook.ddns_template,
                &default_rule_template(NotificationEventType::Ddns),
            ),
            update_template: webhook_text_template(
                &legacy.webhook.update_template,
                &default_rule_template(NotificationEventType::VersionUpdate),
            ),
        });
    }

    push_message_channel_migration(
        &mut channels,
        NotificationChannel::Bark,
        "bark-1",
        &legacy.bark.common,
        &legacy.bark,
        legacy.bark.common.enabled || !legacy.bark.device_key.trim().is_empty(),
    );
    push_message_channel_migration(
        &mut channels,
        NotificationChannel::PushPlus,
        "pushplus-1",
        &legacy.pushplus.common,
        &legacy.pushplus,
        legacy.pushplus.common.enabled || !legacy.pushplus.token.trim().is_empty(),
    );
    push_message_channel_migration(
        &mut channels,
        NotificationChannel::WecomApp,
        "wecom-app-1",
        &legacy.wecom_app.common,
        &legacy.wecom_app,
        legacy.wecom_app.common.enabled
            || !legacy.wecom_app.corp_id.trim().is_empty()
            || !legacy.wecom_app.agent_id.trim().is_empty()
            || !legacy.wecom_app.secret.trim().is_empty(),
    );
    push_message_channel_migration(
        &mut channels,
        NotificationChannel::WecomRobot,
        "wecom-robot-1",
        &legacy.wecom_robot.common,
        &legacy.wecom_robot,
        legacy.wecom_robot.common.enabled
            || !legacy.wecom_robot.webhook_url.trim().is_empty()
            || !legacy.wecom_robot.key.trim().is_empty(),
    );
    push_message_channel_migration(
        &mut channels,
        NotificationChannel::DingtalkRobot,
        "dingtalk-robot-1",
        &legacy.dingtalk_robot.common,
        &legacy.dingtalk_robot,
        legacy.dingtalk_robot.common.enabled
            || !legacy.dingtalk_robot.webhook_url.trim().is_empty()
            || !legacy.dingtalk_robot.access_token.trim().is_empty(),
    );
    push_message_channel_migration(
        &mut channels,
        NotificationChannel::DingtalkApp,
        "dingtalk-app-1",
        &legacy.dingtalk_app.common,
        &legacy.dingtalk_app,
        legacy.dingtalk_app.common.enabled
            || !legacy.dingtalk_app.app_key.trim().is_empty()
            || !legacy.dingtalk_app.app_secret.trim().is_empty()
            || !legacy.dingtalk_app.open_conversation_id.trim().is_empty(),
    );
    push_message_channel_migration(
        &mut channels,
        NotificationChannel::FeishuRobot,
        "feishu-robot-1",
        &legacy.feishu_robot.common,
        &legacy.feishu_robot,
        legacy.feishu_robot.common.enabled
            || !legacy.feishu_robot.webhook_url.trim().is_empty()
            || !legacy.feishu_robot.token.trim().is_empty(),
    );
    push_message_channel_migration(
        &mut channels,
        NotificationChannel::Telegram,
        "telegram-1",
        &legacy.telegram.common,
        &legacy.telegram,
        legacy.telegram.common.enabled
            || !legacy.telegram.bot_token.trim().is_empty()
            || !legacy.telegram.chat_id.trim().is_empty(),
    );

    channels
}

fn push_message_channel_migration<T: Serialize>(
    channels: &mut Vec<LegacyChannelMigration>,
    channel_type: NotificationChannel,
    id: &str,
    common: &MessageChannelConfig,
    config: &T,
    configured: bool,
) {
    if !configured {
        return;
    }
    channels.push(LegacyChannelMigration {
        id: id.to_string(),
        channel_type,
        name: channel_label(channel_type).to_string(),
        enabled: common.enabled,
        config: config_value(config),
        forward_sms: common.forward_sms,
        forward_calls: common.forward_calls,
        forward_ddns: common.forward_ddns,
        forward_updates: common.forward_updates,
        sms_template: non_empty_template(&common.sms_template, NotificationEventType::Sms),
        call_template: non_empty_template(&common.call_template, NotificationEventType::Call),
        ddns_template: non_empty_template(&common.ddns_template, NotificationEventType::Ddns),
        update_template: non_empty_template(
            &common.update_template,
            NotificationEventType::VersionUpdate,
        ),
    });
}

fn push_legacy_rule(
    rules: &mut Vec<NotificationRule>,
    event_type: NotificationEventType,
    name: &str,
    id: &str,
    channels: &[LegacyChannelMigration],
) {
    let selected = channels
        .iter()
        .filter(|channel| match event_type {
            NotificationEventType::Sms => channel.forward_sms,
            NotificationEventType::Call => channel.forward_calls,
            NotificationEventType::Ddns => channel.forward_ddns,
            NotificationEventType::VersionUpdate => channel.forward_updates,
            NotificationEventType::SystemEvent => false,
            NotificationEventType::DeviceStatus => false,
            NotificationEventType::Automation => false,
        })
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return;
    }

    let template = selected
        .first()
        .map(|channel| match event_type {
            NotificationEventType::Sms => channel.sms_template.clone(),
            NotificationEventType::Call => channel.call_template.clone(),
            NotificationEventType::Ddns => channel.ddns_template.clone(),
            NotificationEventType::VersionUpdate => channel.update_template.clone(),
            NotificationEventType::SystemEvent => String::new(),
            NotificationEventType::DeviceStatus => String::new(),
            NotificationEventType::Automation => String::new(),
        })
        .unwrap_or_else(|| default_rule_template(event_type));

    rules.push(NotificationRule {
        id: id.to_string(),
        event_type,
        name: name.to_string(),
        enabled: true,
        matcher: RuleMatcher::default(),
        channel_ids: selected
            .into_iter()
            .map(|channel| channel.id.clone())
            .collect(),
        sim_channel_ids: Vec::new(),
        event_codes: Vec::new(),
        template,
        quiet_hours: Vec::new(),
        ddns_failure_threshold: default_ddns_failure_threshold(),
        device_status_items: default_device_status_items(),
        device_status_schedule: DeviceStatusSchedule::default(),
        device_status_sms_period: default_device_status_sms_period(),
    });
}

fn non_empty_template(template: &str, event_type: NotificationEventType) -> String {
    if template.trim().is_empty() {
        default_rule_template(event_type)
    } else {
        template.to_string()
    }
}

fn webhook_text_template(template: &str, fallback: &str) -> String {
    if template.trim().is_empty() {
        return fallback.to_string();
    }
    if let Ok(value) = serde_json::from_str::<Value>(template) {
        if let Some(text) = value
            .get("content")
            .and_then(|content| content.get("text"))
            .and_then(Value::as_str)
        {
            return text.replace("\\n", "\n");
        }
        if let Some(text) = value.get("text").and_then(Value::as_str) {
            return text.replace("\\n", "\n");
        }
    }
    template.to_string()
}

pub fn default_rule_template(event_type: NotificationEventType) -> String {
    match event_type {
        NotificationEventType::Sms => {
            "📱 短信通知\nSIM通道: {{SIM通道}}\n号码: {{发送方号码}}\n内容: {{短信内容}}\n时间: {{时间}}\n路径: {{短信途径}}\n来源: {{本机号码}}".to_string()
        }
        NotificationEventType::Call => {
            "📞 通话通知\n线路: {{线路ID}}\n号码: {{电话号码}}\n方向: {{方向}}\n时间: {{开始时间}}\n时长: {{时长}} 秒\n已接听: {{已接听}}".to_string()
        }
        NotificationEventType::Ddns => {
            "DDNS 通知\n域名: {{域名}}\nIP 类型: {{IP类型}}\n新 IP: {{新IP}}\n旧 IP: {{旧IP}}\n服务商: {{服务商}}\n记录类型: {{记录类型}}\n状态: {{状态}}\n消息: {{消息}}\n更新时间: {{更新时间}}".to_string()
        }
        NotificationEventType::VersionUpdate => {
            "🚀 SimAdmin 发现新版本\n固件包: {{固件包}}\n版本号: {{版本号}}\nCommit: {{Commit}}\n构建时间: {{构建时间}}\nMD5: {{MD5}}\n来源: {{本机号码}}".to_string()
        }
        NotificationEventType::SystemEvent => {
            "系统事件通知\n分类: {{分类}}\n事件: {{事件}}\n等级: {{等级}}\n状态: {{状态}}\n对象: {{对象}}\n消息: {{消息}}\n时间: {{时间}}".to_string()
        }
        NotificationEventType::DeviceStatus => {
            "设备状态报告\n【{{状态分类}}】\n{{状态内容}}\n\n时间: {{时间}}".to_string()
        }
        NotificationEventType::Automation => {
            "🤖 自动化事件通知\n线路: {{线路ID}}\n任务名称: {{任务名称}}\n任务类型: {{任务类型}}\n执行状态: {{任务状态}}\n详情: {{任务详情}}\n时间: {{触发时间}}\n来源: {{本机号码}}".to_string()
        }
    }
}

impl Default for VersionUpdateNotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            proxy_prefix: String::new(),
            last_notified_version: None,
        }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            password_protection_enabled: true,
            password_min_length: default_password_min_length(),
            password_require_letters: true,
            password_require_digits: true,
            password_require_symbols: true,
            session_ttl_seconds: default_session_ttl_seconds(),
            idle_timeout_seconds: default_idle_timeout_seconds(),
        }
    }
}

impl Default for DdnsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_ddns_provider(),
            access_id: String::new(),
            access_secret: String::new(),
            interval_seconds: default_ddns_interval_seconds(),
            ttl: default_ddns_ttl(),
            ipv4: DdnsIpConfig {
                enabled: true,
                get_type: default_ddns_get_type(),
                interface_name: String::new(),
                urls: default_ddns_ipv4_urls(),
                domains: Vec::new(),
            },
            ipv6: default_ddns_ipv6_config(),
        }
    }
}

impl Default for DdnsIpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            get_type: default_ddns_get_type(),
            interface_name: String::new(),
            urls: Vec::new(),
            domains: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub tasks: Vec<AutomationTask>,
}

impl Default for AutomationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            tasks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationTask {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub trigger: AutomationTrigger,
    /// SIM-dependent actions must pin execution to a persistent modem/SIM line
    /// or an external reader reservation. An absent target is valid only for
    /// device-wide actions such as rebooting the host.
    #[serde(default)]
    pub target: Option<AutomationTarget>,
    pub action: AutomationAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AutomationTarget {
    ModemLine { line_id: String },
    StandaloneSimSlot { slot_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "config", rename_all = "snake_case")]
pub enum AutomationTrigger {
    Fixed {
        weekdays: Vec<u8>,
        times: Vec<String>,
    },
    Interval {
        interval_value: u64,
        interval_unit: String,
    },
    Cron {
        expression: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "config", rename_all = "snake_case")]
pub enum AutomationAction {
    RestartBaseband,
    RebootDevice {
        delay_seconds: u32,
    },
    SendSms {
        phone_number: String,
        content: String,
        random_delay_seconds: Option<u32>,
        retry_limit: Option<u32>,
    },
    ConsumeData {
        bytes: u64,
        unit: String,
    },
    DialCall {
        country_code: String,
        phone_number: String,
        duration_seconds: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trunk media cannot enter a worker the bearer has not moved into, so the
    /// trunk gate alone must never advertise a worker-backed RTP path.
    #[test]
    fn trunk_socket_gate_requires_the_three_gpp_bearer_gate() {
        let mut isolation = UeIsolationConfig {
            trunk_sockets_in_worker: true,
            ..UeIsolationConfig::default()
        };
        assert!(!isolation.effective_trunk_sockets_in_worker());
        assert!(isolation.trunk_sockets_gate_suppressed());

        isolation.three_gpp_ims_sockets_in_worker = true;
        assert!(isolation.effective_trunk_sockets_in_worker());
        assert!(!isolation.trunk_sockets_gate_suppressed());

        isolation.trunk_sockets_in_worker = false;
        assert!(!isolation.effective_trunk_sockets_in_worker());
        assert!(!isolation.trunk_sockets_gate_suppressed());
    }

    #[test]
    fn notification_channel_accepts_frontend_pushplus_key() {
        assert!(matches!(
            serde_json::from_str::<NotificationChannel>(r#""pushplus""#).unwrap(),
            NotificationChannel::PushPlus
        ));
        assert!(matches!(
            serde_json::from_str::<NotificationChannel>(r#""push_plus""#).unwrap(),
            NotificationChannel::PushPlus
        ));
        assert_eq!(
            serde_json::to_string(&NotificationChannel::PushPlus).unwrap(),
            r#""pushplus""#
        );
    }

    #[test]
    fn old_config_defaults_to_no_explicit_line_profiles() {
        let config: AppConfig = serde_json::from_str("{}").unwrap();
        assert!(config.line_profiles.is_empty());
        assert!(config.modem_slots.is_empty());
        assert!(LineProfileConfig::for_line("line-test").enabled);
    }

    #[test]
    fn sim_dependent_automation_requires_an_explicit_target() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-automation-target-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let manager = ConfigManager::new(path.clone());
        let mut task = AutomationTask {
            id: "task-a".to_string(),
            name: "restart".to_string(),
            enabled: true,
            trigger: AutomationTrigger::Interval {
                interval_value: 1,
                interval_unit: "hours".to_string(),
            },
            target: None,
            action: AutomationAction::RestartBaseband,
        };
        assert_eq!(
            manager
                .set_automation_config(AutomationConfig {
                    enabled: true,
                    tasks: vec![task.clone()],
                })
                .unwrap_err(),
            "automation_target_line_required"
        );

        task.action = AutomationAction::RebootDevice { delay_seconds: 0 };
        manager
            .set_automation_config(AutomationConfig {
                enabled: true,
                tasks: vec![task],
            })
            .expect("device-wide reboot does not need a line");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn modem_slot_reconciliation_keeps_order_across_sim_changes_and_uim_slots() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-modem-slots-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let manager = ConfigManager::new(path.clone());
        let first = manager
            .reconcile_modem_slots(&[
                ModemSlotObservation {
                    slot_id: "usb-path-b".to_string(),
                    equipment_identifier: "imei-b".to_string(),
                    uim_slot: 1,
                    ..Default::default()
                },
                ModemSlotObservation {
                    slot_id: "usb-path-a".to_string(),
                    equipment_identifier: "imei-a".to_string(),
                    uim_slot: 1,
                    ..Default::default()
                },
            ])
            .unwrap();
        assert_eq!(first["usb-path-a#uim1"].order, 1);
        assert_eq!(first["usb-path-b#uim1"].order, 2);

        let second = manager
            .reconcile_modem_slots(&[
                ModemSlotObservation {
                    slot_id: "usb-path-b".to_string(),
                    equipment_identifier: "imei-b".to_string(),
                    uim_slot: 1,
                    ..Default::default()
                },
                ModemSlotObservation {
                    slot_id: "usb-path-a".to_string(),
                    equipment_identifier: "imei-a".to_string(),
                    uim_slot: 1,
                    ..Default::default()
                },
            ])
            .unwrap();
        assert_eq!(second["usb-path-a#uim1"].order, 1);
        assert_eq!(second["usb-path-b#uim1"].order, 2);

        let third = manager
            .reconcile_modem_slots(&[ModemSlotObservation {
                slot_id: "usb-path-a".to_string(),
                equipment_identifier: "imei-a".to_string(),
                uim_slot: 2,
                ..Default::default()
            }])
            .unwrap();
        assert_eq!(third["usb-path-a#uim2"].order, 3);
        assert_eq!(third["usb-path-a#uim1"].order, 1);

        let reloaded = ConfigManager::new(path.clone());
        let persisted = reloaded
            .reconcile_modem_slots(&[ModemSlotObservation {
                slot_id: "usb-path-a".to_string(),
                equipment_identifier: "imei-a".to_string(),
                uim_slot: 2,
                ..Default::default()
            }])
            .unwrap();
        assert_eq!(persisted["usb-path-a#uim2"].order, 3);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn legacy_modem_slot_migrates_to_physical_anchor_without_changing_order() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-slot-migration-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let manager = ConfigManager::new(path.clone());
        {
            let mut config = manager.config.write().unwrap();
            config.modem_slots.push(ModemSlotConfig {
                hardware_key: "legacy-device-id".to_string(),
                uim_slot: 1,
                order: 4,
                label: "机柜卡槽 4".to_string(),
                ..Default::default()
            });
        }
        manager.save().unwrap();

        let slots = manager
            .reconcile_modem_slots(&[ModemSlotObservation {
                slot_id: "sysfs:devices/platform/slot-4".to_string(),
                legacy_hardware_keys: vec!["legacy-device-id".to_string()],
                equipment_identifier: "imei-new".to_string(),
                uim_slot: 1,
            }])
            .unwrap();
        let migrated = &slots["sysfs:devices/platform/slot-4#uim1"];
        assert_eq!(migrated.order, 4);
        assert_eq!(migrated.label, "机柜卡槽 4");
        assert_eq!(migrated.equipment_identifier, "imei-new");

        let reloaded = ConfigManager::new(path.clone());
        let persisted = reloaded.config.read().unwrap().modem_slots[0].clone();
        assert_eq!(persisted.slot_id, "sysfs:devices/platform/slot-4");
        let _ = std::fs::remove_file(path.with_extension("bak"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn legacy_line_profile_is_copied_to_physical_slot_line_id() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-line-migration-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let manager = ConfigManager::new(path.clone());
        let legacy_line_id = "line-11111111111111111111111111111111";
        let current_line_id = "line-22222222222222222222222222222222";
        {
            let mut config = manager.config.write().unwrap();
            let mut profile = LineProfileConfig::for_line(legacy_line_id);
            profile.volte_connection_enabled = true;
            profile.trunk.context = "from-migrated-slot".to_string();
            config.line_profiles.push(profile);
            config.automation.tasks.push(AutomationTask {
                id: "legacy-line-task".to_string(),
                name: "legacy line task".to_string(),
                enabled: true,
                trigger: AutomationTrigger::Interval {
                    interval_value: 1,
                    interval_unit: "hours".to_string(),
                },
                target: Some(AutomationTarget::ModemLine {
                    line_id: legacy_line_id.to_string(),
                }),
                action: AutomationAction::RestartBaseband,
            });
            let mut rule: NotificationRule = serde_json::from_value(serde_json::json!({
                "id": "legacy-line-rule",
                "type": "sms",
                "name": "legacy line rule"
            }))
            .unwrap();
            rule.sim_channel_ids = vec![legacy_line_id.to_string(), current_line_id.to_string()];
            config.notifications.rules.push(rule);
        }
        manager.save().unwrap();

        assert!(manager
            .migrate_line_profile_aliases(current_line_id, &[legacy_line_id.to_string()])
            .unwrap());
        let migrated = manager.get_line_profile(current_line_id);
        assert!(migrated.volte_connection_enabled);
        assert_eq!(migrated.trunk.context, "from-migrated-slot");
        let config = manager.config.read().unwrap();
        assert!(matches!(
            config.automation.tasks[0].target.as_ref(),
            Some(AutomationTarget::ModemLine { line_id }) if line_id == current_line_id
        ));
        assert_eq!(
            config.notifications.rules[0].sim_channel_ids,
            vec![current_line_id.to_string()]
        );
        drop(config);
        assert!(!manager
            .migrate_line_profile_aliases(current_line_id, &[legacy_line_id.to_string()])
            .unwrap());

        let _ = std::fs::remove_file(path.with_extension("bak"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn standalone_reader_targets_migrate_to_unified_line() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-reader-target-migration-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let manager = ConfigManager::new(path.clone());
        let line_id = "line-33333333333333333333333333333333";
        {
            let mut config = manager.config.write().unwrap();
            config.automation.tasks.push(AutomationTask {
                id: "reader-sms".to_string(),
                name: "reader sms".to_string(),
                enabled: true,
                trigger: AutomationTrigger::Interval {
                    interval_value: 1,
                    interval_unit: "hours".to_string(),
                },
                target: Some(AutomationTarget::StandaloneSimSlot {
                    slot_id: "usb-a".to_string(),
                }),
                action: AutomationAction::SendSms {
                    phone_number: "10086".to_string(),
                    content: "test".to_string(),
                    random_delay_seconds: Some(0),
                    retry_limit: Some(0),
                },
            });
            let mut rule: NotificationRule = serde_json::from_value(serde_json::json!({
                "id": "reader-rule",
                "type": "sms",
                "name": "reader rule"
            }))
            .unwrap();
            rule.sim_channel_ids = vec!["reader:usb-a".to_string()];
            config.notifications.rules.push(rule);
        }
        manager.save().unwrap();

        assert!(manager
            .migrate_standalone_reader_references("usb-a", line_id)
            .unwrap());
        let config = manager.config.read().unwrap();
        assert!(matches!(
            config.automation.tasks[0].target.as_ref(),
            Some(AutomationTarget::ModemLine { line_id: target }) if target == line_id
        ));
        assert_eq!(
            config.notifications.rules[0].sim_channel_ids,
            vec![line_id.to_string()]
        );
        drop(config);
        assert!(!manager
            .migrate_standalone_reader_references("usb-a", line_id)
            .unwrap());

        let _ = std::fs::remove_file(path.with_extension("bak"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn per_line_volte_connection_is_independent_and_persists() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-line-config-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let manager = ConfigManager::new(path.clone());
        let line_a = "line-0123456789abcdef0123456789abcdef";
        let line_b = "line-fedcba9876543210fedcba9876543210";
        let profile = manager
            .set_line_volte_connection_enabled(line_a, true)
            .unwrap();
        assert!(profile.volte_connection_enabled);
        assert!(!manager.get_line_profile(line_b).volte_connection_enabled);

        let reloaded = ConfigManager::new(path.clone());
        assert!(reloaded.get_line_profile(line_a).volte_connection_enabled);
        assert!(!reloaded.get_line_profile(line_b).volte_connection_enabled);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn per_line_media_config_is_explicit_and_persists() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-line-media-migration-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let line_id = "line-0123456789abcdef0123456789abcdef";
        let mut config = AppConfig::default();
        let mut profile = LineProfileConfig::for_line(line_id);
        profile.ims_video.video_payload_type = 111;
        config.line_profiles.push(profile);
        std::fs::write(&path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();

        let manager = ConfigManager::new(path.clone());
        assert!(!manager
            .reconcile_line_profiles(&[line_id.to_string()])
            .unwrap());
        let profile = manager.get_line_profile(line_id);
        assert_eq!(profile.ims_video.video_payload_type, 111);

        let persisted: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            persisted["line_profiles"][0]["ims_video"]["video_payload_type"],
            serde_json::Value::from(111)
        );

        let _ = std::fs::remove_file(path.with_extension("bak"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn line_path_policies_and_apn_are_independent() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-line-policy-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let manager = ConfigManager::new(path.clone());
        let line_a = "line-0123456789abcdef0123456789abcdef";
        let line_b = "line-fedcba9876543210fedcba9876543210";

        // Each newly discovered line receives its own canonical values.
        assert!(!manager.get_line_sms_path_policy(line_a).force_vowifi_send);
        assert_eq!(manager.get_line_apn_config(line_a), ApnConfig::default());

        // Overriding line A must not move line B.
        let mut only_vowifi = SmsPathPolicy::default();
        only_vowifi.force_vowifi_send = true;
        manager
            .set_line_sms_path_policy(line_a, only_vowifi)
            .unwrap();
        assert!(manager.get_line_sms_path_policy(line_a).force_vowifi_send);
        assert!(!manager.get_line_sms_path_policy(line_b).force_vowifi_send);

        let mut only_volte_voice = VoicePathPolicy::default();
        for layer in &mut only_volte_voice.priority {
            if layer.kind == AccessPathKind::Vowifi {
                layer.enabled = false;
            }
        }
        manager
            .set_line_voice_path_policy(line_a, only_volte_voice)
            .unwrap();
        let voice_vowifi_enabled = |policy: VoicePathPolicy| {
            policy
                .priority
                .iter()
                .any(|layer| layer.kind == AccessPathKind::Vowifi && layer.enabled)
        };
        assert!(!voice_vowifi_enabled(
            manager.get_line_voice_path_policy(line_a)
        ));
        assert!(voice_vowifi_enabled(
            manager.get_line_voice_path_policy(line_b)
        ));

        let mut line_apn = ApnConfig::default();
        line_apn.apn = "line-a-apn".to_string();
        manager.set_line_apn_config(line_a, line_apn).unwrap();
        assert_eq!(manager.get_line_apn_config(line_a).apn, "line-a-apn");
        assert_eq!(manager.get_line_apn_config(line_b), ApnConfig::default());

        // Explicit values survive a reload; submitting the initial policy only
        // changes this line.
        let reloaded = ConfigManager::new(path.clone());
        assert!(reloaded.get_line_sms_path_policy(line_a).force_vowifi_send);
        assert!(!voice_vowifi_enabled(
            reloaded.get_line_voice_path_policy(line_a)
        ));
        reloaded
            .set_line_sms_path_policy(line_a, SmsPathPolicy::default())
            .unwrap();
        reloaded
            .set_line_voice_path_policy(line_a, VoicePathPolicy::default())
            .unwrap();
        assert!(!reloaded.get_line_sms_path_policy(line_a).force_vowifi_send);
        assert!(voice_vowifi_enabled(
            reloaded.get_line_voice_path_policy(line_a)
        ));

        let _ = std::fs::remove_file(path);
    }

    /// VoLTE and VoWiFi are two access paths to the same IMS core, not two
    /// mutually exclusive modes.
    ///
    /// Enabling one must never silently disable the other. Doing so removes the
    /// fallback leg that makes a Wi-Fi drop survivable, and turns every access
    /// change into a full re-registration. "Priority switching" patches have
    /// introduced exactly that coupling here before; this test pins it shut.
    #[test]
    fn enabling_one_ims_access_never_disables_the_other() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-ims-coexistence-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let manager = ConfigManager::new(path.clone());
        let line = "line-0123456789abcdef0123456789abcdef";

        manager
            .set_line_volte_connection_enabled(line, true)
            .unwrap();
        manager
            .set_line_vowifi_connection_enabled(line, true)
            .unwrap();

        let profile = manager.get_line_profile(line);
        assert!(
            profile.volte_connection_enabled,
            "enabling the non-3GPP access must not disable the 3GPP access"
        );
        assert!(profile.vowifi.enabled);

        // Re-asserting VoLTE must likewise leave VoWiFi alone.
        manager
            .set_line_volte_connection_enabled(line, true)
            .unwrap();
        let profile = manager.get_line_profile(line);
        assert!(
            profile.vowifi.enabled,
            "enabling the 3GPP access must not disable the non-3GPP access"
        );
        assert!(profile.volte_connection_enabled);

        // Turning one leg off is a change to that leg only.
        manager
            .set_line_vowifi_connection_enabled(line, false)
            .unwrap();
        let profile = manager.get_line_profile(line);
        assert!(!profile.vowifi.enabled);
        assert!(
            profile.volte_connection_enabled,
            "disabling one access must not cascade into the other"
        );

        let _ = std::fs::remove_file(path);
    }

    /// The registration preference must never edit the enable intent.
    ///
    /// Companion to `enabling_one_ims_access_never_disables_the_other`: that test
    /// pins the two enable switches apart from each other, this one pins them
    /// apart from the preference. If setting `CellularPreferred` also cleared
    /// `vowifi.enabled`, flipping the preference back would silently fail to
    /// restore the WLAN leg.
    #[test]
    fn ims_access_preference_never_edits_the_enable_intent() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-ims-access-pref-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let manager = ConfigManager::new(path.clone());
        let line = "line-0123456789abcdef0123456789abcdef";

        // Default keeps both legs registered, matching the coexistence invariant
        // in services::orchestrator::ims_access.
        assert_eq!(
            manager.get_line_ims_access_preference(line),
            ImsAccessPreference::Concurrent
        );

        manager
            .set_line_volte_connection_enabled(line, true)
            .unwrap();
        manager
            .set_line_vowifi_connection_enabled(line, true)
            .unwrap();

        for preference in [
            ImsAccessPreference::WlanPreferred,
            ImsAccessPreference::CellularPreferred,
            ImsAccessPreference::Concurrent,
        ] {
            manager
                .set_line_ims_access_preference(line, preference)
                .unwrap();
            let profile = manager.get_line_profile(line);
            assert_eq!(profile.ims_access_preference, preference);
            assert!(
                profile.volte_connection_enabled,
                "{preference:?} must not clear the VoLTE enable intent"
            );
            assert!(
                profile.vowifi.enabled,
                "{preference:?} must not clear the VoWiFi enable intent"
            );
        }

        // And it survives a reload.
        manager
            .set_line_ims_access_preference(line, ImsAccessPreference::WlanPreferred)
            .unwrap();
        let reloaded = ConfigManager::new(path.clone());
        assert_eq!(
            reloaded.get_line_ims_access_preference(line),
            ImsAccessPreference::WlanPreferred
        );

        let _ = std::fs::remove_file(path);
    }

    /// A line that never set a preference must deserialize to the coexisting
    /// default rather than failing or silently parking a leg.
    #[test]
    fn line_profile_without_ims_access_preference_defaults_to_concurrent() {
        let profile: LineProfileConfig = serde_json::from_value(serde_json::json!({
            "line_id": "line-0123456789abcdef0123456789abcdef"
        }))
        .unwrap();
        assert_eq!(
            profile.ims_access_preference,
            ImsAccessPreference::Concurrent
        );
    }

    #[test]
    fn esim_reader_settings_are_isolated_per_line() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-esim-reader-lines-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let manager = ConfigManager::new(path.clone());
        let line_a = "line-0123456789abcdef0123456789abcdef";
        let line_b = "line-fedcba9876543210fedcba9876543210";
        let line_c = "line-00112233445566778899aabbccddeeff";

        manager
            .set_line_esim_reader_config(
                line_a,
                EsimReaderConfig {
                    apdu_backend: " pcsc ".to_string(),
                    pcsc_reader_name: " ESTKme-RED ".to_string(),
                    pcsc_reader_index: Some(2),
                    ..EsimReaderConfig::default()
                },
            )
            .unwrap();
        manager
            .set_line_esim_reader_config(
                line_b,
                EsimReaderConfig {
                    apdu_backend: "mbim".to_string(),
                    mbim_device: " /dev/cdc-wdm7 ".to_string(),
                    mbim_uim_slot: 2,
                    mbim_use_proxy: true,
                    mbim_skip_slot_mapping: true,
                    ..EsimReaderConfig::default()
                },
            )
            .unwrap();

        assert_eq!(
            manager.get_line_esim_reader_config(line_a),
            EsimReaderConfig {
                apdu_backend: "pcsc".to_string(),
                pcsc_reader_name: "ESTKme-RED".to_string(),
                pcsc_reader_index: Some(2),
                ..EsimReaderConfig::default()
            }
        );
        assert_eq!(
            manager.get_line_esim_reader_config(line_c),
            EsimReaderConfig::default()
        );

        let reloaded = ConfigManager::new(path.clone());
        let pcsc = reloaded.get_line_esim_reader_config(line_a);
        assert_eq!(pcsc.pcsc_reader_name, "ESTKme-RED");
        assert_eq!(pcsc.pcsc_reader_index, Some(2));
        let mbim = reloaded.get_line_esim_reader_config(line_b);
        assert_eq!(mbim.mbim_device, "/dev/cdc-wdm7");
        assert_eq!(mbim.mbim_uim_slot, 2);
        assert!(mbim.mbim_use_proxy);
        assert!(mbim.mbim_skip_slot_mapping);
        assert_eq!(
            reloaded.get_line_esim_reader_config(line_c),
            EsimReaderConfig::default()
        );
        let _ = std::fs::remove_file(path.with_extension("bak"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn legacy_global_esim_reader_migrates_only_for_one_line() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-esim-reader-migration-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let manager = ConfigManager::new(path.clone());
        {
            let mut config = manager.config.write().unwrap();
            config.esim = EsimConfig {
                apdu_backend: "at".to_string(),
                at_device: "/dev/ttyUSB9".to_string(),
                ..EsimConfig::default()
            };
        }
        manager.save().unwrap();
        let line_id = "line-0123456789abcdef0123456789abcdef";

        assert!(manager
            .reconcile_line_profiles(&[line_id.to_string()])
            .unwrap());
        let reader = manager.get_line_esim_reader_config(line_id);
        assert_eq!(reader.apdu_backend, "at");
        assert_eq!(reader.at_device, "/dev/ttyUSB9");
        assert!(manager.get_esim_config().at_device.is_empty());

        let reloaded = ConfigManager::new(path.clone());
        assert_eq!(
            reloaded.get_line_esim_reader_config(line_id).at_device,
            "/dev/ttyUSB9"
        );
        let _ = std::fs::remove_file(path.with_extension("bak"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn per_line_vowifi_runtime_intent_and_standalone_slots_persist() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-line-vowifi-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let manager = ConfigManager::new(path.clone());
        let line_id = "line-0123456789abcdef0123456789abcdef";
        let profile = manager
            .set_line_vowifi_config(
                line_id,
                LineVowifiConfig {
                    enabled: true,
                    proxy_mode: VowifiProxyMode::Socks5UdpAssociate,
                    proxy_endpoint: "socks5://127.0.0.1:1080".to_string(),
                    ..LineVowifiConfig::default()
                },
            )
            .unwrap();
        assert!(profile.vowifi.enabled);

        let slots = manager
            .set_standalone_sim_slots(vec![StandaloneSimSlotConfig {
                id: "reader-a".to_string(),
                label: "外置读卡器 A".to_string(),
                reader_path: "pcsc://Reader A".to_string(),
                uim_slot: 1,
                enabled: true,
            }])
            .unwrap();
        assert_eq!(slots[0].id, "reader-a");
        let reloaded = ConfigManager::new(path.clone());
        assert_eq!(reloaded.get_standalone_sim_slots().len(), 1);
        assert_eq!(
            reloaded.get_line_profile(line_id).vowifi.proxy_endpoint,
            "socks5://127.0.0.1:1080"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn per_line_airplane_mode_preserves_wifi_services() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-line-airplane-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let manager = ConfigManager::new(path.clone());
        let line_id = "line-0123456789abcdef0123456789abcdef";
        manager
            .set_line_volte_connection_enabled(line_id, true)
            .unwrap();
        manager
            .set_line_vowifi_connection_enabled(line_id, true)
            .unwrap();
        manager
            .set_line_trunk_profile(
                line_id,
                TrunkProfileConfig {
                    enabled: true,
                    asterisk_host: "pbx.example.com".to_string(),
                    local_port: 5062,
                    ..TrunkProfileConfig::default()
                },
            )
            .unwrap();
        manager
            .set_line_data_connection_enabled(line_id, true)
            .unwrap();

        let profile = manager.set_line_airplane_mode(line_id, true).unwrap();
        assert!(profile.airplane_mode_enabled);
        assert!(!profile.data_connection_enabled);
        assert!(!profile.volte_connection_enabled);
        assert!(profile.vowifi.enabled);
        assert!(profile.trunk.enabled);
        assert_eq!(
            manager
                .set_line_data_connection_enabled(line_id, true)
                .unwrap_err(),
            "line_airplane_mode_enabled"
        );

        let profile = manager.set_line_airplane_mode(line_id, false).unwrap();
        assert!(!profile.airplane_mode_enabled);
        assert!(!profile.data_connection_enabled);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn per_line_data_proxy_and_roaming_settings_are_persisted() {
        let dir = std::env::temp_dir().join(format!(
            "simadmin-line-data-proxy-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let manager = ConfigManager::new(path.clone());
        let line_id = "line-0123456789abcdef0123456789abcdef";
        let profile = manager
            .set_line_data_proxy_config(
                line_id,
                LineDataProxyConfig {
                    listen_ip: " 127.0.0.1 ".to_string(),
                    listen_port: 1080,
                    username: " proxy-user ".to_string(),
                    password: "proxy-pass".to_string(),
                },
            )
            .unwrap();
        assert_eq!(profile.data_proxy.listen_ip, "127.0.0.1");
        assert_eq!(profile.data_proxy.listen_port, 1080);
        assert_eq!(profile.data_proxy.username, "proxy-user");
        assert_eq!(profile.data_proxy.password, "proxy-pass");
        assert!(
            !manager
                .set_line_roaming_allowed(line_id, false)
                .unwrap()
                .roaming_allowed
        );

        let reloaded = ConfigManager::new(path.clone()).get_line_profile(line_id);
        assert_eq!(reloaded.data_proxy.listen_ip, "127.0.0.1");
        assert_eq!(reloaded.data_proxy.listen_port, 1080);
        assert_eq!(reloaded.data_proxy.username, "proxy-user");
        assert!(!reloaded.roaming_allowed);
        assert_eq!(
            manager
                .set_line_data_proxy_config(
                    line_id,
                    LineDataProxyConfig {
                        listen_ip: "not-an-ip".to_string(),
                        listen_port: 0,
                        ..LineDataProxyConfig::default()
                    },
                )
                .unwrap_err(),
            "data_proxy_listen_ip_invalid"
        );
        assert_eq!(
            manager
                .set_line_data_proxy_config(
                    line_id,
                    LineDataProxyConfig {
                        username: "user-only".to_string(),
                        ..LineDataProxyConfig::default()
                    },
                )
                .unwrap_err(),
            "data_proxy_auth_credentials_incomplete"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn disabled_line_rejects_data_connection_intent() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-disabled-line-data-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let manager = ConfigManager::new(path.clone());
        let line_id = "line-0123456789abcdef0123456789abcdef";
        manager
            .update_line_profile(line_id, |profile| profile.enabled = false)
            .unwrap();

        assert_eq!(
            manager
                .set_line_data_connection_enabled(line_id, true)
                .unwrap_err(),
            "line_disabled"
        );
        assert!(!manager.get_line_profile(line_id).data_connection_enabled);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn sms_path_policy_default_order_is_vowifi_volte_cs() {
        let policy = SmsPathPolicy::default();
        let order: Vec<AccessPathKind> = policy.enabled_layers().collect();
        assert_eq!(
            order,
            vec![
                AccessPathKind::Vowifi,
                AccessPathKind::Volte,
                AccessPathKind::Cs
            ]
        );
        assert!(policy.dedupe_enabled);
        assert!(policy.cs_fallback_receiver);
        assert!(!policy.force_vowifi_send);
        assert_eq!(
            policy.mid_flight_disable,
            MidFlightDisablePolicy::AutoSwitch
        );
        assert_eq!(policy.dedup_retention_days, 30);
        assert_eq!(policy.message_retention_limit, 10_000);
    }

    #[test]
    fn sms_receive_layers_are_fixed_and_independent_from_legacy_priority() {
        let policy = SmsPathPolicy {
            priority: vec![
                PathLayerConfig {
                    kind: AccessPathKind::Cs,
                    enabled: true,
                },
                PathLayerConfig {
                    kind: AccessPathKind::Volte,
                    enabled: false,
                },
                PathLayerConfig {
                    kind: AccessPathKind::Vowifi,
                    enabled: true,
                },
            ],
            ..SmsPathPolicy::default()
        };
        let ims: Vec<AccessPathKind> = policy.enabled_ims_layers().collect();
        assert_eq!(ims, vec![AccessPathKind::Vowifi, AccessPathKind::Volte]);
        assert!(policy.is_enabled(AccessPathKind::Cs));
    }

    #[test]
    fn sms_path_policy_normalized_resets_legacy_order() {
        let policy = SmsPathPolicy {
            priority: vec![
                PathLayerConfig {
                    kind: AccessPathKind::Volte,
                    enabled: false,
                },
                // duplicate should be dropped
                PathLayerConfig {
                    kind: AccessPathKind::Volte,
                    enabled: true,
                },
            ],
            ..SmsPathPolicy::default()
        }
        .normalized();
        let kinds: Vec<AccessPathKind> = policy.priority.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            vec![
                AccessPathKind::Vowifi,
                AccessPathKind::Volte,
                AccessPathKind::Cs
            ]
        );
        assert!(policy.priority.iter().all(|layer| layer.enabled));
        assert!(policy.is_enabled(AccessPathKind::Volte));
        assert!(policy.is_enabled(AccessPathKind::Vowifi));
    }

    #[test]
    fn sms_path_policy_deserializes_from_partial_json() {
        let line_id = "line-0123456789abcdef0123456789abcdef";
        let cfg: AppConfig = serde_json::from_str(&format!(
            r#"{{"line_profiles":[{{"line_id":"{line_id}"}}]}}"#
        ))
        .unwrap();
        assert_eq!(cfg.line_profiles[0].sms_path, SmsPathPolicy::default());

        let json = format!(
            r#"{{"line_profiles":[{{"line_id":"{line_id}","sms_path":{{"priority":[{{"kind":"cs","enabled":true}}]}}}}]}}"#
        );
        let cfg: AppConfig = serde_json::from_str(&json).unwrap();
        assert!(cfg.line_profiles[0].sms_path.dedupe_enabled);
        assert_eq!(cfg.line_profiles[0].sms_path.priority.len(), 1);
        assert_eq!(
            cfg.line_profiles[0].sms_path.priority[0].kind,
            AccessPathKind::Cs
        );
    }

    #[test]
    fn sms_path_policy_normalizes_retention_bounds() {
        let policy = SmsPathPolicy {
            dedup_retention_days: 0,
            message_retention_limit: u32::MAX,
            ..SmsPathPolicy::default()
        }
        .normalized();
        assert_eq!(policy.dedup_retention_days, 1);
        assert_eq!(policy.message_retention_limit, 100_000);

        let minimum = SmsPathPolicy {
            message_retention_limit: 0,
            ..SmsPathPolicy::default()
        }
        .normalized();
        assert_eq!(minimum.message_retention_limit, 100);
    }

    #[test]
    fn voice_path_policy_is_independent_and_normalized() {
        let line_id = "line-0123456789abcdef0123456789abcdef";
        let config: AppConfig = serde_json::from_str(
            &format!(r#"{{"line_profiles":[{{"line_id":"{line_id}","sms_path":{{"priority":[{{"kind":"cs","enabled":true}}]}},"voice_path":{{"priority":[{{"kind":"cs","enabled":true}},{{"kind":"volte","enabled":false}}]}}}}]}}"#),
        )
        .unwrap();

        assert_eq!(
            config.line_profiles[0]
                .sms_path
                .clone()
                .normalized()
                .priority,
            default_sms_path_order()
        );
        let voice = config.line_profiles[0].voice_path.clone().normalized();
        assert_eq!(voice.priority.len(), 2);
        assert_eq!(voice.priority[0].kind, AccessPathKind::Volte);
        assert!(!voice.priority[0].enabled);
        assert!(voice
            .priority
            .iter()
            .all(|layer| layer.kind != AccessPathKind::Cs));
        assert!(voice.gateway_mode);
    }

    #[test]
    fn voice_path_setter_rejects_cs_trunk_configuration() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-voice-cs-policy-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let manager = ConfigManager::new(path.clone());
        let line_id = "line-0123456789abcdef0123456789abcdef";
        let policy = VoicePathPolicy {
            priority: vec![PathLayerConfig {
                kind: AccessPathKind::Cs,
                enabled: true,
            }],
            gateway_mode: true,
        };

        assert_eq!(
            manager
                .set_line_voice_path_policy(line_id, policy)
                .unwrap_err(),
            "voice_cs_trunk_backend_unavailable"
        );
        assert_eq!(
            manager.get_line_voice_path_policy(line_id),
            VoicePathPolicy::default()
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn removed_legacy_voice_services_config_is_rejected() {
        let error = serde_json::from_str::<AppConfig>(
            r#"{"voice_services":{"feature_enabled":true,"delegate_to_asterisk":true,"marketing_keywords":["推销"]}}"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown field `voice_services`"));
    }

    #[test]
    fn access_path_kind_transport_tags_match_db_contract() {
        assert_eq!(AccessPathKind::Vowifi.transport_tag(), "vowifi_ims");
        assert_eq!(AccessPathKind::Volte.transport_tag(), "volte_ims");
        assert_eq!(AccessPathKind::Cs.transport_tag(), "modem");
        assert!(AccessPathKind::Vowifi.is_ims());
        assert!(AccessPathKind::Volte.is_ims());
        assert!(!AccessPathKind::Cs.is_ims());
    }

    #[test]
    fn legacy_notification_config_migrates_channels_and_rules() {
        let mut legacy = LegacyNotificationConfig::default();
        legacy.webhook.enabled = true;
        legacy.webhook.url = "https://example.com/hook".to_string();
        legacy.webhook.forward_sms = true;
        legacy.webhook.forward_calls = true;
        legacy.webhook.forward_ddns = false;
        legacy.webhook.forward_updates = true;

        let migrated = NotificationConfig::from_legacy(legacy);

        assert_eq!(migrated.version, 2);
        assert_eq!(migrated.channels.len(), 1);
        assert_eq!(migrated.channels[0].id, "webhook-1");
        assert_eq!(
            migrated.channels[0].channel_type,
            NotificationChannel::Webhook
        );
        assert!(migrated.channels[0].enabled);
        assert!(migrated
            .rules
            .iter()
            .any(|rule| rule.event_type == NotificationEventType::Sms
                && rule.channel_ids == vec!["webhook-1".to_string()]));
        assert!(migrated
            .rules
            .iter()
            .any(|rule| rule.event_type == NotificationEventType::Call
                && rule.channel_ids == vec!["webhook-1".to_string()]));
        assert!(!migrated
            .rules
            .iter()
            .any(|rule| rule.event_type == NotificationEventType::Ddns));
        assert!(migrated
            .rules
            .iter()
            .any(|rule| rule.event_type == NotificationEventType::VersionUpdate));
        let channel_config = migrated.channels[0].config.as_object().unwrap();
        assert_eq!(
            channel_config.get("url").and_then(Value::as_str),
            Some("https://example.com/hook")
        );
        for retired in [
            "enabled",
            "forward_sms",
            "forward_calls",
            "forward_ddns",
            "forward_updates",
            "sms_template",
            "call_template",
            "ddns_template",
            "update_template",
        ] {
            assert!(
                channel_config.get(retired).is_none(),
                "retired field {retired}"
            );
        }
    }

    #[test]
    fn line_auto_restore_defaults_are_explicit() {
        let profile = LineProfileConfig::for_line("line-0123456789abcdef0123456789abcdef");
        assert_eq!(profile.volte_auto_restore, AutoRestoreConfig::default());
        assert_eq!(profile.vowifi.auto_restore, AutoRestoreConfig::default());
    }

    #[test]
    fn line_volte_ip_families_round_trip() {
        let mut profile = LineProfileConfig::for_line("line-0123456789abcdef0123456789abcdef");
        assert_eq!(profile.volte_ip_families, default_line_volte_ip_families());
        assert!(profile.volte_ip_families_auto);
        profile.volte_ip_families = vec![VolteIpFamily::Ipv6];
        profile.volte_ip_families_auto = false;
        let round_trip: LineProfileConfig =
            serde_json::from_value(serde_json::to_value(profile).unwrap()).unwrap();
        assert_eq!(round_trip.volte_ip_families, vec![VolteIpFamily::Ipv6]);
        assert!(!round_trip.volte_ip_families_auto);
    }

    #[test]
    fn legacy_line_profile_defaults_ip_family_selection_to_automatic() {
        let profile: LineProfileConfig = serde_json::from_value(serde_json::json!({
            "line_id": "line-0123456789abcdef0123456789abcdef",
            "volte_ip_families": ["ipv4v6", "ipv4", "ipv6"]
        }))
        .unwrap();
        assert!(profile.volte_ip_families_auto);
    }

    #[test]
    fn removed_global_root_keys_are_rejected() {
        for legacy_field in [
            "webhook",
            "vowifi",
            "volte",
            "roaming_allowed",
            "data_enabled",
            "apn",
            "vilte",
            "sms_path",
            "voice_path",
        ] {
            let error = serde_json::from_str::<AppConfig>(&format!(r#"{{"{legacy_field}":null}}"#))
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("unknown field `{legacy_field}`")),
                "removed global field {legacy_field} should be rejected"
            );
        }
    }

    #[test]
    fn invalid_root_settings_block_config_load_without_rewriting_file() {
        let path = std::env::temp_dir().join(format!(
            "simadmin_invalid_root_config_{}_{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let original = r#"{"volte":{"enabled":true}}"#;
        std::fs::write(&path, original).unwrap();

        let error = match ConfigManager::try_new(path.clone()) {
            Ok(_) => panic!("removed root setting must block configuration load"),
            Err(error) => error,
        };
        assert!(error.contains("unknown field `volte`"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("tmp"));
        let _ = std::fs::remove_file(path.with_extension("bak"));
    }

    #[test]
    fn old_line_config_version_blocks_load_without_rewriting_file() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-old-line-config-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let line_id = "line-0123456789abcdef0123456789abcdef";
        let mut config = AppConfig::default();
        config.line_config_version = 1;
        config
            .line_profiles
            .push(LineProfileConfig::for_line(line_id));
        let original = serde_json::to_vec_pretty(&config).unwrap();
        std::fs::write(&path, &original).unwrap();

        let error = match ConfigManager::try_new(path.clone()) {
            Ok(_) => panic!("old line config version must block configuration load"),
            Err(error) => error,
        };
        assert!(error.contains("Unsupported line config version 1"));
        assert!(error.contains(&format!("expected {CURRENT_LINE_CONFIG_VERSION}")));
        assert_eq!(std::fs::read(&path).unwrap(), original);

        let _ = std::fs::remove_file(path.with_extension("bak"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn per_line_voice_and_ims_video_are_isolated_and_persist() {
        let path = std::env::temp_dir().join(format!(
            "simadmin_vilte_gate_{}_{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let manager = ConfigManager::new(path.clone());
        let line_a = "line-0123456789abcdef0123456789abcdef";
        let line_b = "line-fedcba9876543210fedcba9876543210";

        // IMS video follows the access leg's connection: there is no separate
        // voice or video switch to set. Connecting VoLTE on line_b is therefore
        // the whole action, and line_a stays off because it is not connected.
        manager
            .set_line_volte_connection_enabled(line_b, true)
            .unwrap();
        assert!(!manager.get_line_ims_video_config(line_a).volte_enabled);
        assert!(manager.get_line_ims_video_config(line_b).volte_enabled);

        // VoWiFi video follows the VoWiFi connection independently.
        manager
            .set_line_vowifi_connection_enabled(line_b, true)
            .unwrap();
        let vowifi_video = manager.get_line_ims_video_config(line_b);
        assert!(vowifi_video.vowifi_enabled);
        assert!(vowifi_video.volte_enabled);

        assert_eq!(
            manager
                .set_line_ims_video_config(
                    line_b,
                    ImsVideoConfig {
                        codec: "vp8".to_string(),
                        ..ImsVideoConfig::default()
                    },
                )
                .unwrap_err(),
            "vilte_codec_unsupported"
        );
        assert_eq!(
            manager
                .set_line_ims_video_config(
                    line_b,
                    ImsVideoConfig {
                        video_payload_type: 95,
                        ..ImsVideoConfig::default()
                    },
                )
                .unwrap_err(),
            "vilte_payload_type_invalid"
        );

        // Incoming booleans are status mirrors and cannot override the access
        // switches. Only the media parameters are accepted from this API.
        let derived = manager
            .set_line_ims_video_config(
                line_b,
                ImsVideoConfig {
                    volte_enabled: false,
                    vowifi_enabled: false,
                    video_payload_type: 112,
                    ..ImsVideoConfig::default()
                },
            )
            .unwrap();
        assert!(derived.volte_enabled);
        assert!(derived.vowifi_enabled);
        assert_eq!(derived.video_payload_type, 112);

        let reloaded = ConfigManager::new(path.clone());
        assert!(!reloaded.get_line_volte_voice_enabled(line_a));
        assert!(reloaded.get_line_volte_voice_enabled(line_b));
        assert_eq!(
            reloaded
                .get_line_ims_video_config(line_b)
                .video_payload_type,
            112
        );
        assert!(reloaded.get_line_ims_video_config(line_b).volte_enabled);
        assert!(reloaded.get_line_ims_video_config(line_b).vowifi_enabled);
        assert!(!reloaded.get_line_ims_video_config(line_a).volte_enabled);

        reloaded
            .set_line_volte_connection_enabled(line_b, false)
            .unwrap();
        assert!(!reloaded.get_line_ims_video_config(line_b).volte_enabled);
        assert!(reloaded.get_line_ims_video_config(line_b).vowifi_enabled);

        let _ = std::fs::remove_file(path.with_extension("bak"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn stale_ims_video_gates_are_normalized_when_config_loads() {
        let path = std::env::temp_dir().join(format!(
            "simadmin_vilte_normalize_{}_{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let line_id = "line-0123456789abcdef0123456789abcdef";
        let mut config = AppConfig::default();
        let mut profile = LineProfileConfig::for_line(line_id);
        profile.volte_connection_enabled = true;
        profile.vowifi.enabled = true;
        assert!(!profile.ims_video.volte_enabled);
        assert!(!profile.ims_video.vowifi_enabled);
        config.line_profiles.push(profile);
        std::fs::write(&path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();

        let manager = ConfigManager::new(path.clone());
        let normalized = manager.get_line_ims_video_config(line_id);
        assert!(normalized.volte_enabled);
        assert!(normalized.vowifi_enabled);
        let persisted: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            persisted["line_profiles"][0]["ims_video"]["volte_enabled"],
            serde_json::Value::Bool(true)
        );
        assert_eq!(
            persisted["line_profiles"][0]["ims_video"]["vowifi_enabled"],
            serde_json::Value::Bool(true)
        );

        let _ = std::fs::remove_file(path.with_extension("bak"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn legacy_vilte_schema_is_rejected_without_rewriting_source() {
        let path = std::env::temp_dir().join(format!(
            "simadmin_vilte_migration_{}_{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // Old schema: `vilte.feature_enabled` under the per-line profile.
        let json = format!(
            r#"{{
                "line_profiles": [{{
                    "line_id": "line-0123456789abcdef0123456789abcdef",
                    "volte_voice_enabled": true,
                    "vilte": {{ "feature_enabled": true, "video_payload_type": 111 }}
                }}]
            }}"#
        );
        std::fs::write(&path, &json).unwrap();
        let error = ConfigManager::try_new(path.clone())
            .err()
            .expect("legacy schema must be rejected");
        assert!(error.contains("Unsupported line config version"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), json);

        let _ = std::fs::remove_file(path);
    }

    fn trunk_test_manager() -> (ConfigManager, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "simadmin_trunk_{}_{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        (ConfigManager::new(path.clone()), path)
    }

    const TRUNK_TEST_LINE: &str = "line-0123456789abcdef0123456789abcdef";

    #[test]
    fn trunk_defaults_are_inert_and_off() {
        let (manager, path) = trunk_test_manager();
        let profile = manager.get_line_profile(TRUNK_TEST_LINE);
        assert!(!profile.trunk.enabled);
        assert_eq!(profile.trunk.asterisk_port, 5060);
        assert_eq!(profile.trunk.local_port, 0);
        assert_eq!(profile.trunk.register_expiry_secs, 3600);
        assert_eq!(
            profile.trunk.registration_mode,
            TrunkRegistrationMode::StaticPeer
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn trunk_enable_requires_asterisk_host() {
        let (manager, path) = trunk_test_manager();
        let err = manager
            .set_line_trunk_profile(
                TRUNK_TEST_LINE,
                TrunkProfileConfig {
                    enabled: true,
                    ..TrunkProfileConfig::default()
                },
            )
            .unwrap_err();
        assert_eq!(err, "trunk_asterisk_host_required");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn trunk_outbound_register_requires_username() {
        let (manager, path) = trunk_test_manager();
        let err = manager
            .set_line_trunk_profile(
                TRUNK_TEST_LINE,
                TrunkProfileConfig {
                    enabled: true,
                    registration_mode: TrunkRegistrationMode::OutboundRegister,
                    asterisk_host: "pbx.example.com".to_string(),
                    ..TrunkProfileConfig::default()
                },
            )
            .unwrap_err();
        assert_eq!(err, "trunk_username_required");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn trunk_enabled_profile_requires_stable_local_port() {
        let (manager, path) = trunk_test_manager();
        let err = manager
            .set_line_trunk_profile(
                TRUNK_TEST_LINE,
                TrunkProfileConfig {
                    enabled: true,
                    asterisk_host: "192.168.1.10".to_string(),
                    ..TrunkProfileConfig::default()
                },
            )
            .unwrap_err();
        assert_eq!(err, "trunk_local_port_required");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn trunk_enabled_profiles_reject_duplicate_local_ports() {
        let (manager, path) = trunk_test_manager();
        manager
            .set_line_trunk_profile(
                TRUNK_TEST_LINE,
                TrunkProfileConfig {
                    enabled: true,
                    asterisk_host: "192.168.1.10".to_string(),
                    local_port: 5062,
                    ..TrunkProfileConfig::default()
                },
            )
            .unwrap();
        let err = manager
            .set_line_trunk_profile(
                "line-fedcba9876543210fedcba9876543210",
                TrunkProfileConfig {
                    enabled: true,
                    asterisk_host: "192.168.1.10".to_string(),
                    local_port: 5062,
                    ..TrunkProfileConfig::default()
                },
            )
            .unwrap_err();
        assert_eq!(err, "trunk_local_port_in_use");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn trunk_ignores_duplicate_port_from_removed_line() {
        let (manager, path) = trunk_test_manager();
        let removed_line = "line-fedcba9876543210fedcba9876543210";
        manager
            .set_line_trunk_profile(
                removed_line,
                TrunkProfileConfig {
                    enabled: true,
                    asterisk_host: "192.168.1.10".to_string(),
                    local_port: 5062,
                    ..TrunkProfileConfig::default()
                },
            )
            .unwrap();
        let active_lines = std::collections::HashSet::from([TRUNK_TEST_LINE.to_string()]);
        let saved = manager
            .set_line_trunk_profile_for_active_lines(
                TRUNK_TEST_LINE,
                TrunkProfileConfig {
                    enabled: true,
                    asterisk_host: "192.168.1.10".to_string(),
                    local_port: 5062,
                    ..TrunkProfileConfig::default()
                },
                &active_lines,
            )
            .expect("removed line must not reserve a local SIP port");
        assert!(saved.trunk.enabled);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn trunk_active_lines_still_reject_duplicate_local_ports() {
        let (manager, path) = trunk_test_manager();
        let other_line = "line-fedcba9876543210fedcba9876543210";
        manager
            .set_line_trunk_profile(
                other_line,
                TrunkProfileConfig {
                    enabled: true,
                    asterisk_host: "192.168.1.10".to_string(),
                    local_port: 5062,
                    ..TrunkProfileConfig::default()
                },
            )
            .unwrap();
        let active_lines =
            std::collections::HashSet::from([TRUNK_TEST_LINE.to_string(), other_line.to_string()]);
        let error = manager
            .set_line_trunk_profile_for_active_lines(
                TRUNK_TEST_LINE,
                TrunkProfileConfig {
                    enabled: true,
                    asterisk_host: "192.168.1.10".to_string(),
                    local_port: 5062,
                    ..TrunkProfileConfig::default()
                },
                &active_lines,
            )
            .expect_err("active line must keep its local SIP port reservation");
        assert_eq!(error, "trunk_local_port_in_use");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn trunk_invalid_line_id_rejected() {
        let (manager, path) = trunk_test_manager();
        let err = manager
            .set_line_trunk_profile("not-a-line", TrunkProfileConfig::default())
            .unwrap_err();
        assert_eq!(err, "invalid_line_id");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn trunk_static_peer_persists_and_redacts_secret() {
        let (manager, path) = trunk_test_manager();
        let saved = manager
            .set_line_trunk_profile(
                TRUNK_TEST_LINE,
                TrunkProfileConfig {
                    enabled: true,
                    registration_mode: TrunkRegistrationMode::StaticPeer,
                    asterisk_host: "192.168.1.10".to_string(),
                    local_port: 5062,
                    username: "line0".to_string(),
                    secret: "s3cr3t".to_string(),
                    match_host: Some("192.168.1.10".to_string()),
                    ..TrunkProfileConfig::default()
                },
            )
            .unwrap();
        assert!(saved.trunk.enabled);
        assert!(saved.trunk.secret_set());

        // Persisted to disk with the secret intact.
        let reloaded = ConfigManager::new(path.clone());
        assert_eq!(
            reloaded.get_line_profile(TRUNK_TEST_LINE).trunk.secret,
            "s3cr3t"
        );

        // Redacted copy never carries the secret.
        let redacted = saved.redacted();
        assert!(redacted.trunk.secret.is_empty());
        assert!(saved.trunk.secret_set());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn trunk_empty_secret_keeps_stored_secret() {
        let (manager, path) = trunk_test_manager();
        manager
            .set_line_trunk_profile(
                TRUNK_TEST_LINE,
                TrunkProfileConfig {
                    enabled: true,
                    asterisk_host: "192.168.1.10".to_string(),
                    local_port: 5062,
                    secret: "keepme".to_string(),
                    ..TrunkProfileConfig::default()
                },
            )
            .unwrap();

        // Re-submit with a blank secret (as a redacted round-trip would): the
        // stored secret must survive.
        let updated = manager
            .set_line_trunk_profile(
                TRUNK_TEST_LINE,
                TrunkProfileConfig {
                    enabled: true,
                    asterisk_host: "192.168.1.20".to_string(),
                    local_port: 5062,
                    secret: String::new(),
                    ..TrunkProfileConfig::default()
                },
            )
            .unwrap();
        assert_eq!(updated.trunk.asterisk_host, "192.168.1.20");
        assert_eq!(updated.trunk.secret, "keepme");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn trunk_legacy_extension_migrates_to_incoming_binding() {
        let profile: TrunkProfileConfig = serde_json::from_str(r#"{"extension":"6108"}"#).unwrap();
        assert_eq!(profile.incoming_mode, TrunkIncomingMode::BoundPending);
        assert_eq!(profile.incoming_binding, "6108");
        assert!(profile.outgoing_binding.is_empty());
        assert_eq!(profile.ip_connect_mode, TrunkIpConnectMode::GsmAnswer);

        let serialized = serde_json::to_value(profile).unwrap();
        assert!(serialized.get("extension").is_none());
        assert_eq!(serialized["incoming_binding"], "6108");
    }

    #[test]
    fn trunk_routing_fields_are_trimmed_validated_and_persisted() {
        let (manager, path) = trunk_test_manager();
        let saved = manager
            .set_line_trunk_profile(
                TRUNK_TEST_LINE,
                TrunkProfileConfig {
                    enabled: true,
                    registration_mode: TrunkRegistrationMode::OutboundRegister,
                    asterisk_host: "192.168.1.10".to_string(),
                    local_port: 5062,
                    username: "41000".to_string(),
                    register_expiry_secs: 3600,
                    incoming_mode: TrunkIncomingMode::BoundImmediate,
                    incoming_binding: " 6108 ".to_string(),
                    outgoing_binding: " 6109 ".to_string(),
                    ip_connect_mode: TrunkIpConnectMode::FirstRtp,
                    ..TrunkProfileConfig::default()
                },
            )
            .unwrap();
        assert_eq!(saved.trunk.incoming_mode, TrunkIncomingMode::BoundImmediate);
        assert_eq!(saved.trunk.incoming_binding, "6108");
        assert_eq!(saved.trunk.outgoing_binding, "6109");
        assert_eq!(saved.trunk.ip_connect_mode, TrunkIpConnectMode::FirstRtp);

        let legacy_true: TrunkProfileConfig =
            serde_json::from_str(r#"{"ip_connect_on_operator_answer":true}"#).unwrap();
        let legacy_false: TrunkProfileConfig =
            serde_json::from_str(r#"{"ip_connect_on_operator_answer":false}"#).unwrap();
        assert_eq!(legacy_true.ip_connect_mode, TrunkIpConnectMode::GsmAnswer);
        assert_eq!(legacy_false.ip_connect_mode, TrunkIpConnectMode::FirstRtp);

        let invalid_expiry = manager
            .set_line_trunk_profile(
                TRUNK_TEST_LINE,
                TrunkProfileConfig {
                    enabled: true,
                    registration_mode: TrunkRegistrationMode::OutboundRegister,
                    asterisk_host: "192.168.1.10".to_string(),
                    local_port: 5062,
                    username: "41000".to_string(),
                    register_expiry_secs: 59,
                    ..TrunkProfileConfig::default()
                },
            )
            .unwrap_err();
        assert_eq!(invalid_expiry, "trunk_register_expiry_invalid");

        let invalid_binding = manager
            .set_line_trunk_profile(
                TRUNK_TEST_LINE,
                TrunkProfileConfig {
                    incoming_binding: "6108/evil".to_string(),
                    ..TrunkProfileConfig::default()
                },
            )
            .unwrap_err();
        assert_eq!(invalid_binding, "trunk_incoming_binding_invalid");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn trunk_toggle_revalidates_stored_profile() {
        let (manager, path) = trunk_test_manager();
        // Enabling an unconfigured trunk via the toggle is rejected.
        let err = manager
            .set_line_trunk_enabled(TRUNK_TEST_LINE, true)
            .unwrap_err();
        assert_eq!(err, "trunk_asterisk_host_required");

        // Configure it disabled, then the toggle can switch it on.
        manager
            .set_line_trunk_profile(
                TRUNK_TEST_LINE,
                TrunkProfileConfig {
                    enabled: false,
                    asterisk_host: "192.168.1.10".to_string(),
                    local_port: 5062,
                    ..TrunkProfileConfig::default()
                },
            )
            .unwrap();
        let on = manager
            .set_line_trunk_enabled(TRUNK_TEST_LINE, true)
            .unwrap();
        assert!(on.trunk.enabled);
        let off = manager
            .set_line_trunk_enabled(TRUNK_TEST_LINE, false)
            .unwrap();
        assert!(!off.trunk.enabled);
        let _ = std::fs::remove_file(path);
    }

    fn sqlite_config_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "simadmin-config-sqlite-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn sqlite_config_persists_one_versioned_document() {
        let dir = sqlite_config_test_dir("persist");
        let path = dir.join("config.sqlite3");
        let manager = ConfigManager::try_new(path.clone()).unwrap();
        let mut security = manager.get_security();
        security.session_ttl_seconds = 7_200;
        manager.set_security(security).unwrap();
        drop(manager);

        let reloaded = ConfigManager::try_new(path.clone()).unwrap();
        assert_eq!(reloaded.get_security().session_ttl_seconds, 7_200);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let connection = SqliteConnection::open(&path).unwrap();
        let row: (u32, u32, i64) = connection
            .query_row(
                "SELECT storage_schema_version, line_config_version, COUNT(*)
                 FROM app_config WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            (
                CONFIG_STORAGE_SCHEMA_VERSION,
                CURRENT_LINE_CONFIG_VERSION,
                1
            )
        );
        drop(connection);
        drop(reloaded);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_config_rejects_symlink_database_path() {
        use std::os::unix::fs::symlink;

        let dir = sqlite_config_test_dir("symlink");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("target.sqlite3");
        std::fs::write(&target, b"not a database").unwrap();
        let path = dir.join("config.sqlite3");
        symlink(&target, &path).unwrap();

        let error = match ConfigManager::try_new(path) {
            Ok(_) => panic!("symlink database must not be followed"),
            Err(error) => error,
        };
        assert!(error.contains("Refusing symlink config database"));
        assert_eq!(std::fs::read(&target).unwrap(), b"not a database");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sqlite_config_ignores_sibling_json_and_uses_defaults() {
        let dir = sqlite_config_test_dir("no-legacy-import");
        std::fs::create_dir_all(&dir).unwrap();
        let legacy_path = dir.join("config.json");
        let sqlite_path = dir.join("config.sqlite3");
        let mut legacy = AppConfig::default();
        legacy.security.session_ttl_seconds = 9_000;
        let original = serde_json::to_vec_pretty(&legacy).unwrap();
        std::fs::write(&legacy_path, &original).unwrap();

        let manager = ConfigManager::try_new(sqlite_path.clone()).unwrap();
        assert_eq!(
            manager.get_security().session_ttl_seconds,
            AppConfig::default().security.session_ttl_seconds
        );
        assert_eq!(std::fs::read(&legacy_path).unwrap(), original);
        assert!(!legacy_path.with_extension("json.migrated.bak").exists());
        assert!(sqlite_path.exists());
        drop(manager);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sqlite_config_invalid_document_fails_closed() {
        let dir = sqlite_config_test_dir("invalid");
        let path = dir.join("config.sqlite3");
        drop(ConfigManager::try_new(path.clone()).unwrap());
        let connection = SqliteConnection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE app_config SET config_json = ?1 WHERE singleton = 1",
                [r#"{"unknown_root_setting":true}"#],
            )
            .unwrap();
        drop(connection);

        let error = match ConfigManager::try_new(path.clone()) {
            Ok(_) => panic!("invalid SQLite config must not fall back to defaults"),
            Err(error) => error,
        };
        assert!(error.contains("unknown field `unknown_root_setting`"));
        let connection = SqliteConnection::open(&path).unwrap();
        let stored: String = connection
            .query_row(
                "SELECT config_json FROM app_config WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, r#"{"unknown_root_setting":true}"#);
        drop(connection);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sqlite_config_unknown_storage_schema_fails_closed() {
        let dir = sqlite_config_test_dir("schema");
        let path = dir.join("config.sqlite3");
        drop(ConfigManager::try_new(path.clone()).unwrap());
        let connection = SqliteConnection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE app_config SET storage_schema_version = 999 WHERE singleton = 1",
                [],
            )
            .unwrap();
        drop(connection);

        let error = match ConfigManager::try_new(path.clone()) {
            Ok(_) => panic!("unknown storage schema must not load"),
            Err(error) => error,
        };
        assert!(error.contains("Unsupported config storage schema version 999"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sqlite_config_and_sim_overrides_share_file_without_clobbering() {
        use crate::connectivity::modems::ims::profile_override::{
            ImsCommonOverride, SimBindingKey, SimOverride, SimOverrideStore,
        };

        let dir = sqlite_config_test_dir("shared");
        let path = dir.join("config.sqlite3");
        let manager = ConfigManager::try_new(path.clone()).unwrap();
        let overrides = SimOverrideStore::sqlite(path.clone());
        let binding = SimBindingKey::resolve(Some("8986001234567890123"), None).unwrap();
        let override_ = SimOverride {
            ims_common: ImsCommonOverride {
                voicemail_number: Some("123".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        overrides.save(&binding, &override_).unwrap();
        let mut security = manager.get_security();
        security.session_ttl_seconds = 10_000;
        manager.set_security(security).unwrap();
        drop(manager);

        let reloaded = ConfigManager::try_new(path.clone()).unwrap();
        assert_eq!(reloaded.get_security().session_ttl_seconds, 10_000);
        assert_eq!(
            overrides
                .load(&binding)
                .unwrap()
                .unwrap()
                .ims_common
                .voicemail_number
                .as_deref(),
            Some("123")
        );
        drop(reloaded);
        let _ = std::fs::remove_dir_all(dir);
    }
}

fn default_ddns_provider() -> String {
    "tencentcloud".to_string()
}

fn default_ddns_interval_seconds() -> u64 {
    300
}

fn default_ddns_ttl() -> u32 {
    600
}

fn default_ddns_get_type() -> String {
    "interface".to_string()
}

fn default_ddns_ipv4_urls() -> Vec<String> {
    vec![
        "https://api.ipify.org".to_string(),
        "https://ip.3322.net".to_string(),
        "https://4.ident.me".to_string(),
        "https://ddns.oray.com/checkip".to_string(),
        "https://4.ipw.cn".to_string(),
    ]
}

fn default_ddns_ipv6_urls() -> Vec<String> {
    vec![
        "https://api6.ipify.org".to_string(),
        "https://speed.neu6.edu.cn/getIP.php".to_string(),
        "https://v6.ident.me".to_string(),
        "https://myip6.ipip.net".to_string(),
        "https://6.ipw.cn".to_string(),
    ]
}

fn default_ddns_ipv6_config() -> DdnsIpConfig {
    DdnsIpConfig {
        enabled: false,
        get_type: default_ddns_get_type(),
        interface_name: String::new(),
        urls: default_ddns_ipv6_urls(),
        domains: Vec::new(),
    }
}

fn default_roaming_allowed() -> bool {
    true
}

fn default_password_min_length() -> u8 {
    8
}

fn default_session_ttl_seconds() -> i64 {
    7 * 24 * 60 * 60
}

fn default_idle_timeout_seconds() -> i64 {
    60 * 60
}

fn default_apn_protocol() -> String {
    "dual".to_string()
}

fn default_apn_auth_method() -> String {
    "chap".to_string()
}

fn default_lpac_path() -> String {
    "/opt/simadmin/lpac/lpac".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApnConfig {
    #[serde(default)]
    pub apn: String,
    #[serde(default = "default_apn_protocol")]
    pub protocol: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_apn_auth_method")]
    pub auth_method: String,
}

impl Default for ApnConfig {
    fn default() -> Self {
        Self {
            apn: String::new(),
            protocol: default_apn_protocol(),
            username: String::new(),
            password: String::new(),
            auth_method: default_apn_auth_method(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EsimConfig {
    #[serde(default = "default_lpac_path")]
    pub lpac_path: String,
    #[serde(default)]
    pub custom_memory_total_kb: Option<u32>,
    /// Deprecated pre-multi-line reader settings. They are retained only so a
    /// single discovered line can migrate them into `LineProfileConfig`.
    #[serde(default)]
    pub apdu_backend: String,
    #[serde(default)]
    pub http_backend: String,
    #[serde(default)]
    pub at_device: String,
    #[serde(default)]
    pub qmi_device: String,
    #[serde(default)]
    pub qmi_uim_slot: u8,
}

impl Default for EsimConfig {
    fn default() -> Self {
        Self {
            lpac_path: default_lpac_path(),
            custom_memory_total_kb: None,
            apdu_backend: "qmi".to_string(),
            http_backend: "curl".to_string(),
            at_device: String::new(),
            qmi_device: String::new(),
            qmi_uim_slot: 0,
        }
    }
}

/// lpac reader selection owned by one line. Device-wide lpac installation
/// settings remain in `EsimConfig`; ports and APDU routing must never be shared
/// across lines.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EsimReaderConfig {
    /// lpac APDU backend, for example `qmi`, `qmi_qrtr`, `at`, or `at_csim`.
    #[serde(default = "default_esim_apdu_backend")]
    pub apdu_backend: String,
    /// lpac HTTP backend, normally `curl`.
    #[serde(default = "default_esim_http_backend")]
    pub http_backend: String,
    /// Optional AT port for the `at`/`at_csim` APDU backends.
    #[serde(default)]
    pub at_device: String,
    /// Optional QMI override. Empty uses the selected line's registered reader.
    #[serde(default)]
    pub qmi_device: String,
    /// Optional QMI UIM slot override. Zero uses the selected line's slot.
    #[serde(default)]
    pub qmi_uim_slot: u8,
    /// Optional PC/SC reader name. Empty lets lpac use its first available reader.
    #[serde(default)]
    pub pcsc_reader_name: String,
    /// Optional PC/SC reader index. `None` lets lpac choose automatically.
    #[serde(default)]
    pub pcsc_reader_index: Option<u16>,
    /// MBIM control device used by the MBIM APDU backend.
    #[serde(default)]
    pub mbim_device: String,
    /// MBIM UIM slot. Zero uses lpac's default slot.
    #[serde(default)]
    pub mbim_uim_slot: u8,
    /// Connect through mbim-proxy instead of opening the device directly.
    #[serde(default)]
    pub mbim_use_proxy: bool,
    /// Keep the modem's current slot mapping instead of changing it through MBIM.
    #[serde(default)]
    pub mbim_skip_slot_mapping: bool,
}

fn default_esim_apdu_backend() -> String {
    "qmi".to_string()
}

fn default_esim_http_backend() -> String {
    "curl".to_string()
}

impl Default for EsimReaderConfig {
    fn default() -> Self {
        Self {
            apdu_backend: default_esim_apdu_backend(),
            http_backend: default_esim_http_backend(),
            at_device: String::new(),
            qmi_device: String::new(),
            qmi_uim_slot: 0,
            pcsc_reader_name: String::new(),
            pcsc_reader_index: None,
            mbim_device: String::new(),
            mbim_uim_slot: 0,
            mbim_use_proxy: false,
            mbim_skip_slot_mapping: false,
        }
    }
}

fn default_volte_auto_restore_initial_delay_secs() -> u64 {
    60
}

fn default_volte_auto_restore_attempts() -> u8 {
    3
}

fn default_volte_auto_restore_retry_delay_secs() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutoRestoreConfig {
    #[serde(default = "default_volte_auto_restore_initial_delay_secs")]
    pub initial_delay_secs: u64,
    #[serde(default = "default_volte_auto_restore_attempts")]
    pub attempts: u8,
    #[serde(default = "default_volte_auto_restore_retry_delay_secs")]
    pub retry_delay_secs: u64,
}

impl Default for AutoRestoreConfig {
    fn default() -> Self {
        Self {
            initial_delay_secs: default_volte_auto_restore_initial_delay_secs(),
            attempts: default_volte_auto_restore_attempts(),
            retry_delay_secs: default_volte_auto_restore_retry_delay_secs(),
        }
    }
}

/// IMS bearer IP address-family attempt order. The runtime always asks the
/// modem for dual-stack first; this preference decides which single family is
/// tried first when the network does NOT force a specific one, and the order in
/// which the bearer's local addresses are offered to SIP/REGISTER. When the
/// network explicitly signals `Ipv6OnlyAllowed`/`Ipv4OnlyAllowed`, that forced
/// family is honored regardless of this preference. Default is `Ipv4First`:
/// on an unclear failure, IPv4 is tried before IPv6.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum VolteIpFamilyPreference {
    Ipv6First,
    #[default]
    Ipv4First,
    Ipv6Only,
    Ipv4Only,
}

impl VolteIpFamilyPreference {
    /// The equivalent ordered attempt list used by per-line profiles and IMS
    /// planning helpers.
    /// The `*First` presets lead with dual-stack, matching the historical
    /// "always try dual-stack first, then fall back to single families" behaviour.
    pub fn to_families(self) -> Vec<VolteIpFamily> {
        match self {
            Self::Ipv6First => vec![
                VolteIpFamily::Ipv4v6,
                VolteIpFamily::Ipv6,
                VolteIpFamily::Ipv4,
            ],
            Self::Ipv4First => vec![
                VolteIpFamily::Ipv4v6,
                VolteIpFamily::Ipv4,
                VolteIpFamily::Ipv6,
            ],
            Self::Ipv6Only => vec![VolteIpFamily::Ipv6],
            Self::Ipv4Only => vec![VolteIpFamily::Ipv4],
        }
    }
}

/// One IMS bearer attempt a line may enable: dual-stack or a single family. The
/// order of a `Vec<VolteIpFamily>` is the attempt/fallback order, so a line can
/// place `Ipv4v6` (dual-stack) anywhere in the sequence rather than it always
/// being tried first. A one-element list means "only this attempt, no fallback".
/// This is the per-line, web-editable form of the legacy
/// [`VolteIpFamilyPreference`] and is a strict superset of it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VolteIpFamily {
    Ipv4v6,
    Ipv4,
    Ipv6,
}

impl VolteIpFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ipv4v6 => "ipv4v6",
            Self::Ipv4 => "ipv4",
            Self::Ipv6 => "ipv6",
        }
    }
}

fn default_line_volte_ip_families() -> Vec<VolteIpFamily> {
    VolteIpFamilyPreference::default().to_families()
}

fn default_line_volte_ip_families_auto() -> bool {
    true
}

/// How this line's logical SIP trunk associates with the remote Asterisk/FreePBX.
///
/// Both modes share the same SIP transport and RTP relay; the only difference is
/// whether SimAdmin actively REGISTERs to Asterisk. Decided 2026-07-16 to support
/// both and let the user pick per line (see extension doc §8.1 / §17.2).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrunkRegistrationMode {
    /// Static peer: both sides pin each other's IP:port and do not REGISTER.
    /// SIP requests remain bidirectional; `match_host` identifies the peer.
    #[default]
    StaticPeer,
    /// SimAdmin actively REGISTERs to Asterisk as an endpoint and refreshes it
    /// every `register_expiry_secs`. NAT-friendly and supports dynamic presence.
    OutboundRegister,
}

/// How a mobile-terminated operator call is presented to Asterisk.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrunkIncomingMode {
    /// Route to the configured Asterisk IVR/secondary-dial extension. SimAdmin
    /// remains a transparent media relay; Asterisk owns prompts and digit use.
    SecondaryDial,
    /// Ring the bound extension and answer IMS only after Asterisk answers.
    #[default]
    BoundPending,
    /// Answer IMS immediately, then ring the bound Asterisk extension.
    BoundImmediate,
}

/// When an Asterisk-originated call should receive its final 200 response.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrunkIpConnectMode {
    /// Complete the IP leg after the first valid RTP packet from the operator.
    FirstRtp,
    /// Complete the IP leg as soon as the operator/GSM leg answers.
    #[default]
    GsmAnswer,
}

fn deserialize_trunk_ip_connect_mode<'de, D>(
    deserializer: D,
) -> Result<TrunkIpConnectMode, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Value {
        Mode(TrunkIpConnectMode),
        LegacyBool(bool),
    }

    Ok(match Value::deserialize(deserializer)? {
        Value::Mode(mode) => mode,
        Value::LegacyBool(true) => TrunkIpConnectMode::GsmAnswer,
        Value::LegacyBool(false) => TrunkIpConnectMode::FirstRtp,
    })
}

fn default_trunk_asterisk_port() -> u16 {
    5060
}

fn default_trunk_register_expiry_secs() -> u32 {
    3600
}

/// Per-line SIP trunk settings toward a remote Asterisk/FreePBX. This is a pure
/// configuration record (stage D3b); the actual SIP endpoint / RTP bridge that
/// consumes it lands in the `trunk/` module (stage D4/D5). All fields default to
/// an inert, disabled state so existing configs deserialize unchanged and the
/// feature stays off until explicitly configured.
///
/// `secret` is persisted to the on-disk config but MUST be redacted before it
/// crosses any API boundary — callers use [`TrunkProfileConfig::redacted`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrunkProfileConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub registration_mode: TrunkRegistrationMode,
    /// Asterisk/FreePBX host (IP or DNS name). Empty until configured.
    #[serde(default)]
    pub asterisk_host: String,
    #[serde(default = "default_trunk_asterisk_port")]
    pub asterisk_port: u16,
    /// Local UDP port used by this logical endpoint. Zero asks the OS for an
    /// ephemeral port and is suitable for outbound REGISTER. Static peers
    /// should use a unique, explicitly configured port per line.
    #[serde(default)]
    pub local_port: u16,
    /// Endpoint / auth username presented to Asterisk.
    #[serde(default)]
    pub username: String,
    /// Digest secret. Persisted on disk; redacted on every API response.
    #[serde(default)]
    pub secret: String,
    /// Expected Asterisk dialplan context. This is deployment metadata for UI
    /// and generated configuration; SIP requests do not carry a context name.
    #[serde(default)]
    pub context: String,
    /// Mobile-terminated routing behavior toward Asterisk.
    #[serde(default)]
    pub incoming_mode: TrunkIncomingMode,
    /// Asterisk extension targeted for operator-originated incoming calls.
    /// `extension` is accepted as a legacy on-disk/API alias.
    #[serde(default, alias = "extension")]
    pub incoming_binding: String,
    /// Optional Asterisk From-user allowed to originate calls through this SIM.
    /// Empty keeps backward-compatible per-peer routing without user binding.
    #[serde(default)]
    pub outgoing_binding: String,
    /// Select whether operator RTP or operator/GSM answer completes the IP leg.
    /// The alias accepts the short-lived boolean field introduced before this
    /// was corrected to two explicit choices (`true` -> GSM answer).
    #[serde(
        default,
        alias = "ip_connect_on_operator_answer",
        deserialize_with = "deserialize_trunk_ip_connect_mode"
    )]
    pub ip_connect_mode: TrunkIpConnectMode,
    /// Codec allow-list advertised toward Asterisk (pass-through, never
    /// transcoded here). Empty means "advertise the negotiated defaults".
    #[serde(default)]
    pub codec_allow: Vec<String>,
    /// OutboundRegister only: registration lifetime / refresh period.
    #[serde(default = "default_trunk_register_expiry_secs")]
    pub register_expiry_secs: u32,
    /// StaticPeer only: the far-end host used to identify inbound requests.
    #[serde(default)]
    pub match_host: Option<String>,
}

impl Default for TrunkProfileConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            registration_mode: TrunkRegistrationMode::StaticPeer,
            asterisk_host: String::new(),
            asterisk_port: default_trunk_asterisk_port(),
            local_port: 0,
            username: String::new(),
            secret: String::new(),
            context: String::new(),
            incoming_mode: TrunkIncomingMode::BoundPending,
            incoming_binding: String::new(),
            outgoing_binding: String::new(),
            ip_connect_mode: TrunkIpConnectMode::GsmAnswer,
            codec_allow: Vec::new(),
            register_expiry_secs: default_trunk_register_expiry_secs(),
            match_host: None,
        }
    }
}

impl TrunkProfileConfig {
    /// A copy safe to serialize across an API boundary: the secret is blanked and
    /// its presence is not otherwise revealed. Callers that need to tell the UI
    /// whether a secret is set should surface a separate `secret_set` flag.
    pub fn redacted(&self) -> Self {
        Self {
            secret: String::new(),
            ..self.clone()
        }
    }

    /// Whether a non-empty secret is currently stored (for UI hints without
    /// leaking the value).
    pub fn secret_set(&self) -> bool {
        !self.secret.is_empty()
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// How this line's IKEv2/NAT-T traffic leaves the host.
///
/// Only transports that can actually carry UDP are offered. Plain HTTP CONNECT is
/// absent by design — it tunnels TCP, so it cannot carry the UDP 500/4500 traffic
/// IKEv2 and NAT-T need.
///
/// `ConnectUdpMasque` was removed after evaluation: the only widely reachable
/// MASQUE deployment (Cloudflare WARP) speaks Connect-IP (RFC 9484) rather than
/// Connect-UDP (RFC 9298), requires account and device enrollment, and only
/// egresses into Cloudflare's own network — it cannot reach an operator ePDG at an
/// arbitrary host:port. Re-add it only alongside a real RFC 9298 proxy.
pub enum VowifiProxyMode {
    #[default]
    Direct,
    Socks5UdpAssociate,
    UdpRelay,
}

/// Per-line WiFi Calling runtime intent. SIM-bound carrier selection, DNS,
/// ePDG and IMS values live exclusively in `SimOverrideStore`; only the host
/// egress proxy remains attached to the physical line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LineVowifiConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub proxy_mode: VowifiProxyMode,
    #[serde(default)]
    pub proxy_endpoint: String,
    #[serde(default)]
    pub auto_restore: AutoRestoreConfig,
}

impl Default for LineVowifiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            proxy_mode: VowifiProxyMode::Direct,
            proxy_endpoint: String::new(),
            auto_restore: AutoRestoreConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StandaloneSimSlotConfig {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub reader_path: String,
    #[serde(default = "default_uim_slot")]
    pub uim_slot: u8,
    #[serde(default = "default_line_enabled")]
    pub enabled: bool,
}

/// Persisted controls for one stable physical-modem + SIM line. Every
/// connectivity intent and Trunk setting is owned by this profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LineProfileConfig {
    pub line_id: String,
    #[serde(default = "default_line_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub volte_connection_enabled: bool,
    #[serde(default)]
    pub volte_auto_restore: AutoRestoreConfig,
    #[serde(default, alias = "vilte")]
    pub ims_video: ImsVideoConfig,
    #[serde(default)]
    pub vowifi: LineVowifiConfig,
    #[serde(default)]
    pub trunk: TrunkProfileConfig,
    /// Whether the user explicitly enabled cellular data for this physical line.
    #[serde(default)]
    pub data_connection_enabled: bool,
    /// Listener configuration for the HTTP/SOCKS5 cellular data proxy.
    #[serde(default)]
    pub data_proxy: LineDataProxyConfig,
    /// Whether this physical line may establish a roaming cellular data bearer.
    #[serde(default = "default_roaming_allowed")]
    pub roaming_allowed: bool,
    /// Per-line simulated airplane mode. It disables cellular radio services
    /// while preserving Wi-Fi based VoWiFi and Asterisk Trunk intents.
    #[serde(default)]
    pub airplane_mode_enabled: bool,
    /// Per-line SMS path priority.
    #[serde(default)]
    pub sms_path: SmsPathPolicy,
    /// Per-line voice path priority.
    #[serde(default)]
    pub voice_path: VoicePathPolicy,
    /// Which IMS access leg may hold the *registration* when both VoLTE and
    /// VoWiFi are switched on.
    ///
    /// Distinct from `voice_path`, which orders **originating** calls across
    /// legs that are already registered. The default keeps both legs registered
    /// (each with its own RFC 5626 `reg-id`), preserving the live fallback that
    /// `services::orchestrator::ims_access` treats as an invariant. The
    /// single-registration modes follow GSMA IR.51 instead (§2.2.1
    /// re-registration on handover, §4.8 keep the same P-CSCF) and are opt-in.
    /// See `connectivity::core::ims_access` for the full reasoning.
    #[serde(default)]
    pub ims_access_preference: ImsAccessPreference,
    /// Per-line ordered IMS IP-family attempt order. The list elements are the families to
    /// enable, in fallback order. `[Ipv4v6, Ipv4, Ipv6]` tries dual-stack, then
    /// IPv4, then IPv6; `[Ipv6]` is IPv6-only. An empty list is invalid.
    #[serde(default = "default_line_volte_ip_families")]
    pub volte_ip_families: Vec<VolteIpFamily>,
    /// Whether the family order is still automatic. Automatic lines may use
    /// the carrier catalog's LTE `ip_family` as a hint; saving the order from
    /// the UI turns this off so the user's choice always wins.
    #[serde(default = "default_line_volte_ip_families_auto")]
    pub volte_ip_families_auto: bool,
    /// Per-line APN.
    #[serde(default)]
    pub apn: ApnConfig,
    /// Per-line eSIM (eUICC) management control. `None` means "auto": eSIM
    /// management is offered only when the line's SIM reports a eUICC chip
    /// (`esim_status`/`sim_type`). `Some(true)` forces eSIM management on even
    /// when detection is inconclusive (e.g. a reader that cannot report
    /// EsimStatus but is known to hold an eUICC), and `Some(false)` forces the
    /// line to be treated as a plain SIM so no lpac calls are ever issued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub esim_control: Option<bool>,
    /// Per-line lpac reader and transport settings.
    #[serde(default)]
    pub esim_reader: EsimReaderConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LineDataProxyConfig {
    #[serde(default = "default_data_proxy_listen_ip")]
    pub listen_ip: String,
    /// Zero asks the operating system to allocate an available port.
    #[serde(default)]
    pub listen_port: u16,
    /// Optional proxy credentials. Authentication is disabled only when both
    /// fields are empty; partial credentials are rejected during validation.
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}

impl Default for LineDataProxyConfig {
    fn default() -> Self {
        Self {
            listen_ip: default_data_proxy_listen_ip(),
            listen_port: 0,
            username: String::new(),
            password: String::new(),
        }
    }
}

impl LineDataProxyConfig {
    pub fn redacted(&self) -> Self {
        Self {
            password: String::new(),
            ..self.clone()
        }
    }
}

fn default_data_proxy_listen_ip() -> String {
    "0.0.0.0".to_string()
}

/// A discovered physical modem slot used to reconcile the persisted slot map.
/// `legacy_hardware_keys` contains selectors from older SimAdmin releases so
/// existing slot orders can be migrated when the physical anchor changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModemSlotObservation {
    pub slot_id: String,
    pub legacy_hardware_keys: Vec<String>,
    pub equipment_identifier: String,
    pub uim_slot: u8,
}

/// Persisted identity for a physical modem slot. `slot_id` is the physical
/// anchor (udev/sysfs/board slot), while `equipment_identifier` records the
/// current module occupying that slot. `hardware_key` is retained only as a
/// migration alias for configurations written by the previous implementation.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModemSlotConfig {
    #[serde(default)]
    pub slot_id: String,
    #[serde(default)]
    pub hardware_key: String,
    #[serde(default = "default_uim_slot")]
    pub uim_slot: u8,
    #[serde(default)]
    pub order: u32,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub equipment_identifier: String,
    #[serde(default)]
    pub last_seen_at: Option<String>,
    #[serde(default)]
    pub retired: bool,
}

fn default_uim_slot() -> u8 {
    1
}

fn default_line_enabled() -> bool {
    true
}

impl LineProfileConfig {
    /// Mirror each access leg's presence onto its IMS video state.
    ///
    /// These are status mirrors, not feature switches. IMS voice and video are
    /// the reason this project implements user-space IMS registration, so there
    /// is no separate "voice enabled" or "video enabled" opinion to consult: a
    /// leg that is connected offers MMTEL voice and video, and a carrier that
    /// does not permit them answers with a SIP error (488 on the media, 403/420
    /// or 380 on the registration) which the runtime surfaces as-is.
    fn sync_ims_video_access_gates(&mut self) {
        self.ims_video.volte_enabled = self.volte_connection_enabled;
        self.ims_video.vowifi_enabled = self.vowifi.enabled;
    }

    pub fn for_line(line_id: impl Into<String>) -> Self {
        Self {
            line_id: line_id.into(),
            enabled: true,
            volte_connection_enabled: false,
            volte_auto_restore: AutoRestoreConfig::default(),
            ims_video: ImsVideoConfig::default(),
            volte_ip_families: default_line_volte_ip_families(),
            volte_ip_families_auto: default_line_volte_ip_families_auto(),
            vowifi: LineVowifiConfig::default(),
            trunk: TrunkProfileConfig::default(),
            data_connection_enabled: false,
            data_proxy: LineDataProxyConfig::default(),
            roaming_allowed: default_roaming_allowed(),
            airplane_mode_enabled: false,
            sms_path: SmsPathPolicy::default().normalized(),
            voice_path: VoicePathPolicy::default().normalized(),
            ims_access_preference: ImsAccessPreference::default(),
            apn: ApnConfig::default(),
            esim_control: None,
            esim_reader: EsimReaderConfig::default(),
        }
    }

    /// A copy safe to serialize across an API boundary (trunk secret redacted).
    pub fn redacted(&self) -> Self {
        Self {
            trunk: self.trunk.redacted(),
            data_proxy: self.data_proxy.redacted(),
            ..self.clone()
        }
    }
}

impl Default for LineProfileConfig {
    fn default() -> Self {
        Self::for_line(String::new())
    }
}

fn sync_line_ims_video_access_gates(config: &mut AppConfig) -> bool {
    let mut changed = false;
    for profile in &mut config.line_profiles {
        let before = (
            profile.ims_video.volte_enabled,
            profile.ims_video.vowifi_enabled,
        );
        profile.sync_ims_video_access_gates();
        changed |= before
            != (
                profile.ims_video.volte_enabled,
                profile.ims_video.vowifi_enabled,
            );
    }
    changed
}

fn valid_line_id(line_id: &str) -> bool {
    line_id.strip_prefix("line-").is_some_and(|suffix| {
        suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn validate_line_data_proxy_config(config: &mut LineDataProxyConfig) -> Result<(), String> {
    config.listen_ip = config.listen_ip.trim().to_string();
    config.username = config.username.trim().to_string();
    if config.listen_ip.parse::<std::net::IpAddr>().is_err() {
        return Err("data_proxy_listen_ip_invalid".to_string());
    }
    if config.username.is_empty() != config.password.is_empty() {
        return Err("data_proxy_auth_credentials_incomplete".to_string());
    }
    if config.username.len() > u8::MAX as usize || config.password.len() > u8::MAX as usize {
        return Err("data_proxy_auth_credentials_too_long".to_string());
    }
    Ok(())
}

fn validate_line_vowifi_config(config: &mut LineVowifiConfig) -> Result<(), String> {
    config.proxy_endpoint = config.proxy_endpoint.trim().to_string();
    match config.proxy_mode {
        VowifiProxyMode::Direct => config.proxy_endpoint.clear(),
        VowifiProxyMode::Socks5UdpAssociate => {
            if !config.proxy_endpoint.starts_with("socks5://")
                && !config.proxy_endpoint.starts_with("socks5h://")
            {
                return Err("vowifi_proxy_endpoint_invalid".to_string());
            }
        }
        VowifiProxyMode::UdpRelay => {
            if !config.proxy_endpoint.starts_with("udp://") {
                return Err("vowifi_proxy_endpoint_invalid".to_string());
            }
        }
    }
    Ok(())
}

fn validate_esim_reader_config(config: &mut EsimReaderConfig) -> Result<(), String> {
    config.apdu_backend = config.apdu_backend.trim().to_ascii_lowercase();
    config.http_backend = config.http_backend.trim().to_ascii_lowercase();
    config.at_device = config.at_device.trim().to_string();
    config.qmi_device = config.qmi_device.trim().to_string();
    config.pcsc_reader_name = config.pcsc_reader_name.trim().to_string();
    config.mbim_device = config.mbim_device.trim().to_string();
    if !matches!(
        config.apdu_backend.as_str(),
        "qmi" | "qmi_qrtr" | "at" | "at_csim" | "pcsc" | "mbim"
    ) {
        return Err("esim_apdu_backend_invalid".to_string());
    }
    if !matches!(config.http_backend.as_str(), "curl" | "stdio") {
        return Err("esim_http_backend_invalid".to_string());
    }
    if matches!(config.apdu_backend.as_str(), "at" | "at_csim") && config.at_device.is_empty() {
        return Err("esim_at_device_required".to_string());
    }
    if config.apdu_backend == "mbim" && config.mbim_device.is_empty() {
        return Err("esim_mbim_device_required".to_string());
    }
    Ok(())
}

fn legacy_esim_reader_config(config: &EsimConfig) -> Option<EsimReaderConfig> {
    let mut reader = EsimReaderConfig {
        apdu_backend: config.apdu_backend.clone(),
        http_backend: config.http_backend.clone(),
        at_device: config.at_device.clone(),
        qmi_device: config.qmi_device.clone(),
        qmi_uim_slot: config.qmi_uim_slot,
        ..EsimReaderConfig::default()
    };
    if validate_esim_reader_config(&mut reader).is_err() || reader == EsimReaderConfig::default() {
        None
    } else {
        Some(reader)
    }
}

fn valid_trunk_binding(binding: &str) -> bool {
    binding.is_empty()
        || binding.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'_' | b'.' | b'*' | b'#')
        })
}

// ===================== Phase F: shared IMS video (ViLTE / VoWiFi video) =====================

fn default_vilte_codec() -> String {
    "h264".to_string()
}

fn default_vilte_video_payload_type() -> u8 {
    // Dynamic payload type; 99 is a common ViLTE choice for H.264.
    99
}

fn default_vilte_h264_fmtp() -> String {
    // Baseline profile, packetization-mode 1 (non-interleaved). profile-level-id
    // 42e01f = Constrained Baseline, level 3.1 — a widely interoperable IMS video
    // default. The relay never transcodes, so this is purely what we advertise
    // to the far end on the offer/answer; the negotiated value is carried
    // through verbatim.
    "profile-level-id=42e01f;packetization-mode=1".to_string()
}

/// Shared IMS video (ViLTE / VoWiFi video) media configuration.
///
/// Video rides the *same* IMS voice session as the access's voice call (one
/// INVITE, an audio `m=` line plus a video `m=` line). VoLTE and VoWiFi each
/// expose their effective state through `volte_enabled` and `vowifi_enabled`.
/// Those fields are maintained by `ConfigManager`: VoLTE video follows the
/// line's VoLTE connection plus voice gateway, while VoWiFi video follows the
/// line's VoWiFi connection. They are status mirrors, not independent switches.
/// On the target hardware class (no audio/video capture) the device is a pure
/// media relay: it forwards RTP between the operator IMS leg and an internal
/// SIP UA and never encodes/decodes video. Therefore only pass-through codecs
/// are meaningful — `codec` is what we advertise, not something we transcode to.
///
/// Schema migration: the historical field `feature_enabled` (a single gate that
/// implicitly meant VoLTE) is accepted as an alias for `volte_enabled` on load,
/// so existing persisted configs migrate in place.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImsVideoConfig {
    /// Effective configured state for the VoLTE (LTE) access leg.
    #[serde(default, alias = "feature_enabled")]
    pub volte_enabled: bool,
    /// Effective configured state for the VoWiFi (WiFi/ePDG) access leg.
    #[serde(default)]
    pub vowifi_enabled: bool,
    /// Advertised video codec name (relay is pass-through; H.264 is the IMS
    /// video baseline mandated by GSMA IR.94).
    #[serde(default = "default_vilte_codec")]
    pub codec: String,
    /// Dynamic RTP payload type to advertise for the video stream.
    #[serde(default = "default_vilte_video_payload_type")]
    pub video_payload_type: u8,
    /// `a=fmtp` parameters advertised for the video codec.
    #[serde(default = "default_vilte_h264_fmtp")]
    pub h264_fmtp: String,
}

impl Default for ImsVideoConfig {
    fn default() -> Self {
        Self {
            volte_enabled: false,
            vowifi_enabled: false,
            codec: default_vilte_codec(),
            video_payload_type: default_vilte_video_payload_type(),
            h264_fmtp: default_vilte_h264_fmtp(),
        }
    }
}

// ===================== Phase C: multi-path SMS orchestrator =====================

/// One access path the orchestrator can route SMS/voice through.
///
/// The set is closed (VoWiFi / VoLTE / CS), matching the `AccessLeg` enum
/// discussed in the design doc §4.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessPathKind {
    /// VoWiFi (IMS over WiFi / ePDG).
    Vowifi,
    /// VoLTE (IMS over LTE / kernel xfrm).
    Volte,
    /// Circuit-switched (ModemManager baseband).
    Cs,
}

impl AccessPathKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AccessPathKind::Vowifi => "vowifi",
            AccessPathKind::Volte => "volte",
            AccessPathKind::Cs => "cs",
        }
    }

    /// Transport tag used in `db::SmsMessage.transport`.
    pub fn transport_tag(self) -> &'static str {
        match self {
            AccessPathKind::Vowifi => "vowifi_ims",
            AccessPathKind::Volte => "volte_ims",
            AccessPathKind::Cs => "modem",
        }
    }

    /// Whether this path is an IMS leg (needs registration / listener election).
    pub fn is_ims(self) -> bool {
        matches!(self, AccessPathKind::Vowifi | AccessPathKind::Volte)
    }
}

/// Behavior when the leg currently sending a message is disabled mid-flight
/// (user turns off the line while a send is still in progress and not yet
/// confirmed on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MidFlightDisablePolicy {
    /// Automatically fall through to the next enabled leg (default).
    #[default]
    AutoSwitch,
    /// Report failure to the caller; do not auto-switch.
    Fail,
}

impl MidFlightDisablePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            MidFlightDisablePolicy::AutoSwitch => "auto_switch",
            MidFlightDisablePolicy::Fail => "fail",
        }
    }
}

/// One layer in a priority-ordered path policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathLayerConfig {
    pub kind: AccessPathKind,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_sms_path_order() -> Vec<PathLayerConfig> {
    vec![
        PathLayerConfig {
            kind: AccessPathKind::Vowifi,
            enabled: true,
        },
        PathLayerConfig {
            kind: AccessPathKind::Volte,
            enabled: true,
        },
        PathLayerConfig {
            kind: AccessPathKind::Cs,
            enabled: true,
        },
    ]
}

/// SMS routing policy. Legacy priority fields remain serializable so existing
/// installations upgrade without losing their config, but normalization resets
/// them to the fixed VoWiFi -> VoLTE -> CS order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmsPathPolicy {
    /// Legacy compatibility field. User-defined ordering is no longer applied.
    #[serde(default = "default_sms_path_order")]
    pub priority: Vec<PathLayerConfig>,
    /// Require SMS sends to use VoWiFi only. No VoLTE/CS fallback is attempted.
    #[serde(default)]
    pub force_vowifi_send: bool,
    /// Cross-transport dedup on receive.
    #[serde(default = "default_true")]
    pub dedupe_enabled: bool,
    /// Keep the CS listener as a fallback receiver even while an IMS leg is the
    /// active listener (with dedup enforced) instead of pausing it entirely.
    #[serde(default = "default_true")]
    pub cs_fallback_receiver: bool,
    /// What to do when the sending leg is disabled mid-flight.
    #[serde(default)]
    pub mid_flight_disable: MidFlightDisablePolicy,
    /// Retention window (days) for dedup fingerprint rows before cleanup.
    #[serde(default = "default_sms_dedup_retention_days")]
    pub dedup_retention_days: u32,
    /// Maximum number of user-visible SMS rows retained in SQLite. Oldest rows
    /// are pruned after the limit is exceeded so long-running devices cannot
    /// grow the database without bound.
    #[serde(default = "default_sms_message_retention_limit")]
    pub message_retention_limit: u32,
}

fn default_sms_dedup_retention_days() -> u32 {
    30
}

fn default_sms_message_retention_limit() -> u32 {
    10_000
}

impl Default for SmsPathPolicy {
    fn default() -> Self {
        Self {
            priority: default_sms_path_order(),
            force_vowifi_send: false,
            dedupe_enabled: true,
            cs_fallback_receiver: true,
            mid_flight_disable: MidFlightDisablePolicy::AutoSwitch,
            dedup_retention_days: default_sms_dedup_retention_days(),
            message_retention_limit: default_sms_message_retention_limit(),
        }
    }
}

impl SmsPathPolicy {
    /// Fixed send order, reduced to VoWiFi only when explicitly forced.
    pub fn enabled_layers(&self) -> impl Iterator<Item = AccessPathKind> + '_ {
        const AUTO: [AccessPathKind; 3] = [
            AccessPathKind::Vowifi,
            AccessPathKind::Volte,
            AccessPathKind::Cs,
        ];
        let count = if self.force_vowifi_send {
            1
        } else {
            AUTO.len()
        };
        AUTO[..count].iter().copied()
    }

    /// Receive-side IMS order is fixed and independent from the send-only switch.
    pub fn enabled_ims_layers(&self) -> impl Iterator<Item = AccessPathKind> + '_ {
        [AccessPathKind::Vowifi, AccessPathKind::Volte].into_iter()
    }

    /// All receive paths stay enabled; the user switch only constrains sending.
    pub fn is_enabled(&self, _kind: AccessPathKind) -> bool {
        true
    }

    /// Normalize legacy fields to the fixed routing and reliable receive policy.
    pub fn normalized(mut self) -> Self {
        self.priority = default_sms_path_order();
        self.dedupe_enabled = true;
        self.cs_fallback_receiver = true;
        self.mid_flight_disable = MidFlightDisablePolicy::AutoSwitch;
        self.dedup_retention_days = self.dedup_retention_days.clamp(1, 3650);
        self.message_retention_limit = self.message_retention_limit.clamp(100, 100_000);
        self
    }
}

// ===================== Voice routing =====================

fn default_voice_path_order() -> Vec<PathLayerConfig> {
    vec![
        PathLayerConfig {
            kind: AccessPathKind::Vowifi,
            enabled: true,
        },
        PathLayerConfig {
            kind: AccessPathKind::Volte,
            enabled: true,
        },
    ]
}

/// Voice path selection is deliberately independent from the SMS policy.
/// `gateway_mode` remains true on the Qualcomm 410 because the host has no
/// microphone/speaker and hands media to the per-line Asterisk trunk. CS calls
/// remain available through the line-scoped ModemManager call API, but are not
/// exposed here because there is no CS media backend behind the SIP trunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoicePathPolicy {
    #[serde(default = "default_voice_path_order")]
    pub priority: Vec<PathLayerConfig>,
    #[serde(default = "default_true")]
    pub gateway_mode: bool,
}

impl Default for VoicePathPolicy {
    fn default() -> Self {
        Self {
            priority: default_voice_path_order(),
            gateway_mode: true,
        }
    }
}

impl VoicePathPolicy {
    pub fn enabled_layers(&self) -> impl Iterator<Item = AccessPathKind> + '_ {
        self.priority
            .iter()
            .filter(|layer| layer.enabled)
            .map(|layer| layer.kind)
    }

    pub fn normalized(mut self) -> Self {
        let mut seen: Vec<AccessPathKind> = Vec::new();
        let mut deduped: Vec<PathLayerConfig> = Vec::new();
        for layer in self.priority.into_iter() {
            if layer.kind.is_ims() && !seen.contains(&layer.kind) {
                seen.push(layer.kind);
                deduped.push(layer);
            }
        }
        for kind in [AccessPathKind::Vowifi, AccessPathKind::Volte] {
            if !seen.contains(&kind) {
                deduped.push(PathLayerConfig {
                    kind,
                    enabled: true,
                });
            }
        }
        self.priority = deduped;
        self
    }
}

/// 应用配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    /// Version of the persisted line-profile schema.
    #[serde(default)]
    pub line_config_version: u32,
    #[serde(default)]
    pub notifications: NotificationConfig,
    #[serde(default)]
    pub device_network: DeviceNetworkConfig,
    #[serde(default)]
    pub ue_isolation: UeIsolationConfig,
    #[serde(default)]
    pub version_update_notifications: VersionUpdateNotificationConfig,
    #[serde(default)]
    pub github_download_proxy: GithubDownloadProxyConfig,
    #[serde(default)]
    pub diagnostic_log: DiagnosticLogConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub esim: EsimConfig,
    #[serde(default)]
    pub automation: AutomationConfig,
    #[serde(default)]
    pub line_profiles: Vec<LineProfileConfig>,
    #[serde(default)]
    pub modem_slots: Vec<ModemSlotConfig>,
    #[serde(default)]
    pub standalone_sim_slots: Vec<StandaloneSimSlotConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            line_config_version: CURRENT_LINE_CONFIG_VERSION,
            notifications: NotificationConfig::default(),
            device_network: DeviceNetworkConfig::default(),
            ue_isolation: UeIsolationConfig::default(),
            version_update_notifications: VersionUpdateNotificationConfig::default(),
            github_download_proxy: GithubDownloadProxyConfig::default(),
            diagnostic_log: DiagnosticLogConfig::default(),
            security: SecurityConfig::default(),
            esim: EsimConfig::default(),
            automation: AutomationConfig::default(),
            line_profiles: Vec::new(),
            modem_slots: Vec::new(),
            standalone_sim_slots: Vec::new(),
        }
    }
}

pub(crate) const CURRENT_LINE_CONFIG_VERSION: u32 = 4;
pub(crate) const CONFIG_STORAGE_SCHEMA_VERSION: u32 = 1;

fn migrate_template_string(template: &mut String) -> bool {
    let mut changed = false;
    let md5_patterns = [
        "OTA包 MD5: {{md5}}",
        "OTA包 MD5: {{MD5}}",
        "OTA包MD5: {{md5}}",
        "OTA包MD5: {{MD5}}",
        "MD5: {{md5}}",
        "MD5: {{MD5}}",
        "校验值: {{md5}}",
        "校验值: {{MD5}}",
        "二进制MD5: {{binary_md5}}",
        "前端MD5: {{frontend_md5}}",
        "{{md5}}",
        "{{MD5}}",
        "{{binary_md5}}",
        "{{frontend_md5}}",
    ];

    for pattern in md5_patterns {
        // Try replacing with leading newline (escaped JSON or real)
        let with_escaped_newline = format!("\\n{}", pattern);
        if template.contains(&with_escaped_newline) {
            *template = template.replace(&with_escaped_newline, "");
            changed = true;
        }
        let with_newline = format!("\n{}", pattern);
        if template.contains(&with_newline) {
            *template = template.replace(&with_newline, "");
            changed = true;
        }

        // Try replacing with trailing newline (escaped JSON or real)
        let with_escaped_trailing = format!("{}\\n", pattern);
        if template.contains(&with_escaped_trailing) {
            *template = template.replace(&with_escaped_trailing, "");
            changed = true;
        }
        let with_trailing = format!("{}\n", pattern);
        if template.contains(&with_trailing) {
            *template = template.replace(&with_trailing, "");
            changed = true;
        }

        // Fallback: replace pattern directly
        if template.contains(pattern) {
            *template = template.replace(pattern, "");
            changed = true;
        }
    }

    let time_replacements = [
        ("构建时间: {{构建时间}}", "时间: {{时间}}"),
        ("构建时间: {{build_time}}", "时间: {{time}}"),
        ("{{build_time}}", "{{time}}"),
        ("{{构建时间}}", "{{时间}}"),
    ];
    for (old, new) in time_replacements {
        if template.contains(old) {
            *template = template.replace(old, new);
            changed = true;
        }
    }

    changed
}

fn migrate_templates_to_remove_md5(config: &mut AppConfig) -> bool {
    let mut changed = false;

    // 1. Notification rules templates
    for rule in &mut config.notifications.rules {
        if rule.event_type == NotificationEventType::VersionUpdate
            && migrate_template_string(&mut rule.template)
        {
            changed = true;
        }
    }

    // 2. Notification channels templates
    for channel in &mut config.notifications.channels {
        if let Some(obj) = channel.config.as_object_mut() {
            // E.g. BarkConfig, PushPlusConfig, WecomAppConfig etc have nested "common"
            if let Some(common) = obj.get_mut("common").and_then(|v| v.as_object_mut()) {
                if let Some(serde_json::Value::String(tpl)) = common.get_mut("update_template") {
                    if migrate_template_string(tpl) {
                        changed = true;
                    }
                }
            }
            if let Some(serde_json::Value::String(tpl)) = obj.get_mut("update_template") {
                if migrate_template_string(tpl) {
                    changed = true;
                }
            }
        }
    }

    changed
}

/// 配置管理器
pub struct ConfigManager {
    config: Arc<RwLock<AppConfig>>,
    storage: ConfigStorage,
    save_lock: Mutex<()>,
}

#[derive(Debug, Clone)]
enum ConfigStorage {
    /// Internal test backend. Production paths are always SQLite.
    Json(PathBuf),
    /// Production backend. The carrier catalog and runtime/event database stay
    /// separate; this database owns user configuration only.
    Sqlite(PathBuf),
}

impl ConfigStorage {
    fn from_path(path: PathBuf) -> Self {
        let is_sqlite = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "sqlite" | "sqlite3" | "db"
                )
            });
        if is_sqlite {
            Self::Sqlite(path)
        } else {
            Self::Json(path)
        }
    }

    fn path(&self) -> &PathBuf {
        match self {
            Self::Json(path) | Self::Sqlite(path) => path,
        }
    }
}

fn open_config_database(path: &Path) -> Result<SqliteConnection, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create config database directory {}: {error}",
                parent.display()
            )
        })?;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!(
                "Refusing symlink config database {}",
                path.display()
            ));
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(format!(
                "Config database path is not a regular file: {}",
                path.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(path).map_err(|error| {
                format!(
                    "Failed to create config database {}: {error}",
                    path.display()
                )
            })?;
        }
        Err(error) => {
            return Err(format!(
                "Failed to inspect config database {}: {error}",
                path.display()
            ));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
            format!(
                "Failed to set config database permissions {}: {error}",
                path.display()
            )
        })?;
    }
    let connection = SqliteConnection::open(path)
        .map_err(|error| format!("Failed to open config database {}: {error}", path.display()))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| format!("Failed to configure config database timeout: {error}"))?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(|error| format!("Failed to enable config database WAL: {error}"))?;
    connection
        .execute_batch(
            "PRAGMA synchronous = FULL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS app_config (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 storage_schema_version INTEGER NOT NULL,
                 line_config_version INTEGER NOT NULL,
                 config_json TEXT NOT NULL CHECK (json_valid(config_json)),
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
        .map_err(|error| format!("Failed to initialize config database schema: {error}"))?;
    let quick_check = connection
        .query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0))
        .map_err(|error| format!("Failed to check config database integrity: {error}"))?;
    if quick_check != "ok" {
        return Err(format!(
            "Config database integrity check failed: {quick_check}"
        ));
    }
    Ok(connection)
}

fn load_config_document(
    connection: &SqliteConnection,
    path: &Path,
) -> Result<Option<(AppConfig, bool)>, String> {
    let row = connection
        .query_row(
            "SELECT storage_schema_version, line_config_version, config_json
             FROM app_config WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("Failed to read config database {}: {error}", path.display()))?;
    let Some((storage_version, stored_line_version, content)) = row else {
        return Ok(None);
    };
    if storage_version != CONFIG_STORAGE_SCHEMA_VERSION {
        return Err(format!(
            "Unsupported config storage schema version {storage_version} in {}; expected {}",
            path.display(),
            CONFIG_STORAGE_SCHEMA_VERSION
        ));
    }
    let config = parse_current_config(&content, path)?;
    if stored_line_version != config.line_config_version {
        return Err(format!(
            "Config database version mismatch in {}: column={stored_line_version}, document={}",
            path.display(),
            config.line_config_version
        ));
    }
    let canonical_rewrite_required = serde_json::from_str::<serde_json::Value>(&content)
        .ok()
        .zip(serde_json::to_value(&config).ok())
        .is_some_and(|(stored, canonical)| stored != canonical);
    Ok(Some((config, canonical_rewrite_required)))
}

fn parse_current_config(content: &str, source: &Path) -> Result<AppConfig, String> {
    let config = serde_json::from_str::<AppConfig>(content).map_err(|error| {
        format!(
            "Failed to parse configuration {}: {error}",
            source.display()
        )
    })?;
    if config.line_config_version != CURRENT_LINE_CONFIG_VERSION {
        return Err(format!(
            "Unsupported line config version {} in {}; expected {}",
            config.line_config_version,
            source.display(),
            CURRENT_LINE_CONFIG_VERSION
        ));
    }
    Ok(config)
}

fn save_config_document(path: &Path, content: &str) -> Result<(), String> {
    let mut connection = open_config_database(path)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("Failed to begin config transaction: {error}"))?;
    transaction
        .execute(
            "INSERT INTO app_config (
                 singleton, storage_schema_version, line_config_version, config_json, updated_at
             ) VALUES (1, ?1, ?2, ?3, ?4)
             ON CONFLICT(singleton) DO UPDATE SET
                 storage_schema_version = excluded.storage_schema_version,
                 line_config_version = excluded.line_config_version,
                 config_json = excluded.config_json,
                 updated_at = excluded.updated_at",
            params![
                CONFIG_STORAGE_SCHEMA_VERSION,
                CURRENT_LINE_CONFIG_VERSION,
                content,
                chrono::Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|error| format!("Failed to write config document: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("Failed to commit config transaction: {error}"))
}

fn save_json_document(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create config directory: {error}"))?;
    }

    let temp_path = path.with_extension("tmp");
    let backup_path = path.with_extension("bak");
    let mut temp_file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
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
        if cfg!(windows) && path.exists() {
            fs::copy(&temp_path, path)
                .map_err(|error| format!("Failed to replace config file: {error}"))?;
            fs::remove_file(&temp_path)
                .map_err(|error| format!("Failed to remove temporary config file: {error}"))?;
        } else {
            return Err(format!(
                "Failed to atomically replace config file: {rename_error}"
            ));
        }
    }

    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
            let _ = directory.sync_all();
        }
    }
    Ok(())
}

impl ConfigManager {
    /// Load a persisted configuration. An existing file must parse exactly as
    /// the current schema; silently replacing it with defaults hides invalid
    /// per-line settings and can direct operations to the wrong SIM.
    pub fn try_new(config_path: PathBuf) -> Result<Self, String> {
        match ConfigStorage::from_path(config_path) {
            ConfigStorage::Json(path) => Self::try_new_json(path),
            ConfigStorage::Sqlite(path) => Self::try_new_sqlite(path),
        }
    }

    fn try_new_json(config_path: PathBuf) -> Result<Self, String> {
        let mut canonical_rewrite_required = false;
        let mut config = if config_path.exists() {
            let content = fs::read_to_string(&config_path).map_err(|error| {
                format!(
                    "Failed to read config file {}: {error}",
                    config_path.display()
                )
            })?;
            let config = serde_json::from_str::<AppConfig>(&content).map_err(|error| {
                format!(
                    "Failed to parse config file {}: {error}",
                    config_path.display()
                )
            })?;
            if config.line_config_version != CURRENT_LINE_CONFIG_VERSION {
                return Err(format!(
                    "Unsupported line config version {} in {}; expected {}",
                    config.line_config_version,
                    config_path.display(),
                    CURRENT_LINE_CONFIG_VERSION
                ));
            }
            canonical_rewrite_required = serde_json::from_str::<serde_json::Value>(&content)
                .ok()
                .zip(serde_json::to_value(&config).ok())
                .is_some_and(|(stored, canonical)| stored != canonical);
            config
        } else {
            info!("No config file found, using defaults");
            AppConfig::default()
        };

        let templates_changed = migrate_templates_to_remove_md5(&mut config);
        let video_gates_changed = sync_line_ims_video_access_gates(&mut config);
        let changed = templates_changed || video_gates_changed || canonical_rewrite_required;

        let manager = Self {
            config: Arc::new(RwLock::new(config)),
            storage: ConfigStorage::Json(config_path),
            save_lock: Mutex::new(()),
        };

        // Rewrite only current-schema files that need canonical formatting.
        if !manager.storage.path().exists() || changed {
            manager.save()?;
        }
        // `vowifi-profiles.conf` is no longer created or rewritten here. Custom
        // carrier profiles live in the database; an existing file is migrated
        // once at startup and then archived.

        Ok(manager)
    }

    fn try_new_sqlite(config_path: PathBuf) -> Result<Self, String> {
        let connection = open_config_database(&config_path)?;
        let stored = load_config_document(&connection, &config_path)?;
        let database_was_empty = stored.is_none();

        let (mut config, canonical_rewrite_required) = match stored {
            Some((config, rewrite)) => (config, rewrite),
            None => {
                info!(path = ?config_path, "No SQLite configuration found, using defaults");
                (AppConfig::default(), false)
            }
        };

        let templates_changed = migrate_templates_to_remove_md5(&mut config);
        let video_gates_changed = sync_line_ims_video_access_gates(&mut config);
        let changed = templates_changed || video_gates_changed || canonical_rewrite_required;
        let manager = Self {
            config: Arc::new(RwLock::new(config)),
            storage: ConfigStorage::Sqlite(config_path.clone()),
            save_lock: Mutex::new(()),
        };

        if database_was_empty || changed {
            manager.save()?;
        }
        Ok(manager)
    }

    #[cfg(test)]
    fn new(config_path: PathBuf) -> Self {
        Self::try_new(config_path).expect("test configuration must load")
    }

    /// 获取通知配置
    pub fn get_notifications(&self) -> NotificationConfig {
        self.config.read().unwrap().notifications.clone()
    }

    /// 获取自动化配置
    pub fn get_automation_config(&self) -> AutomationConfig {
        self.config.read().unwrap().automation.clone()
    }

    /// 更新自动化配置
    pub fn set_automation_config(&self, automation: AutomationConfig) -> Result<(), String> {
        let mut ids = std::collections::HashSet::new();
        for task in &automation.tasks {
            if task.id.trim().is_empty() || !ids.insert(task.id.clone()) {
                return Err("automation_task_id_invalid_or_duplicate".to_string());
            }
            if let Some(target) = &task.target {
                match target {
                    AutomationTarget::ModemLine { line_id } if line_id.trim().is_empty() => {
                        return Err("automation_target_line_id_required".to_string());
                    }
                    AutomationTarget::StandaloneSimSlot { slot_id }
                        if slot_id.trim().is_empty() =>
                    {
                        return Err("automation_target_slot_id_required".to_string());
                    }
                    _ => {}
                }
            }
            if !matches!(&task.action, AutomationAction::RebootDevice { .. })
                && task.target.is_none()
            {
                return Err("automation_target_line_required".to_string());
            }
            match &task.action {
                AutomationAction::ConsumeData { bytes, unit } => {
                    let multiplier = match unit.as_str() {
                        "auto" | "bytes" => Some(1u64),
                        "kb" => Some(1024),
                        "mb" => Some(1024 * 1024),
                        _ => None,
                    };
                    if *bytes == 0
                        || multiplier
                            .and_then(|value| bytes.checked_mul(value))
                            .is_none_or(|amount| amount > 1024 * 1024 * 1024)
                    {
                        return Err("automation_consume_data_invalid".to_string());
                    }
                }
                AutomationAction::DialCall {
                    country_code,
                    phone_number,
                    duration_seconds,
                } if !country_code.starts_with('+')
                    || country_code.len() < 2
                    || !country_code[1..].chars().all(|c| c.is_ascii_digit())
                    || phone_number.trim().is_empty()
                    || !phone_number.chars().all(|c| c.is_ascii_digit())
                    || *duration_seconds == 0
                    || *duration_seconds > 7_200 =>
                {
                    return Err("automation_dial_call_invalid".to_string());
                }
                _ => {}
            }
            if let AutomationTrigger::Cron { expression } = &task.trigger {
                if expression.split_whitespace().count() != 5 || expression.len() > 128 {
                    return Err("automation_cron_expression_invalid".to_string());
                }
            }
        }
        {
            let mut config = self.config.write().unwrap();
            config.automation = automation;
        }
        self.save()
    }

    pub fn get_esim_config(&self) -> EsimConfig {
        self.config.read().unwrap().esim.clone()
    }

    pub fn get_line_profiles(&self) -> Vec<LineProfileConfig> {
        self.config.read().unwrap().line_profiles.clone()
    }

    /// Ensure every discovered physical line has one explicit persisted profile.
    pub fn reconcile_line_profiles(&self, line_ids: &[String]) -> Result<bool, String> {
        let mut ordered_ids = Vec::new();
        for line_id in line_ids.iter().filter(|line_id| valid_line_id(line_id)) {
            if !ordered_ids.contains(line_id) {
                ordered_ids.push(line_id.clone());
            }
        }
        if ordered_ids.is_empty() {
            return Ok(false);
        }

        let migrated = {
            let mut config = self.config.write().unwrap();
            let mut changed = false;
            for profile in &mut config.line_profiles {
                let normalized = profile.voice_path.clone().normalized();
                if normalized != profile.voice_path {
                    profile.voice_path = normalized;
                    changed = true;
                }
            }
            for line_id in &ordered_ids {
                if !config
                    .line_profiles
                    .iter()
                    .any(|profile| &profile.line_id == line_id)
                {
                    config
                        .line_profiles
                        .push(LineProfileConfig::for_line(line_id));
                    changed = true;
                }
            }
            // Old releases stored reader ports globally. Copy them only when
            // there is exactly one physical line; applying one port to multiple
            // lines would route lpac to the wrong card.
            if ordered_ids.len() == 1 {
                if let Some(legacy_reader) = legacy_esim_reader_config(&config.esim) {
                    if let Some(profile) = config
                        .line_profiles
                        .iter_mut()
                        .find(|profile| profile.line_id == ordered_ids[0])
                    {
                        if profile.esim_reader == EsimReaderConfig::default() {
                            profile.esim_reader = legacy_reader;
                            config.esim.apdu_backend = default_esim_apdu_backend();
                            config.esim.http_backend = default_esim_http_backend();
                            config.esim.at_device.clear();
                            config.esim.qmi_device.clear();
                            config.esim.qmi_uim_slot = 0;
                            changed = true;
                        }
                    }
                }
            }
            config
                .line_profiles
                .sort_by(|left, right| left.line_id.cmp(&right.line_id));
            changed
        };

        if migrated {
            self.save()?;
        }
        Ok(migrated)
    }

    /// Reconcile discovered physical hardware with persistent display slots.
    /// Missing hardware is retained so a modem returns to its original slot
    /// after service restart, USB re-enumeration, or a temporary disconnect.
    pub fn reconcile_modem_slots(
        &self,
        observations: &[ModemSlotObservation],
    ) -> Result<HashMap<String, ModemSlotConfig>, String> {
        let (slots, changed) = {
            let mut config = self.config.write().unwrap();
            let mut changed = false;

            for slot in &mut config.modem_slots {
                if slot.uim_slot == 0 {
                    slot.uim_slot = 1;
                    changed = true;
                }
                if slot.slot_id.trim() != slot.slot_id {
                    slot.slot_id = slot.slot_id.trim().to_string();
                    changed = true;
                }
                if slot.hardware_key.trim() != slot.hardware_key {
                    slot.hardware_key = slot.hardware_key.trim().to_string();
                    changed = true;
                }
            }
            let original_slot_count = config.modem_slots.len();
            let mut seen_slot_keys = std::collections::HashSet::new();
            config.modem_slots.retain(|slot| {
                let identity = if slot.slot_id.is_empty() {
                    slot.hardware_key.clone()
                } else {
                    slot.slot_id.clone()
                };
                !identity.is_empty() && seen_slot_keys.insert((identity, slot.uim_slot))
            });
            changed |= config.modem_slots.len() != original_slot_count;

            let mut sorted_observations = observations.to_vec();
            sorted_observations.sort_by(|left, right| {
                left.slot_id
                    .cmp(&right.slot_id)
                    .then_with(|| left.uim_slot.cmp(&right.uim_slot))
            });

            for observation in &sorted_observations {
                let uim_slot = observation.uim_slot.max(1);
                let matching_index = config.modem_slots.iter().position(|slot| {
                    (slot.slot_id == observation.slot_id
                        || (slot.slot_id.is_empty()
                            && observation
                                .legacy_hardware_keys
                                .iter()
                                .any(|key| key == &slot.hardware_key)))
                        && slot.uim_slot == uim_slot
                });

                if let Some(index) = matching_index {
                    let slot = &mut config.modem_slots[index];
                    if slot.slot_id != observation.slot_id {
                        slot.slot_id = observation.slot_id.clone();
                        changed = true;
                    }
                    if slot.hardware_key.is_empty() {
                        slot.hardware_key = observation
                            .legacy_hardware_keys
                            .first()
                            .cloned()
                            .unwrap_or_default();
                        changed = true;
                    }
                    if slot.equipment_identifier != observation.equipment_identifier {
                        slot.equipment_identifier = observation.equipment_identifier.clone();
                        slot.last_seen_at = Some(chrono::Utc::now().to_rfc3339());
                        changed = true;
                    }
                    if slot.retired {
                        slot.retired = false;
                        changed = true;
                    }
                }
            }

            let mut next_order = config
                .modem_slots
                .iter()
                .map(|slot| slot.order)
                .max()
                .unwrap_or(0)
                .saturating_add(1)
                .max(1);
            for observation in &sorted_observations {
                let slot_id = observation.slot_id.trim();
                let uim_slot = observation.uim_slot.max(1);
                if slot_id.is_empty()
                    || config
                        .modem_slots
                        .iter()
                        .any(|slot| slot.slot_id == slot_id && slot.uim_slot == uim_slot)
                {
                    continue;
                }
                config.modem_slots.push(ModemSlotConfig {
                    slot_id: slot_id.to_string(),
                    hardware_key: observation
                        .legacy_hardware_keys
                        .first()
                        .cloned()
                        .unwrap_or_default(),
                    uim_slot,
                    order: next_order,
                    label: format!("基带 {next_order}"),
                    equipment_identifier: observation.equipment_identifier.clone(),
                    last_seen_at: Some(chrono::Utc::now().to_rfc3339()),
                    retired: false,
                });
                next_order = next_order.saturating_add(1);
                changed = true;
            }

            let mut used_orders = std::collections::HashSet::new();
            let mut repair_order = config
                .modem_slots
                .iter()
                .map(|slot| slot.order)
                .max()
                .unwrap_or(0)
                .saturating_add(1)
                .max(1);
            config.modem_slots.sort_by(|left, right| {
                left.order
                    .cmp(&right.order)
                    .then_with(|| left.slot_id.cmp(&right.slot_id))
                    .then_with(|| left.hardware_key.cmp(&right.hardware_key))
            });
            for slot in &mut config.modem_slots {
                if slot.order == 0 || !used_orders.insert(slot.order) {
                    slot.order = repair_order;
                    used_orders.insert(repair_order);
                    repair_order = repair_order.saturating_add(1);
                    changed = true;
                }
                let normalized_label = slot.label.trim().to_string();
                if normalized_label != slot.label {
                    slot.label = normalized_label;
                    changed = true;
                }
                if slot.label.is_empty() {
                    slot.label = format!("基带 {}", slot.order);
                    changed = true;
                }
            }
            config.modem_slots.sort_by(|left, right| {
                left.order
                    .cmp(&right.order)
                    .then_with(|| left.slot_id.cmp(&right.slot_id))
                    .then_with(|| left.hardware_key.cmp(&right.hardware_key))
            });

            let slots = config
                .modem_slots
                .iter()
                .filter(|slot| !slot.slot_id.is_empty())
                .cloned()
                .map(|slot| (format!("{}#uim{}", slot.slot_id, slot.uim_slot), slot))
                .collect::<HashMap<_, _>>();
            (slots, changed)
        };

        if changed {
            self.save()?;
        }
        Ok(slots)
    }

    /// Move physical-line references from older device/SIM-derived IDs to the
    /// current physical-slot ID. The old profile is retained as history while
    /// live automation and notification references are rewritten in place.
    pub fn migrate_line_profile_aliases(
        &self,
        line_id: &str,
        legacy_line_ids: &[String],
    ) -> Result<bool, String> {
        if !valid_line_id(line_id) {
            return Err("invalid_line_id".to_string());
        }
        let migrated = {
            let mut config = self.config.write().unwrap();
            let aliases = legacy_line_ids
                .iter()
                .map(|id| id.trim())
                .filter(|id| valid_line_id(id) && *id != line_id)
                .collect::<std::collections::HashSet<_>>();
            let mut changed = false;

            if !config
                .line_profiles
                .iter()
                .any(|profile| profile.line_id == line_id)
            {
                if let Some(source) = config
                    .line_profiles
                    .iter()
                    .find(|profile| aliases.contains(profile.line_id.as_str()))
                    .cloned()
                {
                    config.line_profiles.push(LineProfileConfig {
                        line_id: line_id.to_string(),
                        ..source
                    });
                    config
                        .line_profiles
                        .sort_by(|left, right| left.line_id.cmp(&right.line_id));
                    changed = true;
                }
            }

            for task in &mut config.automation.tasks {
                if let Some(AutomationTarget::ModemLine { line_id: target }) = &mut task.target {
                    if aliases.contains(target.trim()) {
                        *target = line_id.to_string();
                        changed = true;
                    }
                }
            }

            for rule in &mut config.notifications.rules {
                let mut rewritten = Vec::with_capacity(rule.sim_channel_ids.len());
                for target in &rule.sim_channel_ids {
                    let target = if aliases.contains(target.trim()) {
                        line_id
                    } else {
                        target.as_str()
                    };
                    if !rewritten.iter().any(|existing| existing == target) {
                        rewritten.push(target.to_string());
                    }
                }
                if rewritten != rule.sim_channel_ids {
                    rule.sim_channel_ids = rewritten;
                    changed = true;
                }
            }

            changed
        };
        if migrated {
            self.save()?;
        }
        Ok(migrated)
    }

    /// Convert reader reservations created by the removed standalone-reader UI
    /// into the unified line target used by SMS, calls, notifications, and the
    /// automation scheduler.
    pub fn migrate_standalone_reader_references(
        &self,
        slot_id: &str,
        line_id: &str,
    ) -> Result<bool, String> {
        let slot_id = slot_id.trim();
        if slot_id.is_empty() || !valid_line_id(line_id) {
            return Err("invalid_reader_line_migration".to_string());
        }
        let legacy_notification_id = format!("reader:{slot_id}");
        let migrated = {
            let mut config = self.config.write().unwrap();
            let mut changed = false;
            for task in &mut config.automation.tasks {
                if matches!(
                    task.target.as_ref(),
                    Some(AutomationTarget::StandaloneSimSlot { slot_id: target }) if target.trim() == slot_id
                ) {
                    task.target = Some(AutomationTarget::ModemLine {
                        line_id: line_id.to_string(),
                    });
                    changed = true;
                }
            }
            for rule in &mut config.notifications.rules {
                let mut rewritten = Vec::with_capacity(rule.sim_channel_ids.len());
                for target in &rule.sim_channel_ids {
                    let target = if target.trim() == legacy_notification_id {
                        line_id
                    } else {
                        target.as_str()
                    };
                    if !rewritten.iter().any(|existing| existing == target) {
                        rewritten.push(target.to_string());
                    }
                }
                if rewritten != rule.sim_channel_ids {
                    rule.sim_channel_ids = rewritten;
                    changed = true;
                }
            }
            changed
        };
        if migrated {
            self.save()?;
        }
        Ok(migrated)
    }

    pub fn get_line_profile(&self, line_id: &str) -> LineProfileConfig {
        let config = self.config.read().unwrap();
        let mut profile = config
            .line_profiles
            .iter()
            .find(|profile| profile.line_id == line_id)
            .cloned()
            .unwrap_or_else(|| LineProfileConfig::for_line(line_id));
        profile.sync_ims_video_access_gates();
        profile
    }

    pub fn set_line_volte_connection_enabled(
        &self,
        line_id: &str,
        enabled: bool,
    ) -> Result<LineProfileConfig, String> {
        if !valid_line_id(line_id) {
            return Err("invalid_line_id".to_string());
        }
        let next = {
            let mut config = self.config.write().unwrap();
            let profile = if let Some(profile) = config
                .line_profiles
                .iter_mut()
                .find(|profile| profile.line_id == line_id)
            {
                profile
            } else {
                config
                    .line_profiles
                    .push(LineProfileConfig::for_line(line_id));
                config.line_profiles.last_mut().expect("profile inserted")
            };
            if enabled && !profile.enabled {
                return Err("line_disabled".to_string());
            }
            profile.volte_connection_enabled = enabled;
            profile.sync_ims_video_access_gates();
            let next = profile.clone();
            config
                .line_profiles
                .sort_by(|left, right| left.line_id.cmp(&right.line_id));
            next
        };
        self.save()?;
        Ok(next)
    }

    /// Set this line's eSIM management override. `None` returns the line to the
    /// automatic policy (managed only when the SIM reports a eUICC chip);
    /// `Some(true)` force-enables the lpac eSIM controls even when auto-detection
    /// is uncertain, and `Some(false)` treats the line as a plain SIM regardless.
    pub fn set_line_esim_control(
        &self,
        line_id: &str,
        control: Option<bool>,
    ) -> Result<LineProfileConfig, String> {
        if !valid_line_id(line_id) {
            return Err("invalid_line_id".to_string());
        }
        let next = {
            let mut config = self.config.write().unwrap();
            let profile = if let Some(profile) = config
                .line_profiles
                .iter_mut()
                .find(|profile| profile.line_id == line_id)
            {
                profile
            } else {
                config
                    .line_profiles
                    .push(LineProfileConfig::for_line(line_id));
                config.line_profiles.last_mut().expect("profile inserted")
            };
            profile.esim_control = control;
            let next = profile.clone();
            config
                .line_profiles
                .sort_by(|left, right| left.line_id.cmp(&right.line_id));
            next
        };
        self.save()?;
        Ok(next)
    }

    pub fn get_line_esim_reader_config(&self, line_id: &str) -> EsimReaderConfig {
        self.get_line_profile(line_id).esim_reader
    }

    pub fn set_line_esim_reader_config(
        &self,
        line_id: &str,
        mut reader: EsimReaderConfig,
    ) -> Result<EsimReaderConfig, String> {
        validate_esim_reader_config(&mut reader)?;
        let persisted = reader.clone();
        self.update_line_profile(line_id, |profile| {
            profile.esim_reader = persisted;
        })?;
        Ok(reader)
    }

    /// Set this line's explicit ordered VoLTE IMS address-family list.
    pub fn set_line_volte_ip_families(
        &self,
        line_id: &str,
        families: Vec<VolteIpFamily>,
    ) -> Result<LineProfileConfig, String> {
        if !valid_line_id(line_id) {
            return Err("invalid_line_id".to_string());
        }
        if families.is_empty() {
            return Err("volte_ip_families_empty".to_string());
        }
        let mut seen = Vec::new();
        for family in &families {
            if seen.contains(family) {
                return Err("volte_ip_families_duplicate".to_string());
            }
            seen.push(*family);
        }
        let next = {
            let mut config = self.config.write().unwrap();
            let profile = if let Some(profile) = config
                .line_profiles
                .iter_mut()
                .find(|profile| profile.line_id == line_id)
            {
                profile
            } else {
                config
                    .line_profiles
                    .push(LineProfileConfig::for_line(line_id));
                config.line_profiles.last_mut().expect("profile inserted")
            };
            profile.volte_ip_families = families;
            profile.volte_ip_families_auto = false;
            let next = profile.clone();
            config
                .line_profiles
                .sort_by(|left, right| left.line_id.cmp(&right.line_id));
            next
        };
        self.save()?;
        Ok(next)
    }

    pub fn get_line_volte_ip_families(&self, line_id: &str) -> Vec<VolteIpFamily> {
        self.get_line_profile(line_id).volte_ip_families
    }

    pub fn get_line_volte_ip_families_auto(&self, line_id: &str) -> bool {
        self.get_line_profile(line_id).volte_ip_families_auto
    }

    pub fn set_line_vowifi_config(
        &self,
        line_id: &str,
        mut vowifi: LineVowifiConfig,
    ) -> Result<LineProfileConfig, String> {
        if !valid_line_id(line_id) {
            return Err("invalid_line_id".to_string());
        }
        validate_line_vowifi_config(&mut vowifi)?;
        let next = {
            let mut config = self.config.write().unwrap();
            let profile = if let Some(profile) = config
                .line_profiles
                .iter_mut()
                .find(|profile| profile.line_id == line_id)
            {
                profile
            } else {
                config
                    .line_profiles
                    .push(LineProfileConfig::for_line(line_id));
                config.line_profiles.last_mut().expect("profile inserted")
            };
            if vowifi.enabled && !profile.enabled {
                return Err("line_disabled".to_string());
            }
            profile.vowifi = vowifi;
            profile.sync_ims_video_access_gates();
            let next = profile.clone();
            config
                .line_profiles
                .sort_by(|left, right| left.line_id.cmp(&right.line_id));
            next
        };
        self.save()?;
        Ok(next)
    }

    pub fn set_line_vowifi_connection_enabled(
        &self,
        line_id: &str,
        enabled: bool,
    ) -> Result<LineProfileConfig, String> {
        let current = self.get_line_profile(line_id).vowifi;
        self.set_line_vowifi_config(line_id, LineVowifiConfig { enabled, ..current })
    }

    /// Mutate one line's profile, creating it if this is the first setting the
    /// line has ever had, then persist. Keeps the per-line setters from each
    /// repeating the find-or-insert / sort / save dance.
    fn update_line_profile<F>(&self, line_id: &str, mutate: F) -> Result<LineProfileConfig, String>
    where
        F: FnOnce(&mut LineProfileConfig),
    {
        if !valid_line_id(line_id) {
            return Err("invalid_line_id".to_string());
        }
        let next = {
            let mut config = self.config.write().unwrap();
            let profile = if let Some(profile) = config
                .line_profiles
                .iter_mut()
                .find(|profile| profile.line_id == line_id)
            {
                profile
            } else {
                config
                    .line_profiles
                    .push(LineProfileConfig::for_line(line_id));
                config.line_profiles.last_mut().expect("profile inserted")
            };
            mutate(profile);
            let next = profile.clone();
            config
                .line_profiles
                .sort_by(|a, b| a.line_id.cmp(&b.line_id));
            next
        };
        self.save()?;
        Ok(next)
    }

    pub fn set_line_data_connection_enabled(
        &self,
        line_id: &str,
        enabled: bool,
    ) -> Result<LineProfileConfig, String> {
        if !valid_line_id(line_id) {
            return Err("invalid_line_id".to_string());
        }
        let next = {
            let mut config = self.config.write().unwrap();
            let profile = if let Some(profile) = config
                .line_profiles
                .iter_mut()
                .find(|profile| profile.line_id == line_id)
            {
                profile
            } else {
                config
                    .line_profiles
                    .push(LineProfileConfig::for_line(line_id));
                config.line_profiles.last_mut().expect("profile inserted")
            };
            if enabled && !profile.enabled {
                return Err("line_disabled".to_string());
            }
            if enabled && profile.airplane_mode_enabled {
                return Err("line_airplane_mode_enabled".to_string());
            }
            profile.data_connection_enabled = enabled;
            let next = profile.clone();
            config
                .line_profiles
                .sort_by(|a, b| a.line_id.cmp(&b.line_id));
            next
        };
        self.save()?;
        Ok(next)
    }

    pub fn set_line_data_proxy_config(
        &self,
        line_id: &str,
        mut data_proxy: LineDataProxyConfig,
    ) -> Result<LineProfileConfig, String> {
        if !valid_line_id(line_id) {
            return Err("invalid_line_id".to_string());
        }
        validate_line_data_proxy_config(&mut data_proxy)?;
        let next = {
            let mut config = self.config.write().unwrap();
            let profile = if let Some(profile) = config
                .line_profiles
                .iter_mut()
                .find(|profile| profile.line_id == line_id)
            {
                profile
            } else {
                config
                    .line_profiles
                    .push(LineProfileConfig::for_line(line_id));
                config.line_profiles.last_mut().expect("profile inserted")
            };
            profile.data_proxy = data_proxy;
            let next = profile.clone();
            config
                .line_profiles
                .sort_by(|left, right| left.line_id.cmp(&right.line_id));
            next
        };
        self.save()?;
        Ok(next)
    }

    pub fn set_line_roaming_allowed(
        &self,
        line_id: &str,
        allowed: bool,
    ) -> Result<LineProfileConfig, String> {
        if !valid_line_id(line_id) {
            return Err("invalid_line_id".to_string());
        }
        let next = {
            let mut config = self.config.write().unwrap();
            let profile = if let Some(profile) = config
                .line_profiles
                .iter_mut()
                .find(|profile| profile.line_id == line_id)
            {
                profile
            } else {
                config
                    .line_profiles
                    .push(LineProfileConfig::for_line(line_id));
                config.line_profiles.last_mut().expect("profile inserted")
            };
            profile.roaming_allowed = allowed;
            let next = profile.clone();
            config
                .line_profiles
                .sort_by(|left, right| left.line_id.cmp(&right.line_id));
            next
        };
        self.save()?;
        Ok(next)
    }

    pub fn set_line_airplane_mode(
        &self,
        line_id: &str,
        enabled: bool,
    ) -> Result<LineProfileConfig, String> {
        if !valid_line_id(line_id) {
            return Err("invalid_line_id".to_string());
        }
        let next = {
            let mut config = self.config.write().unwrap();
            let profile = if let Some(profile) = config
                .line_profiles
                .iter_mut()
                .find(|profile| profile.line_id == line_id)
            {
                profile
            } else {
                config
                    .line_profiles
                    .push(LineProfileConfig::for_line(line_id));
                config.line_profiles.last_mut().expect("profile inserted")
            };
            profile.airplane_mode_enabled = enabled;
            if enabled {
                profile.data_connection_enabled = false;
                profile.volte_connection_enabled = false;
            }
            profile.sync_ims_video_access_gates();
            let next = profile.clone();
            config
                .line_profiles
                .sort_by(|a, b| a.line_id.cmp(&b.line_id));
            next
        };
        self.save()?;
        Ok(next)
    }

    pub fn get_standalone_sim_slots(&self) -> Vec<StandaloneSimSlotConfig> {
        self.config.read().unwrap().standalone_sim_slots.clone()
    }

    pub fn set_standalone_sim_slots(
        &self,
        mut slots: Vec<StandaloneSimSlotConfig>,
    ) -> Result<Vec<StandaloneSimSlotConfig>, String> {
        if slots.len() > 64 {
            return Err("standalone_sim_slot_limit_exceeded".to_string());
        }
        let mut ids = std::collections::HashSet::new();
        for slot in &mut slots {
            slot.id = slot.id.trim().to_string();
            slot.label = slot.label.trim().to_string();
            slot.reader_path = slot.reader_path.trim().to_string();
            if slot.id.is_empty()
                || !slot
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                || !ids.insert(slot.id.clone())
            {
                return Err("standalone_sim_slot_id_invalid".to_string());
            }
            if slot.label.is_empty() || slot.reader_path.is_empty() || slot.uim_slot == 0 {
                return Err("standalone_sim_slot_invalid".to_string());
            }
        }
        slots.sort_by(|left, right| left.label.cmp(&right.label).then(left.id.cmp(&right.id)));
        {
            self.config.write().unwrap().standalone_sim_slots = slots.clone();
        }
        self.save()?;
        Ok(slots)
    }

    /// Replace one line's trunk settings (stage D3b). Gating mirrors the VoLTE
    /// line toggle: enabling requires the line itself to be enabled, a non-empty
    /// Asterisk host, and — in `OutboundRegister` mode — a username. An empty
    /// incoming `secret` means "keep the stored secret" so the UI can round-trip
    /// redacted responses without wiping credentials.
    pub fn set_line_trunk_profile(
        &self,
        line_id: &str,
        trunk: TrunkProfileConfig,
    ) -> Result<LineProfileConfig, String> {
        self.set_line_trunk_profile_scoped(line_id, trunk, None)
    }

    /// Replace one line's trunk settings while limiting local-port collision
    /// checks to lines that currently exist in the runtime registry. Persisted
    /// profiles for removed hardware remain available for later reuse, but they
    /// must not block a live line from enabling its trunk.
    pub fn set_line_trunk_profile_for_active_lines(
        &self,
        line_id: &str,
        trunk: TrunkProfileConfig,
        active_line_ids: &std::collections::HashSet<String>,
    ) -> Result<LineProfileConfig, String> {
        self.set_line_trunk_profile_scoped(line_id, trunk, Some(active_line_ids))
    }

    fn set_line_trunk_profile_scoped(
        &self,
        line_id: &str,
        trunk: TrunkProfileConfig,
        active_line_ids: Option<&std::collections::HashSet<String>>,
    ) -> Result<LineProfileConfig, String> {
        if !valid_line_id(line_id) {
            return Err("invalid_line_id".to_string());
        }
        let next = {
            let mut config = self.config.write().unwrap();
            let profile_index = if let Some(index) = config
                .line_profiles
                .iter()
                .position(|profile| profile.line_id == line_id)
            {
                index
            } else {
                config
                    .line_profiles
                    .push(LineProfileConfig::for_line(line_id));
                config.line_profiles.len() - 1
            };
            let mut incoming = trunk;
            if incoming.secret.is_empty() {
                incoming.secret = config.line_profiles[profile_index].trunk.secret.clone();
            }
            incoming.incoming_binding = incoming.incoming_binding.trim().to_string();
            incoming.outgoing_binding = incoming.outgoing_binding.trim().to_string();
            if !valid_trunk_binding(&incoming.incoming_binding) {
                return Err("trunk_incoming_binding_invalid".to_string());
            }
            if !valid_trunk_binding(&incoming.outgoing_binding) {
                return Err("trunk_outgoing_binding_invalid".to_string());
            }
            if incoming.enabled {
                if !config.line_profiles[profile_index].enabled {
                    return Err("line_disabled".to_string());
                }
                if incoming.asterisk_host.trim().is_empty() {
                    return Err("trunk_asterisk_host_required".to_string());
                }
                if incoming.registration_mode == TrunkRegistrationMode::OutboundRegister
                    && incoming.username.trim().is_empty()
                {
                    return Err("trunk_username_required".to_string());
                }
                if incoming.registration_mode == TrunkRegistrationMode::OutboundRegister
                    && !(60..=86_400).contains(&incoming.register_expiry_secs)
                {
                    return Err("trunk_register_expiry_invalid".to_string());
                }
                if incoming.local_port == 0 {
                    return Err("trunk_local_port_required".to_string());
                }
                if config.line_profiles.iter().any(|profile| {
                    profile.line_id != line_id
                        && active_line_ids
                            .is_none_or(|line_ids| line_ids.contains(&profile.line_id))
                        && profile.trunk.enabled
                        && profile.trunk.local_port == incoming.local_port
                }) {
                    return Err("trunk_local_port_in_use".to_string());
                }
            }
            let profile = &mut config.line_profiles[profile_index];
            profile.trunk = incoming;
            let next = profile.clone();
            config
                .line_profiles
                .sort_by(|left, right| left.line_id.cmp(&right.line_id));
            next
        };
        self.save()?;
        Ok(next)
    }

    /// Toggle one line's trunk without resubmitting the full settings. Enabling
    /// revalidates the stored profile so a half-configured trunk cannot be
    /// switched on.
    pub fn set_line_trunk_enabled(
        &self,
        line_id: &str,
        enabled: bool,
    ) -> Result<LineProfileConfig, String> {
        let current = self.get_line_profile(line_id).trunk;
        self.set_line_trunk_profile(line_id, TrunkProfileConfig { enabled, ..current })
    }

    pub fn set_line_trunk_enabled_for_active_lines(
        &self,
        line_id: &str,
        enabled: bool,
        active_line_ids: &std::collections::HashSet<String>,
    ) -> Result<LineProfileConfig, String> {
        let current = self.get_line_profile(line_id).trunk;
        self.set_line_trunk_profile_for_active_lines(
            line_id,
            TrunkProfileConfig { enabled, ..current },
            active_line_ids,
        )
    }

    /// Whether the VoLTE voice leg is available for one line.
    ///
    /// MMTEL voice is the purpose of registering IMS at all, so this follows the
    /// line's VoLTE connection rather than a separate switch. A carrier that
    /// does not permit voice answers the REGISTER or the INVITE with a SIP
    /// error, which the runtime reports instead of pre-emptively refusing.
    pub fn get_line_volte_voice_enabled(&self, line_id: &str) -> bool {
        self.get_line_profile(line_id).volte_connection_enabled
    }

    /// SMS path policy for one line.
    pub fn get_line_sms_path_policy(&self, line_id: &str) -> SmsPathPolicy {
        self.get_line_profile(line_id).sms_path.normalized()
    }

    /// Set one line's explicit SMS path policy.
    pub fn set_line_sms_path_policy(
        &self,
        line_id: &str,
        policy: SmsPathPolicy,
    ) -> Result<SmsPathPolicy, String> {
        let normalized = policy.normalized();
        self.update_line_profile(line_id, |profile| {
            profile.sms_path = normalized.clone();
        })?;
        Ok(self.get_line_sms_path_policy(line_id))
    }

    /// APN for one line.
    pub fn get_line_apn_config(&self, line_id: &str) -> ApnConfig {
        self.get_line_profile(line_id).apn
    }

    /// Set one line's explicit APN configuration.
    pub fn set_line_apn_config(&self, line_id: &str, apn: ApnConfig) -> Result<ApnConfig, String> {
        self.update_line_profile(line_id, |profile| {
            profile.apn = apn.clone();
        })?;
        Ok(self.get_line_apn_config(line_id))
    }

    /// Voice path policy for one line.
    pub fn get_line_voice_path_policy(&self, line_id: &str) -> VoicePathPolicy {
        self.get_line_profile(line_id).voice_path.normalized()
    }

    /// Which IMS access legs may hold a registration for this line.
    pub fn get_line_ims_access_preference(&self, line_id: &str) -> ImsAccessPreference {
        self.get_line_profile(line_id).ims_access_preference
    }

    /// Set one line's IMS access (registration) preference.
    ///
    /// Deliberately independent of `volte_connection_enabled` and
    /// `vowifi.enabled`: this says which *enabled* legs may register, and must
    /// never edit the enable intent itself. Coupling the two is the bug
    /// `enabling_one_ims_access_never_disables_the_other` pins shut — the user's
    /// switch has to survive a preference change so flipping it back is enough
    /// to restore the leg.
    pub fn set_line_ims_access_preference(
        &self,
        line_id: &str,
        preference: ImsAccessPreference,
    ) -> Result<ImsAccessPreference, String> {
        self.update_line_profile(line_id, |profile| {
            profile.ims_access_preference = preference;
        })?;
        Ok(self.get_line_ims_access_preference(line_id))
    }

    /// Set one line's explicit voice path policy.
    pub fn set_line_voice_path_policy(
        &self,
        line_id: &str,
        policy: VoicePathPolicy,
    ) -> Result<VoicePathPolicy, String> {
        if policy
            .priority
            .iter()
            .any(|layer| layer.kind == AccessPathKind::Cs)
        {
            return Err("voice_cs_trunk_backend_unavailable".to_string());
        }
        let normalized = policy.normalized();
        if !normalized.gateway_mode {
            return Err("voice_gateway_mode_required_on_this_device".to_string());
        }
        self.update_line_profile(line_id, |profile| {
            profile.voice_path = normalized.clone();
        })?;
        Ok(self.get_line_voice_path_policy(line_id))
    }

    pub fn get_line_ims_video_config(&self, line_id: &str) -> ImsVideoConfig {
        self.get_line_profile(line_id).ims_video
    }

    /// Whether the VoWiFi voice leg is enabled for one line. VoWiFi voice rides
    /// the line's VoWiFi connection, so this is `vowifi.enabled`.
    pub fn get_line_vowifi_voice_enabled(&self, line_id: &str) -> bool {
        self.get_line_profile(line_id).vowifi.enabled
    }

    /// Replace one line's IMS video media parameters. Access enablement is
    /// derived from the corresponding VoLTE/VoWiFi voice configuration, so API
    /// clients cannot leave a hidden video gate out of sync.
    pub fn set_line_ims_video_config(
        &self,
        line_id: &str,
        ims_video: ImsVideoConfig,
    ) -> Result<ImsVideoConfig, String> {
        if !ims_video.codec.trim().eq_ignore_ascii_case("h264") {
            return Err("vilte_codec_unsupported".to_string());
        }
        if !(96..=127).contains(&ims_video.video_payload_type) {
            return Err("vilte_payload_type_invalid".to_string());
        }
        let profile = self.update_line_profile(line_id, |profile| {
            let persisted = ims_video;
            profile.ims_video = persisted;
            profile.sync_ims_video_access_gates();
        })?;
        Ok(profile.ims_video)
    }

    pub fn set_esim_config(&self, mut esim: EsimConfig) -> Result<(), String> {
        // Reader routing is line-owned. Accept old request documents for API
        // compatibility, but never persist their global port selection again.
        esim.apdu_backend = default_esim_apdu_backend();
        esim.http_backend = default_esim_http_backend();
        esim.at_device.clear();
        esim.qmi_device.clear();
        esim.qmi_uim_slot = 0;
        {
            let mut c = self.config.write().unwrap();
            c.esim = esim;
        }
        self.save()
    }

    pub fn get_device_network(&self) -> DeviceNetworkConfig {
        self.config.read().unwrap().device_network.clone()
    }

    pub fn get_ddns_config(&self) -> DdnsConfig {
        self.config.read().unwrap().device_network.ddns.clone()
    }

    pub fn get_version_update_notifications(&self) -> VersionUpdateNotificationConfig {
        self.config
            .read()
            .unwrap()
            .version_update_notifications
            .clone()
    }

    pub fn get_github_download_proxy(&self) -> GithubDownloadProxyConfig {
        self.config.read().unwrap().github_download_proxy.clone()
    }

    pub fn set_github_download_proxy(
        &self,
        mut proxy: GithubDownloadProxyConfig,
    ) -> Result<(), String> {
        proxy.proxy_prefix =
            crate::services::system::ota::normalize_proxy_prefix(Some(proxy.proxy_prefix));
        if proxy.enabled && proxy.proxy_prefix.is_empty() {
            return Err("启用 GitHub 下载加速时必须填写加速节点".to_string());
        }
        {
            let mut config = self.config.write().unwrap();
            config.github_download_proxy = proxy;
        }
        self.save()
    }

    pub fn get_diagnostic_log(&self) -> DiagnosticLogConfig {
        self.config.read().unwrap().diagnostic_log.clone()
    }

    pub fn set_diagnostic_log(&self, diagnostic_log: DiagnosticLogConfig) -> Result<(), String> {
        diagnostic_log.validate()?;
        {
            let mut c = self.config.write().unwrap();
            c.diagnostic_log = diagnostic_log;
        }
        self.save()
    }

    pub fn get_security(&self) -> SecurityConfig {
        self.config.read().unwrap().security.clone()
    }

    pub fn set_security(&self, security: SecurityConfig) -> Result<(), String> {
        {
            let mut c = self.config.write().unwrap();
            c.security = security;
        }
        self.save()
    }

    /// Return the current per-UE isolation configuration.
    pub fn get_ue_isolation(&self) -> UeIsolationConfig {
        self.config.read().unwrap().ue_isolation.clone()
    }

    /// Replace the per-UE isolation configuration and persist it.
    pub fn set_ue_isolation(&self, ue_isolation: UeIsolationConfig) -> Result<(), String> {
        {
            let mut c = self.config.write().unwrap();
            c.ue_isolation = ue_isolation;
        }
        self.save()
    }

    pub fn set_ddns_config(&self, ddns: DdnsConfig) -> Result<(), String> {
        {
            let mut c = self.config.write().unwrap();
            c.device_network.ddns = ddns;
        }
        self.save()
    }

    pub fn set_last_notified_update_version(&self, version: String) -> Result<(), String> {
        {
            let mut c = self.config.write().unwrap();
            c.version_update_notifications.last_notified_version = Some(version);
        }
        self.save()
    }

    /// 更新通知配置
    pub fn set_notifications(&self, notifications: NotificationConfig) -> Result<(), String> {
        {
            let mut config = self.config.write().unwrap();
            config.notifications = notifications;
            strip_legacy_notification_channel_fields(&mut config.notifications);
        }
        self.save()
    }

    /// Persist the complete typed configuration through the selected backend.
    /// Production always uses one SQLite transaction; JSON is test-only.
    pub fn save(&self) -> Result<(), String> {
        let _save_guard = self.save_lock.lock().unwrap();
        let content = {
            let config = self.config.read().unwrap();
            serde_json::to_string_pretty(&*config)
                .map_err(|e| format!("Failed to serialize config: {}", e))?
        };
        match &self.storage {
            ConfigStorage::Json(path) => save_json_document(path, &content),
            ConfigStorage::Sqlite(path) => save_config_document(path, &content),
        }
    }
}

/// 获取默认配置文件路径
pub fn get_default_config_path() -> PathBuf {
    if let Some(path) = std::env::var_os("SIMADMIN_CONFIG_DB") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }

    // Production default. An empty database receives the current default
    // document; no legacy file is consulted.
    let device_path = PathBuf::from("/data/config.sqlite3");
    if device_path.parent().map(|p| p.exists()).unwrap_or(false) {
        return device_path;
    }

    // 回退到当前目录
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("config.sqlite3")
}
