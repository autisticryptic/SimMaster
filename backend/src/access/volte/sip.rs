//! VoLTE SIP message construction + response parsing.
//!
//! Clean-room implementation from RFC 3261 (SIP) and 3GPP TS 24.229/24.341.
//! The header set and 3GPP feature tags are reproduced to interoperate with a
//! real operator P-CSCF; they are public-spec constants, not copied source.
//!
//! Transport note: VoLTE carries SIP over the IMS APN bearer. When IPsec is
//! established the signaling rides the kernel xfrm SAs; on fallback it is plain
//! UDP. Either way the wire format built here is identical — only the socket
//! underneath differs. This mirrors the reference behavior where the same
//! `MESSAGE`/`REGISTER` builders are used for both `register_ipsec` and
//! `register_udp` modes.

use std::net::IpAddr;

use super::errors::VolteError;
use crate::ims::sip_message::{SipHeader, SipRequest};

pub use crate::ims::context::{ImsIdentity, ImsRegisterParams, ImsRoute as SipRoute, SipTransport};

/// 3GPP SMS-over-IP ICSI service identifier (TS 24.341).
pub const SMS_ICSI: &str = "urn:urn-7:3gpp-service.ims.icsi.sms";
/// 3GPP MMTel (voice) ICSI service identifier (TS 24.173). URL-encoded form is
/// used inside `+g.3gpp.icsi-ref` feature tags.
pub const MMTEL_ICSI: &str = "urn:urn-7:3gpp-service.ims.icsi.mmtel";
pub const MMTEL_ICSI_REF: &str = "urn%3Aurn-7%3A3gpp-service.ims.icsi.mmtel";
/// Access network info value observed for the LTE (E-UTRAN) path.
pub const PANI_EUTRAN: &str = "3GPP-E-UTRAN-FDD";
pub const USER_AGENT: &str = "SimAdmin VoLTE";
pub const SMS_CONTENT_TYPE: &str = "application/vnd.3gpp.sms";
pub const DTMF_RELAY_CONTENT_TYPE: &str = "application/dtmf-relay";

/// Format a host for a SIP URI: bare IPv4, bracketed IPv6 (RFC 3261 §19.1.2).
/// Delegates to the shared IMS core.
pub fn sip_host(ip: IpAddr) -> String {
    crate::ims::sip_frame::sip_host(ip)
}

/// Escape a quoted SIP header parameter value. Delegates to the shared IMS core.
pub fn quote_sip_param(value: &str) -> String {
    crate::ims::sip_frame::quote_param(value)
}

/// Random lowercase-hex token of `bytes` bytes (result string len = 2*bytes).
/// Falls back to a fixed token if the RNG fails, matching the reference's
/// defensive posture (a failed RNG must not panic the signaling path).
pub fn hex_token(bytes: usize) -> String {
    match random_bytes(bytes) {
        Ok(b) => b.iter().map(|byte| format!("{byte:02x}")).collect(),
        Err(_) => "simadmin".to_string(),
    }
}

fn random_bytes(len: usize) -> Result<Vec<u8>, VolteError> {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut buf = vec![0u8; len];
    SystemRandom::new()
        .fill(&mut buf)
        .map_err(|_| VolteError::new("volte_random_failed"))?;
    Ok(buf)
}

/// A branch parameter with the RFC 3261 magic cookie.
pub fn new_branch() -> String {
    format!("z9hG4bK{}", hex_token(12))
}

/// Builder inputs shared across request types within one dialog/transaction.
#[derive(Debug, Clone)]
pub struct RequestIds {
    pub call_id: String,
    pub from_tag: String,
    pub cseq: u32,
}

impl RequestIds {
    pub fn fresh(cseq: u32) -> Self {
        Self {
            call_id: format!("{}@simadmin", hex_token(16)),
            from_tag: hex_token(8),
            cseq,
        }
    }
}

/// Build a REGISTER request (initial or authenticated).
///
/// `authorization` is the full `Authorization: Digest ...` header line without
/// trailing CRLF, or `None` for the initial empty-AKA register (which the
/// caller can supply via `initial_authorization` instead).
#[allow(clippy::too_many_arguments)]
pub fn build_register(
    identity: &ImsIdentity,
    route: &SipRoute,
    ids: &RequestIds,
    expires: u32,
    authorization: Option<&str>,
    security_client: Option<&str>,
    security_verify: Option<&str>,
    sip_instance: &str,
) -> Vec<u8> {
    build_register_with_security_policy(
        identity,
        route,
        ids,
        expires,
        authorization,
        security_client,
        security_verify,
        sip_instance,
        true,
    )
}

