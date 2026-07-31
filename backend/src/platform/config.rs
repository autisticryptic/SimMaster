//! 配置管理模块
//!
//! 使用 JSON 文件存储用户配置，支持热更新

use serde::{de::Error as DeError, Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use tracing::{info, warn};

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
    forward_ddns: bool,
    forward_updates: bool,
    sms_template: String,
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

    pub fn first_webhook_config(&self) -> Option<WebhookConfig> {
        self.channels
            .iter()
            .find(|channel| channel.channel_type == NotificationChannel::Webhook)
            .and_then(|channel| serde_json::from_value(channel.config.clone()).ok())
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
    serde_json::to_value(config).unwrap_or(Value::Object(Default::default()))
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
            forward_ddns: legacy.webhook.forward_ddns,
            forward_updates: legacy.webhook.forward_updates,
            sms_template: webhook_text_template(
                &legacy.webhook.sms_template,
                &default_rule_template(NotificationEventType::Sms),
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
        forward_ddns: common.forward_ddns,
        forward_updates: common.forward_updates,
        sms_template: non_empty_template(&common.sms_template, NotificationEventType::Sms),
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
            "🤖 自动化事件通知\n任务名称: {{任务名称}}\n任务类型: {{任务类型}}\n执行状态: {{任务状态}}\n详情: {{任务详情}}\n时间: {{触发时间}}\n来源: {{本机号码}}".to_string()
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
    /// SIM-dependent actions may pin execution to a persistent modem/SIM line
    /// or an external reader reservation. Legacy tasks without a target use
    /// the primary modem line for compatibility.
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
        }
        manager.save().unwrap();

        assert!(manager
            .migrate_line_profile_aliases(current_line_id, &[legacy_line_id.to_string()])
            .unwrap());
        let migrated = manager.get_line_profile(current_line_id);
        assert!(migrated.volte_connection_enabled);
        assert_eq!(migrated.trunk.context, "from-migrated-slot");
        assert!(!manager
            .migrate_line_profile_aliases(current_line_id, &[legacy_line_id.to_string()])
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
        let line_id = "line-0123456789abcdef0123456789abcdef";
        let profile = manager
            .set_line_volte_connection_enabled(line_id, true)
            .unwrap();
        assert!(profile.volte_connection_enabled);
        assert!(!manager.get_volte_config().feature_enabled);

        let reloaded = ConfigManager::new(path.clone());
        assert!(reloaded.get_line_profile(line_id).volte_connection_enabled);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn line_path_policies_and_apn_inherit_globals_until_overridden() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-line-policy-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let manager = ConfigManager::new(path.clone());
        let line_a = "line-0123456789abcdef0123456789abcdef";
        let line_b = "line-fedcba9876543210fedcba9876543210";

        // No override yet: both lines see the global policy and APN.
        assert_eq!(
            manager.get_line_sms_path_policy(line_a).priority,
            manager.get_sms_path_policy().priority
        );
        assert_eq!(
            manager.get_line_apn_config(line_a),
            manager.get_apn_config()
        );

        // Overriding line A must not move line B.
        let vowifi_enabled = |policy: SmsPathPolicy| {
            policy
                .priority
                .iter()
                .any(|layer| layer.kind == AccessPathKind::Vowifi && layer.enabled)
        };
        let mut only_volte = manager.get_sms_path_policy();
        for layer in &mut only_volte.priority {
            if layer.kind == AccessPathKind::Vowifi {
                layer.enabled = false;
            }
        }
        manager
            .set_line_sms_path_policy(line_a, Some(only_volte))
            .unwrap();
        assert!(!vowifi_enabled(manager.get_line_sms_path_policy(line_a)));
        assert!(vowifi_enabled(manager.get_line_sms_path_policy(line_b)));
        assert!(vowifi_enabled(manager.get_sms_path_policy()));

        let mut line_apn = manager.get_apn_config();
        line_apn.apn = "line-a-apn".to_string();
        manager.set_line_apn_config(line_a, Some(line_apn)).unwrap();
        assert_eq!(manager.get_line_apn_config(line_a).apn, "line-a-apn");
        assert_eq!(
            manager.get_line_apn_config(line_b),
            manager.get_apn_config()
        );

        // Overrides survive a reload, and clearing one falls back to the global.
        let reloaded = ConfigManager::new(path.clone());
        assert!(!vowifi_enabled(reloaded.get_line_sms_path_policy(line_a)));
        reloaded.set_line_sms_path_policy(line_a, None).unwrap();
        assert!(vowifi_enabled(reloaded.get_line_sms_path_policy(line_a)));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn per_line_vowifi_overrides_and_standalone_slots_persist() {
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
                    dns_server: "1.1.1.1".to_string(),
                    profile_id: Some("gb_ee_23433".to_string()),
                    ..LineVowifiConfig::default()
                },
            )
            .unwrap();
        assert!(profile.vowifi.enabled);
        assert_eq!(profile.vowifi.profile_id.as_deref(), Some("gb_ee_23433"));
        assert!(manager.get_vowifi_config().feature_enabled);

        assert_eq!(
            manager
                .set_line_vowifi_config(
                    line_id,
                    LineVowifiConfig {
                        dns_server: "not-an-ip".to_string(),
                        ..LineVowifiConfig::default()
                    },
                )
                .unwrap_err(),
            "vowifi_dns_server_invalid"
        );

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
            reloaded.get_line_profile(line_id).vowifi.dns_server,
            "1.1.1.1"
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

    /// Nothing writes `vowifi-profiles.conf` any more, but the parser has to keep
    /// reading every shape that was ever written so the one-time migration into
    /// the carrier profile database does not drop an operator's overrides.
    #[test]
    fn legacy_vowifi_profiles_parser_reads_both_historical_layouts() {
        let profile = ExternalVowifiProfile {
            profile_id: "custom-au".to_string(),
            mcc: "505".to_string(),
            mnc: "01".to_string(),
            epdg_host: "epdg.example.test".to_string(),
            epdg_port: 4500,
            ip_stack: "ipv6".to_string(),
            apn: Some("ims".to_string()),
            dns_server: None,
        };

        // Versioned object form, with the comment header the writer used to emit.
        let versioned = format!(
            "# SimAdmin custom VoWiFi/ePDG profiles\n{}\n",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": 1,
                "profiles": [profile.clone()],
            }))
            .unwrap()
        );
        assert_eq!(
            parse_external_vowifi_profiles(&versioned),
            vec![profile.clone()]
        );

        // Older split-marker form with a bare array.
        let legacy = format!(
            "# --- BUILTIN PROFILES (READ ONLY) ---\nignored\n# --- CUSTOM PROFILES ---\n{}",
            serde_json::to_string(&vec![profile.clone()]).unwrap()
        );
        assert_eq!(parse_external_vowifi_profiles(&legacy), vec![profile]);

        // A missing or unreadable file yields nothing rather than failing.
        assert!(parse_external_vowifi_profiles("").is_empty());
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
        assert_eq!(
            policy.mid_flight_disable,
            MidFlightDisablePolicy::AutoSwitch
        );
        assert_eq!(policy.dedup_retention_days, 30);
        assert_eq!(policy.message_retention_limit, 10_000);
    }

    #[test]
    fn sms_path_policy_enabled_ims_layers_skips_cs_and_disabled() {
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
        assert_eq!(ims, vec![AccessPathKind::Vowifi]);
    }

    #[test]
    fn sms_path_policy_normalized_appends_missing_kinds_once() {
        // Only VoLTE supplied; VoWiFi/CS must be appended (enabled) in canonical order.
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
                AccessPathKind::Volte,
                AccessPathKind::Vowifi,
                AccessPathKind::Cs
            ]
        );
        // First VoLTE occurrence (disabled) is kept.
        assert!(!policy.is_enabled(AccessPathKind::Volte));
        assert!(policy.is_enabled(AccessPathKind::Vowifi));
    }

    #[test]
    fn sms_path_policy_deserializes_from_partial_json() {
        // Old config with no sms_path at all → default.
        let cfg: AppConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.sms_path, SmsPathPolicy::default());

        // Partial sms_path: only priority given, other fields defaulted.
        let json = r#"{"sms_path":{"priority":[{"kind":"cs","enabled":true}]}}"#;
        let cfg: AppConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.sms_path.dedupe_enabled);
        assert_eq!(cfg.sms_path.priority.len(), 1);
        assert_eq!(cfg.sms_path.priority[0].kind, AccessPathKind::Cs);
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
        let config: AppConfig = serde_json::from_str(
            r#"{"sms_path":{"priority":[{"kind":"cs","enabled":true}]},"voice_path":{"priority":[{"kind":"volte","enabled":false}]}}"#,
        )
        .unwrap();

        assert_eq!(config.sms_path.priority[0].kind, AccessPathKind::Cs);
        let voice = config.voice_path.normalized();
        assert_eq!(voice.priority.len(), 3);
        assert_eq!(voice.priority[0].kind, AccessPathKind::Volte);
        assert!(!voice.priority[0].enabled);
        assert!(voice.gateway_mode);
    }

    #[test]
    fn legacy_voice_services_config_is_ignored_and_not_serialized() {
        let config: AppConfig = serde_json::from_str(
            r#"{"voice_services":{"feature_enabled":true,"delegate_to_asterisk":true,"marketing_keywords":["推销"]}}"#,
        )
        .unwrap();

        assert!(config.voice_path.gateway_mode);
        let serialized = serde_json::to_value(config).unwrap();
        assert!(serialized.get("voice_services").is_none());
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
        assert!(!migrated
            .rules
            .iter()
            .any(|rule| rule.event_type == NotificationEventType::Ddns));
        assert!(migrated
            .rules
            .iter()
            .any(|rule| rule.event_type == NotificationEventType::VersionUpdate));
    }

    #[test]
    fn vowifi_config_defaults_to_quiet_mode() {
        let config = AppConfig::default();

        assert!(!config.vowifi.feature_enabled);
        assert!(!config.vowifi.connection_enabled);
        assert_eq!(config.vowifi.auto_restore_initial_delay_secs, 60);
        assert_eq!(config.vowifi.auto_restore_attempts, 3);
        assert_eq!(config.vowifi.auto_restore_retry_delay_secs, 30);
    }

    #[test]
    fn volte_ip_family_preference_round_trips() {
        let defaulted: VolteConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(
            defaulted.ip_family_preference,
            VolteIpFamilyPreference::Ipv4First
        );

        let configured: VolteConfig =
            serde_json::from_str(r#"{"ip_family_preference":"ipv4_first"}"#).unwrap();
        assert_eq!(
            configured.ip_family_preference,
            VolteIpFamilyPreference::Ipv4First
        );
        // The preference is now honored by the connect flow, so it must survive a
        // serialize/deserialize round-trip and stay visible to the config UI.
        assert_eq!(
            serde_json::to_value(configured)
                .unwrap()
                .get("ip_family_preference")
                .and_then(|value| value.as_str()),
            Some("ipv4_first")
        );
    }

    #[test]
    fn vowifi_connection_intent_requires_feature_switch() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-vowifi-config-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let manager = ConfigManager::new(path.clone());

        assert_eq!(
            manager.set_vowifi_connection_enabled(true).unwrap_err(),
            "vowifi_feature_disabled"
        );

        let enabled = manager.set_vowifi_feature_enabled(true).unwrap();
        assert!(enabled.feature_enabled);
        assert!(!enabled.connection_enabled);

        let connected = manager.set_vowifi_connection_enabled(true).unwrap();
        assert!(connected.feature_enabled);
        assert!(connected.connection_enabled);

        let disabled = manager.set_vowifi_feature_enabled(false).unwrap();
        assert!(!disabled.feature_enabled);
        assert!(!disabled.connection_enabled);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn vilte_feature_requires_volte_voice() {
        let path = std::env::temp_dir().join(format!(
            "simadmin_vilte_gate_{}_{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let manager = ConfigManager::new(path.clone());

        // Without VoLTE voice, enabling ViLTE is rejected.
        assert_eq!(
            manager.set_vilte_feature_enabled(true).unwrap_err(),
            "volte_voice_disabled"
        );

        // Turn on VoLTE feature then voice, then ViLTE is allowed.
        manager.set_volte_feature_enabled(true).unwrap();
        manager.set_volte_voice_enabled(true).unwrap();
        let vilte = manager.set_vilte_feature_enabled(true).unwrap();
        assert!(vilte.feature_enabled);
        assert_eq!(vilte.codec, "h264");

        assert_eq!(
            manager
                .set_vilte_config(VilteConfig {
                    codec: "vp8".to_string(),
                    ..VilteConfig::default()
                })
                .unwrap_err(),
            "vilte_codec_unsupported"
        );
        assert_eq!(
            manager
                .set_vilte_config(VilteConfig {
                    video_payload_type: 95,
                    ..VilteConfig::default()
                })
                .unwrap_err(),
            "vilte_payload_type_invalid"
        );

        // set_vilte_config forces feature off when voice is off.
        manager.set_volte_voice_enabled(false).unwrap();
        assert!(!manager.get_vilte_config().feature_enabled);
        let forced = manager
            .set_vilte_config(VilteConfig {
                feature_enabled: true,
                ..VilteConfig::default()
            })
            .unwrap();
        assert!(
            !forced.feature_enabled,
            "ViLTE must be forced off when VoLTE voice is disabled"
        );

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

fn default_data_enabled() -> bool {
    false
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
}

impl Default for EsimConfig {
    fn default() -> Self {
        Self {
            lpac_path: default_lpac_path(),
            custom_memory_total_kb: None,
        }
    }
}

fn default_vowifi_auto_restore_initial_delay_secs() -> u64 {
    60
}

fn default_vowifi_auto_restore_attempts() -> u8 {
    3
}

fn default_vowifi_auto_restore_retry_delay_secs() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VowifiConfig {
    #[serde(default)]
    pub feature_enabled: bool,
    #[serde(default)]
    pub connection_enabled: bool,
    #[serde(default = "default_vowifi_auto_restore_initial_delay_secs")]
    pub auto_restore_initial_delay_secs: u64,
    #[serde(default = "default_vowifi_auto_restore_attempts")]
    pub auto_restore_attempts: u8,
    #[serde(default = "default_vowifi_auto_restore_retry_delay_secs")]
    pub auto_restore_retry_delay_secs: u64,
}

impl Default for VowifiConfig {
    fn default() -> Self {
        Self {
            feature_enabled: false,
            connection_enabled: false,
            auto_restore_initial_delay_secs: default_vowifi_auto_restore_initial_delay_secs(),
            auto_restore_attempts: default_vowifi_auto_restore_attempts(),
            auto_restore_retry_delay_secs: default_vowifi_auto_restore_retry_delay_secs(),
        }
    }
}

fn default_volte_sms_enabled() -> bool {
    true
}

fn default_volte_voice_enabled() -> bool {
    false
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
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ipv6First => "ipv6_first",
            Self::Ipv4First => "ipv4_first",
            Self::Ipv6Only => "ipv6_only",
            Self::Ipv4Only => "ipv4_only",
        }
    }

    /// The equivalent ordered attempt list. Lets the runtime treat the legacy
    /// single-select preference and the newer per-line ordered list uniformly.
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

/// VoLTE (IMS over LTE) SMS configuration.
///
/// `feature_enabled`, `sms_enabled`, and `connection_enabled` are retained for
/// backward-compatible config/API deserialization. Physical modem lines use
/// `LineProfileConfig::volte_connection_enabled` as their sole IMS connection
/// intent; these legacy global fields must not gate a line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VolteConfig {
    #[serde(default)]
    pub feature_enabled: bool,
    #[serde(default = "default_volte_sms_enabled")]
    pub sms_enabled: bool,
    #[serde(default = "default_volte_voice_enabled")]
    pub voice_enabled: bool,
    #[serde(default)]
    pub connection_enabled: bool,
    #[serde(default)]
    pub ip_family_preference: VolteIpFamilyPreference,
    #[serde(default = "default_volte_auto_restore_initial_delay_secs")]
    pub auto_restore_initial_delay_secs: u64,
    #[serde(default = "default_volte_auto_restore_attempts")]
    pub auto_restore_attempts: u8,
    #[serde(default = "default_volte_auto_restore_retry_delay_secs")]
    pub auto_restore_retry_delay_secs: u64,
}

impl Default for VolteConfig {
    fn default() -> Self {
        Self {
            feature_enabled: false,
            sms_enabled: default_volte_sms_enabled(),
            voice_enabled: default_volte_voice_enabled(),
            connection_enabled: false,
            ip_family_preference: VolteIpFamilyPreference::default(),
            auto_restore_initial_delay_secs: default_volte_auto_restore_initial_delay_secs(),
            auto_restore_attempts: default_volte_auto_restore_attempts(),
            auto_restore_retry_delay_secs: default_volte_auto_restore_retry_delay_secs(),
        }
    }
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

fn default_vowifi_epdg_port() -> u16 {
    500
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

/// Per-SIM WiFi Calling intent and network overrides. The proxy endpoint is
/// kept separate from the mode because ordinary HTTP CONNECT cannot carry the
/// UDP 500/4500 traffic used by IKEv2/NAT-T.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LineVowifiConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub proxy_mode: VowifiProxyMode,
    #[serde(default)]
    pub proxy_endpoint: String,
    #[serde(default)]
    pub dns_server: String,
    /// Pin this line to a specific carrier profile by `profile_id`. `None`
    /// (the default) resolves the profile automatically from the SIM's IMSI:
    /// database match first, then the built-in / dynamic 3GPP derivation.
    ///
    /// Only a profile that exists in the carrier-profile database is honored; a
    /// pinned id that no longer resolves there falls back to automatic matching,
    /// so deleting a profile can never strand a line. This replaces the old
    /// per-line `epdg_host`/`epdg_port` overrides — the ePDG now always comes
    /// from the resolved profile, editable on the 运营商 Profile page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalVowifiProfile {
    pub profile_id: String,
    pub mcc: String,
    pub mnc: String,
    pub epdg_host: String,
    #[serde(default = "default_vowifi_epdg_port")]
    pub epdg_port: u16,
    #[serde(default = "default_external_ip_stack")]
    pub ip_stack: String,
    #[serde(default)]
    pub apn: Option<String>,
    #[serde(default)]
    pub dns_server: Option<String>,
}

fn default_external_ip_stack() -> String {
    "ipv6".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
struct ExternalVowifiProfilesFile {
    #[serde(default = "default_external_profiles_schema_version")]
    schema_version: u32,
    #[serde(default)]
    profiles: Vec<ExternalVowifiProfile>,
}

fn default_external_profiles_schema_version() -> u32 {
    1
}

/// Parse the legacy `vowifi-profiles.conf`.
///
/// Kept only so the one-time migration into the profile database can read an
/// existing file; nothing writes this format any more.
pub fn parse_external_vowifi_profiles(content: &str) -> Vec<ExternalVowifiProfile> {
    let legacy_or_current = content
        .split("# --- CUSTOM PROFILES ---")
        .nth(1)
        .unwrap_or(content);
    let json = legacy_or_current
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    serde_json::from_str::<ExternalVowifiProfilesFile>(json.trim())
        .map(|file| file.profiles)
        .or_else(|_| serde_json::from_str::<Vec<ExternalVowifiProfile>>(json.trim()))
        .unwrap_or_default()
}

impl Default for LineVowifiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            proxy_mode: VowifiProxyMode::Direct,
            proxy_endpoint: String::new(),
            dns_server: String::new(),
            profile_id: None,
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

/// Persisted controls for one stable physical-modem + SIM line. Trunk settings
/// extend this same profile; keeping the connection flag here makes multi-line
/// auto-restore independent instead of relying on one global bool.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LineProfileConfig {
    pub line_id: String,
    #[serde(default = "default_line_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub volte_connection_enabled: bool,
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
    /// Per-line SMS path priority. `None` inherits the global policy, so
    /// single-line installs and existing config files keep working unchanged.
    /// A SIM that only has working VoLTE and a SIM that only has working VoWiFi
    /// need different orders, which one global list cannot express.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sms_path: Option<SmsPathPolicy>,
    /// Per-line voice path priority; same inheritance rule as `sms_path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_path: Option<VoicePathPolicy>,
    /// Per-line ordered IMS IP-family attempt order. `None` inherits the global
    /// `VolteConfig.ip_family_preference`. The list elements are the families to
    /// enable, in fallback order: `[Ipv4, Ipv6]` tries dual-stack then IPv4 then
    /// IPv6 (== `Ipv4First`), `[Ipv6]` is IPv6-only, and so on. An empty list is
    /// treated as "follow the default" so a line can never disable both families.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volte_ip_families: Option<Vec<VolteIpFamily>>,
    /// Per-line APN. `None` inherits the global APN, which is only correct while
    /// every SIM is on the same carrier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apn: Option<ApnConfig>,
    /// Per-line eSIM (eUICC) management control. `None` means "auto": eSIM
    /// management is offered only when the line's SIM reports a eUICC chip
    /// (`esim_status`/`sim_type`). `Some(true)` forces eSIM management on even
    /// when detection is inconclusive (e.g. a reader that cannot report
    /// EsimStatus but is known to hold an eUICC), and `Some(false)` forces the
    /// line to be treated as a plain SIM so no lpac calls are ever issued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub esim_control: Option<bool>,
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
    pub fn for_line(line_id: impl Into<String>) -> Self {
        Self {
            line_id: line_id.into(),
            enabled: true,
            volte_connection_enabled: false,
            volte_ip_families: None,
            vowifi: LineVowifiConfig::default(),
            trunk: TrunkProfileConfig::default(),
            data_connection_enabled: false,
            data_proxy: LineDataProxyConfig::default(),
            roaming_allowed: default_roaming_allowed(),
            airplane_mode_enabled: false,
            sms_path: None,
            voice_path: None,
            apn: None,
            esim_control: None,
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
    config.dns_server = config.dns_server.trim().to_string();
    config.profile_id = config
        .profile_id
        .as_ref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    if !config.dns_server.is_empty() && config.dns_server.parse::<std::net::IpAddr>().is_err() {
        return Err("vowifi_dns_server_invalid".to_string());
    }
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

fn valid_trunk_binding(binding: &str) -> bool {
    binding.is_empty()
        || binding.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'_' | b'.' | b'*' | b'#')
        })
}

// ===================== Phase F: ViLTE (video telephony over LTE) =====================

fn default_vilte_codec() -> String {
    "h264".to_string()
}

fn default_vilte_video_payload_type() -> u8 {
    // Dynamic payload type; 99 is a common ViLTE choice for H.264.
    99
}

fn default_vilte_h264_fmtp() -> String {
    // Baseline profile, packetization-mode 1 (non-interleaved). profile-level-id
    // 42e01f = Constrained Baseline, level 3.1 — a widely interoperable ViLTE
    // default. The relay never transcodes, so this is purely what we advertise
    // to the far end on the offer/answer; the negotiated value is carried
    // through verbatim.
    "profile-level-id=42e01f;packetization-mode=1".to_string()
}

/// ViLTE (video telephony over LTE) configuration.
///
/// Video rides the *same* IMS voice session as VoLTE voice (one INVITE, an
/// audio `m=` line plus a video `m=` line), so `feature_enabled` here is gated
/// on the VoLTE voice feature at the `ConfigManager` layer. On the target
/// hardware class (no audio/video capture) the device is a pure media relay: it
/// forwards RTP between the operator IMS leg and an internal SIP UA and never
/// encodes/decodes video. Therefore only pass-through codecs are meaningful —
/// `codec` is what we advertise, not something we transcode to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VilteConfig {
    #[serde(default)]
    pub feature_enabled: bool,
    /// Advertised video codec name (relay is pass-through; H.264 is the ViLTE
    /// baseline mandated by GSMA IR.94).
    #[serde(default = "default_vilte_codec")]
    pub codec: String,
    /// Dynamic RTP payload type to advertise for the video stream.
    #[serde(default = "default_vilte_video_payload_type")]
    pub video_payload_type: u8,
    /// `a=fmtp` parameters advertised for the video codec.
    #[serde(default = "default_vilte_h264_fmtp")]
    pub h264_fmtp: String,
}

impl Default for VilteConfig {
    fn default() -> Self {
        Self {
            feature_enabled: false,
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
/// discussed in the design doc §4.3. Kept as a config-level enum so the priority
/// order can be persisted and reordered by the user.
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

/// SMS multi-path routing policy. The `priority` vector's order *is* the
/// priority (index 0 highest). All fields are `#[serde(default)]` so existing
/// config files upgrade transparently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmsPathPolicy {
    /// Priority-ordered layers. Order = preference; each layer independently
    /// enable-able.
    #[serde(default = "default_sms_path_order")]
    pub priority: Vec<PathLayerConfig>,
    /// Cross-transport dedup on receive.
    #[serde(default = "default_true")]
    pub dedupe_enabled: bool,
    /// Keep the CS listener as a fallback receiver even while an IMS leg is the
    /// active listener (with dedup enforced) instead of pausing it entirely.
    #[serde(default)]
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
            dedupe_enabled: true,
            cs_fallback_receiver: false,
            mid_flight_disable: MidFlightDisablePolicy::AutoSwitch,
            dedup_retention_days: default_sms_dedup_retention_days(),
            message_retention_limit: default_sms_message_retention_limit(),
        }
    }
}

impl SmsPathPolicy {
    /// Enabled layers in priority order.
    pub fn enabled_layers(&self) -> impl Iterator<Item = AccessPathKind> + '_ {
        self.priority
            .iter()
            .filter(|layer| layer.enabled)
            .map(|layer| layer.kind)
    }

    /// Enabled IMS layers in priority order (for listener election).
    pub fn enabled_ims_layers(&self) -> impl Iterator<Item = AccessPathKind> + '_ {
        self.enabled_layers().filter(|kind| kind.is_ims())
    }

    /// Whether a given path kind is enabled in the policy.
    pub fn is_enabled(&self, kind: AccessPathKind) -> bool {
        self.priority
            .iter()
            .any(|layer| layer.kind == kind && layer.enabled)
    }

    /// Normalize the priority list so every path kind appears exactly once.
    /// Missing kinds are appended (enabled) in the canonical VoWiFi/VoLTE/CS
    /// order; duplicates keep their first occurrence. This keeps a
    /// user-supplied partial list valid.
    pub fn normalized(mut self) -> Self {
        let mut seen: Vec<AccessPathKind> = Vec::new();
        let mut deduped: Vec<PathLayerConfig> = Vec::new();
        for layer in self.priority.into_iter() {
            if !seen.contains(&layer.kind) {
                seen.push(layer.kind);
                deduped.push(layer);
            }
        }
        for kind in [
            AccessPathKind::Vowifi,
            AccessPathKind::Volte,
            AccessPathKind::Cs,
        ] {
            if !seen.contains(&kind) {
                deduped.push(PathLayerConfig {
                    kind,
                    enabled: true,
                });
            }
        }
        self.priority = deduped;
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
        PathLayerConfig {
            kind: AccessPathKind::Cs,
            enabled: true,
        },
    ]
}

