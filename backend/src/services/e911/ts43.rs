//! Production TS.43 entitlement transport.
//!
//! Runs the GSMA TS.43-style query over HTTPS with the SSRF guard applied at
//! every hop. Implements the [`EntitlementTransport`] seam so the orchestrator
//! stays network-free. Secrets (`ServerFlow_User_Data`) are parsed but never
//! logged; the caller routes them to the E911 secret store.
//!
//! The EAP-AKA challenge/response is delegated to the caller-supplied `sim_auth`
//! closure. RAND/AUTN/RES/CK/IK/AUTS must never be logged by this module or by
//! the closure.

use reqwest::Client;

use crate::connectivity::core::entitlement::{
    E911State, EntitlementQueryOutcome, EntitlementStatusValue,
};
use crate::services::e911::orchestrator::EntitlementTransport;
use crate::services::e911::registry::E911Provider;
use crate::services::e911::ssrf::{
    check_resolved_ip, first_public_ip, validate_entitlement_target, validate_redirect,
    MAX_REDIRECTS, MAX_RESPONSE_BYTES,
};
use crate::services::e911::state_store::E911Secrets;
use futures_util::future::BoxFuture;

/// Production HTTPS transport. `resolver` is injectable for tests that must not
/// touch the network; the default resolves via the system.
pub struct Ts43Transport {
    client: Client,
    resolver: Option<std::sync::Arc<dyn Fn(&str) -> Vec<std::net::IpAddr> + Send + Sync>>,
}

impl Ts43Transport {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                // Never follow redirects automatically: we re-validate each hop.
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("failed to build TS.43 HTTP client"),
            resolver: None,
        }
    }

    pub fn with_resolver(
        resolver: impl Fn(&str) -> Vec<std::net::IpAddr> + Send + Sync + 'static,
    ) -> Self {
        Self {
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("failed to build TS.43 HTTP client"),
            resolver: Some(std::sync::Arc::new(resolver)),
        }
    }

    fn resolve(&self, host: &str) -> Vec<std::net::IpAddr> {
        match &self.resolver {
            Some(resolver) => resolver(host),
            None => tokio::runtime::Handle::current().block_on(async move {
                use tokio::net::lookup_host;
                match lookup_host((host, 443)).await {
                    Ok(iter) => iter.map(|addr| addr.ip()).collect(),
                    Err(_) => Vec::new(),
                }
            }),
        }
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
        secrets: &'a E911Secrets,
        sim_auth: &'a (dyn Fn(&[u8], &[u8]) -> Result<Vec<u8>, String> + Sync),
    ) -> BoxFuture<'a, Result<EntitlementQueryOutcome, String>> {
        Box::pin(self.run_query(provider, secrets, sim_auth))
    }
}

impl Ts43Transport {
    async fn run_query(
        &self,
        provider: &E911Provider,
        secrets: &E911Secrets,
        sim_auth: &(dyn Fn(&[u8], &[u8]) -> Result<Vec<u8>, String> + Sync),
    ) -> Result<EntitlementQueryOutcome, String> {
        let url = provider
            .entitlement_url
            .as_deref()
            .ok_or_else(|| "entitlement_url_missing".to_string())?;
        let allow_list = &provider.host_allow_list;

        // Initial SSRF gate: scheme + host allow-list.
        let target =
            validate_entitlement_target(url, allow_list).map_err(|error| error.to_string())?;
        let host = target
            .host_str()
            .ok_or_else(|| "entitlement_url_missing_host".to_string())?
            .to_ascii_lowercase();

        // DNS gate: at least one resolved address must be public.
        let resolved =
            first_public_ip(&host, |h| self.resolve(h)).map_err(|error| error.to_string())?;
        check_resolved_ip(resolved).map_err(|error| error.to_string())?;

        let mut current_url = target;
        let mut redirects = 0usize;
        loop {
            let response = self
                .client
                .get(current_url.clone())
                .header("Accept", "application/xml, text/xml, */*")
                .send()
                .await
                .map_err(|error| format!("transport:{}", error))?;

            let status = response.status();

            // TS.43 may challenge with HTTP Digest/AKA (401/407). The
            // challenge material is passed to the SIM auth adapter. We only
            // retry once and only when the provider is queryable.
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED
            {
                let challenge = response
                    .headers()
                    .get("www-authenticate")
                    .or_else(|| response.headers().get("proxy-authenticate"))
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                if let Some(outcome) = answer_aka_challenge(
                    &challenge,
                    sim_auth,
                    &self.client,
                    &current_url,
                    &provider.host_allow_list,
                )
                .await
                {
                    return Ok(outcome);
                }
            }

            if status.is_redirection() {
                redirects += 1;
                if redirects > MAX_REDIRECTS {
                    return Err("entitlement_too_many_redirects".to_string());
                }
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| "entitlement_redirect_without_location".to_string())?;
                // Re-validate the redirect target against the same allow-list.
                current_url =
                    validate_redirect(location, allow_list).map_err(|error| error.to_string())?;
                let host = current_url
                    .host_str()
                    .ok_or_else(|| "entitlement_url_missing_host".to_string())?
                    .to_ascii_lowercase();
                let resolved = first_public_ip(&host, |h| self.resolve(h))
                    .map_err(|error| error.to_string())?;
                check_resolved_ip(resolved).map_err(|error| error.to_string())?;
                continue;
            }

            if !status.is_success() {
                return Err(format!("entitlement_http_status:{}", status.as_u16()));
            }

            let bytes = response
                .bytes()
                .await
                .map_err(|error| format!("transport:{}", error))?;
            if bytes.len() > MAX_RESPONSE_BYTES {
                return Err("entitlement_response_too_large".to_string());
            }
            let body = String::from_utf8_lossy(&bytes);
            return Ok(parse_entitlement_response(&body, secrets));
        }
    }
}

