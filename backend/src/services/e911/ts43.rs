//! GSMA TS.43 entitlement transport for VoWiFi/E911 provisioning.
//!
//! Authentication uses the EAP relay JSON exchange from TS.43 section 2.6.1,
//! not HTTP Digest-AKA. Subscriber, device and AKA material are never logged.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

use base64::Engine;
use futures_util::{future::BoxFuture, StreamExt};
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::header::{HeaderMap, ACCEPT, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, SET_COOKIE};
use reqwest::{Method, StatusCode};
use serde_json::json;
use url::Url;

use crate::connectivity::core::entitlement::{
    E911State, EntitlementQueryOutcome, EntitlementStatusValue,
};
use crate::connectivity::modems::ims::vowifi::eap_aka::{
    build_challenge_response, build_identity_response_packet, build_sync_failure_response,
    parse_challenge, EapAkaResponsePacket,
};
use crate::services::e911::orchestrator::{
    EntitlementExchange, EntitlementRequestContext, EntitlementTransport, SimAkaProvider,
};
use crate::services::e911::registry::E911Provider;
use crate::services::e911::ssrf::{
    check_host, check_resolved_ip, is_hostname, validate_entitlement_target, validate_redirect,
    MAX_REDIRECTS, MAX_RESPONSE_BYTES,
};
use crate::services::e911::state_store::E911Secrets;

const EAP_RELAY_CONTENT_TYPE: &str = "application/vnd.gsma.eap-relay.v1.0+json";
const TS43_XML_CONTENT_TYPE: &str = "text/vnd.wap.connectivity-xml";
const EAP_RELAY_PACKET: &str = "eap-relay-packet";
const APP_VOWIFI: &str = "ap2004";
const DEFAULT_ENTITLEMENT_VERSION: &str = "2.0";
const MAX_EAP_AKA_ATTEMPTS: usize = 3;

pub struct Ts43Transport;

impl Ts43Transport {
    pub fn new() -> Self {
        Self
    }

    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String> {
        tokio::net::lookup_host((host, port))
            .await
            .map(|addresses| addresses.map(|address| address.ip()).collect())
            .map_err(|_| "entitlement_dns_failed".to_string())
    }

    async fn pinned_client(&self, target: &Url) -> Result<reqwest::Client, String> {
        let host = target
            .host_str()
            .ok_or_else(|| "entitlement_url_missing_host".to_string())?;
        let port = target.port_or_known_default().unwrap_or(443);
        let candidates = self.resolve(host, port).await?;
        let selected = candidates
            .into_iter()
            .find(|ip| check_resolved_ip(*ip).is_ok())
            .ok_or_else(|| "entitlement_ip_forbidden".to_string())?;
        check_resolved_ip(selected).map_err(|error| error.to_string())?;
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            // Pin the address that passed the private/link-local check. TLS
            // still verifies the original hostname.
            .resolve(host, SocketAddr::new(selected, port))
            .build()
            .map_err(|error| format!("entitlement_transport:{error}"))
    }

