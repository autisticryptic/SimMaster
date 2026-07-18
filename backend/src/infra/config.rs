//! 配置管理模块
//!
//! 使用 JSON 文件存储用户配置，支持热更新

use serde::{de::Error as DeError, Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

use crate::api::models::WorkMode;

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
        "work_mode",
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
            "📱 短信通知\n号码: {{发送方号码}}\n内容: {{短信内容}}\n时间: {{时间}}\n路径: {{短信途径}}\n来源: {{本机号码}}".to_string()
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
    pub action: AutomationAction,
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
            .reconcile_modem_slots(&[("imei-a".to_string(), 1), ("imei-b".to_string(), 1)])
            .unwrap();
        assert_eq!(first["imei-a#uim1"].order, 1);
        assert_eq!(first["imei-b#uim1"].order, 2);

        let second = manager
            .reconcile_modem_slots(&[("imei-b".to_string(), 1), ("imei-a".to_string(), 1)])
            .unwrap();
        assert_eq!(second["imei-a#uim1"].order, 1);
        assert_eq!(second["imei-b#uim1"].order, 2);

        let third = manager
            .reconcile_modem_slots(&[("imei-a".to_string(), 2)])
            .unwrap();
        assert_eq!(third["imei-a#uim2"].order, 3);
        assert_eq!(third["imei-a#uim1"].order, 1);

        let reloaded = ConfigManager::new(path.clone());
        let persisted = reloaded
            .reconcile_modem_slots(&[("imei-a".to_string(), 2)])
            .unwrap();
        assert_eq!(persisted["imei-a#uim2"].order, 3);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn per_line_volte_connection_requires_feature_and_persists() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-line-config-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let manager = ConfigManager::new(path.clone());
        let line_id = "line-0123456789abcdef0123456789abcdef";
        assert_eq!(
            manager
                .set_line_volte_connection_enabled(line_id, true)
                .unwrap_err(),
            "volte_feature_disabled"
        );
        manager.set_volte_feature_enabled(true).unwrap();
        let profile = manager
            .set_line_volte_connection_enabled(line_id, true)
            .unwrap();
        assert!(profile.volte_connection_enabled);

        let reloaded = ConfigManager::new(path.clone());
        assert!(reloaded.get_line_profile(line_id).volte_connection_enabled);
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
    fn voice_services_default_closed_and_normalizes_limits() {
        let defaulted: VoiceServicesConfig = serde_json::from_str("{}").unwrap();
        assert!(!defaulted.feature_enabled);
        assert_eq!(defaulted.unknown_number_action, CallHandlingAction::Screen);
        assert_eq!(defaulted.verification_action, CallHandlingAction::Voicemail);
        assert_eq!(defaulted.marketing_action, CallHandlingAction::Reject);

        let normalized = VoiceServicesConfig {
            feature_enabled: true,
            number_rules: vec![IncomingNumberRule {
                id: " trusted ".to_string(),
                name: " 家人 ".to_string(),
                enabled: true,
                list: NumberListKind::Whitelist,
                matcher: NumberMatchKind::Prefix,
                pattern: " 138 ".to_string(),
                action: CallHandlingAction::Forward,
            }],
            verification_keywords: vec![" 验证码 ".to_string(), "验证码".to_string()],
            marketing_keywords: vec![" 优惠 ".to_string()],
            screening_max_seconds: 2,
            inbox_retention_days: 0,
            inbox_max_entries: 1,
            ..VoiceServicesConfig::default()
        }
        .normalized();
        assert_eq!(normalized.number_rules[0].id, "trusted");
        assert_eq!(normalized.number_rules[0].pattern, "138");
        assert_eq!(normalized.verification_keywords, vec!["验证码"]);
        assert_eq!(normalized.screening_max_seconds, 5);
        assert_eq!(normalized.inbox_retention_days, 1);
        assert_eq!(normalized.inbox_max_entries, 10);
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
    fn legacy_volte_ip_family_preference_is_read_but_not_serialized() {
        let defaulted: VolteConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(
            defaulted.ip_family_preference,
            VolteIpFamilyPreference::Ipv6First
        );

        let configured: VolteConfig =
            serde_json::from_str(r#"{"ip_family_preference":"ipv4_first"}"#).unwrap();
        assert_eq!(
            configured.ip_family_preference,
            VolteIpFamilyPreference::Ipv4First
        );
        assert!(serde_json::to_value(configured)
            .unwrap()
            .get("ip_family_preference")
            .is_none());
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Legacy address-family values accepted while reading older configurations.
/// Runtime selection is now fixed to dual-stack first with bounded IPv4/IPv6
/// fallback, so this value is ignored and no longer serialized or exposed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum VolteIpFamilyPreference {
    #[default]
    Ipv6First,
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
}

/// VoLTE (IMS over LTE) SMS configuration.
///
/// `feature_enabled` + `sms_enabled` mirror the observed persisted config on the
/// reference build (`struct VolteConfig { feature_enabled, sms_enabled }`). The
/// `connection_enabled` gate and auto-restore triple follow the `VowifiConfig`
/// pattern so the two features behave consistently.
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
    #[serde(default, skip_serializing)]
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
    pub trunk: TrunkProfileConfig,
}

/// Persisted identity for a physical modem slot.  ModemManager object paths
/// and SIM-derived line IDs are intentionally excluded: both can change after
/// a restart or SIM replacement, while the hardware key remains stable.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModemSlotConfig {
    pub hardware_key: String,
    /// Physical UIM slot within the modem. This keeps dual-SIM hardware from
    /// collapsing two card slots into one display position.
    #[serde(default = "default_uim_slot")]
    pub uim_slot: u8,
    #[serde(default)]
    pub order: u32,
    #[serde(default)]
    pub label: String,
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
            trunk: TrunkProfileConfig::default(),
        }
    }

    /// A copy safe to serialize across an API boundary (trunk secret redacted).
    pub fn redacted(&self) -> Self {
        Self {
            trunk: self.trunk.redacted(),
            ..self.clone()
        }
    }
}

