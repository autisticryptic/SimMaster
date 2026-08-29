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

/// What the network said about our right to use MMTEL voice/video on this
/// registration.
///
/// SimAdmin does not keep local "voice enabled" switches: MMTEL is the reason
/// the project registers IMS at all, so the UE always advertises the voice
/// feature tags and lets the network decide. That only works if a refusal is
/// actually recognised, which is what this type is for.
///
/// The signals, in the order the 3GPP specs make them authoritative:
///
/// * A `200 OK` to REGISTER carries `P-Associated-URI` (TS 24.229 §5.1.1.2).
///   A voice-capable subscription is given a `tel:` URI or a `sip:` URI with
///   `user=phone` — that is the E.164 identity terminating calls are addressed
///   to. A registration that comes back with only a SIP-URI identity (no
///   telephone identity at all) is registered for messaging, not for calls.
/// * `Service-Route` must be present, or originating requests cannot be routed
///   through the S-CSCF that would select the MMTEL AS.
/// * A refusal arrives as a final status: `403` (not authorised / not
///   provisioned), `420`/`421` (a required extension we did not offer), `380`
///   (Alternative Service — how IMS says "use CS instead", TS 24.229
///   §5.1.2A.1.1), or `503` with a policy `Warning`.
///
/// `Unknown` is deliberately distinct from `Denied`: never report a refusal we
/// did not actually observe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImsServiceVerdict {
    /// Whether the network accepted this registration for MMTEL voice.
    pub voice: ImsServiceState,
    /// Stable machine code describing how the verdict was reached.
    pub code: &'static str,
    /// Whether retrying the same registration could plausibly change this.
    pub retryable: bool,
    /// Carrier-supplied explanatory text, bounded and control-stripped.
    pub carrier_reason: Option<String>,
    /// Set when the network told us to fall back to another access (380).
    pub alternative_service: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImsServiceState {
    /// The registrar returned a telephone-form associated identity and a
    /// Service-Route. This is deliberately weaker than "available": neither
    /// artifact proves MMTEL provisioning or that the TAS will select this
    /// binding for a terminating call. Observed on this device: both artifacts
    /// were present while terminating calls still went to voicemail.
    RegistrarAccepted,
    /// The REGISTER succeeded, but no telephone-form `P-Associated-URI` was
    /// observed. Some operators use a regular SIP IMPU for voice, so this is a
    /// diagnostic observation rather than evidence of messaging-only service.
    WithoutTelephoneIdentity,
    /// The network refused: not provisioned, barred, or redirected to CS.
    Denied,
    /// Nothing observed yet, or the response did not speak to voice at all.
    Unknown,
}

impl ImsServiceState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RegistrarAccepted => "registrar_accepted",
            Self::WithoutTelephoneIdentity => "without_telephone_identity",
            Self::Denied => "denied",
            Self::Unknown => "unknown",
        }
    }

    /// Whether a call may be attempted. Only an observed denial blocks it:
    /// `Unknown` must not become a local gate by the back door.
    pub fn permits_calls(self) -> bool {
        !matches!(self, Self::Denied)
    }
}

impl ImsServiceVerdict {
    pub const fn unknown() -> Self {
        Self {
            voice: ImsServiceState::Unknown,
            code: "ims_voice_service_unknown",
            retryable: true,
            carrier_reason: None,
            alternative_service: None,
        }
    }