    async fn send_checked(
        &self,
        mut url: Url,
        method: Method,
        body: Option<&serde_json::Value>,
        accept: &str,
        cookie: Option<&str>,
        allow_list: &[String],
    ) -> Result<CheckedResponse, String> {
        for redirect_count in 0..=MAX_REDIRECTS {
            let host = url
                .host_str()
                .ok_or_else(|| "entitlement_url_missing_host".to_string())?
                .to_ascii_lowercase();
            check_host(&host, allow_list).map_err(|error| error.to_string())?;
            let client = self.pinned_client(&url).await?;
            let mut request = client
                .request(method.clone(), url.clone())
                .header(ACCEPT, accept);
            if let Some(cookie) = cookie.filter(|value| !value.is_empty()) {
                request = request.header(COOKIE, cookie);
            }
            if let Some(body) = body {
                request = request
                    .header(CONTENT_TYPE, EAP_RELAY_CONTENT_TYPE)
                    .json(body);
            }
            let response = request
                .send()
                .await
                .map_err(|error| format!("entitlement_transport:{error}"))?;
            if response.status().is_redirection() {
                if redirect_count == MAX_REDIRECTS {
                    return Err("entitlement_too_many_redirects".to_string());
                }
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| "entitlement_redirect_without_location".to_string())?;
                url = validate_redirect(location, allow_list).map_err(|error| error.to_string())?;
                continue;
            }
            let status = response.status();
            let headers = response.headers().clone();
            if headers
                .get(CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<usize>().ok())
                .is_some_and(|length| length > MAX_RESPONSE_BYTES)
            {
                return Err("entitlement_response_too_large".to_string());
            }
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|error| format!("entitlement_transport:{error}"))?;
                if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                    return Err("entitlement_response_too_large".to_string());
                }
                bytes.extend_from_slice(&chunk);
            }
            return Ok(CheckedResponse {
                status,
                headers,
                body: bytes,
                final_url: url,
            });
        }
        Err("entitlement_too_many_redirects".to_string())
    }

    async fn run_query(
        &self,
        provider: &E911Provider,
        context: &EntitlementRequestContext,
        stored: &E911Secrets,
        sim_auth: &dyn SimAkaProvider,
    ) -> Result<EntitlementExchange, String> {
        validate_context(context)?;
        let endpoint = provider
            .entitlement_url
            .as_deref()
            .ok_or_else(|| "entitlement_url_missing".to_string())?;
        let base_url = validate_entitlement_target(endpoint, &provider.host_allow_list)
            .map_err(|error| error.to_string())?;
        let mut request_url = base_url.clone();
        append_request_parameters(&mut request_url, context, stored);

        let fast_auth = stored.entitlement_token.is_some();
        let initial_accept = if fast_auth {
            TS43_XML_CONTENT_TYPE
        } else {
            EAP_RELAY_CONTENT_TYPE
        };
        let mut response = self
            .send_checked(
                request_url,
                Method::GET,
                None,
                initial_accept,
                stored.cookie.as_deref(),
                &provider.host_allow_list,
            )
            .await?;
        let mut token_invalidated = false;
        // A carrier may expire a cached token with 401/403. Drop only the
        // fast-auth material and retry the standard EAP relay challenge once.
        if fast_auth
            && matches!(
                response.status,
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            )
        {
            token_invalidated = true;
            request_url = base_url;
            let full_auth_secrets = E911Secrets {
                entitlement_token: None,
                cookie: None,
                ..stored.clone()
            };
            append_request_parameters(&mut request_url, context, &full_auth_secrets);
            response = self
                .send_checked(
                    request_url,
                    Method::GET,
                    None,
                    EAP_RELAY_CONTENT_TYPE,
                    None,
                    &provider.host_allow_list,
                )
                .await?;
        }
        ensure_success(response.status)?;

        let mut updated = stored.clone();
        if token_invalidated {
            updated.entitlement_token = None;
            updated.cookie = None;
        }
        merge_response_cookie(&mut updated, &response.headers);
        let identity = root_nai(context);
        let mut keyed_response: Option<EapAkaResponsePacket> = None;

        for _ in 0..MAX_EAP_AKA_ATTEMPTS {
            let Some(challenge_packet) = extract_eap_relay_packet(&response.body)? else {
                return self.finish_exchange(provider, response, updated).await;
            };
            if challenge_packet.len() < 4 {
                return Err("entitlement_eap_packet_truncated".to_string());
            }
            if challenge_packet[0] != 1 {
                return Err("entitlement_eap_request_expected".to_string());
            }
            let eap_response = match challenge_packet.get(5).copied() {
                Some(5) => build_identity_response_packet(&challenge_packet, &identity)
                    .map_err(|_| "entitlement_eap_identity_failed".to_string())?,
                Some(1) => {
                    let challenge = parse_challenge(&challenge_packet)
                        .map_err(|_| "entitlement_eap_challenge_invalid".to_string())?;
                    let aka = sim_auth
                        .authenticate(&challenge.rand, &challenge.autn)
                        .await?;
                    if let Some(auts) = aka.auts.as_deref() {
                        build_sync_failure_response(&challenge, auts)
                            .map_err(|_| "entitlement_eap_sync_response_failed".to_string())?
                    } else {
                        let packet = build_challenge_response(&challenge, &identity, &aka)
                            .map_err(|_| "entitlement_eap_response_failed".to_string())?;
                        keyed_response = Some(packet.clone());
                        packet
                    }
                }
                Some(12) => keyed_response
                    .as_ref()
                    .ok_or_else(|| "entitlement_eap_key_material_missing".to_string())?
                    .notification_response(&challenge_packet)
                    .map_err(|_| "entitlement_eap_notification_failed".to_string())?,
                _ => return Err("entitlement_eap_subtype_unsupported".to_string()),
            };
            let relay = base64::engine::general_purpose::STANDARD
                .encode(eap_response.expose_for_ike_encryption());
            let body = json!({ EAP_RELAY_PACKET: relay });
            response = self
                .send_checked(
                    response.final_url.clone(),
                    Method::POST,
                    Some(&body),
                    &format!("{EAP_RELAY_CONTENT_TYPE}, {TS43_XML_CONTENT_TYPE}"),
                    updated.cookie.as_deref(),
                    &provider.host_allow_list,
                )
                .await?;
            ensure_success(response.status)?;
            merge_response_cookie(&mut updated, &response.headers);
        }
        Err("entitlement_eap_attempt_limit".to_string())
    }

    async fn finish_exchange(
        &self,
        provider: &E911Provider,
        response: CheckedResponse,
        mut secrets: E911Secrets,
    ) -> Result<EntitlementExchange, String> {
        let body = std::str::from_utf8(&response.body)
            .map_err(|_| "entitlement_response_not_utf8".to_string())?;
        let parsed = parse_entitlement_document(body);
        if let Some(token) = parsed.token {
            secrets.entitlement_token = Some(token);
        }
        if let Some(version) = parsed.configuration_version {
            secrets.configuration_version = Some(version);
        }
        if let Some(url) = parsed.outcome.server_flow_url.as_deref() {
            validate_websheet_url(self, provider, url).await?;
            secrets.server_flow_url = Some(url.to_string());
            secrets.server_flow_user_data = parsed.outcome.server_flow_user_data.clone();
        } else {
            secrets.server_flow_url = None;
            secrets.server_flow_user_data = None;
        }
        Ok(EntitlementExchange {
            outcome: parsed.outcome,
            secrets,
        })
    }
}

