//! Common artifacts returned by a successful IMS REGISTER.

use super::sip_frame;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegisterArtifacts {
    pub expires_seconds: Option<u32>,
    /// Exact Require option-tag from the response, not Supported and not proof
    /// that this client advertised/implemented the full outbound procedure.
    pub outbound_required: bool,
    /// RFC 5626: last Path URI has `ob`, or no Path (registrar is first hop).
    pub first_hop_outbound: bool,
    /// Exact instance/reg-id binding found; never borrow the other flow's TTL.
    pub own_binding_found: bool,
    pub flow_timer_seconds: Option<u32>,
    /// Ordered RFC 3608 service-route set, formatted so it can be copied into
    /// one `Route` header field without losing repeated header lines.
    pub service_route: Option<String>,
    pub service_route_count: usize,
    pub associated_uris: Vec<String>,
    /// Number of non-wildcard Contact bindings enumerated by the registrar.
    /// A REGISTER 200 response contains every current binding for the AOR, not
    /// just the binding created by this process.
    pub contact_binding_count: usize,
    pub wildcard_contact_present: bool,
    /// More than one Contact was present and their per-binding expiry values
    /// could not be reduced to one common lifetime. In that case
    /// `expires_seconds` falls back to the response Expires field (or later to
    /// the profile default) instead of silently borrowing another UE's lease.
    pub contact_expiry_ambiguous: bool,
}

impl RegisterArtifacts {
    pub fn parse(response: &[u8]) -> Self {
        let contacts = contact_bindings(response);
        let wildcard_contact_present = contacts.iter().any(|contact| contact.trim() == "*");
        let contact_expiries = contacts
            .iter()
            .filter(|contact| contact.trim() != "*")
            .map(|contact| contact_expires(contact))
            .collect::<Vec<_>>();
        let contact_binding_count = contact_expiries.len();
        let (contact_expires, contact_expiry_ambiguous) = common_contact_expires(&contact_expiries);
        let expires_seconds = contact_expires.or_else(|| {
            sip_frame::header_value(response, "Expires")
                .and_then(|value| value.trim().parse::<u32>().ok())
        });

        let mut service_routes = Vec::new();
        for value in sip_frame::header_values(response, "Service-Route") {
            for route in split_sip_list(&value) {
                let route = route.trim();
                if !route.is_empty() {
                    service_routes.push(route.to_string());
                }
            }
        }
        let service_route_count = service_routes.len();
        let service_route = (!service_routes.is_empty()).then(|| service_routes.join(", "));

        let mut associated_uris = Vec::new();
        for value in sip_frame::header_values(response, "P-Associated-URI") {
            collect_associated_uris(&value, &mut associated_uris);
        }

        let outbound_required = sip_frame::header_values(response, "Require")
            .iter()
            .flat_map(|value| value.split(','))
            .any(|tag| tag.trim().eq_ignore_ascii_case("outbound"));
        let timers = sip_frame::header_values(response, "Flow-Timer");
        let flow_timer_seconds = match timers.as_slice() {
            [value] => value
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|seconds| *seconds > 0),
            _ => None,
        };
        Self {
            expires_seconds,
            outbound_required,
            first_hop_outbound: first_hop_outbound(response),
            own_binding_found: false,
            flow_timer_seconds,
            service_route,
            service_route_count,
            associated_uris,
            contact_binding_count,
            wildcard_contact_present,
            contact_expiry_ambiguous,
        }
    }

    pub fn parse_for_binding(response: &[u8], instance: &str, reg_id: u32) -> Self {
        let mut artifacts = Self::parse(response);
        let matches = contact_bindings(response)
            .into_iter()
            .filter(|contact| {
                contact_parameter(contact, "+sip.instance").as_deref() == Some(instance)
                    && contact_parameter(contact, "reg-id").and_then(|v| v.parse::<u32>().ok())
                        == Some(reg_id)
            })
            .collect::<Vec<_>>();
        if let [contact] = matches.as_slice() {
            artifacts.own_binding_found = true;
            artifacts.contact_expiry_ambiguous = false;
            // Missing per-binding expires can use only the response's Expires,
            // not another Contact's common/default expiry.
            artifacts.expires_seconds = contact_expires(contact).or_else(|| {
                sip_frame::header_value(response, "Expires").and_then(|v| v.trim().parse().ok())
            });
        } else {
            // Never inherit an unrelated binding's expires when our identifiers
            // are missing/ambiguous. Negotiation validation fails closed later.
            artifacts.contact_expiry_ambiguous = artifacts.contact_binding_count > 1;
            artifacts.expires_seconds = sip_frame::header_value(response, "Expires")
                .and_then(|value| value.trim().parse().ok());
        }
        artifacts
    }

    pub fn default_associated_uri(&self) -> Option<&str> {
        self.associated_uris.first().map(String::as_str)
    }
}

