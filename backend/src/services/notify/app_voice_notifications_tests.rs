//! Pure tests: no NotificationSender construction, private bus, database or HTTP.
use super::*;
use crate::platform::config::{DeviceStatusSchedule, RuleMatcher};

const LINE: &str = "app-account-0123456789abcdef0123456789abcdef";
const OTHER_LINE: &str = "app-account-fedcba9876543210fedcba9876543210";

fn message() -> SmsMessage {
    SmsMessage {
        id: 73,
        direction: "incoming".to_string(),
        phone_number: "synthetic-sender".to_string(),
        content: "【测试】code 482910\n原文 <&> \"quoted\"".to_string(),
        timestamp: "2026-05-14T16:30:45Z".to_string(),
        status: "received".to_string(),
        pdu: None,
        transport: "app_voice".to_string(),
        line_id: Some(LINE.to_string()),
    }
}

fn fixture() -> NotificationConfig {
    NotificationConfig {
        version: 2,
        channels: vec![NotificationChannelInstance {
            id: "channel-original".to_string(),
            channel_type: NotificationChannel::Webhook,
            name: "synthetic channel".to_string(),
            enabled: true,
            rate_limit: NotificationRateLimitConfig::default(),
            // Reserved .invalid host and synthetic values; never contacted.
            config: json!({
                "url": "https://notice.invalid/original",
                "secret": "synthetic-signing-secret",
                "headers": { "Authorization": "synthetic-token" },
                "unknown_settings": { "source": "synthetic-source" }
            }),
        }],
        rules: vec![NotificationRule {
            id: "rule-original".to_string(),
            event_type: NotificationEventType::Sms,
            name: "synthetic rule".to_string(),
            enabled: true,
            matcher: RuleMatcher::default(),
            channel_ids: vec!["channel-original".to_string()],
            sim_channel_ids: vec![LINE.to_string()],
            event_codes: Vec::new(),
            template: "{{content}}".to_string(),
            quiet_hours: Vec::new(),
            ddns_failure_threshold: 1,
            device_status_items: Vec::new(),
            device_status_schedule: DeviceStatusSchedule::default(),
            device_status_sms_period: "last_24h".to_string(),
        }],
        log_cleanup: Default::default(),
    }
}

fn all_day_quiet() -> QuietHoursSchedule {
    QuietHoursSchedule {
        enabled: true,
        weekdays: Vec::new(),
        start: "00:00".to_string(),
        end: "00:00".to_string(),
    }
}

fn snapshot(config: NotificationConfig) -> AppVoiceNotificationSnapshot {
    AppVoiceNotificationSnapshot::new(config).expect("synthetic config is supported")
}

fn targets(config: NotificationConfig, message: &SmsMessage) -> Vec<NoticeTarget> {
    let captured = snapshot(config);
    assert!(captured.plan(message).is_ok());
    captured.plan_targets(message).expect("pure synthetic plan")
}

fn fingerprint(config: &NotificationConfig) -> String {
    pair_fingerprint(&config.rules[0], &config.channels[0]).expect("bounded synthetic pair")
}

fn fanout(count: usize) -> NotificationConfig {
    let mut config = fixture();
    let channel = config.channels[0].clone();
    config.channels = (0..count).map(|index| NotificationChannelInstance {
        id: format!("channel-{index}"),
        ..channel.clone()
    }).collect();
    config.rules[0].channel_ids = config.channels.iter().map(|channel| channel.id.clone()).collect();
    config
}

#[test]
fn app_voice_context_keeps_original_account_line_without_physical_identity() {
    let mut config = fixture();
    config.rules[0].template = [
        "{{sim_channel_id}}", "{{SIM通道ID}}", "{{sim_channel}}", "{{SIM通道}}",
        "{{own_number}}", "{{local_phone_number}}", "{{self_phone_number}}", "{{本机号码}}",
        "{{carrier}}", "{{operator}}", "{{运营商}}", "{{sender}}",
    ].join("|");
    let original = message();
    let result = targets(config, &original);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].body, [
        LINE, LINE, "无忧行 · 应用账号", "无忧行 · 应用账号",
        "", "", "", "", "", "", "", "synthetic-sender",
    ].join("|"));
    assert_eq!(result[0].rule_id, "rule-original");
    assert_eq!(result[0].channel_id, "channel-original");
    assert_eq!(original.line_id.as_deref(), Some(LINE));
    assert_eq!(original.transport, "app_voice");
}

