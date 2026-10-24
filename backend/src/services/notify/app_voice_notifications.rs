//! Pure, captured application-SMS plans and delivery of already-rendered work.
//! This path never discovers a modem, invents a destination, or uses the legacy
//! notification queue. The account worker owns admission and delivery claims.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{
    account_notifications::{
        NoticeDispatcher, NoticeFuture, NoticeOutcome, NoticePlan, NoticePlanner,
        NoticeSnapshot, NoticeTarget, NoticeWork,
    },
    account_store::valid_line_id,
    PROVIDER_ID,
};
use crate::platform::config::NotificationRateLimitConfig;
use ring::digest::{digest, SHA256};
use serde::ser::{SerializeMap, SerializeSeq};
use std::collections::HashSet;
use std::fmt;
use std::io::{self, Write};
use zeroize::Zeroizing;

const MAX_CONFIG_ITEMS: usize = 128;
const MAX_TARGETS: usize = 8;
const MAX_TITLE_BYTES: usize = 8 * 1024;
const MAX_BODY_BYTES: usize = 128 * 1024;
const MAX_PLAN_BYTES: usize = 256 * 1024;
const MAX_PAIR_BYTES: usize = 256 * 1024;
const MAX_JSON_NODES: usize = 16 * 1024;
const MAX_JSON_DEPTH: usize = 64;
const INVALID_CONFIG: &str = "app_account_notification_config_invalid";
const INVALID_MESSAGE: &str = "app_account_notification_message_invalid";
const INVALID_FINGERPRINT: &str = "app_account_notification_fingerprint_invalid";
const PLAN_TOO_LARGE: &str = "app_account_notification_plan_too_large";

/// Config strings retain their existing ownership semantics; only our temporary
/// serialization buffer and rendered-text scratch space are zeroizing.
struct AppVoiceNotificationSnapshot {
    config: NotificationConfig,
}

impl fmt::Debug for AppVoiceNotificationSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AppVoiceNotificationSnapshot(<redacted>)")
    }
}

fn valid_target_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control)
}

fn validate_config(config: &NotificationConfig) -> Result<(), &'static str> {
    if config.rules.len() > MAX_CONFIG_ITEMS || config.channels.len() > MAX_CONFIG_ITEMS {
        return Err(INVALID_CONFIG);
    }
    // IDs must identify exactly one captured object, including disabled objects.
    // Never guess which destination an ambiguous duplicate ID meant.
    let mut rules = HashSet::new();
    let mut channels = HashSet::new();
    if config.rules.iter().any(|rule| {
        !valid_target_id(&rule.id) || !rules.insert(rule.id.as_str())
            || rule.channel_ids.len()>MAX_CONFIG_ITEMS || rule.sim_channel_ids.len()>MAX_CONFIG_ITEMS
            || rule.matcher.field.len()>128 || rule.matcher.value.len()>4096
            || rule.template.len()>MAX_BODY_BYTES
    }) || config.channels.iter().any(|channel| {
        !valid_target_id(&channel.id) || !channels.insert(channel.id.as_str())
    }) {
        return Err(INVALID_CONFIG);
    }
    Ok(())
}

impl AppVoiceNotificationSnapshot {
    fn new(config: NotificationConfig) -> Result<Self, &'static str> {
        validate_config(&config)?;
        Ok(Self { config })
    }

    fn plan_targets(&self, message: &SmsMessage) -> Result<Vec<NoticeTarget>, &'static str> {
        let line = message.line_id.as_deref().filter(|line| valid_line_id(line))
            .ok_or(INVALID_MESSAGE)?;
        if message.transport != PROVIDER_ID
            || message.direction != "incoming"
            || message.pdu.is_some()
        {
            return Err(INVALID_MESSAGE);
        }
        // Use the exact immutable account line, never a SIM or sender identity.
        let context = SmsTemplateContext {
            own_number: String::new(),
            carrier: String::new(),
            sim_channel_id: line.to_string(),
            sim_channel_label: "无忧行 · 应用账号".to_string(),
        };
        let event = NotificationEvent::Sms { message, context: &context };
        let mut targets = Vec::new();
        let mut plan_bytes = 0usize;
        for rule in &self.config.rules {
            if !rule.enabled
                || rule.event_type != NotificationEventType::Sms
                || !rule_matches(rule, &event)
                || quiet_hours_active(&rule.quiet_hours)
            {
                continue;
            }
            // Repeated references in one rule mean one target, not two sends.
            let mut seen = HashSet::new();
            let channels: Vec<_> = rule.channel_ids.iter()
                .filter_map(|id| self.config.channels.iter().find(|channel| {
                    channel.enabled && channel.id == *id
                }))
                .filter(|channel| seen.insert(channel.id.as_str()))
                .collect();
            if channels.is_empty() {
                continue;
            }
            if targets.len() + channels.len() > MAX_TARGETS {
                return Err(PLAN_TOO_LARGE);
            }
            let title = Zeroizing::new(event.title());
            let template=if rule.template.trim().is_empty(){crate::platform::config::default_rule_template(NotificationEventType::Sms)}else{
                if rule.template.len()>MAX_BODY_BYTES{return Err(PLAN_TOO_LARGE);}rule.template.clone()
            };
            let body = Zeroizing::new(render_sms_template_bounded(&template,message,&context,false,MAX_BODY_BYTES)?);
            if title.len() > MAX_TITLE_BYTES || body.len() > MAX_BODY_BYTES {
                return Err(PLAN_TOO_LARGE);
            }
            for channel in channels {
                plan_bytes = plan_bytes.checked_add(title.len() + body.len())
                    .filter(|bytes| *bytes <= MAX_PLAN_BYTES).ok_or(PLAN_TOO_LARGE)?;
                targets.push(NoticeTarget {
                    rule_id: rule.id.clone(),
                    channel_id: channel.id.clone(),
                    fingerprint: pair_fingerprint(rule, channel)?,
                    title: title.to_string(),
                    body: body.to_string(),
                });
            }
        }
        Ok(targets)
    }
}

