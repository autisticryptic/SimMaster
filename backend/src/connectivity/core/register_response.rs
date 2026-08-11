//! Common artifacts returned by a successful IMS REGISTER.

use super::sip_frame;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegisterArtifacts {
    pub expires_seconds: Option<u32>,
    pub service_route: Option<String>,
    pub associated_uris: Vec<String>,
}

impl RegisterArtifacts {
    pub fn parse(response: &[u8]) -> Self {
        let expires_seconds = contact_expires(response).or_else(|| {
            sip_frame::header_value(response, "Expires")
                .and_then(|value| value.trim().parse::<u32>().ok())
        });
        let service_route = sip_frame::header_value(response, "Service-Route")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let mut associated_uris = Vec::new();
        for value in sip_frame::header_values(response, "P-Associated-URI") {
            collect_associated_uris(&value, &mut associated_uris);
        }
        associated_uris.dedup();
        Self {
            expires_seconds,
            service_route,
            associated_uris,
        }
    }

    pub fn default_associated_uri(&self) -> Option<&str> {
        self.associated_uris.first().map(String::as_str)
    }
}

fn contact_expires(response: &[u8]) -> Option<u32> {
    for contact in sip_frame::header_values(response, "Contact") {
        for parameter in contact.split(';').skip(1) {
            let Some((name, value)) = parameter.split_once('=') else {
                continue;
            };
            if name.trim().eq_ignore_ascii_case("expires") {
                let value = value
                    .trim()
                    .trim_matches(|ch: char| ch == '>' || ch == ',' || ch.is_ascii_whitespace());
                if let Ok(expires) = value.parse::<u32>() {
                    return Some(expires);
                }
            }
        }
    }
    None
}

fn collect_associated_uris(value: &str, uris: &mut Vec<String>) {
    let initial_len = uris.len();
    let mut remainder = value;
    while let Some(start) = remainder.find('<') {
        let after_start = &remainder[start + 1..];
        let Some(end) = after_start.find('>') else {
            break;
        };
        push_supported_uri(&after_start[..end], uris);
        remainder = &after_start[end + 1..];
    }

    if uris.len() > initial_len {
        return;
    }
    for entry in value.split(',') {
        if let Some(uri) = sip_frame::uri_from_header_value(entry) {
            push_supported_uri(&uri, uris);
        }
    }
}

fn push_supported_uri(uri: &str, uris: &mut Vec<String>) {
    let uri = uri.trim();
    if (uri.starts_with("sip:") || uri.starts_with("sips:") || uri.starts_with("tel:"))
        && !uris.iter().any(|candidate| candidate == uri)
    {
        uris.push(uri.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_registration_routing_identity_and_contact_expiry() {
        let response = concat!(
            "SIP/2.0 200 OK\r\n",
            "Contact: <sip:user@10.0.0.2>;expires=1200\r\n",
            "Expires: 3600\r\n",
            "Service-Route: <sip:pcscf.example;lr>\r\n",
            "P-Associated-URI: <sip:+601100000001@ims.example>, <tel:+601100000001>\r\n",
            "Content-Length: 0\r\n\r\n",
        );
        let artifacts = RegisterArtifacts::parse(response.as_bytes());

        assert_eq!(artifacts.expires_seconds, Some(1200));
        assert_eq!(
            artifacts.service_route.as_deref(),
            Some("<sip:pcscf.example;lr>")
        );
        assert_eq!(
            artifacts.default_associated_uri(),
            Some("sip:+601100000001@ims.example")
        );
        assert_eq!(artifacts.associated_uris.len(), 2);
    }

    #[test]
    fn accepts_repeated_bare_associated_uri_headers() {
        let response = concat!(
            "SIP/2.0 200 OK\r\n",
            "P-Associated-URI: tel:+601100000001\r\n",
            "P-Associated-URI: sip:user@ims.example\r\n\r\n",
        );
        let artifacts = RegisterArtifacts::parse(response.as_bytes());
        assert_eq!(
            artifacts.associated_uris,
            ["tel:+601100000001", "sip:user@ims.example"]
        );
    }
}