/// REGISTER builder with an explicit sec-agree policy. The normal VoLTE path
/// requires IPsec; the `false` form is reserved for the documented UDP
/// degradation path when the network omits Security-Server entirely.
#[allow(clippy::too_many_arguments)]
pub fn build_register_with_security_policy(
    identity: &ImsIdentity,
    route: &SipRoute,
    ids: &RequestIds,
    expires: u32,
    authorization: Option<&str>,
    security_client: Option<&str>,
    security_verify: Option<&str>,
    sip_instance: &str,
    require_sec_agree: bool,
) -> Vec<u8> {
    let branch = new_branch();
    let local_host = sip_host(route.local_addr.ip());
    let local_port = route.local_addr.port();
    let params = ImsRegisterParams {
        realm: identity.home_domain.clone(),
        domain: identity.home_domain.clone(),
        registrar: None,
        supported_header: if require_sec_agree {
            "path, gruu, sec-agree"
        } else {
            "path, gruu"
        }
        .to_string(),
        require_sec_agree,
        user_agent: USER_AGENT.to_string(),
        pani: Some(PANI_EUTRAN.to_string()),
        visited_network: None,
        allow_header: "INVITE,ACK,CANCEL,BYE,UPDATE,PRACK,MESSAGE,REFER,NOTIFY,INFO,OPTIONS"
            .to_string(),
        expires,
    };
    let request_uri = params.request_uri();
    let to_value = format!("<{}>", identity.public_uri);
    let mut headers = Vec::new();
    if let Some(auth) = authorization {
        if let Some((name, value)) = auth.split_once(':') {
            headers.push(SipHeader::new(name.trim(), value.trim()));
        }
    }
    // Contact with 3GPP access type + smsip feature tag + sip.instance.
    headers.push(SipHeader::new(
        "Contact",
        format!(
        "<sip:{}@{}:{};transport={}>;+g.3gpp.accesstype=\"{}\";+g.3gpp.smsip;+sip.instance=\"<{}>\";expires={}",
        identity.contact_user,
        local_host,
        local_port,
        route.transport.as_param(),
        PANI_EUTRAN,
        sip_instance,
        expires,
    ),
    ));
    headers.push(SipHeader::new("Expires", expires.to_string()));
    headers.push(SipHeader::new("Supported", &params.supported_header));
    if require_sec_agree {
        headers.push(SipHeader::new("Require", "sec-agree"));
        headers.push(SipHeader::new("Proxy-Require", "sec-agree"));
    }
    headers.push(SipHeader::new("Allow", &params.allow_header));
    headers.push(SipHeader::new(
        "P-Preferred-Identity",
        format!("<{}>", identity.public_uri),
    ));
    headers.push(SipHeader::new(
        "P-Access-Network-Info",
        params.pani.as_deref().unwrap_or(PANI_EUTRAN),
    ));
    if let Some(sc) = security_client {
        headers.push(SipHeader::new("Security-Client", sc));
    }
    if let Some(sv) = security_verify {
        headers.push(SipHeader::new("Security-Verify", sv));
    }
    headers.push(SipHeader::new("User-Agent", &params.user_agent));
    crate::ims::sip_message::build_register(&SipRequest {
        method: "REGISTER",
        request_uri: &request_uri,
        route: *route,
        branch: &branch,
        from_uri: &identity.public_uri,
        from_tag: &ids.from_tag,
        to_value: &to_value,
        call_id: &ids.call_id,
        cseq: ids.cseq,
        headers: &headers,
        body: &[],
    })
}

/// Build a SIP MESSAGE carrying a 3GPP SMS RPDU body (MO submit).
#[allow(clippy::too_many_arguments)]
pub fn build_sms_message(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    request_uri: &str,
    to_uri: &str,
    body: &[u8],
    security_verify: Option<&str>,
) -> Vec<u8> {
    let branch = new_branch();
    let route_host = sip_host(route.pcscf_addr.ip());
    let call_id = format!("{}@simadmin", hex_token(16));
    let from_tag = hex_token(8);
    let to_value = format!("<{to_uri}>");
    let route_value = service_route
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("<sip:{route_host}:{};lr>", route.pcscf_addr.port()));
    let mut headers = vec![
        SipHeader::new("Route", route_value),
        SipHeader::new("P-Preferred-Identity", format!("<{}>", identity.public_uri)),
        SipHeader::new("P-Access-Network-Info", PANI_EUTRAN),
        SipHeader::new("P-Preferred-Service", SMS_ICSI),
    ];
    if let Some(sv) = security_verify {
        headers.push(SipHeader::new("Security-Verify", sv));
    }
    headers.push(SipHeader::new("Accept-Contact", "*;+g.3gpp.smsip"));
    headers.push(SipHeader::new("Accept", SMS_CONTENT_TYPE));
    headers.push(SipHeader::new("User-Agent", USER_AGENT));
    headers.push(SipHeader::new("Content-Type", SMS_CONTENT_TYPE));
    crate::ims::sip_message::build_message(&SipRequest {
        method: "MESSAGE",
        request_uri,
        route: *route,
        branch: &branch,
        from_uri: &identity.public_uri,
        from_tag: &from_tag,
        to_value: &to_value,
        call_id: &call_id,
        cseq: 1,
        headers: &headers,
        body,
    })
}

/// Build the RP-ACK MESSAGE sent back to the network for a received MT SMS.
/// The Request-URI/To are taken from the inbound MESSAGE `From` header.
#[allow(clippy::too_many_arguments)]
pub fn build_rp_ack(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    inbound_frame: &[u8],
    body: &[u8],
    fallback_uri: &str,
    security_verify: Option<&str>,
) -> Vec<u8> {
    let request_uri =
        sip_header_uri(inbound_frame, "From").unwrap_or_else(|| fallback_uri.to_string());
    build_sms_message(
        identity,
        route,
        service_route,
        &request_uri,
        &request_uri,
        body,
        security_verify,
    )
}

// ============================ Voice (INVITE dialog) ============================