impl NoticePlanner for NotificationSender {
    fn capture(&self) -> Result<Box<dyn NoticeSnapshot>, &'static str> {
        // The caller captures before entering the SQLite admission transaction.
        // get_config returns an owned clone; plan never consults the sender.
        Ok(Box::new(AppVoiceNotificationSnapshot::new(self.get_config())?))
    }
}

impl NoticeSnapshot for AppVoiceNotificationSnapshot {
    fn plan(&self, message: &SmsMessage) -> Result<NoticePlan, &'static str> {
        NoticePlan::bounded(self.plan_targets(message)?)
    }
}

// Serialize every field of the selected rule/channel pair, including unknown
// nested channel settings. Canonical object ordering is independent of whether
// serde_json was built with preserve_order. No secret-bearing JSON is returned.
#[derive(Serialize)]
struct FingerprintChannel<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    channel_type: &'a NotificationChannel,
    name: &'a str,
    enabled: bool,
    rate_limit: &'a NotificationRateLimitConfig,
    config: CanonicalValue<'a>,
}

impl fmt::Debug for FingerprintChannel<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FingerprintChannel(<redacted>)")
    }
}

struct CanonicalValue<'a>(&'a Value);

impl fmt::Debug for CanonicalValue<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CanonicalValue(<redacted>)")
    }
}

impl Serialize for CanonicalValue<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Object(object) => {
                let mut entries: Vec<_> = object.iter().collect();
                entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    map.serialize_entry(key, &CanonicalValue(value))?;
                }
                map.end()
            }
            Value::Array(values) => {
                let mut sequence = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    sequence.serialize_element(&CanonicalValue(value))?;
                }
                sequence.end()
            }
            scalar => scalar.serialize(serializer),
        }
    }
}

// Bound traversal and borrowed sorting scratch before serialization. The writer
// separately accounts for JSON escapes and every rule/channel field byte.
fn check_json_budget(
    value: &Value,
    depth: usize,
    nodes: &mut usize,
    bytes: &mut usize,
) -> Result<(), &'static str> {
    if depth > MAX_JSON_DEPTH || *nodes == 0 {
        return Err(INVALID_FINGERPRINT);
    }
    *nodes -= 1;
    let cost = match value { Value::String(text) => text.len(), _ => 1 };
    *bytes = bytes.checked_sub(cost).ok_or(INVALID_FINGERPRINT)?;
    match value {
        Value::Object(object) => {
            if object.len() > *nodes {
                return Err(INVALID_FINGERPRINT);
            }
            for (key, value) in object {
                *bytes = bytes.checked_sub(key.len()).ok_or(INVALID_FINGERPRINT)?;
                check_json_budget(value, depth + 1, nodes, bytes)?;
            }
        }
        Value::Array(values) => {
            if values.len() > *nodes {
                return Err(INVALID_FINGERPRINT);
            }
            for value in values {
                check_json_budget(value, depth + 1, nodes, bytes)?;
            }
        }
        _ => {}
    }
    Ok(())
}

struct FingerprintBytes(Zeroizing<Vec<u8>>);

impl fmt::Debug for FingerprintBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FingerprintBytes(<redacted>)")
    }
}

