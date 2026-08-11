//! Access-neutral IMS supplementary-service models and SIP primitives.
//!
//! Ut/XCAP transport remains access-owned, but call-waiting/diversion,
//! presentation and MWI have one semantic model regardless of whether the
//! request travels over the LTE IMS bearer or the VoWiFi tunnel.

use serde::{Deserialize, Serialize};

use super::{
    context::{ImsIdentity, ImsRoute},
    registration::RegisteredImsContext,
    sip_frame,
    sip_message::{build_request, SipHeader, SipRequest},
    ImsError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupplementaryService {
    CallWaiting,
    CommunicationDiversion,
    OriginatingIdentityPresentation,
    OriginatingIdentityRestriction,
    MessageWaiting,
    DialogTransfer,
}

/// RFC 3515 transfer subscription state. This state belongs to one SIP
/// dialog; it is deliberately unrelated to the network-side call-forwarding
/// rules managed through Ut/XCAP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DialogTransferState {
    Pending,
    Accepted,
    Trying,
    Succeeded,
    Failed,
}

impl DialogTransferState {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferSubscriptionState {
    Pending,
    Active,
    Terminated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferNotification {
    pub subscription_state: ReferSubscriptionState,
    pub sip_status: u16,
    pub transfer_state: DialogTransferState,
    /// RFC 3515 event id, when supplied by the notifier. It is the CSeq of
    /// the REFER that created this implicit subscription.
    pub event_id: Option<u32>,
}

/// Monotonic state machine for one implicit `refer` event subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogTransfer {
    state: DialogTransferState,
    expected_event_id: Option<u32>,
}

impl Default for DialogTransfer {
    fn default() -> Self {
        Self {
            state: DialogTransferState::Pending,
            expected_event_id: None,
        }
    }
}

impl DialogTransfer {
    pub fn for_refer_cseq(cseq: u32) -> Self {
        Self {
            state: DialogTransferState::Pending,
            expected_event_id: Some(cseq),
        }
    }

    pub fn state(&self) -> DialogTransferState {
        self.state
    }

    /// Consume the final response to REFER. A 2xx response accepts the
    /// subscription; completion is reported later by a refer-event NOTIFY.
    pub fn on_refer_response(&mut self, status: u16) -> Result<(), ImsError> {
        if self.state != DialogTransferState::Pending {
            return Err(ImsError::new("ims_refer_response_unexpected"));
        }
        match status {
            100..=199 => Ok(()),
            200..=299 => {
                self.state = DialogTransferState::Accepted;
                Ok(())
            }
            300..=699 => {
                self.state = DialogTransferState::Failed;
                Ok(())
            }
            _ => Err(ImsError::new("ims_refer_response_status_invalid")),
        }
    }

    pub fn on_notify(&mut self, notification: &ReferNotification) -> Result<(), ImsError> {
        if self.expected_event_id.is_some()
            && notification.event_id.is_some()
            && self.expected_event_id != notification.event_id
        {
            return Err(ImsError::new("ims_refer_notify_event_id_mismatch"));
        }
        if self.state.is_terminal() {
            return Err(ImsError::new("ims_refer_notify_after_terminal"));
        }
        if self.state == DialogTransferState::Pending {
            // A NOTIFY may race ahead of the 202 response on separate
            // transports, but it still proves that the implicit subscription
            // was accepted.
            self.state = DialogTransferState::Accepted;
        }
        self.state = notification.transfer_state;
        Ok(())
    }
}

/// Dialog identifiers and access-specific headers required to construct a
/// REFER without depending on either the VoLTE or VoWiFi adapter.
pub struct DialogReferRequest<'a> {
    pub request_uri: &'a str,
    pub branch: &'a str,
    pub from_uri: &'a str,
    pub from_tag: &'a str,
    pub to_value: &'a str,
    pub call_id: &'a str,
    pub cseq: u32,
    pub refer_to: &'a str,
    pub referred_by: Option<&'a str>,
}

