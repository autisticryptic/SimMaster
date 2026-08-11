//! Shared Ut/XCAP transaction orchestration.
//!
//! Access adapters implement only `XcapTransport`: VoLTE sends over the IMS
//! bearer and VoWiFi sends through the ePDG tunnel.  The optimistic concurrency
//! and network-authoritative readback rules stay identical.

use std::{net::IpAddr, sync::Arc, time::Duration};

use futures_util::StreamExt;

use crate::connectivity::core::registration::ImsRegistrationAccess;
use crate::connectivity::core::ut::{
    build_xcap_get, build_xcap_partial_put, build_xcap_put, UtDocument, UtDocumentKind, UtError,
    UtMutation, XcapPolicy, XcapRequest,
};
use crate::connectivity::modems::ims::vowifi::profiles::CarrierProfile;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XcapResponse {
    pub status: u16,
    pub etag: Option<String>,
    pub body: Vec<u8>,
}

pub trait XcapTransport: Send + Sync {
    async fn execute(&self, request: XcapRequest) -> Result<XcapResponse, UtError>;
}

/// Access adapters provide line-scoped Digest-AKA proof generation. The
/// transport never stores a nonce or subscriber credential.
pub trait XcapDigestProvider: Send + Sync {
    fn authorize<'a>(
        &'a self,
        challenge: &'a str,
        proxy: bool,
        method: &'a str,
        uri: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<String, UtError>>;
}

/// Immutable access-specific material captured from one active registration.
/// The HTTP client owns no global SIM identity and cannot silently switch to a
/// different line while a request is in flight.
pub struct XcapAccessContext {
    pub access: ImsRegistrationAccess,
    pub profile: &'static CarrierProfile,
    pub local_address: IpAddr,
    pub digest: Arc<dyn XcapDigestProvider>,
}

/// HTTPS XCAP transport shared by VoLTE and VoWiFi. `local_address` pins the
/// socket to the active IMS route (LTE bearer or ePDG/TUN gateway); redirects
/// and oversized bodies fail closed.
pub struct HttpXcapTransport {
    client: reqwest::Client,
    digest: Option<Arc<dyn XcapDigestProvider>>,
    max_response_bytes: usize,
}

impl HttpXcapTransport {
    pub fn new(local_address: Option<IpAddr>, policy: &XcapPolicy) -> Result<Self, UtError> {
        policy.validate()?;
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .https_only(true)
            .min_tls_version(parse_tls_version(&policy.tls_min_version)?)
            .max_tls_version(parse_tls_version(&policy.tls_max_version)?)
            .tls_built_in_root_certs(policy.tls_builtin_roots);
        let additional_ca = policy
            .tls_additional_ca_pem
            .as_deref()
            .filter(|value| !value.trim().is_empty());
        if let Some(pem) = additional_ca {
            let certificate = reqwest::Certificate::from_pem(pem.as_bytes())
                .map_err(|_| UtError::new("ut_xcap_tls_ca_invalid"))?;
            builder = builder.add_root_certificate(certificate);
        }
        if let Some(address) = local_address {
            builder = builder.local_address(address);
        }
        let client = builder.build().map_err(|_| {
            UtError::new(if additional_ca.is_some() {
                "ut_xcap_tls_ca_invalid"
            } else {
                "ut_xcap_client_build_failed"
            })
        })?;
        Ok(Self {
            client,
            digest: None,
            max_response_bytes: 512 * 1024,
        })
    }

    pub fn with_digest_provider(mut self, provider: Arc<dyn XcapDigestProvider>) -> Self {
        self.digest = Some(provider);
        self
    }

    pub fn with_max_response_bytes(mut self, max_response_bytes: usize) -> Self {
        self.max_response_bytes = max_response_bytes.max(1024);
        self
    }

    async fn send_once(
        &self,
        request: &XcapRequest,
        authorization: Option<&str>,
    ) -> Result<reqwest::Response, UtError> {
        let mut builder = match request.method {
            "GET" => self.client.get(&request.uri),
            "PUT" => self.client.put(&request.uri),
            _ => return Err(UtError::new("ut_xcap_method_unsupported")),
        };
        if let Some(etag) = request.if_match.as_deref() {
            builder = builder.header("If-Match", etag);
        }
        if let Some(value) = authorization {
            let (name, value) = value
                .split_once(':')
                .ok_or_else(|| UtError::new("ut_xcap_authorization_invalid"))?;
            builder = builder.header(name.trim(), value.trim());
        }
        if let Some(body) = request.body.as_deref() {
            builder = builder
                .header(
                    "Content-Type",
                    request.content_type.unwrap_or("application/simservs+xml"),
                )
                .body(body.to_string());
        }
        builder
            .send()
            .await
            .map_err(|_| UtError::new("ut_xcap_transport_failed"))
    }
}

