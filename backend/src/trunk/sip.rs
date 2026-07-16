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
        SipHeader::new("Allow", "INVITE, ACK, CANCEL, BYE, OPTIONS"),
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
    let mut response = format!("SIP/2.0 {status} {reason}\r\n");
    for header in ["Via", "From", "To", "Call-ID", "CSeq"] {
        let value = sip_frame::header_value(request, header)
            .ok_or_else(|| format!("trunk_request_{}_missing", header.to_ascii_lowercase()))?;
        response.push_str(header);
        response.push_str(": ");
        response.push_str(&value);
        if header == "To" && !value.to_ascii_lowercase().contains(";tag=") {
            response.push_str(&format!(";tag={}", token(8)));
        }
        response.push_str("\r\n");
    }
    response.push_str(&format!(
        "Server: {USER_AGENT}\r\nContent-Length: 0\r\n\r\n"
    ));
    Ok(response.into_bytes())
}

pub fn response_expiry(frame: &[u8], fallback: u32) -> u32 {
    sip_frame::header_value(frame, "Expires")
        .and_then(|value| value.parse::<u32>().ok())
        .or_else(|| {
            sip_frame::header_values(frame, "Contact")
                .into_iter()
                .find_map(|value| parameter_value(&value, "expires")?.parse::<u32>().ok())
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
    fn response_expiry_prefers_server_value() {
        let frame = b"SIP/2.0 200 OK\r\nContact: <sip:u@127.0.0.1>;expires=120\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(response_expiry(frame, 3600), 120);
    }
}