/// If the challenge is an AKA-style digest challenge, run the SIM auth adapter
/// and retry once with an Authorization header. Returns `Some(outcome)` when a
/// final response was obtained; `None` when the challenge cannot be answered
/// (so the caller reports the original HTTP status).
async fn answer_aka_challenge(
    challenge: &str,
    sim_auth: &(dyn Fn(&[u8], &[u8]) -> Result<Vec<u8>, String> + Sync),
    client: &Client,
    url: &url::Url,
    allow_list: &[String],
) -> Option<EntitlementQueryOutcome> {
    let (rand, autn) = extract_aka_challenge(challenge)?;
    let res = sim_auth(&rand, &autn).ok()?;
    // Minimal EAP-AKA auth digest: base64 of RES, sent as a bearer-ish token is
    // NOT TS.43; the real protocol computes a digest with CK/IK. This module
    // intentionally refuses to fabricate a valid Authorization header and keeps
    // the challenge-response primitive pure so callers can layer the real
    // digest computation. Returning None here makes the query surface the
    // original 401 status until a verified digest adapter exists.
    let _ = (client, url, allow_list, res);
    None
}

/// Pull `rand`/`autn` from an AKA challenge string. This is a minimal parser
/// for `<rand>..</rand><autn>..</autn>` style challenges.
fn extract_aka_challenge(challenge: &str) -> Option<(Vec<u8>, Vec<u8>)> {
    use base64::Engine;
    let rand = extract_tag(challenge, "rand")?;
    let autn = extract_tag(challenge, "autn")?;
    let decode = |value: &str| -> Option<Vec<u8>> {
        let hex = value.trim();
        // Some carriers send hex. Prefer hex when it looks like hex: base64's
        // alphabet overlaps lowercase a-f, so a pure-hex string is also valid
        // base64 and would otherwise be mis-decoded.
        if hex.len() % 2 == 0 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
                .collect::<Result<Vec<_>, _>>()
                .ok();
        }
        base64::engine::general_purpose::STANDARD.decode(value).ok()
    };
    let rand = decode(&rand)?;
    let autn = decode(&autn)?;
    if rand.len() == 16 && autn.len() == 16 {
        Some((rand, autn))
    } else {
        None
    }
}