#[test]
fn app_voice_preserves_native_text_time_and_template_semantics() {
    let original = message();
    let mut config = fixture();
    config.rules[0].template = [
        "literal", "{{id}}", "{{phone_number}}", "{{content}}", "{{短信内容}}",
        "{{timestamp}}", "{{time}}", "{{direction}}", "{{status}}",
        "{{verification_code}}", "{{transport}}", "{{unknown}}",
    ].join("|");
    let result = targets(config, &original);
    assert_eq!(result[0].title, "SimAdmin 短信通知");
    assert_eq!(result[0].body, format!(
        "literal|73|synthetic-sender|{}|{}|2026-05-15 00:30:45|2026-05-15 00:30:45|incoming|received|482910|{}|{{{{unknown}}}}",
        original.content, original.content, sms_transport_label(&original.transport),
    ));
    assert_eq!(original.timestamp, "2026-05-14T16:30:45Z");
    assert_eq!(original.content, "【测试】code 482910\n原文 <&> \"quoted\"");

    // A blank existing template retains the native SMS default, not a new rule.
    let mut config = fixture();
    config.rules[0].template.clear();
    let context = SmsTemplateContext {
        sim_channel_id: LINE.to_string(),
        sim_channel_label: "无忧行 · 应用账号".to_string(),
        ..Default::default()
    };
    let expected = NotificationEvent::Sms { message: &original, context: &context }.render("");
    assert_eq!(targets(config, &original)[0].body, expected);
}

#[test]
fn app_voice_uses_native_matcher_operators_and_original_message_fields() {
    let cases = [
        ("content", MatcherOperator::Contains, "CODE", true),
        ("content", MatcherOperator::NotContains, "CODE", false),
        ("content", MatcherOperator::NotContains, "absent", true),
        ("content", MatcherOperator::Regex, r"\d{6}", true),
        ("content", MatcherOperator::Regex, "[", false),
        ("phone_number", MatcherOperator::Equals, "synthetic-sender", true),
        ("direction", MatcherOperator::Equals, "outgoing", false),
        ("sim_channel_id", MatcherOperator::Equals, LINE, true),
        ("sim_channel_id", MatcherOperator::Equals, OTHER_LINE, false),
        ("sim_channel", MatcherOperator::Equals, "无忧行 · 应用账号", true),
        ("own_number", MatcherOperator::Equals, "", true),
        ("own_number", MatcherOperator::Equals, "synthetic-sender", false),
    ];
    for (field, operator, value, matched) in cases {
        let mut config = fixture();
        config.rules[0].matcher = RuleMatcher {
            field: field.to_string(), operator, value: value.to_string(),
        };
        assert_eq!(targets(config, &message()).len(), usize::from(matched));
    }
    let mut config = fixture();
    config.rules[0].sim_channel_ids.clear();
    assert_eq!(targets(config, &message()).len(), 1);
}

#[test]
fn app_voice_skips_disabled_wrong_type_source_quiet_and_missing_channels() {
    let changes: &[fn(&mut NotificationConfig)] = &[
        |config| config.rules[0].enabled = false,
        |config| config.rules[0].event_type = NotificationEventType::Call,
        |config| config.rules[0].sim_channel_ids = vec![OTHER_LINE.to_string()],
        |config| config.rules[0].quiet_hours = vec![all_day_quiet()],
        |config| config.channels[0].enabled = false,
        |config| config.channels.clear(),
        |config| config.rules[0].channel_ids = vec!["missing".to_string()],
        |config| config.rules[0].channel_ids.clear(),
        |config| config.rules.clear(),
    ];
    for change in changes {
        let mut config = fixture();
        change(&mut config);
        let captured = snapshot(config);
        assert!(captured.plan(&message()).unwrap().is_empty());
        assert!(captured.plan_targets(&message()).unwrap().is_empty());
    }
    let mut config = fixture();
    let mut quiet = all_day_quiet();
    quiet.enabled = false;
    config.rules[0].quiet_hours.push(quiet);
    assert_eq!(targets(config, &message()).len(), 1);
}

