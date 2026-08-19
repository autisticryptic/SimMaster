//! Structured IMS call failure diagnostics.
//!
//! SIP status handling follows RFC 3261 (plus common later SIP extensions).
//! `Reason` parsing follows RFC 3326 and gives Q.850 causes precedence over a
//! generic SIP status. Carrier `Warning` text is only used for a small set of
//! stable, actionable signals; unknown text is retained as bounded metadata and
//! never changes protocol behavior.

use serde::Serialize;

use super::{sip_frame, ImsError};

const MAX_CARRIER_REASON_CHARS: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImsFailureDiagnostic {
    pub code: &'static str,
    pub category: &'static str,
    pub sip_status: u16,
    pub q850_cause: Option<u16>,
    pub retryable: bool,
    pub retry_after_seconds: Option<u32>,
    pub carrier_reason: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct FailureRule {
    code: &'static str,
    category: &'static str,
    retryable: bool,
}

impl ImsFailureDiagnostic {
    pub fn from_status(sip_status: u16) -> Self {
        let rule = classify_sip_status(sip_status);
        Self {
            code: rule.code,
            category: rule.category,
            sip_status,
            q850_cause: None,
            retryable: rule.retryable,
            retry_after_seconds: None,
            carrier_reason: None,
        }
    }

    pub fn from_response(frame: &[u8]) -> Result<Self, ImsError> {
        let sip_status = sip_frame::parse_status(frame)?;
        let q850 = parse_q850_reason(frame);
        let warning = parse_warning_text(frame);
        let rule = warning
            .as_deref()
            .and_then(classify_carrier_warning)
            .or_else(|| q850.map(classify_q850_cause))
            .unwrap_or_else(|| classify_sip_status(sip_status));
        let reason_text = parse_reason_text(frame);

        Ok(Self {
            code: rule.code,
            category: rule.category,
            sip_status,
            q850_cause: q850,
            retryable: rule.retryable,
            retry_after_seconds: parse_retry_after(frame),
            carrier_reason: warning.or(reason_text),
        })
    }

    /// A bounded diagnostic header for the local Asterisk leg. Raw carrier
    /// topology is deliberately not forwarded.
    pub fn local_warning_header(&self) -> String {
        format!("399 simadmin \"{}\"", self.code)
    }

    pub fn local_reason_header(&self) -> String {
        match self.q850_cause {
            Some(cause) => format!("Q.850;cause={cause};text=\"{}\"", self.code),
            None => format!("SIP;cause={};text=\"{}\"", self.sip_status, self.code),
        }
    }
}

fn rule(code: &'static str, category: &'static str, retryable: bool) -> FailureRule {
    FailureRule {
        code,
        category,
        retryable,
    }
}

fn classify_sip_status(status: u16) -> FailureRule {
    match status {
        400 => rule("sip_bad_request", "request", false),
        401 | 407 => rule("sip_authentication_failed", "authentication", false),
        402 => rule("sip_payment_required", "carrier_policy", false),
        403 => rule("sip_forbidden", "authorization", false),
        404 | 604 => rule("number_not_found", "addressing", false),
        405 | 501 => rule("sip_method_not_supported", "capability", false),
        406 => rule("sip_response_not_acceptable", "capability", false),
        408 | 504 => rule("sip_request_timeout", "network_temporary", true),
        410 => rule("number_gone", "addressing", false),
        413 | 513 => rule("sip_message_too_large", "request", false),
        414 => rule("sip_uri_too_long", "addressing", false),
        415 => rule("sip_media_type_unsupported", "media", false),
        416 => rule("sip_uri_scheme_unsupported", "addressing", false),
        420 => rule("sip_extension_unsupported", "capability", false),
        421 => rule("sip_extension_required", "capability", false),
        423 => rule("sip_interval_too_brief", "configuration", true),
        430 => rule("sip_flow_failed", "network_temporary", true),
        439 => rule("sip_outbound_not_supported", "capability", false),
        480 => rule("callee_temporarily_unavailable", "remote_state", true),
        481 => rule("sip_dialog_not_found", "transaction", false),
        482 => rule("sip_loop_detected", "routing", false),
        483 => rule("sip_too_many_hops", "routing", false),
        484 => rule("number_incomplete", "addressing", false),
        485 => rule("number_ambiguous", "addressing", false),
        486 | 600 => rule("callee_busy", "remote_state", false),
        487 => rule("call_cancelled", "cancelled", false),
        488 | 606 => rule("media_not_acceptable", "media", false),
        491 => rule("sip_request_pending", "transaction", true),
        493 => rule("sip_body_undecipherable", "security", false),
        494 => rule("sip_security_agreement_required", "security", false),
        500 => rule("sip_server_error", "network_temporary", true),
        502 => rule("sip_bad_gateway", "network_temporary", true),
        503 => rule("sip_service_unavailable", "network_temporary", true),
        505 => rule("sip_version_unsupported", "capability", false),
        580 => rule("media_precondition_failed", "media", false),
        603 => rule("call_declined", "remote_state", false),
        607 => rule("call_unwanted", "remote_state", false),
        608 => rule("call_rejected_by_policy", "carrier_policy", false),
        300..=399 => rule("sip_redirection", "routing", false),
        400..=499 => rule("sip_client_failure", "request", false),
        500..=599 => rule("sip_network_failure", "network_temporary", true),
        600..=699 => rule("sip_global_failure", "remote_state", false),
        _ => rule("sip_failure_unknown", "unknown", false),
    }
}

fn classify_q850_cause(cause: u16) -> FailureRule {
    match cause {
        1 => rule("number_unallocated", "addressing", false),
        2 => rule("no_route_to_network", "routing", false),
        3 => rule("no_route_to_destination", "routing", false),
        6 => rule("channel_unacceptable", "network_temporary", true),
        16 => rule("normal_call_clearing", "remote_state", false),
        17 => rule("callee_busy", "remote_state", false),
        18 => rule("callee_not_responding", "remote_state", true),
        19 => rule("callee_no_answer", "remote_state", true),
        21 => rule("call_rejected", "remote_state", false),
        22 => rule("number_changed", "addressing", false),
        27 => rule("destination_out_of_order", "network_temporary", true),
        28 => rule("invalid_number_format", "addressing", false),
        29 => rule("facility_rejected", "carrier_policy", false),
        31 => rule("normal_unspecified", "unknown", false),
        34 => rule("no_circuit_available", "network_temporary", true),
        38 => rule("network_out_of_order", "network_temporary", true),
        41 => rule("temporary_failure", "network_temporary", true),
        42 => rule("switching_congestion", "network_temporary", true),
        47 => rule("network_resource_unavailable", "network_temporary", true),
        55 => rule("incoming_calls_barred", "carrier_policy", false),
        57 => rule("bearer_not_authorized", "carrier_policy", false),
        58 => rule("bearer_not_available", "carrier_policy", false),
        63 => rule("service_unavailable", "carrier_policy", false),
        65 => rule("bearer_not_implemented", "capability", false),
        69 => rule("facility_not_implemented", "capability", false),
        79 => rule("service_not_implemented", "capability", false),
        88 => rule("incompatible_destination", "media", false),
        102 => rule("recovery_timer_expired", "network_temporary", true),
        111 => rule("interworking_protocol_error", "network_temporary", true),
        127 => rule("interworking_unspecified", "unknown", false),
        _ => rule("q850_cause_unknown", "unknown", false),
    }
}

fn classify_carrier_warning(text: &str) -> Option<FailureRule> {
    let lower = text.to_ascii_lowercase();
    if lower.contains("release call received from cap") {
        return Some(rule(
            "carrier_service_control_release",
            "carrier_policy",
            false,
        ));
    }
    if lower.contains("insufficient balance")
        || lower.contains("insufficient credit")
        || lower.contains("low balance")
    {
        return Some(rule("carrier_insufficient_credit", "carrier_policy", false));
    }
    if lower.contains("outgoing call barred") || lower.contains("call is barred") {
        return Some(rule("carrier_call_barred", "carrier_policy", false));
    }
    if lower.contains("not provisioned")
        || lower.contains("not subscribed")
        || lower.contains("service not allowed")
    {
        return Some(rule(
            "carrier_service_not_provisioned",
            "carrier_policy",
            false,
        ));
    }
    if lower.contains("precondition") {
        return Some(rule("media_precondition_failed", "media", false));
    }
    None
}

fn parse_q850_reason(frame: &[u8]) -> Option<u16> {
    for value in sip_frame::header_values(frame, "Reason") {
        for reason in split_quoted(&value, ',') {
            let mut parts = reason.split(';');
            if !parts.next()?.trim().eq_ignore_ascii_case("Q.850") {
                continue;
            }
            for parameter in parts {
                let Some((name, value)) = parameter.trim().split_once('=') else {
                    continue;
                };
                if name.trim().eq_ignore_ascii_case("cause") {
                    if let Ok(cause) = value.trim().trim_matches('"').parse() {
                        return Some(cause);
                    }
                }
            }
        }
    }
    None
}

fn parse_reason_text(frame: &[u8]) -> Option<String> {
    for value in sip_frame::header_values(frame, "Reason") {
        for reason in split_quoted(&value, ',') {
            for parameter in reason.split(';').skip(1) {
                let Some((name, value)) = parameter.trim().split_once('=') else {
                    continue;
                };
                if name.trim().eq_ignore_ascii_case("text") {
                    return sanitize_carrier_reason(value.trim().trim_matches('"'));
                }
            }
        }
    }
    None
}

fn parse_warning_text(frame: &[u8]) -> Option<String> {
    for value in sip_frame::header_values(frame, "Warning") {
        let Some(start) = value.find('"') else {
            continue;
        };
        let Some(end) = value.rfind('"').filter(|end| *end > start) else {
            continue;
        };
        if let Some(text) = sanitize_carrier_reason(&value[start + 1..end]) {
            return Some(text);
        }
    }
    None
}

fn parse_retry_after(frame: &[u8]) -> Option<u32> {
    sip_frame::header_value(frame, "Retry-After")?
        .split(|ch: char| !ch.is_ascii_digit())
        .find(|part| !part.is_empty())?
        .parse()
        .ok()
}

fn sanitize_carrier_reason(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let sanitized = value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_CARRIER_REASON_CHARS)
        .collect::<String>();
    (!sanitized.is_empty()).then_some(sanitized)
}