/// Validate an externally supplied REFER target and return only its URI. A
/// display name and surrounding angle brackets are discarded so caller input
/// cannot inject arbitrary SIP headers. URI query parameters such as an
/// encoded `Replaces` value remain intact for attended transfer.
pub fn normalize_refer_target(value: &str) -> Result<String, ImsError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ImsError::new("ims_refer_target_empty"));
    }
    if value.len() > 512 || value.chars().any(char::is_control) {
        return Err(ImsError::new("ims_refer_target_invalid"));
    }
    let uri = if let Some(open) = value.find('<') {
        let close = value[open + 1..]
            .find('>')
            .map(|offset| open + 1 + offset)
            .ok_or(ImsError::new("ims_refer_target_invalid"))?;
        if !value[close + 1..].trim().is_empty() {
            return Err(ImsError::new("ims_refer_target_invalid"));
        }
        value[open + 1..close].trim()
    } else {
        value
    };
    if uri.is_empty()
        || uri.chars().any(char::is_whitespace)
        || !["sip:", "sips:", "tel:"].iter().any(|scheme| {
            uri.len() > scheme.len()
                && uri
                    .get(..scheme.len())
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
        })
    {
        return Err(ImsError::new("ims_refer_target_invalid"));
    }
    Ok(uri.to_string())
}

/// Build an in-dialog REFER using the current registered route. The access
/// adapter supplies PANI/security/User-Agent headers but does not duplicate
/// REFER framing or target validation.
pub fn build_dialog_refer(
    identity: &ImsIdentity,
    route: &ImsRoute,
    registration: &RegisteredImsContext,
    request: &DialogReferRequest<'_>,
    access_headers: &[SipHeader],
) -> Result<Vec<u8>, ImsError> {
    let refer_to = normalize_refer_target(request.refer_to)?;
    let route_value = registration
        .service_route
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            format!(
                "<sip:{}:{};lr>",
                sip_frame::sip_host(route.pcscf_addr.ip()),
                route.pcscf_addr.port()
            )
        });
    let mut headers = vec![
        SipHeader::new("Route", route_value),
        SipHeader::new("P-Preferred-Identity", format!("<{}>", identity.public_uri)),
        SipHeader::new("Refer-To", format!("<{refer_to}>")),
        SipHeader::new("Accept", "message/sipfrag"),
    ];
    if let Some(referred_by) = request.referred_by {
        let referred_by = normalize_refer_target(referred_by)?;
        headers.push(SipHeader::new("Referred-By", format!("<{referred_by}>")));
    }
    headers.extend_from_slice(access_headers);
    Ok(build_request(&SipRequest {
        method: "REFER",
        request_uri: request.request_uri,
        route: *route,
        branch: request.branch,
        from_uri: request.from_uri,
        from_tag: request.from_tag,
        to_value: request.to_value,
        call_id: request.call_id,
        cseq: request.cseq,
        headers: &headers,
        body: &[],
    }))
}