/// Contact parameters are outside the URI and may contain quoted semicolons.
pub(super) fn contact_parameter(contact: &str, wanted: &str) -> Option<String> {
    let parameters = name_addr_range(contact).map_or(contact, |(_, end)| &contact[end + 1..]);
    let values = split_sip_parameters(parameters)
        .into_iter()
        .skip(1)
        .filter_map(|parameter| {
            let (name, value) = parameter.split_once('=')?;
            name.trim().eq_ignore_ascii_case(wanted).then(|| {
                value
                    .trim()
                    .trim_matches('"')
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_string()
            })
        })
        .collect::<Vec<_>>();
    // Duplicate identifiers are ambiguous, not evidence for this binding.
    match values.as_slice() {
        [value] => Some(value.clone()),
        _ => None,
    }
}

fn first_hop_outbound(response: &[u8]) -> bool {
    let paths = sip_frame::header_values(response, "Path");
    if paths.is_empty() {
        return true;
    }
    let routes = paths
        .iter()
        .flat_map(|path| split_sip_list(path))
        .collect::<Vec<_>>();
    let Some(last) = routes.last() else {
        return false;
    };
    let uri = name_addr_range(last).map_or(last.trim(), |(start, end)| &last[start + 1..end]);
    // A query/header value or a Contact-style parameter outside <> is not ob.
    let uri = uri.split('?').next().unwrap_or("");
    uri.split(';')
        .skip(1)
        .any(|p| p.trim().eq_ignore_ascii_case("ob"))
}

/// Locate only the name-addr URI, not brackets in a quoted display name or
/// +sip.instance. SIP name-addr URIs themselves cannot contain literal <>.
pub(super) fn name_addr_range(value: &str) -> Option<(usize, usize)> {
    let mut quoted = false;
    let mut escaped = false;
    let mut start = None;
    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '<' if !quoted => {
                if start.is_some() {
                    return None;
                }
                start = Some(index);
            }
            '>' if !quoted => return start.map(|start| (start, index)),
            _ => {}
        }
    }
    None
}

fn contact_bindings(response: &[u8]) -> Vec<String> {
    let mut contacts = Vec::new();
    for value in sip_frame::header_values(response, "Contact")
        .into_iter()
        .chain(sip_frame::header_values(response, "m"))
    {
        contacts.extend(
            split_sip_list(&value)
                .into_iter()
                .map(str::trim)
                .filter(|contact| !contact.is_empty())
                .map(ToString::to_string),
        );
    }
    contacts
}

fn contact_expires(contact: &str) -> Option<u32> {
    contact_parameter(contact, "expires").and_then(|value| value.parse().ok())
}

fn common_contact_expires(expiries: &[Option<u32>]) -> (Option<u32>, bool) {
    let Some(first) = expiries.first().copied() else {
        return (None, false);
    };
    if expiries.len() == 1 {
        return (first, false);
    }
    if expiries.iter().all(|candidate| *candidate == first) {
        return (first, false);
    }
    (None, true)
}

fn collect_associated_uris(value: &str, uris: &mut Vec<String>) {
    for entry in split_sip_list(value) {
        let entry = entry.trim();
        let uri = entry
            .find('<')
            .and_then(|start| {
                entry[start + 1..]
                    .find('>')
                    .map(|end| &entry[start + 1..start + 1 + end])
            })
            .map(str::to_string)
            .or_else(|| sip_frame::uri_from_header_value(entry));
        if let Some(uri) = uri {
            push_supported_uri(&uri, uris);
        }
    }
}

