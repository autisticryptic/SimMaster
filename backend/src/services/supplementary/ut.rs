//! Shared Ut/XCAP transaction orchestration.
//!
//! Access adapters implement only `XcapTransport`: VoLTE sends over the IMS
//! bearer and VoWiFi sends through the ePDG tunnel.  The optimistic concurrency
//! and network-authoritative readback rules stay identical.

use crate::connectivity::core::ut::{
    build_xcap_get, build_xcap_put, UtDocument, UtDocumentKind, UtError, XcapPolicy, XcapRequest,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XcapResponse {
    pub status: u16,
    pub etag: Option<String>,
    pub body: Vec<u8>,
}

pub trait XcapTransport: Send + Sync {
    async fn execute(&self, request: XcapRequest) -> Result<XcapResponse, UtError>;
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
pub async fn update_document<T, F>(
    transport: &T,
    policy: &XcapPolicy,
    kind: UtDocumentKind,
    mutate: F,
) -> Result<UtUpdateOutcome, UtError>
where
    T: XcapTransport,
    F: FnOnce(&mut UtDocument),
{
    let mut desired = read_document(transport, policy, kind).await?;
    let before = desired.clone();
    mutate(&mut desired);
    if desired.semantically_matches(&before) {
        return Ok(UtUpdateOutcome {
            document: before,
            changed: false,
        });
    }
    let response = transport.execute(build_xcap_put(policy, &desired)?).await?;
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
        }
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
            |document| document.set_call_waiting(true),
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
            |document| document.set_call_waiting(true),
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
            |document| document.set_call_waiting(true),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "ut_xcap_readback_mismatch");
    }
}