/// Parse one refer-event NOTIFY and its `message/sipfrag` progress body.
pub fn parse_refer_notify(frame: &[u8]) -> Result<ReferNotification, ImsError> {
    if !sip_frame::is_request(frame, "NOTIFY") {
        return Err(ImsError::new("ims_refer_notify_request_required"));
    }
    let event = sip_frame::header_value(frame, "Event")
        .ok_or(ImsError::new("ims_refer_notify_event_missing"))?;
    if !event
        .split(';')
        .next()
        .is_some_and(|event| event.trim().eq_ignore_ascii_case("refer"))
    {
        return Err(ImsError::new("ims_refer_notify_event_invalid"));
    }
    let mut event_id = None;
    for parameter in event.split(';').skip(1) {
        let parameter = parameter.trim();
        let Some((name, value)) = parameter.split_once('=') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("id") {
            continue;
        }
        let parsed = value
            .trim()
            .parse::<u32>()
            .map_err(|_| ImsError::new("ims_refer_notify_event_id_invalid"))?;
        if event_id.replace(parsed).is_some() {
            return Err(ImsError::new("ims_refer_notify_event_id_invalid"));
        }
    }
    let content_type = sip_frame::header_value(frame, "Content-Type")
        .ok_or(ImsError::new("ims_refer_notify_content_type_missing"))?;
    if !content_type.split(';').next().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "message/sipfrag" | "application/sipfrag"
        )
    }) {
        return Err(ImsError::new("ims_refer_notify_content_type_invalid"));
    }
    let subscription = sip_frame::header_value(frame, "Subscription-State")
        .ok_or(ImsError::new("ims_refer_subscription_state_missing"))?;
    let subscription_state = match subscription
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "pending" => ReferSubscriptionState::Pending,
        "active" => ReferSubscriptionState::Active,
        "terminated" => ReferSubscriptionState::Terminated,
        _ => return Err(ImsError::new("ims_refer_subscription_state_invalid")),
    };
    let body = std::str::from_utf8(sip_frame::body(frame))
        .map_err(|_| ImsError::new("ims_refer_sipfrag_not_utf8"))?;
    let status = body
        .lines()
        .next()
        .and_then(|line| line.trim().strip_prefix("SIP/2.0 "))
        .and_then(|remainder| remainder.split_whitespace().next())
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|status| (100..=699).contains(status))
        .ok_or(ImsError::new("ims_refer_sipfrag_status_invalid"))?;
    let transfer_state = match status {
        100..=199 => DialogTransferState::Trying,
        200..=299 => DialogTransferState::Succeeded,
        _ => DialogTransferState::Failed,
    };
    if subscription_state == ReferSubscriptionState::Terminated
        && transfer_state == DialogTransferState::Trying
    {
        return Err(ImsError::new("ims_refer_terminated_without_final_status"));
    }
    Ok(ReferNotification {
        subscription_state,
        sip_status: status,
        transfer_state,
        event_id,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityReadiness {
    pub supported: bool,
    pub ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Default for CapabilityReadiness {
    fn default() -> Self {
        Self::unsupported("supplementary_not_connected")
    }
}

impl CapabilityReadiness {
    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self {
            supported: false,
            ready: false,
            reason: Some(reason.into()),
        }
    }

    pub fn supported(ready: bool, reason: Option<String>) -> Self {
        Self {
            supported: true,
            ready,
            reason,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkToggleState {
    Enabled,
    Disabled,
    Unknown,
}

impl Default for NetworkToggleState {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardingCondition {
    Unconditional,
    Busy,
    NoReply,
    NotReachable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallForwardingRule {
    pub condition: ForwardingCondition,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_reply_timer_seconds: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityPresentation {
    Allowed,
    Restricted,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallerIdentitySource {
    AssertedIdentity,
    From,
    RemotePartyId,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallerIdentity {
    pub presentation: IdentityPresentation,
    pub source: CallerIdentitySource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// Resolve caller presentation without leaking an asserted identity hidden by
/// `Privacy: id`. The returned object is safe for normal UI/history paths.
pub fn resolve_caller_identity(frame: &[u8]) -> CallerIdentity {
    let privacy = sip_frame::header_value(frame, "Privacy").unwrap_or_default();
    let remote_party_id = sip_frame::header_value(frame, "Remote-Party-ID");
    let restricted = privacy
        .split([',', ';'])
        .map(str::trim)
        .any(|token| token.eq_ignore_ascii_case("id"))
        || remote_party_id.as_deref().is_some_and(|value| {
            value
                .split(';')
                .any(|parameter| parameter.trim().eq_ignore_ascii_case("privacy=full"))
        });
    if restricted {
        return CallerIdentity {
            presentation: IdentityPresentation::Restricted,
            source: CallerIdentitySource::None,
            uri: None,
            display_name: None,
        };
    }

    for (name, source) in [
        (
            "P-Asserted-Identity",
            CallerIdentitySource::AssertedIdentity,
        ),
        ("From", CallerIdentitySource::From),
        ("Remote-Party-ID", CallerIdentitySource::RemotePartyId),
    ] {
        if let Some(value) = sip_frame::header_value(frame, name) {
            let (display_name, uri) = parse_name_addr(&value);
            if uri.is_some() {
                return CallerIdentity {
                    presentation: IdentityPresentation::Allowed,
                    source,
                    uri,
                    display_name,
                };
            }
        }
    }

    CallerIdentity {
        presentation: IdentityPresentation::Unavailable,
        source: CallerIdentitySource::None,
        uri: None,
        display_name: None,
    }
}

fn parse_name_addr(value: &str) -> (Option<String>, Option<String>) {
    let value = value.trim();
    if let (Some(open), Some(close)) = (value.find('<'), value.find('>')) {
        let display = value[..open].trim().trim_matches('"').trim();
        let uri = value[open + 1..close].trim();
        return (
            (!display.is_empty()).then(|| display.to_string()),
            (!uri.is_empty()).then(|| uri.to_string()),
        );
    }
    let uri = value.split(';').next().unwrap_or_default().trim();
    (
        None,
        (uri.starts_with("sip:") || uri.starts_with("sips:") || uri.starts_with("tel:"))
            .then(|| uri.to_string()),
    )
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageCount {
    pub new: u32,
    pub old: u32,
    pub urgent_new: u32,
    pub urgent_old: u32,
}

/// Authority that produced a voicemail snapshot. IMS MWI and an Asterisk
/// mailbox are intentionally different products even when both expose message
/// counts to the same UI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoicemailSource {
    #[default]
    OperatorIms,
    AsteriskLocal,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageWaitingSummary {
    pub source: VoicemailSource,
    pub messages_waiting: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<MessageCount>,
}

/// Parse `application/simple-message-summary` (RFC 3842). Unknown media-class
/// lines are retained by neither logs nor state; only voice counters are used.
pub fn parse_message_summary(body: &[u8]) -> Result<MessageWaitingSummary, ImsError> {
    let text = std::str::from_utf8(body).map_err(|_| ImsError::new("ims_mwi_summary_not_utf8"))?;
    let mut waiting = None;
    let mut account = None;
    let mut voice = None;
    for line in text.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "messages-waiting" => {
                waiting = match value.trim().to_ascii_lowercase().as_str() {
                    "yes" => Some(true),
                    "no" => Some(false),
                    _ => return Err(ImsError::new("ims_mwi_waiting_value_invalid")),
                }
            }
            "message-account" => {
                let value = value.trim();
                if !value.is_empty() {
                    account = Some(value.to_string());
                }
            }
            "voice-message" => voice = Some(parse_message_count(value.trim())?),
            _ => {}
        }
    }
    Ok(MessageWaitingSummary {
        source: VoicemailSource::OperatorIms,
        messages_waiting: waiting.ok_or(ImsError::new("ims_mwi_waiting_header_missing"))?,
        message_account: account,
        voice,
    })
}

/// Validate and parse an RFC 3842 message-summary NOTIFY. Keeping this check in
/// the shared core prevents one access leg from accidentally treating an
/// unrelated SIP NOTIFY as voicemail state.
pub fn parse_mwi_notify(frame: &[u8]) -> Result<MessageWaitingSummary, ImsError> {
    if !sip_frame::is_request(frame, "NOTIFY") {
        return Err(ImsError::new("ims_mwi_notify_request_required"));
    }
    let event = sip_frame::header_value(frame, "Event")
        .ok_or(ImsError::new("ims_mwi_notify_event_missing"))?;
    if !event
        .split(';')
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("message-summary"))
    {
        return Err(ImsError::new("ims_mwi_notify_event_invalid"));
    }
    let content_type = sip_frame::header_value(frame, "Content-Type")
        .ok_or(ImsError::new("ims_mwi_notify_content_type_missing"))?;
    if !content_type.split(';').next().is_some_and(|value| {
        value
            .trim()
            .eq_ignore_ascii_case("application/simple-message-summary")
    }) {
        return Err(ImsError::new("ims_mwi_notify_content_type_invalid"));
    }
    parse_message_summary(sip_frame::body(frame))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MwiIncomingFrame {
    Notify {
        response_status: u16,
        summary: Option<Result<MessageWaitingSummary, ImsError>>,
    },
    SubscribeResponse {
        status: Result<u16, ImsError>,
        to_tag: Option<String>,
    },
    Other,
}

/// Classify the two MWI transaction frame types identically on every IMS
/// access. The adapters retain ownership of channel I/O and runtime updates.
pub fn classify_mwi_frame(frame: &[u8], subscription_call_id: Option<&str>) -> MwiIncomingFrame {
    let Some(call_id) = sip_frame::header_value(frame, "Call-ID") else {
        return MwiIncomingFrame::Other;
    };
    let is_summary_notify = sip_frame::is_request(frame, "NOTIFY")
        && sip_frame::header_value(frame, "Event").is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|event| event.trim().eq_ignore_ascii_case("message-summary"))
        });
    if is_summary_notify {
        if subscription_call_id != Some(call_id.as_str()) {
            return MwiIncomingFrame::Notify {
                response_status: 481,
                summary: None,
            };
        }
        return MwiIncomingFrame::Notify {
            response_status: 200,
            summary: Some(parse_mwi_notify(frame)),
        };
    }

    let is_subscribe_response = frame.starts_with(b"SIP/2.0 ")
        && subscription_call_id == Some(call_id.as_str())
        && sip_frame::header_value(frame, "CSeq").is_some_and(|value| {
            value
                .split_whitespace()
                .nth(1)
                .is_some_and(|method| method.eq_ignore_ascii_case("SUBSCRIBE"))
        });
    if is_subscribe_response {
        return MwiIncomingFrame::SubscribeResponse {
            status: sip_frame::parse_status(frame),
            to_tag: sip_frame::header_value(frame, "To")
                .and_then(|value| header_parameter(&value, "tag")),
        };
    }
    MwiIncomingFrame::Other
}

fn header_parameter(value: &str, name: &str) -> Option<String> {
    value.split(';').skip(1).find_map(|parameter| {
        let (candidate, value) = parameter.trim().split_once('=')?;
        candidate
            .trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().trim_matches('"').to_string())
    })
}

fn parse_message_count(value: &str) -> Result<MessageCount, ImsError> {
    let (normal, urgent) = match value.split_once('(') {
        Some((normal, urgent)) => (normal.trim(), Some(urgent.trim_end_matches(')').trim())),
        None => (value.trim(), None),
    };
    let (new, old) = parse_count_pair(normal)?;
    let (urgent_new, urgent_old) = urgent.map(parse_count_pair).transpose()?.unwrap_or((0, 0));
    Ok(MessageCount {
        new,
        old,
        urgent_new,
        urgent_old,
    })
}

fn parse_count_pair(value: &str) -> Result<(u32, u32), ImsError> {
    let (new, old) = value
        .split_once('/')
        .ok_or(ImsError::new("ims_mwi_voice_count_invalid"))?;
    Ok((
        new.trim()
            .parse()
            .map_err(|_| ImsError::new("ims_mwi_voice_count_invalid"))?,
        old.trim()
            .parse()
            .map_err(|_| ImsError::new("ims_mwi_voice_count_invalid"))?,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscribeIds {
    pub branch: String,
    pub from_tag: String,
    pub to_tag: Option<String>,
    pub call_id: String,
    pub cseq: u32,
}

/// Build an MWI SUBSCRIBE from the same registration context used by calls and
/// SMS. In particular, `Service-Route` is never copied into a second cache.
#[allow(clippy::too_many_arguments)]
pub fn build_mwi_subscribe(
    identity: &ImsIdentity,
    route: &ImsRoute,
    registration: &RegisteredImsContext,
    ids: &SubscribeIds,
    expires_seconds: u32,
    user_agent: &str,
    access_headers: &[SipHeader],
) -> Vec<u8> {
    let local_host = sip_frame::sip_host(route.local_addr.ip());
    let route_value = registration
        .service_route
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            format!(
                "<sip:{}:{};lr>",
                sip_frame::sip_host(route.pcscf_addr.ip()),
                route.pcscf_addr.port()
            )
        });
    let mut headers = vec![
        SipHeader::new("Route", route_value),
        SipHeader::new("P-Preferred-Identity", format!("<{}>", identity.public_uri)),
        SipHeader::new(
            "Contact",
            format!(
                "<sip:{}@{}:{};transport={}>",
                identity.contact_user,
                local_host,
                route.local_addr.port(),
                route.transport.as_param()
            ),
        ),
        SipHeader::new("Event", "message-summary"),
        SipHeader::new("Accept", "application/simple-message-summary"),
        SipHeader::new("Expires", expires_seconds.to_string()),
    ];
    headers.extend_from_slice(access_headers);
    headers.push(SipHeader::new("User-Agent", user_agent));
    let to_value = match ids.to_tag.as_deref() {
        Some(tag) => format!("<{}>;tag={tag}", identity.public_uri),
        None => format!("<{}>", identity.public_uri),
    };
    build_request(&SipRequest {
        method: "SUBSCRIBE",
        request_uri: &identity.public_uri,
        route: *route,
        branch: &ids.branch,
        from_uri: &identity.public_uri,
        from_tag: &ids.from_tag,
        to_value: &to_value,
        call_id: &ids.call_id,
        cseq: ids.cseq,
        headers: &headers,
        body: &[],
    })
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, time::SystemTime};

    use super::*;
    use crate::connectivity::core::{
        context::SipTransport,
        registration::{ImsRegistrationAccess, RegistrationLease},
    };

    #[test]
    fn parses_voice_message_summary_with_urgent_counts() {
        let summary = parse_message_summary(
            b"Messages-Waiting: yes\r\nMessage-Account: sip:voicemail@example.test\r\nVoice-Message: 4/8 (1/2)\r\n",
        )
        .unwrap();
        assert!(summary.messages_waiting);
        assert_eq!(summary.source, VoicemailSource::OperatorIms);
        assert_eq!(
            summary.message_account.as_deref(),
            Some("sip:voicemail@example.test")
        );
        assert_eq!(
            summary.voice,
            Some(MessageCount {
                new: 4,
                old: 8,
                urgent_new: 1,
                urgent_old: 2,
            })
        );
    }

    #[test]
    fn privacy_id_suppresses_asserted_identity() {
        let frame = b"INVITE sip:user@example SIP/2.0\r\nPrivacy: id\r\nP-Asserted-Identity: \"Private User\" <sip:+15551234567@example>\r\nFrom: Anonymous <sip:anonymous@anonymous.invalid>;tag=a\r\n\r\n";
        let identity = resolve_caller_identity(frame);
        assert_eq!(identity.presentation, IdentityPresentation::Restricted);
        assert!(identity.uri.is_none());
        assert!(identity.display_name.is_none());
    }

    #[test]
    fn asserted_identity_wins_when_presentation_is_allowed() {
        let frame = b"INVITE sip:user@example SIP/2.0\r\nP-Asserted-Identity: \"Alice\" <sip:+15551234567@example>\r\nFrom: Other <sip:other@example>;tag=a\r\n\r\n";
        let identity = resolve_caller_identity(frame);
        assert_eq!(identity.presentation, IdentityPresentation::Allowed);
        assert_eq!(identity.source, CallerIdentitySource::AssertedIdentity);
        assert_eq!(identity.uri.as_deref(), Some("sip:+15551234567@example"));
        assert_eq!(identity.display_name.as_deref(), Some("Alice"));
    }

    #[test]
    fn mwi_subscribe_uses_registered_service_route() {
        let identity = ImsIdentity {
            private_user: "user@ims.example".to_string(),
            public_uri: "sip:user@ims.example".to_string(),
            contact_user: "user".to_string(),
            home_domain: "ims.example".to_string(),
            contact_user_phone: false,
        };
        let route = ImsRoute {
            local_addr: "192.0.2.2:5060".parse::<SocketAddr>().unwrap(),
            pcscf_addr: "192.0.2.1:5060".parse::<SocketAddr>().unwrap(),
            transport: SipTransport::Udp,
        };
        let registration = RegisteredImsContext {
            access: ImsRegistrationAccess::Volte,
            registered_at: SystemTime::now(),
            lease: RegistrationLease::from_expires(3600),
            service_route: Some("<sip:route.ims.example;lr>".to_string()),
            associated_uris: Vec::new(),
        };
        let frame = build_mwi_subscribe(
            &identity,
            &route,
            &registration,
            &SubscribeIds {
                branch: "z9hG4bKtest".to_string(),
                from_tag: "from-tag".to_string(),
                to_tag: None,
                call_id: "mwi@simadmin".to_string(),
                cseq: 1,
            },
            3600,
            "SimAdmin",
            &[],
        );
        assert_eq!(
            sip_frame::header_value(&frame, "Route").as_deref(),
            Some("<sip:route.ims.example;lr>")
        );
        assert_eq!(
            sip_frame::header_value(&frame, "Event").as_deref(),
            Some("message-summary")
        );
    }

    #[test]
    fn refer_builder_uses_registered_route_and_sanitized_target() {
        let identity = ImsIdentity {
            private_user: "user@ims.example".to_string(),
            public_uri: "sip:user@ims.example".to_string(),
            contact_user: "user".to_string(),
            home_domain: "ims.example".to_string(),
            contact_user_phone: false,
        };
        let route = ImsRoute {
            local_addr: "192.0.2.2:5060".parse::<SocketAddr>().unwrap(),
            pcscf_addr: "192.0.2.1:5060".parse::<SocketAddr>().unwrap(),
            transport: SipTransport::Udp,
        };
        let registration = RegisteredImsContext {
            access: ImsRegistrationAccess::Vowifi,
            registered_at: SystemTime::now(),
            lease: RegistrationLease::from_expires(3600),
            service_route: Some("<sip:route.ims.example;lr>".to_string()),
            associated_uris: Vec::new(),
        };
        let request = DialogReferRequest {
            request_uri: "sip:peer@ims.example",
            branch: "z9hG4bKrefer",
            from_uri: "sip:user@ims.example",
            from_tag: "local-tag",
            to_value: "<sip:peer@ims.example>;tag=remote-tag",
            call_id: "dialog@simadmin",
            cseq: 4,
            refer_to: "Transfer <sip:+15551234567@ims.example>",
            referred_by: Some("sip:user@ims.example"),
        };
        let frame = build_dialog_refer(
            &identity,
            &route,
            &registration,
            &request,
            &[SipHeader::new("P-Access-Network-Info", "3GPP-WLAN")],
        )
        .unwrap();
        assert!(sip_frame::is_request(&frame, "REFER"));
        assert_eq!(
            sip_frame::header_value(&frame, "Route").as_deref(),
            Some("<sip:route.ims.example;lr>")
        );
        assert_eq!(
            sip_frame::header_value(&frame, "Refer-To").as_deref(),
            Some("<sip:+15551234567@ims.example>")
        );
        assert_eq!(
            sip_frame::header_value(&frame, "Referred-By").as_deref(),
            Some("<sip:user@ims.example>")
        );
    }

    #[test]
    fn refer_target_rejects_header_injection_and_unknown_schemes() {
        assert_eq!(
            normalize_refer_target("sip:user@example\r\nRoute: <sip:evil>")
                .unwrap_err()
                .code(),
            "ims_refer_target_invalid"
        );
        assert_eq!(
            normalize_refer_target("https://example.test")
                .unwrap_err()
                .code(),
            "ims_refer_target_invalid"
        );
        assert_eq!(
            normalize_refer_target("<tel:+15551234567>").unwrap(),
            "tel:+15551234567"
        );
    }

    #[test]
    fn refer_notify_drives_monotonic_transfer_state() {
        let notify = b"NOTIFY sip:user@example SIP/2.0\r\nEvent: refer;id=1\r\nSubscription-State: active;expires=60\r\nContent-Type: message/sipfrag;version=2.0\r\nContent-Length: 21\r\n\r\nSIP/2.0 180 Ringing\r\n";
        let progress = parse_refer_notify(notify).unwrap();
        assert_eq!(progress.sip_status, 180);
        assert_eq!(progress.transfer_state, DialogTransferState::Trying);
        assert_eq!(progress.event_id, Some(1));

        let mut transfer = DialogTransfer::for_refer_cseq(1);
        transfer.on_refer_response(202).unwrap();
        transfer.on_notify(&progress).unwrap();
        assert_eq!(transfer.state(), DialogTransferState::Trying);

        let final_notify = b"NOTIFY sip:user@example SIP/2.0\r\nEvent: refer\r\nSubscription-State: terminated;reason=noresource\r\nContent-Type: message/sipfrag\r\nContent-Length: 14\r\n\r\nSIP/2.0 200\r\n";
        let final_progress = parse_refer_notify(final_notify).unwrap();
        transfer.on_notify(&final_progress).unwrap();
        assert_eq!(transfer.state(), DialogTransferState::Succeeded);
        assert_eq!(
            transfer.on_notify(&final_progress).unwrap_err().code(),
            "ims_refer_notify_after_terminal"
        );
    }

    #[test]
    fn refer_notify_rejects_malformed_or_wrong_event_id() {
        let malformed = b"NOTIFY sip:user@example SIP/2.0\r\nEvent: refer;id=not-a-cseq\r\nSubscription-State: active\r\nContent-Type: message/sipfrag\r\nContent-Length: 18\r\n\r\nSIP/2.0 100 Trying\r\n";
        assert_eq!(
            parse_refer_notify(malformed).unwrap_err().code(),
            "ims_refer_notify_event_id_invalid"
        );

        let wrong = b"NOTIFY sip:user@example SIP/2.0\r\nEvent: refer;id=8\r\nSubscription-State: active\r\nContent-Type: message/sipfrag\r\nContent-Length: 21\r\n\r\nSIP/2.0 180 Ringing\r\n";
        let notification = parse_refer_notify(wrong).unwrap();
        let mut transfer = DialogTransfer::for_refer_cseq(7);
        transfer.on_refer_response(202).unwrap();
        assert_eq!(
            transfer.on_notify(&notification).unwrap_err().code(),
            "ims_refer_notify_event_id_mismatch"
        );
        assert_eq!(transfer.state(), DialogTransferState::Accepted);
    }

    #[test]
    fn terminated_refer_notify_requires_final_sipfrag() {
        let notify = b"NOTIFY sip:user@example SIP/2.0\r\nEvent: refer\r\nSubscription-State: terminated\r\nContent-Type: application/sipfrag\r\nContent-Length: 18\r\n\r\nSIP/2.0 100 Trying\r\n";
        assert_eq!(
            parse_refer_notify(notify).unwrap_err().code(),
            "ims_refer_terminated_without_final_status"
        );
    }

    #[test]
    fn parses_message_summary_notify_with_parameters() {
        let body = b"Messages-Waiting: no\r\nVoice-Message: 0/3\r\n";
        let frame = format!(
            "NOTIFY sip:user@example SIP/2.0\r\nEvent: message-summary;id=voice\r\nContent-Type: application/simple-message-summary; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body)
        );
        let summary = parse_mwi_notify(frame.as_bytes()).unwrap();
        assert!(!summary.messages_waiting);
        assert_eq!(summary.voice.unwrap().old, 3);
    }

    #[test]
    fn rejects_unrelated_notify_event() {
        let frame = b"NOTIFY sip:user@example SIP/2.0\r\nEvent: refer\r\nContent-Type: application/simple-message-summary\r\nContent-Length: 21\r\n\r\nMessages-Waiting: no\r\n";
        assert_eq!(
            parse_mwi_notify(frame).unwrap_err().code(),
            "ims_mwi_notify_event_invalid"
        );
    }

    #[test]
    fn classifies_subscribe_response_and_notify_dialog() {
        let response = b"SIP/2.0 200 OK\r\nTo: <sip:user@example>;tag=network\r\nCall-ID: mwi@simadmin\r\nCSeq: 1 SUBSCRIBE\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(
            classify_mwi_frame(response, Some("mwi@simadmin")),
            MwiIncomingFrame::SubscribeResponse {
                status: Ok(200),
                to_tag: Some("network".to_string()),
            }
        );

        let notify = b"NOTIFY sip:user@example SIP/2.0\r\nEvent: message-summary\r\nContent-Type: application/simple-message-summary\r\nCall-ID: other@simadmin\r\nContent-Length: 23\r\n\r\nMessages-Waiting: no\r\n";
        assert_eq!(
            classify_mwi_frame(notify, Some("mwi@simadmin")),
            MwiIncomingFrame::Notify {
                response_status: 481,
                summary: None,
            }
        );
    }
}