fn parse_tls_version(value: &str) -> Result<reqwest::tls::Version, UtError> {
    match value.trim() {
        "1.2" | "tls1.2" => Ok(reqwest::tls::Version::TLS_1_2),
        "1.3" | "tls1.3" => Ok(reqwest::tls::Version::TLS_1_3),
        _ => Err(UtError::new("ut_xcap_tls_version_invalid")),
    }
}

/// Extract the explicit XCAP policy from a carrier catalog record. A missing
/// policy means UT is unsupported for that carrier, not that a caller may
/// infer an endpoint from its IMS registrar.
pub fn xcap_policy_from_carrier(profile: &CarrierProfile) -> Result<Option<XcapPolicy>, UtError> {
    if !profile.ut.enabled {
        return Ok(None);
    }
    if profile.ut.authentication != "digest_aka" {
        return Err(UtError::new("ut_xcap_authentication_required"));
    }
    let root = profile
        .ut
        .xcap_root
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| UtError::new("ut_xcap_root_required"))?;
    let document_selector = profile
        .ut
        .document_selector
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| UtError::new("ut_xcap_policy_incomplete"))?;
    let namespace = profile
        .ut
        .namespace
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| UtError::new("ut_xcap_policy_incomplete"))?;
    let policy = XcapPolicy {
        root: root.to_string(),
        document_selector: document_selector.to_string(),
        namespace: namespace.to_string(),
        partial_update: profile.ut.partial_update,
        call_waiting_selector: profile.ut.call_waiting_selector.map(str::to_string),
        diversion_rule_selector: profile.ut.diversion_rule_selector.map(str::to_string),
        oip_selector: profile.ut.oip_selector.map(str::to_string),
        oir_selector: profile.ut.oir_selector.map(str::to_string),
        tls_min_version: profile.ut.tls_min_version.to_string(),
        tls_max_version: profile.ut.tls_max_version.to_string(),
        tls_builtin_roots: profile.ut.tls_builtin_roots,
        tls_additional_ca_pem: profile.ut.tls_additional_ca_pem.map(str::to_string),
    };
    policy.validate()?;
    Ok(Some(policy))
}

