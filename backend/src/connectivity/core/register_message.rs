//! Transport-independent IMS REGISTER message assembly.
//!
//! Access legs resolve carrier policy and security details into
//! [`RegisterHeaderFields`]. This module owns the common, ordered SIP header
//! layout so VoLTE and VoWiFi cannot silently drift in the parts of REGISTER
//! that have identical semantics.

use super::{
    context::ImsRoute,
    sip_message::{self, SipHeader, SipRequest},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterHeaderFields {
    /// Full `Authorization: ...` or `Proxy-Authorization: ...` header line.
    pub authorization: Option<String>,
    pub contact: String,
    pub accept_contacts: Vec<String>,
    pub route: Option<String>,
    pub expires: u32,
    pub supported: Option<String>,
    pub require_sec_agree: bool,
    pub proxy_require_sec_agree: bool,
    pub allow: Option<String>,
    pub preferred_identity: Option<String>,
    pub visited_network: Option<String>,
    pub access_network_info: Option<String>,
    pub cellular_network_info: Option<String>,
    pub security_client: Option<String>,
    pub security_verify: Option<String>,
    pub user_agent: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterRequest {
    pub request_uri: String,
    /// Route used for Via. An access leg may set a protected advertised port
    /// here even when the underlying send socket uses another port.
    pub advertised_route: ImsRoute,
    pub branch: String,
    pub from_uri: String,
    pub from_tag: String,
    pub to_value: String,
    pub call_id: String,
    pub cseq: u32,
    pub headers: RegisterHeaderFields,
}

pub fn build_register(request: &RegisterRequest) -> Vec<u8> {
    let fields = &request.headers;
    let mut headers = Vec::new();
    push_full_header(&mut headers, fields.authorization.as_deref());
    headers.push(SipHeader::new("Contact", &fields.contact));
    headers.extend(
        fields
            .accept_contacts
            .iter()
            .map(|value| SipHeader::new("Accept-Contact", value)),
    );
    push_optional(&mut headers, "Route", fields.route.as_deref());
    headers.push(SipHeader::new("Expires", fields.expires.to_string()));
    push_non_empty(&mut headers, "Supported", fields.supported.as_deref());
    if fields.require_sec_agree {
        headers.push(SipHeader::new("Require", "sec-agree"));
    }
    if fields.proxy_require_sec_agree {
        headers.push(SipHeader::new("Proxy-Require", "sec-agree"));
    }
    push_non_empty(&mut headers, "Allow", fields.allow.as_deref());
    push_optional(
        &mut headers,
        "P-Preferred-Identity",
        fields.preferred_identity.as_deref(),
    );
    push_optional(
        &mut headers,
        "P-Visited-Network-ID",
        fields.visited_network.as_deref(),
    );
    push_optional(
        &mut headers,
        "P-Access-Network-Info",
        fields.access_network_info.as_deref(),
    );
    push_optional(
        &mut headers,
        "Cellular-Network-Info",
        fields.cellular_network_info.as_deref(),
    );
    push_optional(
        &mut headers,
        "Security-Client",
        fields.security_client.as_deref(),
    );
    push_optional(
        &mut headers,
        "Security-Verify",
        fields.security_verify.as_deref(),
    );
    headers.push(SipHeader::new("User-Agent", &fields.user_agent));

    sip_message::build_register(&SipRequest {
        method: "REGISTER",
        request_uri: &request.request_uri,
        route: request.advertised_route,
        branch: &request.branch,
        from_uri: &request.from_uri,
        from_tag: &request.from_tag,
        to_value: &request.to_value,
        call_id: &request.call_id,
        cseq: request.cseq,
        headers: &headers,
        body: &[],
    })
}

fn push_full_header(headers: &mut Vec<SipHeader>, line: Option<&str>) {
    let Some((name, value)) = line.and_then(|line| line.split_once(':')) else {
        return;
    };
    headers.push(SipHeader::new(name.trim(), value.trim()));
}

fn push_optional(headers: &mut Vec<SipHeader>, name: &str, value: Option<&str>) {
    if let Some(value) = value {
        headers.push(SipHeader::new(name, value));
    }
}

fn push_non_empty(headers: &mut Vec<SipHeader>, name: &str, value: Option<&str>) {
    push_optional(
        headers,
        name,
        value.filter(|value| !value.trim().is_empty()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::core::context::SipTransport;
    use std::net::{Ipv4Addr, SocketAddr};

    fn request() -> RegisterRequest {
        RegisterRequest {
            request_uri: "sip:ims.example".into(),
            advertised_route: ImsRoute {
                local_addr: SocketAddr::from((Ipv4Addr::new(10, 0, 0, 2), 5063)),
                pcscf_addr: SocketAddr::from((Ipv4Addr::new(10, 0, 0, 1), 5060)),
                transport: SipTransport::Udp,
            },
            branch: "z9hG4bKregister".into(),
            from_uri: "sip:user@ims.example".into(),
            from_tag: "from-tag".into(),
            to_value: "<sip:user@ims.example>".into(),
            call_id: "register@simadmin".into(),
            cseq: 2,
            headers: RegisterHeaderFields {
                authorization: Some("Authorization: Digest response=\"proof\"".into()),
                contact: "<sip:user@10.0.0.2:5063;transport=udp>".into(),
                accept_contacts: vec!["*;+g.3gpp.smsip".into()],
                route: Some("<sip:10.0.0.1:5060;lr>".into()),
                expires: 3600,
                supported: Some("path, gruu, sec-agree".into()),
                require_sec_agree: true,
                proxy_require_sec_agree: true,
                allow: None,
                preferred_identity: Some("<sip:user@ims.example>".into()),
                visited_network: None,
                access_network_info: Some("IEEE-802.11".into()),
                cellular_network_info: None,
                security_client: Some("ipsec-3gpp;alg=hmac-sha-1-96".into()),
                security_verify: None,
                user_agent: "SimAdmin IMS".into(),
            },
        }
    }

    #[test]
    fn common_register_headers_have_one_stable_order() {
        let frame = String::from_utf8(build_register(&request())).unwrap();
        let names = [
            "Authorization:",
            "Contact:",
            "Accept-Contact:",
            "Route:",
            "Expires:",
            "Supported:",
            "Require:",
            "Proxy-Require:",
            "P-Preferred-Identity:",
            "P-Access-Network-Info:",
            "Security-Client:",
            "User-Agent:",
        ];
        let mut previous = 0;
        for name in names {
            let next = frame.find(name).expect("header is present");
            assert!(next >= previous, "{name} was emitted out of order");
            previous = next;
        }
        assert!(frame.contains("Via: SIP/2.0/UDP 10.0.0.2:5063"));
    }

    #[test]
    fn empty_optional_headers_are_omitted() {
        let mut request = request();
        request.headers.supported = Some("  ".into());
        request.headers.allow = Some(String::new());
        let frame = String::from_utf8(build_register(&request)).unwrap();
        assert!(!frame.contains("Supported:"));
        assert!(!frame.contains("Allow:"));
    }
}