#[test]
fn app_voice_eight_targets_are_allowed_but_ninth_rejects_entire_plan() {
    let captured = snapshot(fanout(8));
    assert_eq!(captured.plan_targets(&message()).unwrap().len(), 8);
    assert!(!captured.plan(&message()).unwrap().is_empty());
    assert_eq!(snapshot(fanout(9)).plan(&message()).err(), Some(PLAN_TOO_LARGE));

    // The cap is per SMS, not per rule, and distinct rules may share a channel.
    let mut config = fixture();
    let rule = config.rules[0].clone();
    config.rules = (0..8).map(|index| NotificationRule {
        id: format!("rule-{index}"), ..rule.clone()
    }).collect();
    assert_eq!(targets(config.clone(), &message()).len(), 8);
    config.rules.push(NotificationRule { id: "rule-ninth".to_string(), ..rule });
    assert_eq!(snapshot(config).plan(&message()).err(), Some(PLAN_TOO_LARGE));
}

#[test]
fn app_voice_repeated_references_dedupe_and_ambiguous_object_ids_fail_closed() {
    let mut config = fixture();
    config.rules[0].channel_ids = vec![config.channels[0].id.clone(); 32];
    assert_eq!(targets(config.clone(), &message()).len(), 1);
    let mut duplicate = config.rules[0].clone();
    duplicate.template = "different rendered content".to_string();
    config.rules.push(duplicate);
    assert_eq!(AppVoiceNotificationSnapshot::new(config).err(), Some(INVALID_CONFIG));

    let mut config = fixture();
    let mut duplicate = config.channels[0].clone();
    duplicate.config["url"] = json!("https://notice.invalid/different");
    config.channels.push(duplicate);
    assert_eq!(AppVoiceNotificationSnapshot::new(config).err(), Some(INVALID_CONFIG));
}

#[test]
fn app_voice_snapshot_bounds_configuration_counts_and_identifiers() {
    let config = fanout(128);
    assert!(AppVoiceNotificationSnapshot::new(config).is_ok());
    assert_eq!(AppVoiceNotificationSnapshot::new(fanout(129)).err(), Some(INVALID_CONFIG));
    let mut config = fixture();
    let rule = config.rules[0].clone();
    config.rules = (0..128).map(|index| NotificationRule {
        id: format!("rule-{index}"), ..rule.clone()
    }).collect();
    assert!(AppVoiceNotificationSnapshot::new(config.clone()).is_ok());
    config.rules.push(NotificationRule { id: "rule-over-bound".to_string(), ..rule });
    assert_eq!(AppVoiceNotificationSnapshot::new(config).err(), Some(INVALID_CONFIG));
    for invalid in [String::new(), "x".repeat(129), "control\nidentifier".to_string()] {
        let mut config = fixture();
        config.rules[0].id = invalid.clone();
        assert_eq!(AppVoiceNotificationSnapshot::new(config).err(), Some(INVALID_CONFIG));
        let mut config = fixture();
        config.channels[0].id = invalid;
        assert_eq!(AppVoiceNotificationSnapshot::new(config).err(), Some(INVALID_CONFIG));
    }
}