impl XcapTransport for HttpXcapTransport {
    async fn execute(&self, request: XcapRequest) -> Result<XcapResponse, UtError> {
        let response = self.send_once(&request, None).await?;
        let response = if matches!(response.status().as_u16(), 401 | 407) {
            let provider = self
                .digest
                .as_ref()
                .ok_or_else(|| UtError::new("ut_xcap_authentication_required"))?;
            let header = response
                .headers()
                .get(if response.status().as_u16() == 407 {
                    "proxy-authenticate"
                } else {
                    "www-authenticate"
                })
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| UtError::new("ut_xcap_challenge_missing"))?
                .to_string();
            let authorization = provider
                .authorize(
                    &header,
                    response.status().as_u16() == 407,
                    request.method,
                    &request.uri,
                )
                .await?;
            self.send_once(&request, Some(&authorization)).await?
        } else {
            response
        };
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            return Err(UtError::new("ut_xcap_redirect_rejected"));
        }
        let etag = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        if response
            .content_length()
            .is_some_and(|length| length > self.max_response_bytes as u64)
        {
            return Err(UtError::new("ut_xcap_response_too_large"));
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| UtError::new("ut_xcap_response_read_failed"))?;
            if body.len().saturating_add(chunk.len()) > self.max_response_bytes {
                return Err(UtError::new("ut_xcap_response_too_large"));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(XcapResponse { status, etag, body })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UtUpdateOutcome {
    pub document: UtDocument,
    pub changed: bool,
}

pub async fn read_document<T: XcapTransport>(
    transport: &T,
    policy: &XcapPolicy,
    kind: UtDocumentKind,
) -> Result<UtDocument, UtError> {
    let response = transport.execute(build_xcap_get(policy, kind)?).await?;
    if response.status != 200 {
        return Err(UtError::new("ut_xcap_get_failed"));
    }
    let mut document = UtDocument::parse(kind, &response.body)?;
    document.etag = response.etag;
    Ok(document)
}

/// GET -> mutate -> If-Match PUT -> GET verify.
///
/// A successful PUT is never treated as authoritative. The returned document
/// is always the second GET, so access handover and server-side normalization
/// cannot leave local state pretending that an unconfirmed rule is active.
pub async fn update_document<T>(
    transport: &T,
    policy: &XcapPolicy,
    kind: UtDocumentKind,
    mutation: UtMutation,
) -> Result<UtUpdateOutcome, UtError>
where
    T: XcapTransport,
{
    let mut desired = read_document(transport, policy, kind).await?;
    let before = desired.clone();
    mutation.apply(&mut desired)?;
    if desired.semantically_matches(&before) {
        return Ok(UtUpdateOutcome {
            document: before,
            changed: false,
        });
    }
    let request = match build_xcap_partial_put(policy, &desired, &mutation)? {
        Some(request) => request,
        None => build_xcap_put(policy, &desired)?,
    };
    let response = transport.execute(request).await?;
    if !matches!(response.status, 200 | 201 | 204) {
        return Err(match response.status {
            409 | 412 => UtError::new("ut_xcap_etag_conflict"),
            401 | 407 => UtError::new("ut_xcap_authentication_required"),
            _ => UtError::new("ut_xcap_put_failed"),
        });
    }
    let confirmed = read_document(transport, policy, kind).await?;
    if !confirmed.semantically_matches(&desired) {
        return Err(UtError::new("ut_xcap_readback_mismatch"));
    }
    Ok(UtUpdateOutcome {
        document: confirmed,
        changed: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, sync::Mutex};

    struct FakeTransport {
        responses: Mutex<VecDeque<XcapResponse>>,
        requests: Mutex<Vec<XcapRequest>>,
    }

    impl FakeTransport {
        fn new(responses: Vec<XcapResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl XcapTransport for FakeTransport {
        async fn execute(&self, request: XcapRequest) -> Result<XcapResponse, UtError> {
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| UtError::new("ut_test_response_missing"))
        }
    }

    fn policy() -> XcapPolicy {
        XcapPolicy {
            root: "https://xcap.example.test".to_string(),
            document_selector: "simadmin/users/subscriber".to_string(),
            namespace: "urn:3gpp:ns:communication-waiting".to_string(),
            partial_update: false,
            call_waiting_selector: None,
            diversion_rule_selector: None,
            oip_selector: None,
            oir_selector: None,
            tls_min_version: "1.2".to_string(),
            tls_max_version: "1.3".to_string(),
            tls_builtin_roots: true,
            tls_additional_ca_pem: None,
        }
    }

    #[test]
    fn carrier_without_explicit_ut_policy_stays_unsupported() {
        let profile =
            crate::connectivity::modems::ims::vowifi::profiles::generate_standard_3gpp_profile(
                "234", "33", 2,
            );
        assert!(xcap_policy_from_carrier(profile).unwrap().is_none());
    }

    #[test]
    fn carrier_ut_policy_requires_digest_aka_and_https() {
        use crate::connectivity::modems::ims::vowifi::profile_record::CarrierProfileRecord;

        let base =
            crate::connectivity::modems::ims::vowifi::profiles::generate_standard_3gpp_profile(
                "234", "33", 2,
            );
        let mut record = CarrierProfileRecord::from_profile(base);
        record.ut.enabled = true;
        record.ut.xcap_root = Some("https://xcap.example.test/root".to_string());
        record.ut.document_selector = Some("simadmin/users/subscriber".to_string());
        record.ut.namespace = Some("urn:3gpp:ns:communication-waiting".to_string());
        record.ut.authentication = "digest_aka".to_string();
        let profile = record.intern();
        assert_eq!(
            xcap_policy_from_carrier(profile).unwrap().unwrap().root,
            "https://xcap.example.test/root"
        );

        record.ut.authentication = "none".to_string();
        let profile = record.intern();
        assert_eq!(
            xcap_policy_from_carrier(profile).unwrap_err().code(),
            "ut_xcap_authentication_required"
        );
    }

    #[test]
    fn https_transport_applies_tls_policy_and_rejects_invalid_carrier_ca() {
        let policy = policy();
        HttpXcapTransport::new(None, &policy).expect("default verified TLS policy");

        let mut private_ca = policy;
        private_ca.tls_builtin_roots = false;
        private_ca.tls_additional_ca_pem =
            Some("-----BEGIN CERTIFICATE-----\ninvalid\n-----END CERTIFICATE-----".to_string());
        assert_eq!(
            HttpXcapTransport::new(None, &private_ca)
                .err()
                .expect("invalid carrier CA must fail")
                .code(),
            "ut_xcap_tls_ca_invalid"
        );
    }

    fn response(status: u16, etag: Option<&str>, active: bool) -> XcapResponse {
        XcapResponse {
            status,
            etag: etag.map(str::to_string),
            body: format!(
                "<communication-waiting><active>{active}</active><vendor:extension xmlns:vendor=\"urn:vendor\">keep</vendor:extension></communication-waiting>"
            )
            .into_bytes(),
        }
    }

    #[tokio::test]
    async fn update_is_get_put_get_and_preserves_extension() {
        let transport = FakeTransport::new(vec![
            response(200, Some("v1"), false),
            response(204, None, false),
            response(200, Some("v2"), true),
        ]);
        let outcome = update_document(
            &transport,
            &policy(),
            UtDocumentKind::CommunicationWaiting,
            UtMutation::CallWaiting(true),
        )
        .await
        .unwrap();
        assert!(outcome.changed);
        assert_eq!(outcome.document.call_waiting, Some(true));
        let requests = transport.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .map(|request| request.method)
                .collect::<Vec<_>>(),
            ["GET", "PUT", "GET"]
        );
        assert_eq!(requests[1].if_match.as_deref(), Some("v1"));
        assert!(requests[1]
            .body
            .as_deref()
            .unwrap()
            .contains("vendor:extension"));
        assert_eq!(requests[1].content_type, Some("application/simservs+xml"));
    }

    #[tokio::test]
    async fn explicit_partial_policy_uses_element_selector_and_still_reads_back() {
        let transport = FakeTransport::new(vec![
            response(200, Some("v1"), false),
            response(204, None, false),
            response(200, Some("v2"), true),
        ]);
        let mut policy = policy();
        policy.partial_update = true;
        policy.call_waiting_selector =
            Some("ss:communication-waiting/ss:active?xmlns(ss=urn:3gpp:ns:xml:simservs)".into());

        let outcome = update_document(
            &transport,
            &policy,
            UtDocumentKind::CommunicationWaiting,
            UtMutation::CallWaiting(true),
        )
        .await
        .unwrap();

        assert!(outcome.changed);
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[1]
            .uri
            .contains("/~~/ss:communication-waiting/ss:active"));
        assert_eq!(requests[1].if_match.as_deref(), Some("v1"));
        assert_eq!(requests[1].content_type, Some("application/xcap-el+xml"));
        assert_eq!(
            requests[1].body.as_deref(),
            Some("<active xmlns=\"urn:3gpp:ns:communication-waiting\">true</active>")
        );
        assert!(!requests[1]
            .body
            .as_deref()
            .unwrap()
            .contains("vendor:extension"));
    }

    #[tokio::test]
    async fn partial_capability_without_selector_for_document_uses_full_put() {
        let transport = FakeTransport::new(vec![
            response(200, Some("v1"), false),
            response(204, None, false),
            response(200, Some("v2"), true),
        ]);
        let mut policy = policy();
        policy.partial_update = true;
        policy.oip_selector = Some("ss:oip/ss:active".into());

        update_document(
            &transport,
            &policy,
            UtDocumentKind::CommunicationWaiting,
            UtMutation::CallWaiting(true),
        )
        .await
        .unwrap();

        let requests = transport.requests.lock().unwrap();
        assert!(!requests[1].uri.contains("/~~/"));
        assert_eq!(requests[1].content_type, Some("application/simservs+xml"));
    }

    #[tokio::test]
    async fn etag_conflict_never_claims_success_or_reads_back() {
        let transport = FakeTransport::new(vec![
            response(200, Some("v1"), false),
            response(412, None, false),
        ]);
        let error = update_document(
            &transport,
            &policy(),
            UtDocumentKind::CommunicationWaiting,
            UtMutation::CallWaiting(true),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "ut_xcap_etag_conflict");
        assert_eq!(transport.requests.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn mismatched_readback_is_a_failure() {
        let transport = FakeTransport::new(vec![
            response(200, Some("v1"), false),
            response(204, None, false),
            response(200, Some("v2"), false),
        ]);
        let error = update_document(
            &transport,
            &policy(),
            UtDocumentKind::CommunicationWaiting,
            UtMutation::CallWaiting(true),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "ut_xcap_readback_mismatch");
    }
}