/// Dialog identifiers for a voice call. Unlike the fire-and-forget MESSAGE
/// transaction, an INVITE dialog must keep a stable Call-ID + local tag across
/// INVITE/ACK/BYE, and learn the remote tag from the answer's To header.
#[derive(Debug, Clone)]
pub struct DialogIds {
    pub call_id: String,
    pub local_tag: String,
    pub remote_tag: Option<String>,
    pub cseq: u32,
}

impl DialogIds {
    /// Fresh dialog for a mobile-originated call.
    pub fn fresh() -> Self {
        Self {
            call_id: format!("{}@simadmin", hex_token(16)),
            local_tag: hex_token(8),
            remote_tag: None,
            cseq: 1,
        }
    }

    /// Record the remote (To) tag learned from a 2xx/early dialog response.
    pub fn set_remote_tag(&mut self, tag: impl Into<String>) {
        self.remote_tag = Some(tag.into());
    }
}

/// Build a SIP INVITE carrying an SDP audio offer (MO voice call).
///
/// `contact_host`/`contact_port` are where the peer should route in-dialog
/// requests (the device's signaling address). The SDP body is produced by the
/// shared voice layer (`build_mo_audio_offer_with_params(...).to_sdp()`).
#[allow(clippy::too_many_arguments)]
pub fn build_invite(
    identity: &ImsIdentity,
    route: &SipRoute,
    dialog: &DialogIds,
    callee_uri: &str,
    sdp_offer: &[u8],
    security_verify: Option<&str>,
) -> Vec<u8> {
    let branch = new_branch();
    let local_host = sip_host(route.local_addr.ip());
    let local_port = route.local_addr.port();
    let route_host = sip_host(route.pcscf_addr.ip());
    let to_value = format!("<{callee_uri}>");
    let mut headers = vec![
        SipHeader::new(
            "Route",
            format!("<sip:{route_host}:{};lr>", route.pcscf_addr.port()),
        ),
        SipHeader::new(
            "Contact",
            format!(
                "<sip:{}@{}:{};transport={}>;+g.3gpp.icsi-ref=\"{}\"",
                identity.contact_user,
                local_host,
                local_port,
                route.transport.as_param(),
                MMTEL_ICSI_REF,
            ),
        ),
        SipHeader::new("P-Preferred-Identity", format!("<{}>", identity.public_uri)),
        SipHeader::new("P-Access-Network-Info", PANI_EUTRAN),
        SipHeader::new("P-Preferred-Service", MMTEL_ICSI),
        SipHeader::new(
            "Accept-Contact",
            format!("*;+g.3gpp.icsi-ref=\"{MMTEL_ICSI_REF}\""),
        ),
        SipHeader::new(
            "Allow",
            "INVITE,ACK,CANCEL,BYE,UPDATE,PRACK,MESSAGE,REFER,NOTIFY,INFO,OPTIONS",
        ),
        SipHeader::new("Supported", "100rel, precondition"),
    ];
    if let Some(sv) = security_verify {
        headers.push(SipHeader::new("Security-Verify", sv));
    }
    headers.push(SipHeader::new("User-Agent", USER_AGENT));
    headers.push(SipHeader::new("Content-Type", "application/sdp"));
    crate::ims::sip_message::build_invite(&SipRequest {
        method: "INVITE",
        request_uri: callee_uri,
        route: *route,
        branch: &branch,
        from_uri: &identity.public_uri,
        from_tag: &dialog.local_tag,
        to_value: &to_value,
        call_id: &dialog.call_id,
        cseq: dialog.cseq,
        headers: &headers,
        body: sdp_offer,
    })
}

