//! Shared SIP request construction primitives.
//!
//! Leg-specific code supplies ordered 3GPP/carrier headers; this module owns
//! the common request line, Via/From/To/dialog identifiers, body framing and
//! binary-safe concatenation.

use super::context::ImsRoute;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SipHeader {
    pub name: String,
    pub value: String,
}

impl SipHeader {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SipRequest<'a> {
    pub method: &'a str,
    pub request_uri: &'a str,
    pub route: ImsRoute,
    pub branch: &'a str,
    pub from_uri: &'a str,
    pub from_tag: &'a str,
    pub to_value: &'a str,
    pub call_id: &'a str,
    pub cseq: u32,
    pub headers: &'a [SipHeader],
    pub body: &'a [u8],
}

pub fn build_request(request: &SipRequest<'_>) -> Vec<u8> {
    let local = super::sip_frame::sip_host(request.route.local_addr.ip());
    let mut text = String::new();
    text.push_str(&format!(
        "{} {} SIP/2.0\r\n",
        request.method, request.request_uri
    ));
    text.push_str(&format!(
        "Via: {} {}:{};branch={};rport\r\n",
        request.route.transport.as_via(),
        local,
        request.route.local_addr.port(),
        request.branch,
    ));
    text.push_str("Max-Forwards: 70\r\n");
    text.push_str(&format!(
        "From: <{}>;tag={}\r\n",
        request.from_uri, request.from_tag
    ));
    text.push_str(&format!("To: {}\r\n", request.to_value));
    text.push_str(&format!("Call-ID: {}\r\n", request.call_id));
    text.push_str(&format!("CSeq: {} {}\r\n", request.cseq, request.method));
    for header in request.headers {
        text.push_str(&header.name);
        text.push_str(": ");
        text.push_str(&header.value);
        text.push_str("\r\n");
    }
    text.push_str(&format!("Content-Length: {}\r\n\r\n", request.body.len()));
    let mut frame = text.into_bytes();
    frame.extend_from_slice(request.body);
    frame
}

pub fn build_register(request: &SipRequest<'_>) -> Vec<u8> {
    debug_assert_eq!(request.method, "REGISTER");
    build_request(request)
}

pub fn build_message(request: &SipRequest<'_>) -> Vec<u8> {
    debug_assert_eq!(request.method, "MESSAGE");
    build_request(request)
}

pub fn build_rp_ack(request: &SipRequest<'_>) -> Vec<u8> {
    build_message(request)
}

pub fn build_invite(request: &SipRequest<'_>) -> Vec<u8> {
    debug_assert_eq!(request.method, "INVITE");
    build_request(request)
}

pub fn build_ack(request: &SipRequest<'_>) -> Vec<u8> {
    debug_assert_eq!(request.method, "ACK");
    build_request(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::core::context::SipTransport;
    use std::net::{Ipv4Addr, SocketAddr};

    #[test]
    fn binary_message_body_is_appended_without_utf8_conversion() {
        let headers = [SipHeader::new("Content-Type", "application/vnd.3gpp.sms")];
        let request = SipRequest {
            method: "MESSAGE",
            request_uri: "sip:smsc.example",
            route: ImsRoute {
                local_addr: SocketAddr::from((Ipv4Addr::new(10, 0, 0, 2), 5060)),
                pcscf_addr: SocketAddr::from((Ipv4Addr::new(10, 0, 0, 1), 5060)),
                transport: SipTransport::Tcp,
            },
            branch: "z9hG4bKtest",
            from_uri: "sip:001@ims.example",
            from_tag: "tag",
            to_value: "<sip:smsc.example>",
            call_id: "call@simadmin",
            cseq: 1,
            headers: &headers,
            body: &[0x00, 0xff, 0x01],
        };
        let frame = build_message(&request);
        let headers = String::from_utf8_lossy(&frame[..frame.len() - 3]);
        assert!(headers.contains("Content-Length: 3"));
        assert_eq!(&frame[frame.len() - 3..], &[0x00, 0xff, 0x01]);
    }
}
