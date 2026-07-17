//! SIP wire-format helpers for the Asterisk-facing endpoint.

use std::net::SocketAddr;

use crate::ims::{
    context::{ImsRoute, SipTransport},
    sip_frame,
    sip_message::{SipHeader, SipRequest},
};

pub const USER_AGENT: &str = "SimAdmin Trunk/1.1.3";

#[derive(Debug, Clone)]
pub struct RegisterDialog {
    pub call_id: String,
    pub from_tag: String,
    pub cseq: u32,
}

impl RegisterDialog {
    pub fn fresh() -> Self {
        Self {
            call_id: format!("{}@simadmin", token(16)),
            from_tag: token(8),
            cseq: 1,
        }
    }

    pub fn next_cseq(&mut self) -> u32 {
        let current = self.cseq;
        self.cseq = self.cseq.saturating_add(1);
        current
    }
}

pub fn registrar_uri(host: &str, port: u16) -> String {
    let host = format_host(host);
    if port == 5060 {
        format!("sip:{host}")
    } else {
        format!("sip:{host}:{port}")
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_register(
    username: &str,
    remote_host: &str,
    remote_port: u16,
    local_addr: SocketAddr,
    dialog: &mut RegisterDialog,
    expires: u32,
    authorization: Option<&str>,
) -> Result<Vec<u8>, String> {
    validate_token(username, "trunk_username_invalid")?;
    validate_token(remote_host, "trunk_asterisk_host_invalid")?;
    let request_uri = registrar_uri(remote_host, remote_port);
    let identity_uri = format!("sip:{username}@{}", format_host(remote_host));
    let branch = format!("z9hG4bK{}", token(12));
    let route = ImsRoute {
        local_addr,
        pcscf_addr: local_addr,
        transport: SipTransport::Udp,
    };
    let contact_host = sip_frame::sip_host(local_addr.ip());
    let mut headers = vec![
        SipHeader::new(
            "Contact",
            format!(
                "<sip:{username}@{contact_host}:{};transport=udp>;expires={expires}",
                local_addr.port()
            ),
        ),
        SipHeader::new("Expires", expires.to_string()),
        SipHeader::new("Allow", "INVITE, ACK, CANCEL, BYE, INFO, OPTIONS"),
        SipHeader::new("Supported", "outbound, path"),
        SipHeader::new("User-Agent", USER_AGENT),
    ];
    if let Some(authorization) = authorization {
        let (name, value) = authorization
            .split_once(':')
            .ok_or_else(|| "trunk_digest_header_invalid".to_string())?;
        headers.push(SipHeader::new(name.trim(), value.trim()));
    }
    let cseq = dialog.next_cseq();
    Ok(crate::ims::sip_message::build_register(&SipRequest {
        method: "REGISTER",
        request_uri: &request_uri,
        route,
        branch: &branch,
        from_uri: &identity_uri,
        from_tag: &dialog.from_tag,
        to_value: &format!("<{identity_uri}>"),
        call_id: &dialog.call_id,
        cseq,
        headers: &headers,
        body: &[],
    }))
}

pub fn build_response(request: &[u8], status: u16, reason: &str) -> Result<Vec<u8>, String> {
    build_response_with_body(request, status, reason, None, &[], &[])
}

pub fn build_response_with_body(
    request: &[u8],
    status: u16,
    reason: &str,
    local_tag: Option<&str>,
    extra_headers: &[SipHeader],
    body: &[u8],
) -> Result<Vec<u8>, String> {
    let mut response = format!("SIP/2.0 {status} {reason}\r\n");
    for value in sip_frame::header_values(request, "Via") {
        response.push_str("Via: ");
        response.push_str(&value);
        response.push_str("\r\n");
    }
    for header in ["From", "To", "Call-ID", "CSeq"] {
        let mut value = sip_frame::header_value(request, header)
            .ok_or_else(|| format!("trunk_request_{}_missing", header.to_ascii_lowercase()))?;
        if header == "To" && !value.to_ascii_lowercase().contains(";tag=") {
            value.push_str(";tag=");
            value.push_str(local_tag.unwrap_or("simadmin"));
        }
        response.push_str(header);
        response.push_str(": ");
        response.push_str(&value);
        response.push_str("\r\n");
    }
    response.push_str(&format!("Server: {USER_AGENT}\r\n"));
    response.push_str("Allow: INVITE, ACK, CANCEL, BYE, INFO, OPTIONS\r\n");
    for header in extra_headers {
        response.push_str(&header.name);
        response.push_str(": ");
        response.push_str(&header.value);
        response.push_str("\r\n");
    }
    if !body.is_empty() {
        response.push_str("Content-Type: application/sdp\r\n");
    }
    response.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    let mut bytes = response.into_bytes();
    bytes.extend_from_slice(body);
    Ok(bytes)
}

#[derive(Debug, Clone)]
pub struct DialogRequest<'a> {
    pub method: &'a str,
    pub request_uri: &'a str,
    pub local_addr: SocketAddr,
    pub from_uri: &'a str,
    pub from_tag: &'a str,
    pub to_uri: &'a str,
    pub to_tag: Option<&'a str>,
    pub call_id: &'a str,
    pub cseq: u32,
    pub contact_uri: Option<&'a str>,
    pub body: &'a [u8],
}

pub fn build_dialog_request(request: &DialogRequest<'_>) -> Result<Vec<u8>, String> {
    validate_token(request.method, "trunk_dialog_method_invalid")?;
    if !request.request_uri.starts_with("sip:")
        || !request.from_uri.starts_with("sip:")
        || !request.to_uri.starts_with("sip:")
    {
        return Err("trunk_dialog_uri_invalid".to_string());
    }
    let host = sip_frame::sip_host(request.local_addr.ip());
    let branch = format!("z9hG4bK{}", token(12));
    let mut frame = format!(
        "{} {} SIP/2.0\r\nVia: SIP/2.0/UDP {}:{};branch={};rport\r\nMax-Forwards: 70\r\nFrom: <{}>;tag={}\r\nTo: <{}>{}\r\nCall-ID: {}\r\nCSeq: {} {}\r\n",
        request.method,
        request.request_uri,
        host,
        request.local_addr.port(),
        branch,
        request.from_uri,
        request.from_tag,
        request.to_uri,
        request
            .to_tag
            .map(|tag| format!(";tag={tag}"))
            .unwrap_or_default(),
        request.call_id,
        request.cseq,
        request.method,
    );
    if let Some(contact) = request.contact_uri {
        frame.push_str(&format!("Contact: <{contact}>\r\n"));
    }
    frame.push_str(&format!("User-Agent: {USER_AGENT}\r\n"));
    frame.push_str("Allow: INVITE, ACK, CANCEL, BYE, INFO, OPTIONS\r\n");
    if !request.body.is_empty() {
        frame.push_str("Content-Type: application/sdp\r\n");
    }
    frame.push_str(&format!("Content-Length: {}\r\n\r\n", request.body.len()));
    let mut bytes = frame.into_bytes();
    bytes.extend_from_slice(request.body);
    Ok(bytes)
}

#[allow(dead_code)]
pub fn build_cancel(invite: &[u8]) -> Result<Vec<u8>, String> {
    let request_uri = request_uri(invite)?;
    let via = sip_frame::header_value(invite, "Via")
        .ok_or_else(|| "trunk_request_via_missing".to_string())?;
    let from = sip_frame::header_value(invite, "From")
        .ok_or_else(|| "trunk_request_from_missing".to_string())?;
    let to = sip_frame::header_value(invite, "To")
        .ok_or_else(|| "trunk_request_to_missing".to_string())?;
    let call_id = sip_frame::header_value(invite, "Call-ID")
        .ok_or_else(|| "trunk_request_call-id_missing".to_string())?;
    let cseq = cseq_number(invite)?;
    Ok(format!(
        "CANCEL {request_uri} SIP/2.0\r\nVia: {via}\r\nMax-Forwards: 70\r\nFrom: {from}\r\nTo: {to}\r\nCall-ID: {call_id}\r\nCSeq: {cseq} CANCEL\r\nUser-Agent: {USER_AGENT}\r\nContent-Length: 0\r\n\r\n"
    )
    .into_bytes())
}

pub fn build_ack_for_final(invite: &[u8], response: &[u8]) -> Result<Vec<u8>, String> {
    let request_uri = request_uri(invite)?;
    let via = sip_frame::header_value(invite, "Via")
        .ok_or_else(|| "trunk_request_via_missing".to_string())?;
    let from = sip_frame::header_value(invite, "From")
        .ok_or_else(|| "trunk_request_from_missing".to_string())?;
    let to = sip_frame::header_value(response, "To")
        .or_else(|| sip_frame::header_value(invite, "To"))
        .ok_or_else(|| "trunk_request_to_missing".to_string())?;
    let call_id = sip_frame::header_value(invite, "Call-ID")
        .ok_or_else(|| "trunk_request_call-id_missing".to_string())?;
    let cseq = cseq_number(invite)?;
    Ok(format!(
        "ACK {request_uri} SIP/2.0\r\nVia: {via}\r\nMax-Forwards: 70\r\nFrom: {from}\r\nTo: {to}\r\nCall-ID: {call_id}\r\nCSeq: {cseq} ACK\r\nUser-Agent: {USER_AGENT}\r\nContent-Length: 0\r\n\r\n"
    )
    .into_bytes())
}

fn request_uri(frame: &[u8]) -> Result<String, String> {
    let line_end = frame
        .windows(2)
        .position(|window| window == b"\r\n")
        .ok_or_else(|| "trunk_request_line_missing".to_string())?;
    let line = std::str::from_utf8(&frame[..line_end])
        .map_err(|_| "trunk_request_line_invalid".to_string())?;
    line.split_whitespace()
        .nth(1)
        .map(str::to_string)
        .ok_or_else(|| "trunk_request_uri_invalid".to_string())
}

fn cseq_number(frame: &[u8]) -> Result<u32, String> {
    sip_frame::header_value(frame, "CSeq")
        .and_then(|value| value.split_whitespace().next()?.parse::<u32>().ok())
        .ok_or_else(|| "trunk_cseq_invalid".to_string())
}
pub fn response_expiry(frame: &[u8], fallback: u32) -> u32 {
    // RFC 3261 section 10.2.4: a Contact-level expires parameter overrides
    // the response's generic Expires value for that registration binding.
    sip_frame::header_values(frame, "Contact")
        .into_iter()
        .find_map(|value| parameter_value(&value, "expires")?.parse::<u32>().ok())
        .or_else(|| {
            sip_frame::header_value(frame, "Expires").and_then(|value| value.parse::<u32>().ok())
        })
        .unwrap_or(fallback)
}

pub fn min_expires(frame: &[u8]) -> Option<u32> {
    sip_frame::header_value(frame, "Min-Expires")?.parse().ok()
}

pub fn status(frame: &[u8]) -> Result<u16, String> {
    sip_frame::parse_status(frame).map_err(|error| error.code().to_string())
}

pub fn is_request(frame: &[u8]) -> bool {
    !frame.starts_with(b"SIP/2.0 ")
}

fn parameter_value<'a>(input: &'a str, name: &str) -> Option<&'a str> {
    input.split(';').skip(1).find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        key.eq_ignore_ascii_case(name)
            .then(|| value.trim().trim_matches('"'))
    })
}