/// Build an in-dialog **re-INVITE** to renegotiate media on an *already
/// established* call — this is how VoLTE⇄ViLTE switching works (add or drop the
/// `m=video` section mid-call), exactly like "turn video on/off during a call"
/// on a smartphone.
///
/// Differences from the initial [`build_invite`] (RFC 3261 §14):
///   - It is sent **within the confirmed dialog**, so `To` carries the learned
///     `remote_tag` (a re-INVITE is not a dialog-creating request).
///   - `CSeq` is the next value in the dialog (the caller bumps `dialog.cseq`
///     before calling; we assert the remote tag is present).
///   - The SDP offer is the *new* media description (audio-only to downgrade
///     ViLTE→VoLTE, or audio+video to upgrade VoLTE→ViLTE).
///
/// The far end answers with a 200 OK (new SDP answer), which the caller ACKs via
/// [`build_ack`]. If the peer rejects the change it returns e.g. 488 Not
/// Acceptable Here and the *existing* media continues unchanged.
#[allow(clippy::too_many_arguments)]
pub fn build_reinvite(
    identity: &ImsIdentity,
    route: &SipRoute,
    dialog: &DialogIds,
    callee_uri: &str,
    sdp_offer: &[u8],
    security_verify: Option<&str>,
) -> Vec<u8> {
    let branch = new_branch();
    let local_host = sip_host(route.local_addr.ip());
    let local_port = route.local_addr.port();
    let route_host = sip_host(route.pcscf_addr.ip());
    // In-dialog: To MUST carry the remote tag. If it is somehow absent we still
    // emit a tagless To (degrades to initial-INVITE semantics) rather than panic.
    let to = match &dialog.remote_tag {
        Some(tag) => format!("<{callee_uri}>;tag={tag}"),
        None => format!("<{callee_uri}>"),
    };
    let mut headers = vec![
        SipHeader::new(
            "Route",
            format!("<sip:{route_host}:{};lr>", route.pcscf_addr.port()),
        ),
        SipHeader::new(
            "Contact",
            format!(
                "<sip:{}@{}:{};transport={}>;+g.3gpp.icsi-ref=\"{}\"",
                identity.contact_user,
                local_host,
                local_port,
                route.transport.as_param(),
                MMTEL_ICSI_REF,
            ),
        ),
        SipHeader::new("P-Preferred-Identity", format!("<{}>", identity.public_uri)),
        SipHeader::new("P-Access-Network-Info", PANI_EUTRAN),
        SipHeader::new("P-Preferred-Service", MMTEL_ICSI),
        SipHeader::new(
            "Accept-Contact",
            format!("*;+g.3gpp.icsi-ref=\"{MMTEL_ICSI_REF}\""),
        ),
        SipHeader::new(
            "Allow",
            "INVITE,ACK,CANCEL,BYE,UPDATE,PRACK,MESSAGE,REFER,NOTIFY,INFO,OPTIONS",
        ),
        SipHeader::new("Supported", "100rel, precondition"),
    ];
    if let Some(sv) = security_verify {
        headers.push(SipHeader::new("Security-Verify", sv));
    }
    headers.push(SipHeader::new("User-Agent", USER_AGENT));
    headers.push(SipHeader::new("Content-Type", "application/sdp"));
    crate::ims::sip_message::build_invite(&SipRequest {
        method: "INVITE",
        request_uri: callee_uri,
        route: *route,
        branch: &branch,
        from_uri: &identity.public_uri,
        from_tag: &dialog.local_tag,
        to_value: &to,
        call_id: &dialog.call_id,
        cseq: dialog.cseq,
        headers: &headers,
        body: sdp_offer,
    })
}

/// Build the ACK for a 2xx INVITE response (confirms the dialog). Uses the
/// remote tag learned from the 200 OK To header. Per RFC 3261 the ACK for a 2xx
/// is a separate transaction and carries the same CSeq number as the INVITE.
pub fn build_ack(
    identity: &ImsIdentity,
    route: &SipRoute,
    dialog: &DialogIds,
    callee_uri: &str,
) -> Vec<u8> {
    let branch = new_branch();
    let route_host = sip_host(route.pcscf_addr.ip());
    let to = match &dialog.remote_tag {
        Some(tag) => format!("<{callee_uri}>;tag={tag}"),
        None => format!("<{callee_uri}>"),
    };
    let headers = [
        SipHeader::new(
            "Route",
            format!("<sip:{route_host}:{};lr>", route.pcscf_addr.port()),
        ),
        SipHeader::new("User-Agent", USER_AGENT),
    ];
    crate::ims::sip_message::build_ack(&SipRequest {
        method: "ACK",
        request_uri: callee_uri,
        route: *route,
        branch: &branch,
        from_uri: &identity.public_uri,
        from_tag: &dialog.local_tag,
        to_value: &to,
        call_id: &dialog.call_id,
        cseq: dialog.cseq,
        headers: &headers,
        body: &[],
    })
}

/// Build PRACK for a reliable provisional response (RFC 3262). The dialog must
/// already contain the To-tag learned from the 18x response. `cseq` is the next
/// local in-dialog sequence while `invite_cseq` remains the INVITE sequence
/// referenced by RAck.
#[allow(clippy::too_many_arguments)]
pub fn build_prack(
    identity: &ImsIdentity,
    route: &SipRoute,
    dialog: &DialogIds,
    callee_uri: &str,
    cseq: u32,
    rseq: u32,
    invite_cseq: u32,
) -> Vec<u8> {
    let branch = new_branch();
    let route_host = sip_host(route.pcscf_addr.ip());
    let to = match &dialog.remote_tag {
        Some(tag) => format!("<{callee_uri}>;tag={tag}"),
        None => format!("<{callee_uri}>"),
    };
    let headers = [
        SipHeader::new(
            "Route",
            format!("<sip:{route_host}:{};lr>", route.pcscf_addr.port()),
        ),
        SipHeader::new("RAck", format!("{rseq} {invite_cseq} INVITE")),
        SipHeader::new("User-Agent", USER_AGENT),
    ];
    crate::ims::sip_message::build_request(&SipRequest {
        method: "PRACK",
        request_uri: callee_uri,
        route: *route,
        branch: &branch,
        from_uri: &identity.public_uri,
        from_tag: &dialog.local_tag,
        to_value: &to,
        call_id: &dialog.call_id,
        cseq,
        headers: &headers,
        body: &[],
    })
}