/// Voice path selection is deliberately independent from the SMS policy.
/// `gateway_mode` remains true on the Qualcomm 410 because the host has no
/// microphone/speaker and must hand media to a future internal UA or WebRTC
/// adapter.
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
            if !seen.contains(&layer.kind) {
                seen.push(layer.kind);
                deduped.push(layer);
            }
        }
        for kind in [
            AccessPathKind::Vowifi,
            AccessPathKind::Volte,
            AccessPathKind::Cs,
        ] {
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
pub struct AppConfig {
    #[serde(default)]
    pub webhook: WebhookConfig,
    #[serde(default)]
    pub notifications: NotificationConfig,
    #[serde(default)]
    pub device_network: DeviceNetworkConfig,
    #[serde(default)]
    pub version_update_notifications: VersionUpdateNotificationConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    /// 是否允许蜂窝数据漫游（写入 ModemManager Simple.Connect 的 allow-roaming）
    #[serde(default = "default_roaming_allowed")]
    pub roaming_allowed: bool,
    #[serde(default = "default_data_enabled")]
    pub data_enabled: bool,
    #[serde(default)]
    pub apn: ApnConfig,
    #[serde(default)]
    pub esim: EsimConfig,
    #[serde(default)]
    pub automation: AutomationConfig,
    #[serde(default)]
    pub vowifi: VowifiConfig,
    #[serde(default)]
    pub volte: VolteConfig,
    #[serde(default)]
    pub line_profiles: Vec<LineProfileConfig>,
    #[serde(default)]
    pub modem_slots: Vec<ModemSlotConfig>,
    #[serde(default)]
    pub standalone_sim_slots: Vec<StandaloneSimSlotConfig>,
    #[serde(default)]
    pub vilte: VilteConfig,
    #[serde(default)]
    pub sms_path: SmsPathPolicy,
    #[serde(default)]
    pub voice_path: VoicePathPolicy,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            webhook: WebhookConfig::default(),
            notifications: NotificationConfig::default(),
            device_network: DeviceNetworkConfig::default(),
            version_update_notifications: VersionUpdateNotificationConfig::default(),
            security: SecurityConfig::default(),
            roaming_allowed: default_roaming_allowed(),
            data_enabled: default_data_enabled(),
            apn: ApnConfig::default(),
            esim: EsimConfig::default(),
            automation: AutomationConfig::default(),
            vowifi: VowifiConfig::default(),
            volte: VolteConfig::default(),
            line_profiles: Vec::new(),
            modem_slots: Vec::new(),
            standalone_sim_slots: Vec::new(),
            vilte: VilteConfig::default(),
            sms_path: SmsPathPolicy::default(),
            voice_path: VoicePathPolicy::default(),
        }
    }
}