fn validate_token(value: &str, error: &str) -> Result<(), String> {
    if value.trim().is_empty()
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(error.to_string());
    }
    Ok(())
}

fn format_host(host: &str) -> String {
    let trimmed = host.trim().trim_matches(['[', ']']);
    if trimmed.contains(':') {
        format!("[{trimmed}]")
    } else {
        trimmed.to_string()
    }
}

pub fn token(bytes: usize) -> String {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut random = vec![0u8; bytes];
    if SystemRandom::new().fill(&mut random).is_err() {
        return "simadmin".to_string();
    }
    random.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn register_contains_required_trunk_headers() {
        let mut dialog = RegisterDialog::fresh();
        let frame = build_register(
            "4101",
            "pbx.example.com",
            5060,
            SocketAddr::from((Ipv4Addr::new(10, 0, 0, 2), 5062)),
            &mut dialog,
            3600,
            None,
        )
        .unwrap();
        let text = String::from_utf8(frame).unwrap();
        assert!(text.starts_with("REGISTER sip:pbx.example.com SIP/2.0\r\n"));
        assert!(text.contains("From: <sip:4101@pbx.example.com>;tag="));
        assert!(text.contains("Contact: <sip:4101@10.0.0.2:5062;transport=udp>;expires=3600"));
        assert!(text.contains("Expires: 3600\r\n"));
    }

    #[test]
    fn registrar_uri_brackets_ipv6_literal() {
        let host = IpAddr::V6(Ipv6Addr::LOCALHOST).to_string();
        assert_eq!(registrar_uri(&host, 5070), "sip:[::1]:5070");
    }

    #[test]
    fn response_copies_transaction_headers_and_adds_tag() {
        let request = b"OPTIONS sip:4101@pbx SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1:5060;branch=z9hG4bK1\r\nFrom: <sip:pbx@local>;tag=a\r\nTo: <sip:4101@pbx>\r\nCall-ID: c\r\nCSeq: 1 OPTIONS\r\nContent-Length: 0\r\n\r\n";
        let response = String::from_utf8(build_response(request, 200, "OK").unwrap()).unwrap();
        assert!(response.starts_with("SIP/2.0 200 OK"));
        assert!(response.contains("To: <sip:4101@pbx>;tag="));
        assert!(response.contains("CSeq: 1 OPTIONS"));
    }

    #[test]
    fn response_uses_stable_tag_and_sdp_length() {
        let request = b"INVITE sip:4101@pbx SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1:5060;branch=z9hG4bK1\r\nFrom: <sip:pbx@local>;tag=a\r\nTo: <sip:4101@pbx>\r\nCall-ID: c\r\nCSeq: 1 INVITE\r\nContent-Length: 0\r\n\r\n";
        let body = b"v=0\r\n";
        let response = String::from_utf8(
            build_response_with_body(request, 200, "OK", Some("local-a"), &[], body).unwrap(),
        )
        .unwrap();
        assert!(response.contains("To: <sip:4101@pbx>;tag=local-a\r\n"));
        assert!(response.contains("Content-Type: application/sdp\r\n"));
        assert!(response.contains("Content-Length: 5\r\n\r\nv=0\r\n"));
    }

    #[test]
    fn dialog_requests_include_tags_cseq_and_body() {
        let frame = build_dialog_request(&DialogRequest {
            method: "INVITE",
            request_uri: "sip:41000@pbx",
            local_addr: SocketAddr::from((Ipv4Addr::new(10, 0, 0, 2), 5062)),
            from_uri: "sip:+8613800@simadmin",
            from_tag: "local-a",
            to_uri: "sip:6108@pbx",
            to_tag: None,
            call_id: "call-a@simadmin",
            cseq: 7,
            contact_uri: Some("sip:41000@10.0.0.2:5062"),
            body: b"v=0\r\n",
        })
        .unwrap();
        let text = String::from_utf8(frame).unwrap();
        assert!(text.starts_with("INVITE sip:41000@pbx SIP/2.0\r\n"));
        assert!(text.contains("CSeq: 7 INVITE\r\n"));
        assert!(text.contains("Content-Length: 5\r\n\r\nv=0\r\n"));
    }

    #[test]
    fn cancel_and_ack_reuse_invite_transaction_identity() {
        let invite = b"INVITE sip:6108@pbx SIP/2.0\r\nVia: SIP/2.0/UDP 10.0.0.2:5062;branch=z9hG4bKsame\r\nFrom: <sip:41000@simadmin>;tag=local\r\nTo: <sip:6108@pbx>\r\nCall-ID: call-a\r\nCSeq: 9 INVITE\r\nContent-Length: 0\r\n\r\n";
        let cancel = String::from_utf8(build_cancel(invite).unwrap()).unwrap();
        assert!(cancel.contains("branch=z9hG4bKsame"));
        assert!(cancel.contains("CSeq: 9 CANCEL"));
        let response =
            b"SIP/2.0 200 OK\r\nTo: <sip:6108@pbx>;tag=remote\r\nContent-Length: 0\r\n\r\n";
        let ack = String::from_utf8(build_ack_for_final(invite, response).unwrap()).unwrap();
        assert!(ack.contains("To: <sip:6108@pbx>;tag=remote"));
        assert!(ack.contains("CSeq: 9 ACK"));
    }

    #[test]
    fn response_expiry_prefers_server_value() {
        let frame = b"SIP/2.0 200 OK\r\nContact: <sip:u@127.0.0.1>;expires=120\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(response_expiry(frame, 3600), 120);
    }

    #[test]
    fn contact_expiry_overrides_generic_response_expiry() {
        let frame = b"SIP/2.0 200 OK\r\nExpires: 3600\r\nContact: <sip:u@127.0.0.1>;expires=1800\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(response_expiry(frame, 7200), 1800);
    }
}