/// Build a BYE to tear down a confirmed dialog. CSeq must be incremented past
/// the INVITE (the caller passes the next value).
pub fn build_bye(
    identity: &ImsIdentity,
    route: &SipRoute,
    dialog: &DialogIds,
    callee_uri: &str,
    cseq: u32,
) -> Vec<u8> {
    let branch = new_branch();
    let local_host = sip_host(route.local_addr.ip());
    let local_port = route.local_addr.port();
    let route_host = sip_host(route.pcscf_addr.ip());
    let to = match &dialog.remote_tag {
        Some(tag) => format!("<{callee_uri}>;tag={tag}"),
        None => format!("<{callee_uri}>"),
    };

    let mut h = String::new();
    h.push_str(&format!("BYE {callee_uri} SIP/2.0\r\n"));
    h.push_str(&format!(
        "Via: {} {local_host}:{local_port};branch={branch};rport\r\n",
        route.transport.as_via()
    ));
    h.push_str("Max-Forwards: 70\r\n");
    h.push_str(&format!(
        "Route: <sip:{route_host}:{};lr>\r\n",
        route.pcscf_addr.port()
    ));
    h.push_str(&format!(
        "From: <{}>;tag={}\r\n",
        identity.public_uri, dialog.local_tag
    ));
    h.push_str(&format!("To: {to}\r\n"));
    h.push_str(&format!("Call-ID: {}\r\n", dialog.call_id));
    h.push_str(&format!("CSeq: {cseq} BYE\r\n"));
    h.push_str(&format!("User-Agent: {USER_AGENT}\r\n"));
    h.push_str("Content-Length: 0\r\n\r\n");
    h.into_bytes()
}

/// Build an in-dialog SIP INFO carrying one DTMF digit. This is the signaling
/// fallback when the operator dialog did not negotiate RFC 4733
/// `telephone-event`, or when Asterisk explicitly delivered DTMF via INFO.
/// RTP telephone-event remains preferred because it stays on the media path.
pub fn build_dtmf_info(
    identity: &ImsIdentity,
    route: &SipRoute,
    dialog: &DialogIds,
    callee_uri: &str,
    cseq: u32,
    digit: char,
    duration_ms: u16,
) -> Result<Vec<u8>, VolteError> {
    let digit = digit.to_ascii_uppercase();
    if !matches!(digit, '0'..='9' | '*' | '#' | 'A'..='D') {
        return Err(VolteError::new("volte_dtmf_digit_invalid"));
    }
    if !(40..=5000).contains(&duration_ms) {
        return Err(VolteError::new("volte_dtmf_duration_invalid"));
    }
    let branch = new_branch();
    let local_host = sip_host(route.local_addr.ip());
    let local_port = route.local_addr.port();
    let route_host = sip_host(route.pcscf_addr.ip());
    let to = match &dialog.remote_tag {
        Some(tag) => format!("<{callee_uri}>;tag={tag}"),
        None => format!("<{callee_uri}>"),
    };
    let body = format!("Signal={digit}\r\nDuration={duration_ms}\r\n");
    let mut h = String::new();
    h.push_str(&format!("INFO {callee_uri} SIP/2.0\r\n"));
    h.push_str(&format!(
        "Via: {} {local_host}:{local_port};branch={branch};rport\r\n",
        route.transport.as_via()
    ));
    h.push_str("Max-Forwards: 70\r\n");
    h.push_str(&format!(
        "Route: <sip:{route_host}:{};lr>\r\n",
        route.pcscf_addr.port()
    ));
    h.push_str(&format!(
        "From: <{}>;tag={}\r\n",
        identity.public_uri, dialog.local_tag
    ));
    h.push_str(&format!("To: {to}\r\n"));
    h.push_str(&format!("Call-ID: {}\r\n", dialog.call_id));
    h.push_str(&format!("CSeq: {cseq} INFO\r\n"));
    h.push_str(&format!("P-Access-Network-Info: {PANI_EUTRAN}\r\n"));
    h.push_str(&format!("User-Agent: {USER_AGENT}\r\n"));
    h.push_str(&format!("Content-Type: {DTMF_RELAY_CONTENT_TYPE}\r\n"));
    h.push_str("Content-Disposition: signal;handling=optional\r\n");
    h.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    h.push_str(&body);
    Ok(h.into_bytes())
}

/// Build a CANCEL for a not-yet-answered INVITE. Per RFC 3261 the CANCEL copies
/// the INVITE's Call-ID/From/To/CSeq-number (method CANCEL) and top Via branch.
pub fn build_cancel(
    identity: &ImsIdentity,
    route: &SipRoute,
    dialog: &DialogIds,
    callee_uri: &str,
    invite_branch: &str,
) -> Vec<u8> {
    let local_host = sip_host(route.local_addr.ip());
    let local_port = route.local_addr.port();
    let route_host = sip_host(route.pcscf_addr.ip());

    let mut h = String::new();
    h.push_str(&format!("CANCEL {callee_uri} SIP/2.0\r\n"));
    // CANCEL MUST carry the same top Via branch as the INVITE it cancels.
    h.push_str(&format!(
        "Via: {} {local_host}:{local_port};branch={invite_branch};rport\r\n",
        route.transport.as_via()
    ));
    h.push_str("Max-Forwards: 70\r\n");
    h.push_str(&format!(
        "Route: <sip:{route_host}:{};lr>\r\n",
        route.pcscf_addr.port()
    ));
    h.push_str(&format!(
        "From: <{}>;tag={}\r\n",
        identity.public_uri, dialog.local_tag
    ));
    h.push_str(&format!("To: <{callee_uri}>\r\n"));
    h.push_str(&format!("Call-ID: {}\r\n", dialog.call_id));
    h.push_str(&format!("CSeq: {} CANCEL\r\n", dialog.cseq));
    h.push_str(&format!("User-Agent: {USER_AGENT}\r\n"));
    h.push_str("Content-Length: 0\r\n\r\n");
    h.into_bytes()
}