fn migrate_legacy_webhook_config(config: &mut AppConfig) {
    if config.notifications.channels.is_empty()
        && config.notifications.rules.is_empty()
        && config.webhook != WebhookConfig::default()
    {
        let legacy = LegacyNotificationConfig {
            webhook: config.webhook.clone(),
            ..Default::default()
        };
        config.notifications = NotificationConfig::from_legacy(legacy);
    }
    config.webhook = config
        .notifications
        .first_webhook_config()
        .unwrap_or_default();
}

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

    // 1. Webhook template
    if migrate_template_string(&mut config.webhook.update_template) {
        changed = true;
    }

    // 2. Notification rules templates
    for rule in &mut config.notifications.rules {
        if rule.event_type == NotificationEventType::VersionUpdate
            && migrate_template_string(&mut rule.template)
        {
            changed = true;
        }
    }

    // 3. Notification channels templates
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
    config_path: PathBuf,
    save_lock: Mutex<()>,
}

impl ConfigManager {
    /// 创建新的配置管理器
    pub fn new(config_path: PathBuf) -> Self {
        let mut config = if config_path.exists() {
            match fs::read_to_string(&config_path) {
                Ok(content) => match serde_json::from_str::<AppConfig>(&content) {
                    Ok(cfg) => cfg,
                    Err(e) => {
                        warn!(error = %e, "Failed to parse config file, using defaults");
                        AppConfig::default()
                    }
                },
                Err(e) => {
                    warn!(error = %e, "Failed to read config file, using defaults");
                    AppConfig::default()
                }
            }
        } else {
            info!("No config file found, using defaults");
            AppConfig::default()
        };

        migrate_legacy_webhook_config(&mut config);
        let changed = migrate_templates_to_remove_md5(&mut config);

        let manager = Self {
            config: Arc::new(RwLock::new(config)),
            config_path,
            save_lock: Mutex::new(()),
        };

        // 保存配置（如果文件不存在，或者配置模板发生了自动清理）
        if !manager.config_path.exists() || changed {
            let _ = manager.save();
        }
        // `vowifi-profiles.conf` is no longer created or rewritten here. Custom
        // carrier profiles live in the database; an existing file is migrated
        // once at startup and then archived.

        manager
    }