    /// Classify the call-related artifacts observed in a successful REGISTER.
    /// This never proves end-to-end voice capability; only an explicit network
    /// refusal is allowed to become a local call gate.
    pub fn from_register_success(frame: &[u8]) -> Self {
        let associated = sip_frame::header_values(frame, "P-Associated-URI");
        let has_service_route = !sip_frame::header_values(frame, "Service-Route").is_empty();
        let has_telephone_identity = associated
            .iter()
            .flat_map(|value| split_quoted(value, ','))
            .any(uri_is_telephone_identity);

        if has_telephone_identity && has_service_route {
            // The registrar's answer is the limit of what this proves: it
            // returned call-related identity/routing artifacts. Whether a
            // terminating INVITE arrives is decided later by the TAS and by
            // whichever current Contact binding it picks.
            return Self {
                voice: ImsServiceState::RegistrarAccepted,
                code: "ims_voice_registrar_accepted",
                retryable: false,
                carrier_reason: None,
                alternative_service: None,
            };
        }
        if has_telephone_identity {
            // A missing Service-Route means the registrar did not publish an
            // originating route set. Some deployments still use a configured
            // outbound proxy, so keep the result unknown rather than denying
            // calls or claiming availability.
            return Self {
                voice: ImsServiceState::Unknown,
                code: "ims_voice_service_route_missing",
                retryable: true,
                carrier_reason: None,
                alternative_service: None,
            };
        }
        Self {
            voice: ImsServiceState::WithoutTelephoneIdentity,
            code: "ims_voice_no_telephone_identity",
            retryable: false,
            carrier_reason: None,
            alternative_service: None,
        }
    }

    /// Classify a REGISTER failure. Only refusals that genuinely speak to
    /// service entitlement produce `Denied`; a transport or authentication
    /// problem leaves the verdict `Unknown` so it is retried, not reported as
    /// a carrier refusal.
    pub fn from_register_failure(frame: &[u8]) -> Self {
        let Ok(status) = sip_frame::parse_status(frame) else {
            return Self::unknown();
        };
        let carrier_reason = parse_warning_text(frame).or_else(|| parse_reason_text(frame));
        let alternative_service = (status == 380)
            .then(|| sip_frame::header_value(frame, "Contact"))
            .flatten()
            .and_then(|value| sanitize_carrier_reason(&value));

        // A policy Warning is authoritative regardless of the status it rides
        // on: carriers attach "not provisioned" to 403, 503 and 480 alike.
        let policy_denial = carrier_reason
            .as_deref()
            .and_then(classify_carrier_warning)
            .is_some_and(|rule| rule.category == "carrier_policy");

        let (voice, code, retryable) = match status {
            380 => (
                ImsServiceState::Denied,
                "ims_voice_alternative_service",
                false,
            ),
            403 => (ImsServiceState::Denied, "ims_voice_forbidden", false),
            // 420/421 are SIP extension negotiation, NOT an entitlement answer.
            // Observed on live Maxis: the first REGISTER variant is answered 421
            // and the next variant (offering sec-agree) registers successfully.
            // Classifying that as a denial would report voice unavailable on
            // every session that in fact ends up registered for voice.
            420 | 421 => (
                ImsServiceState::Unknown,
                "ims_voice_extension_negotiation",
                true,
            ),
            _ if policy_denial => (
                ImsServiceState::Denied,
                "ims_voice_carrier_policy_denied",
                false,
            ),
            // 401/407 are the ordinary AKA challenge; 5xx and timeouts are
            // transport. Neither says anything about entitlement.
            _ => (
                ImsServiceState::Unknown,
                "ims_voice_service_unknown",
                !matches!(status, 400..=499),
            ),
        };

        Self {
            voice,
            code,
            retryable,
            carrier_reason,
            alternative_service,
        }
    }
}

/// Extract this line's own telephone numbers from a REGISTER `200 OK`.
///
/// The registrar's `P-Associated-URI` set (TS 24.229 §5.1.1.2) is the only place
/// a data-only line's own MSISDN is observable: the SIM's EF-MSISDN is commonly
/// unprogrammed, ModemManager then reports nothing, and USSD needs a circuit
/// this bearer does not provide. So when the network hands us
/// `<tel:+60174231067>` in the registration answer, that *is* the number.
///
/// Only genuine telephone identities are returned, using the same rule
/// [`ImsServiceVerdict::from_register_success`] applies — a SIP-URI identity
/// with no `user=phone` is an IMS identity, not a dialable number. Values are
/// normalised to bare E.164 (`+` plus digits) and de-duplicated, preserving the
/// registrar's order so the default identity stays first.
pub fn telephone_numbers_from_register_success(frame: &[u8]) -> Vec<String> {
    telephone_numbers_from_associated_uris(&sip_frame::header_values(frame, "P-Associated-URI"))
}