/// Build a response to an inbound request (e.g. 200 OK to an MT INVITE, or 486
/// Busy Here). Echoes Via/From/To/Call-ID/CSeq from the request per RFC 3261,
/// adds our tag to To, and optionally attaches an SDP answer body.
#[allow(clippy::too_many_arguments)]
pub fn build_response(
    request: &[u8],
    status: u16,
    reason: &str,
    local_tag: Option<&str>,
    contact: Option<&str>,
    sdp_answer: Option<&[u8]>,
) -> Vec<u8> {
    let mut h = String::new();
    h.push_str(&format!("SIP/2.0 {status} {reason}\r\n"));
    // Echo all Via headers in order (RFC 3261 §8.2.6.2).
    for via in header_values(request, "Via") {
        h.push_str(&format!("Via: {via}\r\n"));
    }
    for record_route in header_values(request, "Record-Route") {
        h.push_str(&format!("Record-Route: {record_route}\r\n"));
    }
    if let Some(from) = header_value(request, "From") {
        h.push_str(&format!("From: {from}\r\n"));
    }
    // Add our tag to To if the request's To has none and we supply one.
    if let Some(to) = header_value(request, "To") {
        if let Some(tag) = local_tag {
            if !to.contains(";tag=") {
                h.push_str(&format!("To: {to};tag={tag}\r\n"));
            } else {
                h.push_str(&format!("To: {to}\r\n"));
            }
        } else {
            h.push_str(&format!("To: {to}\r\n"));
        }
    }
    if let Some(call_id) = header_value(request, "Call-ID") {
        h.push_str(&format!("Call-ID: {call_id}\r\n"));
    }
    if let Some(cseq) = header_value(request, "CSeq") {
        h.push_str(&format!("CSeq: {cseq}\r\n"));
    }
    if let Some(contact) = contact {
        h.push_str(&format!("Contact: <{contact}>\r\n"));
    }
    h.push_str(&format!("User-Agent: {USER_AGENT}\r\n"));
    match sdp_answer {
        Some(body) => {
            h.push_str("Content-Type: application/sdp\r\n");
            h.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
            let mut frame = h.into_bytes();
            frame.extend_from_slice(body);
            frame
        }
        None => {
            h.push_str("Content-Length: 0\r\n\r\n");
            h.into_bytes()
        }
    }
}

// SIP framing/parsing primitives are shared with every IMS leg — they live in
// `crate::ims::sip_frame` (single implementation). The wrappers below keep the
// volte-facing names/signatures (e.g. `parse_status` returning `VolteError`) so
// existing volte call sites are unchanged.

/// Parse the SIP status code (delegates to shared framing; remaps the error).
pub fn parse_status(frame: &[u8]) -> Result<u16, VolteError> {
    crate::ims::sip_frame::parse_status(frame)
        .map_err(|_| VolteError::new("volte_sip_status_invalid"))
}

/// Everything after the header terminator (may be empty).
pub fn sip_body(frame: &[u8]) -> &[u8] {
    crate::ims::sip_frame::body(frame)
}

/// TCP de-coalescing: exact byte length of one complete SIP message, or None.
pub fn complete_frame_len(buf: &[u8]) -> Option<usize> {
    crate::ims::sip_frame::complete_frame_len(buf)
}

pub fn is_complete(buf: &[u8]) -> bool {
    crate::ims::sip_frame::is_complete(buf)
}

/// Whether a frame is a SIP request for the given method (start line check).
pub fn is_request(frame: &[u8], method: &str) -> bool {
    crate::ims::sip_frame::is_request(frame, method)
}

/// Collect all values of a header (case-insensitive name, first-colon split).
pub fn header_values(frame: &[u8], header_name: &str) -> Vec<String> {
    crate::ims::sip_frame::header_values(frame, header_name)
}

/// First value of a header, if present.
pub fn header_value(frame: &[u8], header_name: &str) -> Option<String> {
    crate::ims::sip_frame::header_value(frame, header_name)
}