    /// 获取通知配置
    pub fn get_notifications(&self) -> NotificationConfig {
        self.config.read().unwrap().notifications.clone()
    }

    /// 获取自动化配置
    pub fn get_automation_config(&self) -> AutomationConfig {
        self.config.read().unwrap().automation.clone()
    }

    /// Path of the retired `vowifi-profiles.conf`. Custom carrier profiles now
    /// live in the `vowifi_carrier_profiles` database table; this only exists so
    /// the one-time migration knows where to look.
    pub fn legacy_vowifi_profiles_path(&self) -> PathBuf {
        self.config_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("vowifi-profiles.conf")
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

    pub fn get_roaming_allowed(&self) -> bool {
        self.config.read().unwrap().roaming_allowed
    }

    pub fn get_data_enabled(&self) -> bool {
        self.config.read().unwrap().data_enabled
    }

    pub fn get_apn_config(&self) -> ApnConfig {
        self.config.read().unwrap().apn.clone()
    }

    pub fn get_esim_config(&self) -> EsimConfig {
        self.config.read().unwrap().esim.clone()
    }

    pub fn get_vowifi_config(&self) -> VowifiConfig {
        self.config.read().unwrap().vowifi.clone()
    }

    pub fn set_vowifi_feature_enabled(&self, enabled: bool) -> Result<VowifiConfig, String> {
        let next = {
            let mut c = self.config.write().unwrap();
            c.vowifi.feature_enabled = enabled;
            if !enabled {
                c.vowifi.connection_enabled = false;
            }
            c.vowifi.clone()
        };
        self.save()?;
        Ok(next)
    }

    pub fn set_vowifi_connection_enabled(&self, enabled: bool) -> Result<VowifiConfig, String> {
        let next = {
            let mut c = self.config.write().unwrap();
            if enabled && !c.vowifi.feature_enabled {
                return Err("vowifi_feature_disabled".to_string());
            }
            c.vowifi.connection_enabled = enabled;
            c.vowifi.clone()
        };
        self.save()?;
        Ok(next)
    }

    pub fn get_volte_config(&self) -> VolteConfig {
        self.config.read().unwrap().volte.clone()
    }

    pub fn set_volte_feature_enabled(&self, enabled: bool) -> Result<VolteConfig, String> {
        let next = {
            let mut c = self.config.write().unwrap();
            c.volte.feature_enabled = enabled;
            if !enabled {
                c.volte.connection_enabled = false;
            }
            c.volte.clone()
        };
        self.save()?;
        Ok(next)
    }

    pub fn get_line_profiles(&self) -> Vec<LineProfileConfig> {
        self.config.read().unwrap().line_profiles.clone()
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

    /// Preserve per-SIM line settings when an older device-derived line ID is
    /// replaced by the physical-slot-derived ID. The old profile is retained
    /// as history; only the current line receives a copied profile.
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
            if config
                .line_profiles
                .iter()
                .any(|profile| profile.line_id == line_id)
            {
                false
            } else if let Some(source) = config
                .line_profiles
                .iter()
                .find(|profile| legacy_line_ids.iter().any(|id| id == &profile.line_id))
                .cloned()
            {
                config.line_profiles.push(LineProfileConfig {
                    line_id: line_id.to_string(),
                    ..source
                });
                config
                    .line_profiles
                    .sort_by(|left, right| left.line_id.cmp(&right.line_id));
                true
            } else {
                false
            }
        };
        if migrated {
            self.save()?;
        }
        Ok(migrated)
    }