/// [`telephone_numbers_from_register_success`] for callers that already parsed
/// the header set into `RegisteredImsContext::associated_uris`.
///
/// Each element may itself be a comma-separated list, because a registrar is
/// free to fold several URIs into one header line.
pub fn telephone_numbers_from_associated_uris(uris: &[String]) -> Vec<String> {
    let mut numbers = Vec::new();
    for value in uris {
        for entry in split_quoted(value, ',') {
            if !uri_is_telephone_identity(entry) {
                continue;
            }
            if let Some(number) = e164_from_uri(entry) {
                if !numbers.contains(&number) {
                    numbers.push(number);
                }
            }
        }
    }
    numbers
}

/// Pull a bare `+E.164` out of a `tel:` or `sip:...;user=phone` URI.
///
/// Rejects anything that is not a plausible international number so a malformed
/// or unexpected URI cannot end up displayed as this line's phone number.
fn e164_from_uri(value: &str) -> Option<String> {
    let trimmed = value.trim().trim_start_matches('<').trim_end_matches('>');
    // Strip the scheme, then the host part and any URI parameters: both
    // `tel:+6017...;phone-context=...` and `sip:+6017...@domain;user=phone`
    // reduce to the user part.
    let without_scheme = trimmed
        .split_once(':')
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    let user = without_scheme
        .split(['@', ';'])
        .next()
        .unwrap_or(without_scheme)
        .trim();
    // Visual separators are permitted in tel: URIs (RFC 3966 §3).
    let digits: String = user
        .chars()
        .filter(|ch| !matches!(ch, '-' | '.' | '(' | ')' | ' '))
        .collect();
    let bare = digits.strip_prefix('+').unwrap_or(&digits);
    if bare.len() < 8 || bare.len() > 15 || !bare.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(format!("+{bare}"))
}