/// Extract the bracketed `<sip:...>` URI from a named header value.
pub fn sip_header_uri(frame: &[u8], header_name: &str) -> Option<String> {
    crate::ims::sip_frame::header_uri(frame, header_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

    fn ident() -> ImsIdentity {
        ImsIdentity {
            private_user: "460001234567890@ims.mnc000.mcc460.3gppnetwork.org".to_string(),
            public_uri: "sip:460001234567890@ims.mnc000.mcc460.3gppnetwork.org".to_string(),
            contact_user: "460001234567890".to_string(),
            home_domain: "ims.mnc000.mcc460.3gppnetwork.org".to_string(),
            contact_user_phone: false,
        }
    }

    fn route_udp() -> SipRoute {
        SipRoute {
            local_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)), 5060),
            pcscf_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 5060),
            transport: SipTransport::Udp,
        }
    }

    #[test]
    fn sip_host_brackets_ipv6_only() {
        assert_eq!(sip_host(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))), "1.2.3.4");
        assert_eq!(sip_host(IpAddr::V6(Ipv6Addr::LOCALHOST)), "[::1]");
    }

    #[test]
    fn register_contains_mandatory_headers_and_sms_feature_tag() {
        let ids = RequestIds::fresh(1);
        let frame = build_register(
            &ident(),
            &route_udp(),
            &ids,
            3600,
            None,
            Some("ipsec-3gpp; alg=hmac-md5-96; spi-c=1; spi-s=2; port-c=6000; port-s=6001"),
            None,
            "urn:uuid:00000000-0000-4000-8000-000000000000",
        );
        let text = String::from_utf8(frame).unwrap();
        assert!(text.starts_with("REGISTER sip:ims.mnc000.mcc460.3gppnetwork.org SIP/2.0\r\n"));
        assert!(text.contains("Via: SIP/2.0/UDP 10.0.0.2:5060;branch=z9hG4bK"));
        assert!(text.contains("CSeq: 1 REGISTER\r\n"));
        assert!(text.contains("+g.3gpp.smsip"));
        assert!(text.contains("Require: sec-agree\r\n"));
        assert!(text.contains("Security-Client: ipsec-3gpp"));
        assert!(text.contains("P-Access-Network-Info: 3GPP-E-UTRAN-FDD\r\n"));
        assert!(text.ends_with("Content-Length: 0\r\n\r\n"));
    }

    #[test]
    fn sms_message_appends_binary_body_after_headers() {
        let body = vec![0x01, 0x02, 0x03, 0xff];
        let frame = build_sms_message(
            &ident(),
            &route_udp(),
            Some("<sip:service-route.example:9900;lr>"),
            "sip:+8613800100500@ims.mnc000.mcc460.3gppnetwork.org",
            "sip:+8619399144749@ims.mnc000.mcc460.3gppnetwork.org",
            &body,
            None,
        );
        // Body must be preserved verbatim after CRLFCRLF.
        assert_eq!(sip_body(&frame), &body[..]);
        let text = String::from_utf8_lossy(&frame);
        assert!(text.contains("Content-Type: application/vnd.3gpp.sms\r\n"));
        assert!(text.contains("Accept: application/vnd.3gpp.sms\r\n"));
        assert!(text.contains("Route: <sip:service-route.example:9900;lr>\r\n"));
        assert!(text.starts_with(
            "MESSAGE sip:+8613800100500@ims.mnc000.mcc460.3gppnetwork.org SIP/2.0\r\n"
        ));
        assert!(text.contains("To: <sip:+8619399144749@ims.mnc000.mcc460.3gppnetwork.org>\r\n"));
        assert!(text.contains("Content-Length: 4\r\n"));
        assert!(text.contains("Accept-Contact: *;+g.3gpp.smsip\r\n"));
    }

    #[test]
    fn parse_status_reads_code() {
        assert_eq!(parse_status(b"SIP/2.0 200 OK\r\n\r\n").unwrap(), 200);
        assert_eq!(
            parse_status(b"SIP/2.0 401 Unauthorized\r\nWWW-Authenticate: Digest\r\n\r\n").unwrap(),
            401
        );
        assert!(parse_status(b"garbage").is_err());
    }

    #[test]
    fn complete_frame_len_honors_content_length_and_coalescing() {
        // Two messages coalesced in one TCP read.
        let msg1 = b"SIP/2.0 200 OK\r\nContent-Length: 3\r\n\r\nabc";
        let msg2 = b"MESSAGE sip:x SIP/2.0\r\nContent-Length: 0\r\n\r\n";
        let mut buf = Vec::new();
        buf.extend_from_slice(msg1);
        buf.extend_from_slice(msg2);
        let len1 = complete_frame_len(&buf).unwrap();
        assert_eq!(len1, msg1.len());
        // Remaining bytes form the second complete frame.
        let rest = &buf[len1..];
        assert_eq!(complete_frame_len(rest).unwrap(), msg2.len());
    }

    #[test]
    fn complete_frame_len_needs_more_when_body_truncated() {
        let partial = b"SIP/2.0 200 OK\r\nContent-Length: 10\r\n\r\nabc";
        assert!(complete_frame_len(partial).is_none());
        let no_terminator = b"SIP/2.0 200 OK\r\nContent-Length: 10\r\n";
        assert!(complete_frame_len(no_terminator).is_none());
    }

    #[test]
    fn is_request_matches_method() {
        assert!(is_request(b"MESSAGE sip:x SIP/2.0\r\n\r\n", "MESSAGE"));
        assert!(!is_request(b"SIP/2.0 200 OK\r\n\r\n", "MESSAGE"));
        assert!(!is_request(b"INVITE sip:x SIP/2.0\r\n\r\n", "MESSAGE"));
    }

    #[test]
    fn header_values_are_case_insensitive_and_multi() {
        let frame = b"SIP/2.0 200 OK\r\nVia: a\r\nvia: b\r\nContact: <sip:x@h>\r\n\r\n";
        assert_eq!(header_values(frame, "via"), vec!["a", "b"]);
        assert_eq!(header_value(frame, "CONTACT").as_deref(), Some("<sip:x@h>"));
    }

    #[test]
    fn sip_header_uri_extracts_bracketed_and_bare() {
        let frame = b"MESSAGE sip:x SIP/2.0\r\nFrom: <sip:+8613800138000@h>;tag=abc\r\n\r\n";
        assert_eq!(
            sip_header_uri(frame, "From").as_deref(),
            Some("sip:+8613800138000@h")
        );
        let bare = b"MESSAGE sip:x SIP/2.0\r\nFrom: sip:user@h;tag=abc\r\n\r\n";
        assert_eq!(sip_header_uri(bare, "From").as_deref(), Some("sip:user@h"));
    }

    #[test]
    fn rp_ack_targets_inbound_from_uri() {
        let inbound = b"MESSAGE sip:me SIP/2.0\r\nFrom: <sip:+8613800138000@h>;tag=x\r\n\r\nBODY";
        let frame = build_rp_ack(
            &ident(),
            &route_udp(),
            None,
            inbound,
            &[0x02, 0x00],
            "sip:fallback@h",
            None,
        );
        let text = String::from_utf8_lossy(&frame);
        assert!(text.starts_with("MESSAGE sip:+8613800138000@h SIP/2.0\r\n"));
        assert_eq!(sip_body(&frame), &[0x02, 0x00]);
    }

    #[test]
    fn invite_carries_sdp_and_mmtel_service_tags() {
        let dialog = DialogIds::fresh();
        let sdp = b"v=0\r\no=- 1 1 IN IP4 10.0.0.2\r\ns=SimAdmin\r\n";
        let frame = build_invite(
            &ident(),
            &route_udp(),
            &dialog,
            "sip:+8613800138000@ims.mnc000.mcc460.3gppnetwork.org",
            sdp,
            None,
        );
        let text = String::from_utf8_lossy(&frame);
        assert!(text.starts_with(
            "INVITE sip:+8613800138000@ims.mnc000.mcc460.3gppnetwork.org SIP/2.0\r\n"
        ));
        assert!(text.contains("CSeq: 1 INVITE\r\n"));
        assert!(text.contains("P-Preferred-Service: urn:urn-7:3gpp-service.ims.icsi.mmtel\r\n"));
        assert!(text.contains("Content-Type: application/sdp\r\n"));
        assert!(text.contains(&format!("Content-Length: {}\r\n", sdp.len())));
        // SDP body preserved verbatim after the header terminator.
        assert_eq!(sip_body(&frame), &sdp[..]);
    }

    #[test]
    fn ack_uses_remote_tag_and_invite_cseq() {
        let mut dialog = DialogIds::fresh();
        dialog.set_remote_tag("remotetag123");
        let frame = build_ack(&ident(), &route_udp(), &dialog, "sip:+8613800138000@h");
        let text = String::from_utf8_lossy(&frame);
        assert!(text.starts_with("ACK sip:+8613800138000@h SIP/2.0\r\n"));
        assert!(text.contains("To: <sip:+8613800138000@h>;tag=remotetag123\r\n"));
        assert!(text.contains("CSeq: 1 ACK\r\n"));
        assert!(text.ends_with("Content-Length: 0\r\n\r\n"));
    }

    #[test]
    fn prack_references_reliable_provisional_response() {
        let mut dialog = DialogIds::fresh();
        dialog.set_remote_tag("early-tag");
        let frame = build_prack(
            &ident(),
            &route_udp(),
            &dialog,
            "sip:+8613800138000@h",
            2,
            77,
            1,
        );
        let text = String::from_utf8(frame).unwrap();
        assert!(text.starts_with("PRACK sip:+8613800138000@h SIP/2.0\r\n"));
        assert!(text.contains("To: <sip:+8613800138000@h>;tag=early-tag\r\n"));
        assert!(text.contains("RAck: 77 1 INVITE\r\n"));
        assert!(text.contains("CSeq: 2 PRACK\r\n"));
    }

    #[test]
    fn bye_increments_cseq_and_targets_remote_tag() {
        let mut dialog = DialogIds::fresh();
        dialog.set_remote_tag("rt");
        let frame = build_bye(&ident(), &route_udp(), &dialog, "sip:+8613800138000@h", 2);
        let text = String::from_utf8_lossy(&frame);
        assert!(text.starts_with("BYE sip:+8613800138000@h SIP/2.0\r\n"));
        assert!(text.contains("CSeq: 2 BYE\r\n"));
        assert!(text.contains("To: <sip:+8613800138000@h>;tag=rt\r\n"));
    }

    #[test]
    fn dtmf_info_uses_confirmed_dialog_and_dtmf_relay_body() {
        let mut dialog = DialogIds::fresh();
        dialog.set_remote_tag("remote-tag");
        let frame = build_dtmf_info(
            &ident(),
            &route_udp(),
            &dialog,
            "sip:+8613800138000@h",
            3,
            '5',
            240,
        )
        .unwrap();
        let text = String::from_utf8(frame).unwrap();
        assert!(text.starts_with("INFO sip:+8613800138000@h SIP/2.0\r\n"));
        assert!(text.contains("To: <sip:+8613800138000@h>;tag=remote-tag\r\n"));
        assert!(text.contains("CSeq: 3 INFO\r\n"));
        assert!(text.contains("Content-Type: application/dtmf-relay\r\n"));
        assert!(text.ends_with("Signal=5\r\nDuration=240\r\n"));
    }

    #[test]
    fn dtmf_info_rejects_invalid_digit_and_duration() {
        let dialog = DialogIds::fresh();
        assert!(build_dtmf_info(
            &ident(),
            &route_udp(),
            &dialog,
            "sip:+8613800138000@h",
            2,
            'Z',
            160,
        )
        .is_err());
        assert!(build_dtmf_info(
            &ident(),
            &route_udp(),
            &dialog,
            "sip:+8613800138000@h",
            2,
            '1',
            10,
        )
        .is_err());
    }
}