fn extract_tag(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)?;
    let value = body[start..start + end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Parse a TS.43 entitlement XML response into an outcome. `secrets` is only
/// used as the sink for the secret `ServerFlow_User_Data` — it is never echoed.
pub fn parse_entitlement_response(body: &str, _secrets: &E911Secrets) -> EntitlementQueryOutcome {
    let prov_raw = extract_tag(body, "provStatus").unwrap_or_default();
    let tc_raw = extract_tag(body, "tcStatus").unwrap_or_default();
    let addr_raw = extract_tag(body, "addrStatus").unwrap_or_default();
    let provider_reference = extract_tag(body, "ref");
    let server_flow_url = extract_tag(body, "ServerFlow_URL");
    let server_flow_user_data = extract_tag(body, "ServerFlow_User_Data");
    let retry_after = extract_tag(body, "retryAfter").and_then(|value| value.parse::<u64>().ok());

    let status_value = |value: &str| match value.to_ascii_lowercase().as_str() {
        "set" | "accepted" | "provisioned" | "confirmed" => EntitlementStatusValue::Set,
        "rejected" | "failed" => EntitlementStatusValue::Rejected,
        "not_required" | "notrequired" => EntitlementStatusValue::NotRequired,
        "not_set" | "notset" | "unknown" => EntitlementStatusValue::NotSet,
        _ => EntitlementStatusValue::Unknown,
    };

    let prov = status_value(&prov_raw);
    let tc = status_value(&tc_raw);
    let addr = status_value(&addr_raw);
    let confirmed = server_flow_url.is_none()
        && prov == EntitlementStatusValue::Set
        && tc != EntitlementStatusValue::Rejected
        && addr == EntitlementStatusValue::Set;
    let state = if confirmed {
        E911State::Provisioned
    } else if prov == EntitlementStatusValue::Rejected || addr == EntitlementStatusValue::Rejected {
        E911State::Rejected
    } else if server_flow_url.is_some() && tc == EntitlementStatusValue::NotSet {
        E911State::NeedsTerms
    } else if server_flow_url.is_some() {
        E911State::NeedsAddress
    } else {
        E911State::Unconfigured
    };

    EntitlementQueryOutcome {
        state,
        prov_status: prov,
        tc_status: tc,
        addr_status: addr,
        provider_reference,
        server_flow_url,
        server_flow_user_data,
        retry_after_seconds: retry_after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_provisioned_response() {
        let secrets = E911Secrets::default();
        let body = r#"<entitlementResponse>
            <ref>ref-77</ref>
            <provStatus>set</provStatus>
            <tcStatus>set</tcStatus>
            <addrStatus>set</addrStatus>
        </entitlementResponse>"#;
        let outcome = parse_entitlement_response(body, &secrets);
        assert_eq!(outcome.state, E911State::Provisioned);
        assert!(outcome.is_carrier_confirmed());
        assert_eq!(outcome.provider_reference.as_deref(), Some("ref-77"));
        assert!(outcome.server_flow_url.is_none());
        assert!(outcome.server_flow_user_data.is_none());
    }

    #[test]
    fn parses_websheet_response_with_secret_data() {
        let secrets = E911Secrets::default();
        let body = r#"<entitlementResponse>
            <provStatus>not_set</provStatus>
            <tcStatus>set</tcStatus>
            <addrStatus>not_set</addrStatus>
            <ServerFlow_URL>https://websheet.example.net/terms</ServerFlow_URL>
            <ServerFlow_User_Data>csrf-secret-value</ServerFlow_User_Data>
            <retryAfter>300</retryAfter>
        </entitlementResponse>"#;
        let outcome = parse_entitlement_response(body, &secrets);
        assert_eq!(outcome.state, E911State::NeedsAddress);
        assert!(!outcome.is_carrier_confirmed());
        assert_eq!(
            outcome.server_flow_url.as_deref(),
            Some("https://websheet.example.net/terms")
        );
        assert_eq!(
            outcome.server_flow_user_data.as_deref(),
            Some("csrf-secret-value")
        );
        assert_eq!(outcome.retry_after_seconds, Some(300));
    }

    #[test]
    fn parses_rejected_response() {
        let secrets = E911Secrets::default();
        let body = r#"<entitlementResponse>
            <addrStatus>rejected</addrStatus>
        </entitlementResponse>"#;
        let outcome = parse_entitlement_response(body, &secrets);
        assert_eq!(outcome.state, E911State::Rejected);
        assert_eq!(outcome.addr_status, EntitlementStatusValue::Rejected);
        assert!(!outcome.is_carrier_confirmed());
    }

    #[test]
    fn empty_body_is_unknown_unconfigured() {
        let secrets = E911Secrets::default();
        let outcome = parse_entitlement_response("", &secrets);
        assert_eq!(outcome.state, E911State::Unconfigured);
        assert_eq!(outcome.prov_status, EntitlementStatusValue::Unknown);
        assert!(!outcome.is_carrier_confirmed());
    }

    #[test]
    fn extracts_aka_challenge_from_digest() {
        let challenge = r#"Digest realm="entitlement", nonce="..." <rand>AQIDBAUGBwgJCgsMDQ4PEA==</rand><autn>ERESREVGR0hJSktMTQ4PEA==</autn>"#;
        let (rand, autn) = extract_aka_challenge(challenge).unwrap();
        assert_eq!(rand.len(), 16);
        assert_eq!(autn.len(), 16);
    }

    #[test]
    fn extract_aka_challenge_handles_hex() {
        let challenge = "<rand>0102030405060708090a0b0c0d0e0f10</rand><autn>1112131415161718191a1b1c1d1e1f20</autn>";
        let (rand, autn) = extract_aka_challenge(challenge).unwrap();
        assert_eq!(rand[0], 0x01);
        assert_eq!(autn[15], 0x20);
    }

    #[test]
    fn extract_tag_is_empty_safe() {
        assert_eq!(extract_tag("<a>x</a>", "b"), None);
        assert_eq!(extract_tag("", "a"), None);
        assert_eq!(extract_tag("<a>   </a>", "a"), None);
    }
}
