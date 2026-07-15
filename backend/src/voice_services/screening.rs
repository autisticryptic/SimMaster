use crate::infra::config::{
    CallHandlingAction, IncomingNumberRule, NumberListKind, NumberMatchKind, VoiceServicesConfig,
};
use crate::messaging::verification_code::extract_verification_code;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CallCategory {
    Whitelisted,
    Blacklisted,
    Verification,
    Marketing,
    Ordinary,
    #[default]
    Unknown,
}

impl CallCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Whitelisted => "whitelisted",
            Self::Blacklisted => "blacklisted",
            Self::Verification => "verification",
            Self::Marketing => "marketing",
            Self::Ordinary => "ordinary",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CallScreeningDecision {
    pub phase: String,
    pub category: CallCategory,
    pub action: CallHandlingAction,
    pub normalized_number: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_rule_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_code: Option<String>,
    pub reason: String,
}

/// Decide a call's disposition. Number rules are evaluated before content:
/// whitelisted numbers can ring immediately and blacklisted numbers never need
/// to be answered just to classify their audio. Unknown numbers optionally
/// advance to transcript classification after a media adapter captures speech.
pub fn decide_call(
    config: &VoiceServicesConfig,
    phone_number: &str,
    transcript: Option<&str>,
) -> CallScreeningDecision {
    let normalized_number = normalize_phone_number(phone_number);
    if !config.feature_enabled {
        return CallScreeningDecision {
            phase: "pre_answer".to_string(),
            category: CallCategory::Unknown,
            action: CallHandlingAction::Forward,
            normalized_number,
            matched_rule_id: None,
            verification_code: None,
            reason: "voice_services_disabled".to_string(),
        };
    }

    if let Some(rule) = config
        .number_rules
        .iter()
        .find(|rule| number_rule_matches(rule, phone_number))
    {
        return CallScreeningDecision {
            phase: "pre_answer".to_string(),
            category: match rule.list {
                NumberListKind::Whitelist => CallCategory::Whitelisted,
                NumberListKind::Blacklist => CallCategory::Blacklisted,
            },
            action: rule.action,
            normalized_number,
            matched_rule_id: Some(rule.id.clone()),
            verification_code: None,
            reason: match rule.list {
                NumberListKind::Whitelist => "whitelist_rule_matched",
                NumberListKind::Blacklist => "blacklist_rule_matched",
            }
            .to_string(),
        };
    }

    let Some(transcript) = transcript.map(str::trim).filter(|text| !text.is_empty()) else {
        return CallScreeningDecision {
            phase: "pre_answer".to_string(),
            category: CallCategory::Unknown,
            action: config.unknown_number_action,
            normalized_number,
            matched_rule_id: None,
            verification_code: None,
            reason: "no_number_rule_or_transcript".to_string(),
        };
    };

    let normalized_transcript = transcript.to_lowercase();
    let verification_keyword =
        contains_keyword(&normalized_transcript, &config.verification_keywords);
    let verification_code = extract_verification_code(transcript).or_else(|| {
        verification_keyword
            .then(|| extract_spoken_verification_code(transcript, &config.verification_keywords))
            .flatten()
    });
    if verification_code.is_some() || verification_keyword {
        return CallScreeningDecision {
            phase: "post_transcript".to_string(),
            category: CallCategory::Verification,
            action: config.verification_action,
            normalized_number,
            matched_rule_id: None,
            verification_code,
            reason: if verification_keyword {
                "verification_speech_detected"
            } else {
                "verification_code_detected"
            }
            .to_string(),
        };
    }

    if contains_keyword(&normalized_transcript, &config.marketing_keywords) {
        return CallScreeningDecision {
            phase: "post_transcript".to_string(),
            category: CallCategory::Marketing,
            action: config.marketing_action,
            normalized_number,
            matched_rule_id: None,
            verification_code: None,
            reason: "marketing_speech_detected".to_string(),
        };
    }

    CallScreeningDecision {
        phase: "post_transcript".to_string(),
        category: CallCategory::Ordinary,
        action: config.ordinary_action,
        normalized_number,
        matched_rule_id: None,
        verification_code: None,
        reason: "ordinary_speech".to_string(),
    }
}

pub fn normalize_phone_number(value: &str) -> String {
    let trimmed = value.trim();
    let has_plus = trimmed.starts_with('+');
    let digits: String = trimmed.chars().filter(char::is_ascii_digit).collect();
    if has_plus && !digits.is_empty() {
        format!("+{digits}")
    } else {
        digits
    }
}

fn number_rule_matches(rule: &IncomingNumberRule, phone_number: &str) -> bool {
    if !rule.enabled || rule.pattern.trim().is_empty() {
        return false;
    }
    let numbers = phone_variants(phone_number);
    let patterns = phone_variants(&rule.pattern);
    numbers.iter().any(|number| {
        patterns.iter().any(|pattern| match rule.matcher {
            NumberMatchKind::Exact => number == pattern,
            NumberMatchKind::Prefix => number.starts_with(pattern),
            NumberMatchKind::Suffix => number.ends_with(pattern),
            NumberMatchKind::Contains => number.contains(pattern),
        })
    })
}