fn valid_line_id(line_id: &str) -> bool {
    line_id.strip_prefix("line-").is_some_and(|suffix| {
        suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
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

// ===================== Voice routing and call screening =====================

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CallHandlingAction {
    /// Hand the call to an internal phone client (Linphone or a browser UA).
    Forward,
    /// Answer into a screening adapter and classify the first speech segment.
    #[default]
    Screen,
    /// Keep the call in the voice inbox and do not ring an internal client.
    Voicemail,
    /// Reject or terminate the call without forwarding it.
    Reject,
}

impl CallHandlingAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Screen => "screen",
            Self::Voicemail => "voicemail",
            Self::Reject => "reject",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumberListKind {
    Whitelist,
    Blacklist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NumberMatchKind {
    #[default]
    Exact,
    Prefix,
    Suffix,
    Contains,
}

/// One ordered incoming-number rule. Vector order is precedence, avoiding a
/// second priority field that can drift out of sync with the UI order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncomingNumberRule {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub list: NumberListKind,
    #[serde(default)]
    pub matcher: NumberMatchKind,
    #[serde(default)]
    pub pattern: String,
    pub action: CallHandlingAction,
}

fn default_verification_voice_keywords() -> Vec<String> {
    [
        "验证码",
        "校验码",
        "动态码",
        "认证码",
        "安全码",
        "verification code",
        "security code",
        "one time password",
        "otp",
    ]
    .into_iter()
    .map(ToString::to_string)
    .collect()
}

fn default_marketing_voice_keywords() -> Vec<String> {
    [
        "优惠活动",
        "贷款",
        "保险",
        "房产",
        "理财",
        "推销",
        "营销",
        "免费领取",
        "限时优惠",
    ]
    .into_iter()
    .map(ToString::to_string)
    .collect()
}

fn default_voice_inbox_retention_days() -> u32 {
    30
}

fn default_voice_inbox_max_entries() -> u32 {
    2_000
}

fn default_screening_max_seconds() -> u16 {
    30
}

/// Business rules above the future call/media adapter. No SIP, Asterisk,
/// Trunk, WebRTC or speech provider is selected here; adapters only feed caller
/// metadata and transcripts into this stable layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceServicesConfig {
    #[serde(default)]
    pub feature_enabled: bool,
    #[serde(default)]
    pub number_rules: Vec<IncomingNumberRule>,
    #[serde(default)]
    pub unknown_number_action: CallHandlingAction,
    #[serde(default = "default_verification_voice_keywords")]
    pub verification_keywords: Vec<String>,
    #[serde(default = "default_marketing_voice_keywords")]
    pub marketing_keywords: Vec<String>,
    #[serde(default = "default_verification_action")]
    pub verification_action: CallHandlingAction,
    #[serde(default = "default_marketing_action")]
    pub marketing_action: CallHandlingAction,
    #[serde(default = "default_ordinary_action")]
    pub ordinary_action: CallHandlingAction,
    #[serde(default = "default_ordinary_action")]
    pub uncertain_action: CallHandlingAction,
    #[serde(default = "default_screening_max_seconds")]
    pub screening_max_seconds: u16,
    #[serde(default = "default_voice_inbox_retention_days")]
    pub inbox_retention_days: u32,
    #[serde(default = "default_voice_inbox_max_entries")]
    pub inbox_max_entries: u32,
}

fn default_verification_action() -> CallHandlingAction {
    CallHandlingAction::Voicemail
}

fn default_marketing_action() -> CallHandlingAction {
    CallHandlingAction::Reject
}

fn default_ordinary_action() -> CallHandlingAction {
    CallHandlingAction::Forward
}

impl Default for VoiceServicesConfig {
    fn default() -> Self {
        Self {
            feature_enabled: false,
            number_rules: Vec::new(),
            unknown_number_action: CallHandlingAction::Screen,
            verification_keywords: default_verification_voice_keywords(),
            marketing_keywords: default_marketing_voice_keywords(),
            verification_action: default_verification_action(),
            marketing_action: default_marketing_action(),
            ordinary_action: default_ordinary_action(),
            uncertain_action: default_ordinary_action(),
            screening_max_seconds: default_screening_max_seconds(),
            inbox_retention_days: default_voice_inbox_retention_days(),
            inbox_max_entries: default_voice_inbox_max_entries(),
        }
    }
}

impl VoiceServicesConfig {
    pub fn normalized(mut self) -> Self {
        self.number_rules.retain(|rule| !rule.id.trim().is_empty());
        for rule in &mut self.number_rules {
            rule.id = rule.id.trim().to_string();
            rule.name = rule.name.trim().to_string();
            rule.pattern = rule.pattern.trim().to_string();
        }
        self.number_rules.retain(|rule| !rule.pattern.is_empty());
        self.verification_keywords = normalize_keywords(self.verification_keywords);
        self.marketing_keywords = normalize_keywords(self.marketing_keywords);
        self.screening_max_seconds = self.screening_max_seconds.clamp(5, 120);
        self.inbox_retention_days = self.inbox_retention_days.clamp(1, 3650);
        self.inbox_max_entries = self.inbox_max_entries.clamp(10, 100_000);
        self
    }
}

fn normalize_keywords(keywords: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for keyword in keywords {
        let keyword = keyword.trim().to_lowercase();
        if !keyword.is_empty() && !normalized.contains(&keyword) {
            normalized.push(keyword);
        }
    }
    normalized
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
    pub work_mode: WorkMode,
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
    pub vilte: VilteConfig,
    #[serde(default)]
    pub sms_path: SmsPathPolicy,
    #[serde(default)]
    pub voice_path: VoicePathPolicy,
    #[serde(default)]
    pub voice_services: VoiceServicesConfig,
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
            work_mode: WorkMode::default(),
            esim: EsimConfig::default(),
            automation: AutomationConfig::default(),
            vowifi: VowifiConfig::default(),
            volte: VolteConfig::default(),
            line_profiles: Vec::new(),
            modem_slots: Vec::new(),
            vilte: VilteConfig::default(),
            sms_path: SmsPathPolicy::default(),
            voice_path: VoicePathPolicy::default(),
            voice_services: VoiceServicesConfig::default(),
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
        };

        // 保存配置（如果文件不存在，或者配置模板发生了自动清理）
        if !manager.config_path.exists() || changed {
            let _ = manager.save();
        }

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

    /// 更新自动化配置
    pub fn set_automation_config(&self, automation: AutomationConfig) -> Result<(), String> {
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

    pub fn get_work_mode(&self) -> WorkMode {
        self.config.read().unwrap().work_mode
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
                for profile in &mut c.line_profiles {
                    profile.volte_connection_enabled = false;
                }
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
        hardware_slots: &[(String, u8)],
    ) -> Result<HashMap<String, ModemSlotConfig>, String> {
        let (slots, changed) = {
            let mut config = self.config.write().unwrap();
            let mut changed = false;

            for slot in &mut config.modem_slots {
                if slot.uim_slot == 0 {
                    slot.uim_slot = 1;
                    changed = true;
                }
            }
            let original_slot_count = config.modem_slots.len();
            let mut seen_hardware_keys = std::collections::HashSet::new();
            config.modem_slots.retain(|slot| {
                !slot.hardware_key.trim().is_empty()
                    && seen_hardware_keys.insert((slot.hardware_key.clone(), slot.uim_slot))
            });
            changed |= config.modem_slots.len() != original_slot_count;
            config.modem_slots.sort_by(|left, right| {
                left.order
                    .cmp(&right.order)
                    .then_with(|| left.hardware_key.cmp(&right.hardware_key))
            });

            let mut next_order = config
                .modem_slots
                .iter()
                .map(|slot| slot.order)
                .max()
                .unwrap_or(0)
                .saturating_add(1)
                .max(1);
            for (hardware_key, uim_slot) in hardware_slots {
                let hardware_key = hardware_key.trim();
                let uim_slot = (*uim_slot).max(1);
                if hardware_key.is_empty()
                    || config
                        .modem_slots
                        .iter()
                        .any(|slot| slot.hardware_key == hardware_key && slot.uim_slot == uim_slot)
                {
                    continue;
                }
                config.modem_slots.push(ModemSlotConfig {
                    hardware_key: hardware_key.to_string(),
                    uim_slot,
                    order: next_order,
                    label: format!("基带 {next_order}"),
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
                    .then_with(|| left.hardware_key.cmp(&right.hardware_key))
            });

            let slots = config
                .modem_slots
                .iter()
                .cloned()
                .map(|slot| (format!("{}#uim{}", slot.hardware_key, slot.uim_slot), slot))
                .collect::<HashMap<_, _>>();
            (slots, changed)
        };

        if changed {
            self.save()?;
        }
        Ok(slots)
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
            if enabled && !config.volte.feature_enabled {
                return Err("volte_feature_disabled".to_string());
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

    /// Toggle the VoLTE voice (gateway) feature. Requires the VoLTE feature to
    /// be enabled first (voice rides the same IMS registration as SMS).
    pub fn set_volte_voice_enabled(&self, enabled: bool) -> Result<VolteConfig, String> {
        let next = {
            let mut c = self.config.write().unwrap();
            if enabled && !c.volte.feature_enabled {
                return Err("volte_feature_disabled".to_string());
            }
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

    pub fn get_voice_path_policy(&self) -> VoicePathPolicy {
        self.config.read().unwrap().voice_path.clone().normalized()
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

    pub fn get_voice_services_config(&self) -> VoiceServicesConfig {
        self.config
            .read()
            .unwrap()
            .voice_services
            .clone()
            .normalized()
    }

    pub fn set_voice_services_config(
        &self,
        config: VoiceServicesConfig,
    ) -> Result<VoiceServicesConfig, String> {
        let next = config.normalized();
        {
            let mut c = self.config.write().unwrap();
            c.voice_services = next.clone();
        }
        self.save()?;
        Ok(next)
    }

    pub fn get_vilte_config(&self) -> VilteConfig {
        self.config.read().unwrap().vilte.clone()
    }

    /// Toggle the ViLTE video feature. Video rides the VoLTE voice session, so
    /// enabling ViLTE requires the VoLTE voice feature to be on (which in turn
    /// requires the VoLTE feature). This keeps the gating chain
    /// `volte.feature_enabled -> volte.voice_enabled -> vilte.feature_enabled`
    /// consistent with the "video is an add-on to the voice call" model.
    pub fn set_vilte_feature_enabled(&self, enabled: bool) -> Result<VilteConfig, String> {
        let next = {
            let mut c = self.config.write().unwrap();
            if enabled && !(c.volte.feature_enabled && c.volte.voice_enabled) {
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
        let next = {
            let mut c = self.config.write().unwrap();
            let mut incoming = vilte;
            if incoming.feature_enabled && !(c.volte.feature_enabled && c.volte.voice_enabled) {
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

    pub fn set_work_mode(&self, mode: WorkMode) -> Result<(), String> {
        {
            let mut c = self.config.write().unwrap();
            c.work_mode = mode;
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
        let config = self.config.read().unwrap();
        let content = serde_json::to_string_pretty(&*config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        // 确保目录存在
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config directory: {}", e))?;
        }

        fs::write(&self.config_path, content)
            .map_err(|e| format!("Failed to write config file: {}", e))?;

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