impl Default for Ts43Transport {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Ts43Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ts43Transport").finish_non_exhaustive()
    }
}

impl EntitlementTransport for Ts43Transport {
    fn query<'a>(
        &'a self,
        provider: &'a E911Provider,
        context: &'a EntitlementRequestContext,
        secrets: &'a E911Secrets,
        sim_auth: &'a dyn SimAkaProvider,
    ) -> BoxFuture<'a, Result<EntitlementExchange, String>> {
        Box::pin(self.run_query(provider, context, secrets, sim_auth))
    }
}

struct CheckedResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
    final_url: Url,
}

fn ensure_success(status: StatusCode) -> Result<(), String> {
    if status.is_success() {
        Ok(())
    } else {
        Err(format!("entitlement_http_status:{}", status.as_u16()))
    }
}

fn validate_context(context: &EntitlementRequestContext) -> Result<(), String> {
    if context.imsi.len() < 5 || !context.imsi.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("entitlement_imsi_invalid".to_string());
    }
    if context.mcc.len() != 3
        || !(2..=3).contains(&context.mnc.len())
        || !context.mcc.bytes().all(|byte| byte.is_ascii_digit())
        || !context.mnc.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("entitlement_home_plmn_invalid".to_string());
    }
    Ok(())
}

fn root_nai(context: &EntitlementRequestContext) -> String {
    format!(
        "0{}@nai.epc.mnc{:0>3}.mcc{}.3gppnetwork.org",
        context.imsi, context.mnc, context.mcc
    )
}

fn append_request_parameters(
    url: &mut Url,
    context: &EntitlementRequestContext,
    secrets: &E911Secrets,
) {
    let mut query = url.query_pairs_mut();
    if let Some(token) = secrets.entitlement_token.as_deref() {
        query.append_pair("IMSI", &context.imsi);
        query.append_pair("token", token);
    } else {
        query.append_pair("EAP_ID", &root_nai(context));
    }
    if let Some(terminal_id) = context.terminal_id.as_deref() {
        query.append_pair("terminal_id", terminal_id);
    }
    query.append_pair("app", APP_VOWIFI);
    query.append_pair("terminal_vendor", &truncate(&context.terminal_vendor, 4));
    query.append_pair("terminal_model", &truncate(&context.terminal_model, 10));
    query.append_pair(
        "terminal_sw_version",
        &truncate(&context.terminal_sw_version, 20),
    );
    query.append_pair(
        "vers",
        &secrets.configuration_version.unwrap_or(0).to_string(),
    );
    query.append_pair("entitlement_version", DEFAULT_ENTITLEMENT_VERSION);
}