fn phone_variants(value: &str) -> Vec<String> {
    let normalized = normalize_phone_number(value);
    let digits = normalized.trim_start_matches('+').to_string();
    let mut variants = vec![digits.clone()];
    if digits.len() == 13 && digits.starts_with("86") {
        variants.push(digits[2..].to_string());
    }
    variants
}

fn contains_keyword(content: &str, keywords: &[String]) -> bool {
    keywords.iter().any(|keyword| {
        let keyword = keyword.trim().to_lowercase();
        !keyword.is_empty() && content.contains(&keyword)
    })
}

/// Speech-to-text engines often emit codes as grouped digits (`123 456`) or
/// Chinese digit words (`一二三四五六`). This fallback is only used after a
/// verification keyword matched, avoiding arbitrary phone/order numbers.
fn extract_spoken_verification_code(content: &str, keywords: &[String]) -> Option<String> {
    let lower = content.to_lowercase();
    let start = keywords
        .iter()
        .filter_map(|keyword| {
            let keyword = keyword.trim().to_lowercase();
            lower.find(&keyword).map(|index| index + keyword.len())
        })
        .min()
        .unwrap_or(0);
    let tail = lower.get(start..).unwrap_or(&lower);
    let mut digits = String::new();
    let mut started = false;
    for ch in tail.chars().take(80) {
        if let Some(digit) = spoken_digit(ch) {
            started = true;
            digits.push(digit);
            if digits.len() == 8 {
                break;
            }
            continue;
        }
        if !started {
            continue;
        }
        if ch.is_whitespace() || matches!(ch, '-' | '—') {
            continue;
        }
        break;
    }
    (4..=8).contains(&digits.len()).then_some(digits)
}

fn spoken_digit(ch: char) -> Option<char> {
    match ch {
        '0'..='9' => Some(ch),
        '零' | '〇' => Some('0'),
        '一' | '幺' => Some('1'),
        '二' | '两' => Some('2'),
        '三' => Some('3'),
        '四' => Some('4'),
        '五' => Some('5'),
        '六' => Some('6'),
        '七' => Some('7'),
        '八' => Some('8'),
        '九' => Some('9'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled_config() -> VoiceServicesConfig {
        VoiceServicesConfig {
            feature_enabled: true,
            ..VoiceServicesConfig::default()
        }
    }

    #[test]
    fn disabled_feature_forwards_without_screening() {
        let decision = decide_call(&VoiceServicesConfig::default(), "138 0013 8000", None);
        assert_eq!(decision.action, CallHandlingAction::Forward);
        assert_eq!(decision.reason, "voice_services_disabled");
        assert_eq!(decision.normalized_number, "13800138000");
    }

    #[test]
    fn whitelist_precedes_transcript_classification() {
        let mut config = enabled_config();
        config.number_rules.push(IncomingNumberRule {
            id: "family".to_string(),
            name: "family".to_string(),
            enabled: true,
            list: NumberListKind::Whitelist,
            matcher: NumberMatchKind::Exact,
            pattern: "13800138000".to_string(),
            action: CallHandlingAction::Forward,
        });
        let decision = decide_call(&config, "+86 138 0013 8000", Some("您的验证码是 123456"));
        assert_eq!(decision.category, CallCategory::Whitelisted);
        assert_eq!(decision.action, CallHandlingAction::Forward);
        assert_eq!(decision.matched_rule_id.as_deref(), Some("family"));
    }

    #[test]
    fn blacklist_can_reject_before_answer() {
        let mut config = enabled_config();
        config.number_rules.push(IncomingNumberRule {
            id: "sales-prefix".to_string(),
            name: String::new(),
            enabled: true,
            list: NumberListKind::Blacklist,
            matcher: NumberMatchKind::Prefix,
            pattern: "400".to_string(),
            action: CallHandlingAction::Reject,
        });
        let decision = decide_call(&config, "400-800-1234", None);
        assert_eq!(decision.category, CallCategory::Blacklisted);
        assert_eq!(decision.action, CallHandlingAction::Reject);
    }

    #[test]
    fn extracts_spoken_verification_code_and_keeps_it_from_forwarding() {
        let decision = decide_call(
            &enabled_config(),
            "10690000",
            Some("您的登录验证码是 123 456，五分钟内有效，请勿泄露"),
        );
        assert_eq!(decision.category, CallCategory::Verification);
        assert_eq!(decision.action, CallHandlingAction::Voicemail);
        assert_eq!(decision.verification_code.as_deref(), Some("123456"));

        let chinese = decide_call(
            &enabled_config(),
            "10690000",
            Some("您的动态验证码为一二三四五六，请勿告诉他人"),
        );
        assert_eq!(chinese.verification_code.as_deref(), Some("123456"));
    }

    #[test]
    fn detects_marketing_and_forwards_ordinary_speech() {
        let marketing = decide_call(
            &enabled_config(),
            "01012345678",
            Some("您好，我们有一项限时优惠活动"),
        );
        assert_eq!(marketing.category, CallCategory::Marketing);
        assert_eq!(marketing.action, CallHandlingAction::Reject);

        let ordinary = decide_call(
            &enabled_config(),
            "01012345678",
            Some("您好，您的快递已经放在门口了"),
        );
        assert_eq!(ordinary.category, CallCategory::Ordinary);
        assert_eq!(ordinary.action, CallHandlingAction::Forward);
    }
}