#[test]
fn app_voice_fingerprint_covers_destination_credentials_policy_and_source_fields() {
    let original = fixture();
    let before = fingerprint(&original);
    assert_eq!(before.len(), 64);
    assert!(before.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let changes: &[fn(&mut NotificationConfig)] = &[
        |config| config.channels[0].config["url"] = json!("https://notice.invalid/new"),
        |config| config.channels[0].config["secret"] = json!("synthetic-new-secret"),
        |config| config.channels[0].config["headers"]["Authorization"] = json!("synthetic-new-token"),
        |config| config.channels[0].config["unknown_settings"]["source"] = json!("synthetic-new-source"),
        |config| config.channels[0].config["unrecognized_secret"] = json!({"nested": ["synthetic"]}),
        |config| config.channels[0].rate_limit.window_seconds += 1,
        |config| config.channels[0].rate_limit.max_messages += 1,
        |config| config.channels[0].rate_limit.enabled = false,
        |config| config.channels[0].enabled = false,
        |config| config.channels[0].channel_type = NotificationChannel::Bark,
        |config| config.channels[0].name.push_str(" changed"),
        |config| config.channels[0].id.push_str("-changed"),
        |config| config.rules[0].template.push_str(" changed"),
        |config| config.rules[0].matcher.value = "changed".to_string(),
        |config| config.rules[0].sim_channel_ids.push(OTHER_LINE.to_string()),
        |config| config.rules[0].channel_ids.push("different".to_string()),
        |config| config.rules[0].quiet_hours.push(all_day_quiet()),
        |config| config.rules[0].event_codes.push("unused-for-sms-but-fingerprinted".to_string()),
        |config| config.rules[0].device_status_sms_period = "changed".to_string(),
        |config| config.rules[0].name.push_str(" changed"),
        |config| config.rules[0].id.push_str("-changed"),
        |config| config.rules[0].enabled = false,
        |config| config.rules[0].event_type = NotificationEventType::Call,
    ];
    for change in changes {
        let mut changed = original.clone();
        change(&mut changed);
        assert_ne!(fingerprint(&changed), before);
    }
}

#[test]
fn app_voice_fingerprint_is_canonical_selected_pair_only_and_snapshot_is_immutable() {
    let original = fixture();
    let expected = fingerprint(&original);
    assert_eq!(fingerprint(&original), expected);
    let mut reordered = original.clone();
    let mut headers = Map::new();
    headers.insert("second".to_string(), json!(2));
    headers.insert("first".to_string(), json!(1));
    reordered.channels[0].config = json!({"nested": [Value::Object(headers)]});
    let mut equivalent = reordered.clone();
    let mut headers = Map::new();
    headers.insert("first".to_string(), json!(1));
    headers.insert("second".to_string(), json!(2));
    equivalent.channels[0].config = json!({"nested": [Value::Object(headers)]});
    assert_eq!(fingerprint(&reordered), fingerprint(&equivalent));

    let captured = snapshot(original.clone());
    let mut changed = original.clone();
    changed.channels[0].config["url"] = json!("https://notice.invalid/replacement");
    assert_ne!(fingerprint(&changed), expected);
    assert_eq!(captured.plan_targets(&message()).unwrap()[0].fingerprint, expected);

    let mut unrelated = original.clone();
    let mut channel = unrelated.channels[0].clone();
    channel.id = "unrelated-channel".to_string();
    channel.config = json!({"secret": "synthetic-unrelated"});
    unrelated.channels.push(channel);
    let mut rule = unrelated.rules[0].clone();
    rule.id = "unrelated-rule".to_string();
    rule.enabled = false;
    unrelated.rules.push(rule);
    unrelated.log_cleanup.retention_days += 1;
    assert_eq!(targets(unrelated, &message())[0].fingerprint, expected);
    let mut different_message = message();
    different_message.phone_number = "another-synthetic-sender".to_string();
    different_message.content = "different text".to_string();
    different_message.timestamp = "2026-05-15T01:00:00Z".to_string();
    assert_eq!(targets(original, &different_message)[0].fingerprint, expected);
}

#[test]
fn app_voice_debug_wrappers_redact_snapshot_targets_plan_and_serialization() {
    let captured = snapshot(fixture());
    assert_eq!(format!("{captured:?}"), "AppVoiceNotificationSnapshot(<redacted>)");
    let plan = captured.plan(&message()).unwrap();
    assert_eq!(format!("{plan:?}"), "NoticePlan(<redacted>)");
    let targets = captured.plan_targets(&message()).unwrap();
    assert_eq!(format!("{:?}", targets[0]), "NoticeTarget(<redacted>)");
    let value = CanonicalValue(&captured.config.channels[0].config);
    assert_eq!(format!("{value:?}"), "CanonicalValue(<redacted>)");
    let bytes = FingerprintBytes(Zeroizing::new(b"synthetic-secret".to_vec()));
    assert_eq!(format!("{bytes:?}"), "FingerprintBytes(<redacted>)");
}

#[test]
fn app_voice_rejects_invalid_line_transport_direction_and_any_pdu() {
    let captured = snapshot(fixture());
    for line in [
        None, Some(""), Some("unassigned"), Some("reader:slot-1"),
        Some("line-0123456789abcdef0123456789abcdef"),
        Some("app-account-0123456789ABCDEF0123456789ABCDEF"),
        Some("app-account-0123456789abcdef0123456789abcde"),
        Some("app-account-0123456789abcdef0123456789abcdef0"),
        Some(" app-account-0123456789abcdef0123456789abcdef"),
        Some("app-account-0123456789abcdef0123456789abcdef\n"),
    ] {
        let mut invalid = message();
        invalid.line_id = line.map(str::to_string);
        assert_eq!(captured.plan(&invalid).err(), Some(INVALID_MESSAGE));
    }
    for transport in ["modem", "cellular_ims", "vowifi_ims", "app-voice", "APP_VOICE", "app_voice "] {
        let mut invalid = message();
        invalid.transport = transport.to_string();
        assert_eq!(captured.plan(&invalid).err(), Some(INVALID_MESSAGE));
    }
    for direction in ["outgoing", "incoming ", "INCOMING", ""] {
        let mut invalid = message();
        invalid.direction = direction.to_string();
        assert_eq!(captured.plan(&invalid).err(), Some(INVALID_MESSAGE));
    }
    for pdu in ["", "synthetic-pdu"] {
        let mut invalid = message();
        invalid.pdu = Some(pdu.to_string());
        assert_eq!(captured.plan(&invalid).err(), Some(INVALID_MESSAGE));
    }
}

#[test]
fn app_voice_bounds_template_expansion_before_allocating_and_preserves_small_cascades() {
    let mut config=fixture();config.rules[0].template="{{content}}".repeat(4096);
    let mut original=message();original.content="x".repeat(64*1024);
    assert_eq!(snapshot(config).plan(&original).err(),Some(PLAN_TOO_LARGE));
    let mut config=fixture();config.rules[0].template="{{content}}".into();
    original.content="{{短信内容}}".repeat(2048);
    assert_eq!(snapshot(config.clone()).plan(&original).err(),Some(PLAN_TOO_LARGE));
    original.content="{{sender}}".into();
    assert_eq!(targets(config,&original)[0].body,original.phone_number);
}

#[test]
fn app_voice_label_is_application_data_not_a_cs_or_ims_route() {
    assert_eq!(sms_transport_label("app_voice"),"应用消息");
    assert_eq!(sms_transport_label("modem"),"CS");
    assert_eq!(sms_transport_label("vowifi_ims"),"VoWiFi");
}

#[test]
fn app_voice_bounds_rendered_body_total_plan_and_fingerprint_serialization() {
    let mut large_message = message();
    large_message.content = "x".repeat(MAX_BODY_BYTES);
    assert!(snapshot(fixture()).plan(&large_message).is_ok());
    large_message.content.push('x');
    assert_eq!(snapshot(fixture()).plan(&large_message).err(), Some(PLAN_TOO_LARGE));
    large_message.content = "x".repeat(40 * 1024);
    assert_eq!(snapshot(fanout(8)).plan(&large_message).err(), Some(PLAN_TOO_LARGE));

    let mut config = fixture();
    config.channels[0].config["secret"] = json!("x".repeat(MAX_PAIR_BYTES + 1));
    assert_eq!(pair_fingerprint(&config.rules[0], &config.channels[0]).err(), Some(INVALID_FINGERPRINT));
    // Small input that JSON escaping expands beyond the serialization bound.
    config.channels[0].config = json!("\u{0000}".repeat(MAX_PAIR_BYTES / 3));
    assert_eq!(pair_fingerprint(&config.rules[0], &config.channels[0]).err(), Some(INVALID_FINGERPRINT));
    // Rule fields, not only channel.config, must fit the same bounded writer.
    config = fixture();
    config.rules[0].matcher.value = "x".repeat(MAX_PAIR_BYTES + 1);
    assert_eq!(pair_fingerprint(&config.rules[0], &config.channels[0]).err(), Some(INVALID_FINGERPRINT));
    config = fixture();
    config.channels[0].config = Value::Array(vec![Value::Null; MAX_JSON_NODES + 1]);
    assert_eq!(pair_fingerprint(&config.rules[0], &config.channels[0]).err(), Some(INVALID_FINGERPRINT));
    let mut nested = Value::Null;
    for _ in 0..MAX_JSON_DEPTH + 1 {
        nested = Value::Array(vec![nested]);
    }
    config.channels[0].config = nested;
    assert_eq!(pair_fingerprint(&config.rules[0], &config.channels[0]).err(), Some(INVALID_FINGERPRINT));
}