fn truncate(value: &str, length: usize) -> String {
    value.chars().take(length).collect()
}

fn extract_eap_relay_packet(body: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let value: serde_json::Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let Some(packet) = value.get(EAP_RELAY_PACKET).and_then(|value| value.as_str()) else {
        return Ok(None);
    };
    base64::engine::general_purpose::STANDARD
        .decode(packet)
        .map(Some)
        .map_err(|_| "entitlement_eap_base64_invalid".to_string())
}

fn merge_response_cookie(secrets: &mut E911Secrets, headers: &HeaderMap) {
    let values = headers
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>();
    if !values.is_empty() {
        secrets.cookie = Some(values.join("; "));
    }
}

async fn validate_websheet_url(
    transport: &Ts43Transport,
    provider: &E911Provider,
    raw: &str,
) -> Result<(), String> {
    if provider.websheet_host_policy.is_none() {
        return Err("entitlement_websheet_not_allowed".to_string());
    }
    let url = Url::parse(raw).map_err(|_| "entitlement_websheet_url_invalid".to_string())?;
    if url.scheme() != "https" {
        return Err("entitlement_url_must_be_https".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "entitlement_url_missing_host".to_string())?;
    if !is_hostname(host) {
        return Err("entitlement_ip_literal_rejected".to_string());
    }
    if provider.websheet_host_policy.as_deref() != Some("public_https") {
        check_host(host, &provider.host_allow_list).map_err(|error| error.to_string())?;
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses = transport.resolve(host, port).await?;
    if !addresses
        .into_iter()
        .any(|ip| check_resolved_ip(ip).is_ok())
    {
        return Err("entitlement_ip_forbidden".to_string());
    }
    Ok(())
}

struct ParsedEntitlement {
    outcome: EntitlementQueryOutcome,
    token: Option<String>,
    configuration_version: Option<u64>,
}

/// Parse TS.43 WAP provisioning XML. The legacy direct-element form remains
/// accepted for existing fixtures, but only the ap2004 APPLICATION is used.
fn parse_entitlement_document(body: &str) -> ParsedEntitlement {
    let nodes = parse_characteristics(body);
    let vowifi = nodes.get(APP_VOWIFI).cloned().unwrap_or_default();
    let token_node = nodes.get("TOKEN").cloned().unwrap_or_default();
    let version_node = nodes.get("VERS").cloned().unwrap_or_default();

    let entitlement_raw = value_or_tag(&vowifi, "EntitlementStatus", body);
    let prov_raw = value_or_tag(&vowifi, "ProvStatus", body);
    let tc_raw = value_or_tag(&vowifi, "TC_Status", body).or_else(|| extract_tag(body, "tcStatus"));
    let addr_raw = value_or_tag(&vowifi, "AddrStatus", body);
    let server_flow_url = value_or_tag(&vowifi, "ServiceFlow_URL", body)
        .or_else(|| extract_tag(body, "ServerFlow_URL"));
    let server_flow_user_data = value_or_tag(&vowifi, "ServiceFlow_UserData", body)
        .or_else(|| extract_tag(body, "ServerFlow_User_Data"));

    let entitlement = entitlement_value(entitlement_raw.as_deref());
    let prov = substatus_value(prov_raw.as_deref(), false);
    let tc = substatus_value(tc_raw.as_deref(), true);
    let addr = substatus_value(addr_raw.as_deref(), true);
    let confirmed = server_flow_url.is_none()
        && entitlement == EntitlementStatusValue::Set
        && matches!(
            prov,
            EntitlementStatusValue::Set | EntitlementStatusValue::NotRequired
        )
        && matches!(
            tc,
            EntitlementStatusValue::Set | EntitlementStatusValue::NotRequired
        )
        && matches!(
            addr,
            EntitlementStatusValue::Set | EntitlementStatusValue::NotRequired
        );
    let state = if confirmed {
        E911State::Provisioned
    } else if entitlement == EntitlementStatusValue::Rejected
        || prov == EntitlementStatusValue::Rejected
        || addr == EntitlementStatusValue::Rejected
    {
        E911State::Rejected
    } else if server_flow_url.is_some() && tc == EntitlementStatusValue::NotSet {
        E911State::NeedsTerms
    } else if server_flow_url.is_some() && addr == EntitlementStatusValue::NotSet {
        E911State::NeedsAddress
    } else if server_flow_url.is_some() {
        E911State::NeedsUserAction
    } else {
        E911State::Unconfigured
    };

    ParsedEntitlement {
        outcome: EntitlementQueryOutcome {
            state,
            entitlement_status: entitlement,
            prov_status: prov,
            tc_status: tc,
            addr_status: addr,
            provider_reference: extract_tag(body, "ref"),
            server_flow_url,
            server_flow_user_data,
            retry_after_seconds: extract_tag(body, "retryAfter")
                .and_then(|value| value.parse::<u64>().ok()),
        },
        token: token_node.get("token").cloned(),
        configuration_version: version_node
            .get("version")
            .and_then(|value| value.parse::<u64>().ok()),
    }
}

fn parse_characteristics(body: &str) -> HashMap<String, HashMap<String, String>> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut nodes = HashMap::new();
    let mut current_type: Option<String> = None;
    let mut current_params = HashMap::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) if event.name().as_ref() == b"characteristic" => {
                current_type = attribute(&reader, &event, b"type");
                current_params.clear();
            }
            Ok(Event::Empty(event)) if event.name().as_ref() == b"parm" => {
                if current_type.is_some() {
                    if let (Some(name), Some(value)) = (
                        attribute(&reader, &event, b"name"),
                        attribute(&reader, &event, b"value"),
                    ) {
                        current_params.insert(name, value);
                    }
                }
            }
            Ok(Event::End(event)) if event.name().as_ref() == b"characteristic" => {
                if let Some(kind) = current_type.take() {
                    let key = if kind == "APPLICATION" {
                        current_params.get("AppID").cloned().unwrap_or(kind)
                    } else {
                        kind
                    };
                    nodes.insert(key, std::mem::take(&mut current_params));
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    nodes
}

fn attribute(
    reader: &Reader<&[u8]>,
    event: &quick_xml::events::BytesStart<'_>,
    name: &[u8],
) -> Option<String> {
    event
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.as_ref() == name)
        .and_then(|attribute| {
            attribute
                .decode_and_unescape_value(reader.decoder())
                .ok()
                .map(|value| value.into_owned())
        })
}

fn value_or_tag(values: &HashMap<String, String>, name: &str, body: &str) -> Option<String> {
    values
        .get(name)
        .cloned()
        .or_else(|| extract_tag(body, name))
        .or_else(|| extract_tag(body, &lower_first(name)))
}

fn lower_first(value: &str) -> String {
    let mut chars = value.chars();
    chars
        .next()
        .map(|first| first.to_ascii_lowercase().to_string() + chars.as_str())
        .unwrap_or_default()
}

fn extract_tag(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)?;
    let value = body[start..start + end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn entitlement_value(value: Option<&str>) -> EntitlementStatusValue {
    match value.unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "1" | "set" | "enabled" | "accepted" => EntitlementStatusValue::Set,
        "2" | "rejected" | "failed" | "incompatible" => EntitlementStatusValue::Rejected,
        "0" | "3" | "not_set" | "notset" | "disabled" | "provisioning" => {
            EntitlementStatusValue::NotSet
        }
        _ => EntitlementStatusValue::Unknown,
    }
}

fn substatus_value(value: Option<&str>, supports_not_required: bool) -> EntitlementStatusValue {
    match value.unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "1" | "set" | "available" | "accepted" | "provisioned" | "confirmed" => {
            EntitlementStatusValue::Set
        }
        "2" | "not_required" | "notrequired" if supports_not_required => {
            EntitlementStatusValue::NotRequired
        }
        "rejected" | "failed" => EntitlementStatusValue::Rejected,
        "0" | "3" | "not_set" | "notset" | "unknown" | "in_progress" => {
            EntitlementStatusValue::NotSet
        }
        _ => EntitlementStatusValue::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> EntitlementRequestContext {
        EntitlementRequestContext {
            imsi: "310260123456789".to_string(),
            mcc: "310".to_string(),
            mnc: "260".to_string(),
            terminal_id: Some("351234567890123".to_string()),
            terminal_vendor: "SimAdmin".to_string(),
            terminal_model: "test-model-long".to_string(),
            terminal_sw_version: "1.1.3".to_string(),
        }
    }

    #[test]
    fn builds_root_nai_and_full_auth_parameters() {
        let mut url = Url::parse("https://entitlement.example/query").unwrap();
        append_request_parameters(&mut url, &context(), &E911Secrets::default());
        let values = url.query_pairs().collect::<HashMap<_, _>>();
        assert_eq!(
            values.get("app").map(|value| value.as_ref()),
            Some(APP_VOWIFI)
        );
        assert_eq!(
            values.get("EAP_ID").map(|value| value.as_ref()),
            Some("0310260123456789@nai.epc.mnc260.mcc310.3gppnetwork.org")
        );
        assert_eq!(
            values.get("terminal_vendor").map(|value| value.as_ref()),
            Some("SimA")
        );
        assert!(!values.contains_key("IMSI"));
    }

    #[test]
    fn fast_auth_uses_token_and_imsi_without_eap_id() {
        let mut url = Url::parse("https://entitlement.example/query").unwrap();
        let secrets = E911Secrets {
            entitlement_token: Some("secret-token".to_string()),
            configuration_version: Some(7),
            ..E911Secrets::default()
        };
        append_request_parameters(&mut url, &context(), &secrets);
        let values = url.query_pairs().collect::<HashMap<_, _>>();
        assert_eq!(
            values.get("IMSI").map(|value| value.as_ref()),
            Some("310260123456789")
        );
        assert_eq!(values.get("vers").map(|value| value.as_ref()), Some("7"));
        assert!(!values.contains_key("EAP_ID"));
    }

    #[test]
    fn parses_wap_vowifi_status_token_and_websheet() {
        let body = r#"<wap-provisioningdoc>
          <characteristic type="VERS"><parm name="version" value="9"/></characteristic>
          <characteristic type="TOKEN"><parm name="token" value="opaque"/></characteristic>
          <characteristic type="APPLICATION">
            <parm name="AppID" value="ap2004"/>
            <parm name="EntitlementStatus" value="0"/>
            <parm name="ProvStatus" value="0"/>
            <parm name="TC_Status" value="1"/>
            <parm name="AddrStatus" value="0"/>
            <parm name="ServiceFlow_URL" value="https://websheet.example/address"/>
            <parm name="ServiceFlow_UserData" value="secret-state"/>
          </characteristic>
        </wap-provisioningdoc>"#;
        let parsed = parse_entitlement_document(body);
        assert_eq!(parsed.configuration_version, Some(9));
        assert_eq!(parsed.token.as_deref(), Some("opaque"));
        assert_eq!(parsed.outcome.state, E911State::NeedsAddress);
        assert_eq!(
            parsed.outcome.server_flow_url.as_deref(),
            Some("https://websheet.example/address")
        );
        assert!(!parsed.outcome.is_carrier_confirmed());
    }

    #[test]
    fn only_enabled_complete_status_is_confirmed() {
        let body = r#"<wap-provisioningdoc><characteristic type="APPLICATION">
          <parm name="AppID" value="ap2004"/>
          <parm name="EntitlementStatus" value="1"/>
          <parm name="ProvStatus" value="1"/>
          <parm name="TC_Status" value="2"/>
          <parm name="AddrStatus" value="1"/>
        </characteristic></wap-provisioningdoc>"#;
        let parsed = parse_entitlement_document(body);
        assert_eq!(parsed.outcome.state, E911State::Provisioned);
        assert!(parsed.outcome.is_carrier_confirmed());
    }

    #[test]
    fn decodes_eap_relay_json() {
        let body = br#"{"eap-relay-packet":"AQIDBA=="}"#;
        assert_eq!(
            extract_eap_relay_packet(body).unwrap(),
            Some(vec![1, 2, 3, 4])
        );
        assert_eq!(extract_eap_relay_packet(b"<xml/>").unwrap(), None);
    }
}