fn split_quoted(value: &str, separator: char) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            _ if ch == separator && !quoted => {
                result.push(value[start..index].trim());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    result.push(value[start..].trim());
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_cap_release_is_actionable_carrier_policy() {
        let response = b"SIP/2.0 480 Temporarily Unavailable\r\nWarning: 399 172.20.58.196:5082 \"Release Call received from CAP\"\r\nRetry-After: 30\r\nContent-Length: 0\r\n\r\n";
        let diagnostic = ImsFailureDiagnostic::from_response(response).unwrap();

        assert_eq!(diagnostic.code, "carrier_service_control_release");
        assert_eq!(diagnostic.category, "carrier_policy");
        assert!(!diagnostic.retryable);
        assert_eq!(diagnostic.retry_after_seconds, Some(30));
        assert_eq!(
            diagnostic.carrier_reason.as_deref(),
            Some("Release Call received from CAP")
        );
    }

    #[test]
    fn q850_reason_overrides_generic_sip_status() {
        let response = b"SIP/2.0 480 Temporarily Unavailable\r\nReason: Q.850;cause=34;text=\"No circuit/channel available\"\r\n\r\n";
        let diagnostic = ImsFailureDiagnostic::from_response(response).unwrap();

        assert_eq!(diagnostic.code, "no_circuit_available");
        assert_eq!(diagnostic.q850_cause, Some(34));
        assert!(diagnostic.retryable);
    }

    #[test]
    fn common_media_and_auth_failures_have_distinct_codes() {
        assert_eq!(
            ImsFailureDiagnostic::from_status(488).code,
            "media_not_acceptable"
        );
        assert_eq!(ImsFailureDiagnostic::from_status(403).code, "sip_forbidden");
        assert_eq!(
            ImsFailureDiagnostic::from_status(503).code,
            "sip_service_unavailable"
        );
        assert!(ImsFailureDiagnostic::from_status(503).retryable);
    }

    #[test]
    fn quoted_reason_lists_do_not_split_on_text_commas() {
        let response = b"SIP/2.0 500 Error\r\nReason: SIP;cause=500;text=\"Try later, please\", Q.850;cause=41;text=\"Temporary failure\"\r\n\r\n";
        let diagnostic = ImsFailureDiagnostic::from_response(response).unwrap();
        assert_eq!(diagnostic.q850_cause, Some(41));
        assert_eq!(diagnostic.code, "temporary_failure");
    }
}