/// Whether an associated URI is an E.164 telephone identity: a `tel:` URI, or
/// a `sip:` URI carrying `user=phone` (TS 24.229 / RFC 3261 §19.1.1).
fn uri_is_telephone_identity(value: &str) -> bool {
    let value = value.trim().trim_start_matches('<').trim_end_matches('>');
    let lower = value.to_ascii_lowercase();
    lower.starts_with("tel:")
        || (lower.starts_with("sip:") || lower.starts_with("sips:"))
            && lower.split(';').skip(1).any(|parameter| {
                parameter
                    .split_once('=')
                    .is_some_and(|(name, value)| name.trim() == "user" && value.trim() == "phone")
            })
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

    #[test]
    fn register_success_with_telephone_identity_permits_voice() {
        // The device's real answer: a tel: URI plus a sip: URI with user=phone,
        // and a Service-Route. That is the registrar accepting a voice-capable
        // binding -- deliberately not reported as end-to-end "available", since
        // this exact answer coexisted with terminating calls going to voicemail.
        let response = b"SIP/2.0 200 OK\r\nP-Associated-URI: <sip:+60174231067@ims.mnc012.mcc502.3gppnetwork.org>, <tel:+60174231067>\r\nService-Route: <sip:orig@scscf.example:5060;lr>\r\nContent-Length: 0\r\n\r\n";
        let verdict = ImsServiceVerdict::from_register_success(response);

        assert_eq!(verdict.voice, ImsServiceState::RegistrarAccepted);
        assert_eq!(verdict.code, "ims_voice_registrar_accepted");
        assert!(verdict.voice.permits_calls());
    }

    #[test]
    fn registrar_acceptance_never_claims_end_to_end_availability() {
        // Guard against the overclaim being reintroduced: no state and no code
        // emitted by from_register_success may read as plain "available".
        let response = b"SIP/2.0 200 OK\r\nP-Associated-URI: <tel:+60174231067>\r\nService-Route: <sip:orig@scscf.example;lr>\r\n\r\n";
        let verdict = ImsServiceVerdict::from_register_success(response);
        assert_ne!(verdict.voice.as_str(), "available");
        assert_ne!(verdict.code, "ims_voice_service_available");
    }

    #[test]
    fn register_success_without_telephone_identity_remains_observational() {
        // A regular SIP IMPU may still be voice-capable on some operators. Keep
        // the legacy diagnostic state, but it must continue permitting calls.
        let response = b"SIP/2.0 200 OK\r\nP-Associated-URI: <sip:460001234567890@ims.example>\r\nService-Route: <sip:orig@scscf.example:5060;lr>\r\n\r\n";
        let verdict = ImsServiceVerdict::from_register_success(response);

        assert_eq!(verdict.voice, ImsServiceState::WithoutTelephoneIdentity);
        assert_eq!(verdict.code, "ims_voice_no_telephone_identity");
        assert!(verdict.voice.permits_calls());
    }

    #[test]
    fn user_phone_parameter_counts_as_a_telephone_identity() {
        let response = b"SIP/2.0 200 OK\r\nP-Associated-URI: <sip:+8613800138000@ims.example;user=phone>\r\nService-Route: <sip:orig@scscf.example;lr>\r\n\r\n";
        assert_eq!(
            ImsServiceVerdict::from_register_success(response).voice,
            ImsServiceState::RegistrarAccepted
        );
    }

    #[test]
    fn own_number_is_extracted_from_the_registrars_associated_uris() {
        // The observed Maxis answer: the IMSI-derived SIP IMPU plus the
        // MSISDN-associated tel: URI. Only the second is a dialable number, and
        // it is the only place this line's own number is observable.
        let response = b"SIP/2.0 200 OK\r\nP-Associated-URI: <sip:+60174231067@ims.mnc012.mcc502.3gppnetwork.org>, <tel:+60174231067>\r\nService-Route: <sip:orig@scscf.example:5060;lr>\r\n\r\n";
        assert_eq!(
            telephone_numbers_from_register_success(response),
            vec!["+60174231067".to_string()],
            "the sip: and tel: forms of one number must not be reported twice"
        );
    }

    #[test]
    fn a_sip_identity_without_user_phone_is_not_reported_as_a_number() {
        // An IMS identity is not a dialable number. Reporting it would put an
        // IMSI on screen labelled as the subscriber's phone number.
        let response =
            b"SIP/2.0 200 OK\r\nP-Associated-URI: <sip:460001234567890@ims.example>\r\n\r\n";
        assert!(telephone_numbers_from_register_success(response).is_empty());
    }

    #[test]
    fn own_numbers_keep_registrar_order_and_survive_folded_headers() {
        // A registrar may fold several URIs onto one line or split them across
        // header lines; both must yield the same list, default identity first.
        let folded =
            b"SIP/2.0 200 OK\r\nP-Associated-URI: <tel:+60174231067>, <tel:+60199999999>\r\n\r\n";
        let split = b"SIP/2.0 200 OK\r\nP-Associated-URI: <tel:+60174231067>\r\nP-Associated-URI: <tel:+60199999999>\r\n\r\n";
        let expected = vec!["+60174231067".to_string(), "+60199999999".to_string()];
        assert_eq!(telephone_numbers_from_register_success(folded), expected);
        assert_eq!(telephone_numbers_from_register_success(split), expected);
    }

    #[test]
    fn tel_uri_visual_separators_and_parameters_are_normalised_away() {
        // RFC 3966 §3 permits visual separators, and phone-context is common.
        let response =
            b"SIP/2.0 200 OK\r\nP-Associated-URI: <tel:+60-17-423.1067;phone-context=+60>\r\n\r\n";
        assert_eq!(
            telephone_numbers_from_register_success(response),
            vec!["+60174231067".to_string()]
        );
    }

    #[test]
    fn implausible_values_are_rejected_rather_than_displayed() {
        // Guard the display path: a short extension, a non-numeric user part, or
        // an over-long value must never reach the UI as a phone number.
        for header in [
            "<tel:911>",
            "<tel:+1-800-FLOWERS>",
            "<sip:1234567@ims.example;user=phone>",
            "<tel:+1234567890123456789>",
        ] {
            let frame = format!("SIP/2.0 200 OK\r\nP-Associated-URI: {header}\r\n\r\n");
            assert!(
                telephone_numbers_from_register_success(frame.as_bytes()).is_empty(),
                "{header} must not be reported as this line's number"
            );
        }
    }

    #[test]
    fn a_register_answer_without_associated_uris_yields_nothing() {
        let response = b"SIP/2.0 200 OK\r\nService-Route: <sip:orig@scscf.example;lr>\r\n\r\n";
        assert!(telephone_numbers_from_register_success(response).is_empty());
    }

    #[test]
    fn parsed_uri_lists_agree_with_parsing_the_frame() {
        // The two entry points must not drift: one takes the raw frame, the
        // other the already-parsed `associated_uris` a leg holds.
        let response = b"SIP/2.0 200 OK\r\nP-Associated-URI: <sip:+60174231067@ims.example;user=phone>, <tel:+60199999999>\r\n\r\n";
        let from_frame = telephone_numbers_from_register_success(response);
        let from_parsed = telephone_numbers_from_associated_uris(&[
            "<sip:+60174231067@ims.example;user=phone>, <tel:+60199999999>".to_string(),
        ]);
        assert_eq!(from_frame, from_parsed);
        assert_eq!(
            from_frame,
            vec!["+60174231067".to_string(), "+60199999999".to_string()]
        );
    }

    #[test]
    fn register_403_is_an_observed_voice_denial() {
        // With the local switches gone, a carrier that does not provision MMTEL
        // must be recognised from its own answer.
        let verdict = ImsServiceVerdict::from_register_failure(b"SIP/2.0 403 Forbidden\r\n\r\n");

        assert_eq!(verdict.voice, ImsServiceState::Denied);
        assert_eq!(verdict.code, "ims_voice_forbidden");
        assert!(!verdict.retryable);
        assert!(!verdict.voice.permits_calls());
    }

    #[test]
    fn alternative_service_redirect_is_a_denial_that_names_the_target() {
        let verdict = ImsServiceVerdict::from_register_failure(
            b"SIP/2.0 380 Alternative Service\r\nContact: <sip:cs@carrier.example>\r\n\r\n",
        );

        assert_eq!(verdict.voice, ImsServiceState::Denied);
        assert_eq!(verdict.code, "ims_voice_alternative_service");
        assert_eq!(
            verdict.alternative_service.as_deref(),
            Some("<sip:cs@carrier.example>")
        );
    }

    #[test]
    fn policy_warning_denies_voice_whatever_status_it_rides_on() {
        let verdict = ImsServiceVerdict::from_register_failure(
            b"SIP/2.0 503 Service Unavailable\r\nWarning: 399 pcscf \"IMS voice not provisioned\"\r\n\r\n",
        );

        assert_eq!(verdict.voice, ImsServiceState::Denied);
        assert_eq!(verdict.code, "ims_voice_carrier_policy_denied");
        assert_eq!(
            verdict.carrier_reason.as_deref(),
            Some("IMS voice not provisioned")
        );
    }

    #[test]
    fn negotiation_and_transport_failures_never_deny_voice() {
        // 421 is answered to this device's first REGISTER variant on live Maxis
        // and the next variant registers fine; 401/407 are the AKA challenge;
        // 5xx is transport. None of these is an entitlement answer, so none may
        // become a local gate by the back door.
        for status in [401u16, 407, 420, 421, 494, 500, 503] {
            let frame = format!("SIP/2.0 {status} Something\r\n\r\n");
            let verdict = ImsServiceVerdict::from_register_failure(frame.as_bytes());
            assert_ne!(
                verdict.voice,
                ImsServiceState::Denied,
                "{status} must not be reported as a voice denial"
            );
            assert!(verdict.voice.permits_calls());
        }
    }

    #[test]
    fn unknown_state_permits_calls_so_it_cannot_act_as_a_gate() {
        let verdict = ImsServiceVerdict::unknown();
        assert_eq!(verdict.voice, ImsServiceState::Unknown);
        assert!(verdict.voice.permits_calls());
    }
}