fn push_supported_uri(uri: &str, uris: &mut Vec<String>) {
    let uri = uri.trim();
    let lower = uri.to_ascii_lowercase();
    if (lower.starts_with("sip:") || lower.starts_with("sips:") || lower.starts_with("tel:"))
        && !uris.iter().any(|candidate| candidate == uri)
    {
        uris.push(uri.to_string());
    }
}

/// Split a comma-separated SIP header list without treating commas inside a
/// quoted display name or a name-addr URI as element separators.
pub(super) fn split_sip_list(value: &str) -> Vec<&str> {
    split_sip_delimited(value, ',')
}

pub(super) fn split_sip_parameters(value: &str) -> Vec<&str> {
    split_sip_delimited(value, ';')
}

fn split_sip_delimited(value: &str, delimiter: char) -> Vec<&str> {
    let mut values = Vec::new();
    let mut start = 0usize;
    let mut in_quote = false;
    let mut escaped = false;
    let mut angle_depth = 0u16;

    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quote => escaped = true,
            '"' => in_quote = !in_quote,
            '<' if !in_quote => angle_depth = angle_depth.saturating_add(1),
            '>' if !in_quote => angle_depth = angle_depth.saturating_sub(1),
            ch if ch == delimiter && !in_quote && angle_depth == 0 => {
                values.push(&value[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    values.push(&value[start..]);
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbound_requires_exact_require_option_tag_including_repeated_headers() {
        let response = b"SIP/2.0 200 OK\r\nRequire: path, sec-agree\r\nRequire: Outbound, gruu\r\nFlow-Timer: 25\r\n\r\n";
        let artifacts = RegisterArtifacts::parse(response);
        assert!(artifacts.outbound_required);
        assert_eq!(artifacts.flow_timer_seconds, Some(25));
        for response in [
            "SIP/2.0 200 OK\r\nSupported: outbound\r\n\r\n",
            "SIP/2.0 200 OK\r\nRequire: x-outbound, outbound-extra\r\n\r\n",
            "SIP/2.0 200 OK\r\nContact: <sip:user@example>;reg-id=2\r\n\r\n",
        ] {
            assert!(!RegisterArtifacts::parse(response.as_bytes()).outbound_required);
        }
    }

    #[test]
    fn invalid_or_repeated_flow_timers_are_not_interpreted_as_keepalive_support() {
        for headers in [
            "",
            "Flow-Timer: 0\r\n",
            "Flow-Timer: -1\r\n",
            "Flow-Timer: 20, 30\r\n",
            "Flow-Timer: 4294967296\r\n",
            "Flow-Timer: 20\r\nFlow-Timer: 30\r\n",
        ] {
            let response = format!("SIP/2.0 200 OK\r\n{headers}\r\n");
            assert_eq!(
                RegisterArtifacts::parse(response.as_bytes()).flow_timer_seconds,
                None
            );
        }
    }

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
        assert_eq!(artifacts.service_route_count, 1);
        assert_eq!(artifacts.contact_binding_count, 1);
        assert!(!artifacts.contact_expiry_ambiguous);
        assert_eq!(
            artifacts.default_associated_uri(),
            Some("sip:+601100000001@ims.example")
        );
        assert_eq!(artifacts.associated_uris.len(), 2);
    }

    #[test]
    fn accepts_repeated_bare_associated_uri_headers_case_insensitively() {
        let response = concat!(
            "SIP/2.0 200 OK\r\n",
            "P-Associated-URI: TEL:+601100000001\r\n",
            "P-Associated-URI: SIP:user@ims.example\r\n\r\n",
        );
        let artifacts = RegisterArtifacts::parse(response.as_bytes());
        assert_eq!(
            artifacts.associated_uris,
            ["TEL:+601100000001", "SIP:user@ims.example"]
        );
    }

    #[test]
    fn preserves_repeated_service_route_values_in_wire_order() {
        let response = concat!(
            "SIP/2.0 200 OK\r\n",
            "Service-Route: \"edge, primary\" <sip:edge.example;lr>\r\n",
            "Service-Route: <sip:scscf-a.example;lr>, <sip:scscf-b.example;lr>\r\n\r\n",
        );
        let artifacts = RegisterArtifacts::parse(response.as_bytes());

        assert_eq!(artifacts.service_route_count, 3);
        assert_eq!(
            artifacts.service_route.as_deref(),
            Some(
                "\"edge, primary\" <sip:edge.example;lr>, <sip:scscf-a.example;lr>, <sip:scscf-b.example;lr>"
            )
        );
    }

    #[test]
    fn uri_parameter_named_expires_is_not_a_binding_lifetime() {
        let response = concat!(
            "SIP/2.0 200 OK\r\n",
            "Contact: <sip:user@192.0.2.2;expires=99;transport=udp>;q=0.5\r\n",
            "Expires: 1800\r\n\r\n",
        );
        let artifacts = RegisterArtifacts::parse(response.as_bytes());

        assert_eq!(artifacts.contact_binding_count, 1);
        assert_eq!(artifacts.expires_seconds, Some(1800));
    }

    #[test]
    fn differing_contact_binding_expiries_do_not_select_another_ues_lease() {
        let response = concat!(
            "SIP/2.0 200 OK\r\n",
            "Contact: <sip:old@192.0.2.1>;expires=60, <sip:new@192.0.2.2>;expires=3600\r\n",
            "Expires: 1800\r\n\r\n",
        );
        let artifacts = RegisterArtifacts::parse(response.as_bytes());

        assert_eq!(artifacts.contact_binding_count, 2);
        assert!(artifacts.contact_expiry_ambiguous);
        assert_eq!(artifacts.expires_seconds, Some(1800));
    }

    #[test]
    fn common_expiry_across_multiple_bindings_is_safe_to_use() {
        let response = concat!(
            "SIP/2.0 200 OK\r\n",
            "Contact: <sip:a@192.0.2.1>;expires=600\r\n",
            "Contact: <sip:b@192.0.2.2>;expires=600\r\n\r\n",
        );
        let artifacts = RegisterArtifacts::parse(response.as_bytes());

        assert_eq!(artifacts.contact_binding_count, 2);
        assert!(!artifacts.contact_expiry_ambiguous);
        assert_eq!(artifacts.expires_seconds, Some(600));
    }

    #[test]
    fn wildcard_contact_is_reported_but_not_counted_as_a_binding() {
        let response = b"SIP/2.0 200 OK\r\nContact: *\r\nExpires: 0\r\n\r\n";
        let artifacts = RegisterArtifacts::parse(response);

        assert!(artifacts.wildcard_contact_present);
        assert_eq!(artifacts.contact_binding_count, 0);
        assert_eq!(artifacts.expires_seconds, Some(0));
    }
}

#[cfg(test)]
mod outbound_binding_tests {
    use super::*;
    #[test]
    fn selects_own_lease_and_checks_last_path_not_first() {
        let response = b"SIP/2.0 200 OK\r\nRequire: outbound\r\nPath: <sip:registrar.test;lr;ob>, <sip:edge.test;lr>\r\nContact: <sip:a@192.0.2.1>;+sip.instance=\"<urn:uuid:i>\";reg-id=1;expires=90\r\nContact: <sip:a@192.0.2.2>;+sip.instance=\"<urn:uuid:i>\";reg-id=2;expires=600\r\n\r\n";
        let artifacts = RegisterArtifacts::parse_for_binding(response, "urn:uuid:i", 2);
        assert!(artifacts.own_binding_found);
        assert_eq!(artifacts.expires_seconds, Some(600));
        assert!(!artifacts.first_hop_outbound);
        assert!(
            !RegisterArtifacts::parse_for_binding(response, "urn:uuid:other", 2).own_binding_found
        );
        let response = b"SIP/2.0 200 OK\r\nPath: <sip:registrar.test;lr>\r\nPath: <sip:edge.test;lr;ob>\r\n\r\n";
        assert!(RegisterArtifacts::parse(response).first_hop_outbound);
    }
}