impl Write for FingerprintBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_PAIR_BYTES - self.0.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, INVALID_FINGERPRINT));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn pair_fingerprint(
    rule: &NotificationRule,
    channel: &NotificationChannelInstance,
) -> Result<String, &'static str> {
    // Exhaustive destructuring intentionally makes added channel fields require
    // an explicit fingerprint decision rather than silently ignoring them.
    let NotificationChannelInstance {
        id, channel_type, name, enabled, rate_limit, config,
    } = channel;
    let mut remaining_nodes = MAX_JSON_NODES;
    let mut remaining_bytes = MAX_PAIR_BYTES;
    check_json_budget(config, 0, &mut remaining_nodes, &mut remaining_bytes)?;
    let canonical = FingerprintChannel {
        id, channel_type, name, enabled: *enabled, rate_limit,
        config: CanonicalValue(config),
    };
    // Reserve the full bound once: growing a secret-bearing Vec could leave an
    // unwiped old allocation behind. Write refuses to exceed this capacity.
    let mut bytes = FingerprintBytes(Zeroizing::new(Vec::with_capacity(MAX_PAIR_BYTES)));
    serde_json::to_writer(&mut bytes, &(rule, canonical))
        .map_err(|_| INVALID_FINGERPRINT)?;
    let hashed = digest(&SHA256, bytes.0.as_slice());
    let mut fingerprint = String::with_capacity(64);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &byte in hashed.as_ref() {
        fingerprint.push(HEX[(byte >> 4) as usize] as char);
        fingerprint.push(HEX[(byte & 15) as usize] as char);
    }
    Ok(fingerprint)
}

impl NoticeDispatcher for NotificationSender {
    fn deliver<'a>(&'a self, work: &'a NoticeWork) -> NoticeFuture<'a> {
        Box::pin(async move {
            if !valid_line_id(&work.line_id)
                || work.id <= 0 || work.sms_id <= 0
                || !valid_target_id(&work.rule_id) || !valid_target_id(&work.channel_id)
                || work.fingerprint.len() != 64
                || !work.fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
                || work.title.len() > MAX_TITLE_BYTES || work.body.len() > MAX_BODY_BYTES
            {
                return NoticeOutcome::Failed;
            }
            // One dispatch snapshot binds both policy checks and the actual
            // send. Do not look the destination up again after the fingerprint.
            let config = self.get_config();
            if validate_config(&config).is_err() {
                return NoticeOutcome::TargetChanged;
            }
            let Some(rule) = config.rules.iter().find(|rule| {
                rule.enabled && rule.id == work.rule_id
                    && rule.event_type == NotificationEventType::Sms
                    && rule.channel_ids.iter().any(|id| id == &work.channel_id)
                    && (rule.sim_channel_ids.is_empty()
                        || rule.sim_channel_ids.iter().any(|id| id == &work.line_id))
            }) else {
                return NoticeOutcome::TargetChanged;
            };
            let Some(channel) = config.channels.iter().find(|channel| {
                channel.enabled && channel.id == work.channel_id
            }) else {
                return NoticeOutcome::TargetChanged;
            };
            match pair_fingerprint(rule, channel) {
                Ok(fingerprint) if fingerprint == work.fingerprint => {}
                _ => return NoticeOutcome::TargetChanged,
            }
            if quiet_hours_active(&rule.quiet_hours) {
                return NoticeOutcome::Deferred(Duration::from_secs(60));
            }
            match self.rate_limit_reason(channel) {
                Ok(None) => {}
                Ok(Some(reason)) => {
                    drop(Zeroizing::new(reason));
                    return NoticeOutcome::Deferred(Duration::from_secs(
                        u64::from(channel.rate_limit.window_seconds).clamp(5, 3600),
                    ));
                }
                Err(error) => {
                    drop(Zeroizing::new(error));
                    return NoticeOutcome::Failed;
                }
            }
            match self.send_text_to_channel(channel, work.title.as_str(), work.body.as_str()).await {
                Ok(response) => drop(Zeroizing::new(response)),
                Err(error) => {
                    drop(Zeroizing::new(error));
                    return NoticeOutcome::Failed;
                }
            }
            // These names belong to the original fingerprinted pair. Neither
            // remote response text nor SMS content/phone/config enters this log.
            self.record_notification_log_raw(NewNotificationLog {
                line_id: Some(&work.line_id),
                event_type: "sms",
                status: "success",
                summary: "应用短信通知",
                rule_id: &work.rule_id,
                rule_name: &rule.name,
                channel_id: &work.channel_id,
                channel_name: &channel.name,
                message: "delivered",
            });
            NoticeOutcome::Delivered
        })
    }
}

#[cfg(test)]
#[path = "app_voice_notifications_tests.rs"]
mod tests;