    pub fn get_line_profile(&self, line_id: &str) -> LineProfileConfig {
        self.config
            .read()
            .unwrap()
            .line_profiles
            .iter()
            .find(|profile| profile.line_id == line_id)
            .cloned()
            .unwrap_or_else(|| LineProfileConfig::for_line(line_id))
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

    /// Set this line's ordered VoLTE IMS address-family list. `None` clears the
    /// per-line override so the line falls back to the global
    /// `VolteConfig::ip_family_preference`. An empty or duplicated list is
    /// rejected so a saved override always means something the runtime can use.
    pub fn set_line_volte_ip_families(
        &self,
        line_id: &str,
        families: Option<Vec<VolteIpFamily>>,
    ) -> Result<LineProfileConfig, String> {
        if !valid_line_id(line_id) {
            return Err("invalid_line_id".to_string());
        }
        if let Some(families) = families.as_ref() {
            if families.is_empty() {
                return Err("volte_ip_families_empty".to_string());
            }
            let mut seen = Vec::new();
            for family in families {
                if seen.contains(family) {
                    return Err("volte_ip_families_duplicate".to_string());
                }
                seen.push(*family);
            }
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
            let next = profile.clone();
            config
                .line_profiles
                .sort_by(|left, right| left.line_id.cmp(&right.line_id));
            next
        };
        self.save()?;
        Ok(next)
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
            if vowifi.enabled {
                config.vowifi.feature_enabled = true;
            }
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

    pub fn set_volte_connection_enabled(&self, enabled: bool) -> Result<VolteConfig, String> {
        let next = {
            let mut c = self.config.write().unwrap();
            if enabled && !c.volte.feature_enabled {
                return Err("volte_feature_disabled".to_string());
            }
            c.volte.connection_enabled = enabled;
            c.volte.clone()
        };
        self.save()?;
        Ok(next)
    }

    /// Toggle VoLTE voice handling for registered per-line IMS sessions.
    pub fn set_volte_voice_enabled(&self, enabled: bool) -> Result<VolteConfig, String> {
        let next = {
            let mut c = self.config.write().unwrap();
            c.volte.voice_enabled = enabled;
            if !enabled {
                c.vilte.feature_enabled = false;
            }
            c.volte.clone()
        };
        self.save()?;
        Ok(next)
    }

    /// Current SMS multi-path routing policy (normalized so every path kind is
    /// present exactly once).
    pub fn get_sms_path_policy(&self) -> SmsPathPolicy {
        self.config.read().unwrap().sms_path.clone().normalized()
    }

    /// Replace the SMS multi-path routing policy. The incoming policy is
    /// normalized before persisting so a partial/duplicated priority list from
    /// the UI can never leave the config in an invalid state.
    pub fn set_sms_path_policy(&self, policy: SmsPathPolicy) -> Result<SmsPathPolicy, String> {
        let next = policy.normalized();
        {
            let mut c = self.config.write().unwrap();
            c.sms_path = next.clone();
        }
        self.save()?;
        Ok(next)
    }

    /// SMS path policy that applies to one line: its own override when set,
    /// otherwise the global policy.
    pub fn get_line_sms_path_policy(&self, line_id: &str) -> SmsPathPolicy {
        self.get_line_profile(line_id)
            .sms_path
            .map(|policy| policy.normalized())
            .unwrap_or_else(|| self.get_sms_path_policy())
    }

    /// Set or clear (`None`) one line's SMS path override.
    pub fn set_line_sms_path_policy(
        &self,
        line_id: &str,
        policy: Option<SmsPathPolicy>,
    ) -> Result<SmsPathPolicy, String> {
        let normalized = policy.map(|policy| policy.normalized());
        self.update_line_profile(line_id, |profile| {
            profile.sms_path = normalized.clone();
        })?;
        Ok(self.get_line_sms_path_policy(line_id))
    }

    pub fn get_voice_path_policy(&self) -> VoicePathPolicy {
        self.config.read().unwrap().voice_path.clone().normalized()
    }

    /// APN that applies to one line: its own override when set, otherwise the
    /// global APN.
    pub fn get_line_apn_config(&self, line_id: &str) -> ApnConfig {
        self.get_line_profile(line_id)
            .apn
            .unwrap_or_else(|| self.get_apn_config())
    }

    /// Set or clear (`None`) one line's APN override.
    pub fn set_line_apn_config(
        &self,
        line_id: &str,
        apn: Option<ApnConfig>,
    ) -> Result<ApnConfig, String> {
        self.update_line_profile(line_id, |profile| {
            profile.apn = apn.clone();
        })?;
        Ok(self.get_line_apn_config(line_id))
    }

    /// Voice path policy that applies to one line; same inheritance as SMS.
    pub fn get_line_voice_path_policy(&self, line_id: &str) -> VoicePathPolicy {
        self.get_line_profile(line_id)
            .voice_path
            .map(|policy| policy.normalized())
            .unwrap_or_else(|| self.get_voice_path_policy())
    }

    /// Set or clear (`None`) one line's voice path override.
    pub fn set_line_voice_path_policy(
        &self,
        line_id: &str,
        policy: Option<VoicePathPolicy>,
    ) -> Result<VoicePathPolicy, String> {
        let normalized = match policy {
            Some(policy) => {
                let policy = policy.normalized();
                if !policy.gateway_mode {
                    return Err("voice_gateway_mode_required_on_this_device".to_string());
                }
                Some(policy)
            }
            None => None,
        };
        self.update_line_profile(line_id, |profile| {
            profile.voice_path = normalized.clone();
        })?;
        Ok(self.get_line_voice_path_policy(line_id))
    }

    pub fn set_voice_path_policy(
        &self,
        policy: VoicePathPolicy,
    ) -> Result<VoicePathPolicy, String> {
        let next = policy.normalized();
        if !next.gateway_mode {
            return Err("voice_gateway_mode_required_on_this_device".to_string());
        }
        {
            let mut c = self.config.write().unwrap();
            c.voice_path = next.clone();
        }
        self.save()?;
        Ok(next)
    }

    pub fn get_vilte_config(&self) -> VilteConfig {
        self.config.read().unwrap().vilte.clone()
    }

    /// Toggle the ViLTE video feature. Video rides the VoLTE voice session, so
    /// enabling ViLTE requires VoLTE voice handling to be enabled. The actual
    /// IMS availability is derived from per-line profiles at runtime.
    pub fn set_vilte_feature_enabled(&self, enabled: bool) -> Result<VilteConfig, String> {
        let next = {
            let mut c = self.config.write().unwrap();
            if enabled && !c.volte.voice_enabled {
                return Err("volte_voice_disabled".to_string());
            }
            c.vilte.feature_enabled = enabled;
            c.vilte.clone()
        };
        self.save()?;
        Ok(next)
    }

    /// Replace the full ViLTE config (codec / payload type / fmtp). Does not
    /// change the gating; `feature_enabled` in the incoming value is honored
    /// only if VoLTE voice is enabled, otherwise it is forced off.
    pub fn set_vilte_config(&self, vilte: VilteConfig) -> Result<VilteConfig, String> {
        if !vilte.codec.trim().eq_ignore_ascii_case("h264") {
            return Err("vilte_codec_unsupported".to_string());
        }
        if !(96..=127).contains(&vilte.video_payload_type) {
            return Err("vilte_payload_type_invalid".to_string());
        }
        let next = {
            let mut c = self.config.write().unwrap();
            let mut incoming = vilte;
            if incoming.feature_enabled && !c.volte.voice_enabled {
                incoming.feature_enabled = false;
            }
            c.vilte = incoming;
            c.vilte.clone()
        };
        self.save()?;
        Ok(next)
    }

    pub fn set_esim_config(&self, esim: EsimConfig) -> Result<(), String> {
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

    pub fn set_data_enabled(&self, enabled: bool) -> Result<(), String> {
        {
            let mut c = self.config.write().unwrap();
            c.data_enabled = enabled;
        }
        self.save()
    }

    pub fn set_apn_config(&self, apn: ApnConfig) -> Result<(), String> {
        {
            let mut c = self.config.write().unwrap();
            c.apn = apn;
        }
        self.save()
    }

    pub fn set_roaming_allowed(&self, allowed: bool) -> Result<(), String> {
        {
            let mut c = self.config.write().unwrap();
            c.roaming_allowed = allowed;
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
            config.webhook = notifications.first_webhook_config().unwrap_or_default();
            config.notifications = notifications;
        }
        self.save()
    }

    /// 保存配置到文件
    pub fn save(&self) -> Result<(), String> {
        let _save_guard = self.save_lock.lock().unwrap();
        let content = {
            let config = self.config.read().unwrap();
            serde_json::to_string_pretty(&*config)
                .map_err(|e| format!("Failed to serialize config: {}", e))?
        };

        // 确保目录存在
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config directory: {}", e))?;
        }

        let temp_path = self.config_path.with_extension("tmp");
        let backup_path = self.config_path.with_extension("bak");
        let mut temp_file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temp_path)
            .map_err(|e| format!("Failed to open temporary config file: {e}"))?;
        temp_file
            .write_all(content.as_bytes())
            .map_err(|e| format!("Failed to write temporary config file: {e}"))?;
        temp_file
            .sync_all()
            .map_err(|e| format!("Failed to sync temporary config file: {e}"))?;
        drop(temp_file);

        if self.config_path.exists() {
            fs::copy(&self.config_path, &backup_path)
                .map_err(|e| format!("Failed to back up config file: {e}"))?;
        }

        if let Err(rename_error) = fs::rename(&temp_path, &self.config_path) {
            // Windows does not consistently replace an existing destination.
            // Production Linux uses the atomic rename path above; this fallback
            // keeps local development and migration tooling functional.
            if cfg!(windows) && self.config_path.exists() {
                fs::copy(&temp_path, &self.config_path)
                    .map_err(|e| format!("Failed to replace config file: {e}"))?;
                fs::remove_file(&temp_path)
                    .map_err(|e| format!("Failed to remove temporary config file: {e}"))?;
            } else {
                return Err(format!(
                    "Failed to atomically replace config file: {rename_error}"
                ));
            }
        }

        #[cfg(unix)]
        if let Some(parent) = self.config_path.parent() {
            if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
                let _ = directory.sync_all();
            }
        }

        Ok(())
    }
}

/// 获取默认配置文件路径
pub fn get_default_config_path() -> PathBuf {
    // Tests, recovery tools and side-by-side release candidates must be able
    // to avoid the device-wide `/data/config.json` without moving or editing
    // the production file.
    if let Some(path) = std::env::var_os("SIMADMIN_CONFIG_PATH") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }

    // 尝试 /data/config.json（设备上的持久化目录）
    let device_path = PathBuf::from("/data/config.json");
    if device_path.parent().map(|p| p.exists()).unwrap_or(false) {
        return device_path;
    }

    // 回退到当前目录
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("config.json")
}
