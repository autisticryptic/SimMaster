use super::*;
use crate::hardware::devices::{
    ims_bearer_policy::{ImsApnAuthentication, DEFAULT_IMS_BEARER_REQUIREMENTS},
    transport::{ImsBearerHandle, ImsBearerInfo, ImsBearerTransport, TransportFuture},
};
use std::sync::atomic::{AtomicUsize, Ordering};

fn authenticated(method: ImsApnAuthMethod) -> ImsBearerRequirements {
    ImsBearerRequirements { authentication: ImsApnAuthentication {
        method, username: Some("fixture-user".into()), password: Some("fixture-password".into()),
    }, ..Default::default() }
}

struct Legacy {
    capabilities: ImsBearerCapabilities,
    calls: AtomicUsize,
}
impl ImsBearerTransport for Legacy {
    fn ims_bearer_capabilities(&self) -> ImsBearerCapabilities { self.capabilities }
    fn endpoint_available(&self, _: &str) -> bool { true }
    fn establish_ims_bearer<'a>(&'a self, _: &'a str, _: &'a str, _: &'a str,
        _: Option<u32>, _: u8, _: &'a [u8], _: bool, _: Option<(&'a str, u8)>,
    ) -> TransportFuture<'a, Result<(ImsBearerInfo, Box<dyn ImsBearerHandle + Send>), ImsBearerError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::AcqRel);
            Err(ImsBearerError { kind: ImsBearerErrorKind::SessionStartFailed,
                hint: ImsBearerFailureHint::None, detail: "fixture_dispatched".into() })
        })
    }
}
async fn dispatch(transport: &Legacy, requirements: &ImsBearerRequirements) -> ImsBearerError {
    transport.establish_ims_bearer_with_requirements("/dev/fixture", "fixture", "ims", None, 1,
        &[6, 4], false, None, requirements).await.err().unwrap()
}

#[test]
fn unknown_and_unsupported_are_not_executable_capabilities() {
    for support in [CapabilitySupport::Unknown, CapabilitySupport::Unsupported] {
        let capabilities = ImsBearerCapabilities { pap: support, chap: support,
            pap_or_chap: support, ..Default::default() };
        for method in [ImsApnAuthMethod::Pap, ImsApnAuthMethod::Chap, ImsApnAuthMethod::PapOrChap] {
            assert_eq!(capabilities.check(&authenticated(method)).unwrap_err().kind,
                ImsBearerErrorKind::RequirementsUnsupported);
        }
    }
}

#[tokio::test]
async fn old_default_provider_path_is_unchanged() {
    let transport = Legacy { capabilities: Default::default(), calls: AtomicUsize::new(0) };
    let result = dispatch(&transport, &DEFAULT_IMS_BEARER_REQUIREMENTS).await;
    assert_eq!(result.detail, "fixture_dispatched");
    assert_eq!(transport.calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn advertised_auth_without_an_adapter_cannot_drop_credentials() {
    let transport = Legacy { capabilities: ImsBearerCapabilities {
        pap: CapabilitySupport::Supported, ..Default::default()
    }, calls: AtomicUsize::new(0) };
    let result = dispatch(&transport, &authenticated(ImsApnAuthMethod::Pap)).await;
    assert_eq!(result.kind, ImsBearerErrorKind::RequirementsUnsupported);
    assert_eq!(result.detail, "ims_bearer_requirement_adapter_missing");
    assert_eq!(transport.calls.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn explicit_pco_and_no_auth_do_not_reuse_legacy_profile_defaults() {
    let transport = Legacy { capabilities: ImsBearerCapabilities::retained_mm(), calls: AtomicUsize::new(0) };
    for policy in [PcoRequestPolicy::Required, PcoRequestPolicy::Disabled] {
        let requirements = ImsBearerRequirements { pcscf_via_pco: policy, ..Default::default() };
        assert_eq!(dispatch(&transport, &requirements).await.kind, ImsBearerErrorKind::RequirementsUnsupported);
    }
    let requirements = ImsBearerRequirements { authentication: ImsApnAuthentication {
        method: ImsApnAuthMethod::None, ..Default::default()
    }, ..Default::default() };
    assert_eq!(dispatch(&transport, &requirements).await.kind, ImsBearerErrorKind::RequirementsUnsupported);
    assert_eq!(transport.calls.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn invalid_auth_is_rejected_before_dispatch_even_when_declared_supported() {
    let transport = Legacy { capabilities: ImsBearerCapabilities { pap: CapabilitySupport::Supported,
        ..Default::default() }, calls: AtomicUsize::new(0) };
    let mut requirements = authenticated(ImsApnAuthMethod::Pap);
    requirements.authentication.password = None;
    let result = dispatch(&transport, &requirements).await;
    assert_eq!(result.kind, ImsBearerErrorKind::RequirementsInvalid);
    assert_eq!(transport.calls.load(Ordering::Acquire), 0);
    let text = format!("{requirements:?} {result:?}");
    assert!(!text.contains("fixture-user"));
    assert!(!text.contains("fixture-password"));
}

#[tokio::test]
async fn dhcp_information_is_explicit_and_does_not_claim_address_assignment() {
    let requirements = ImsBearerRequirements { pcscf_via_dhcp: DhcpPcscfPolicy::InformationRequest,
        ..Default::default() };
    let unknown = Legacy { capabilities: Default::default(), calls: AtomicUsize::new(0) };
    assert_eq!(dispatch(&unknown, &requirements).await.kind, ImsBearerErrorKind::RequirementsUnsupported);
    assert_eq!(unknown.calls.load(Ordering::Acquire), 0);
    let supported = Legacy { capabilities: ImsBearerCapabilities { dhcp_information: CapabilitySupport::Supported,
        ..Default::default() }, calls: AtomicUsize::new(0) };
    assert_eq!(dispatch(&supported, &requirements).await.detail, "fixture_dispatched");
    assert_eq!(supported.calls.load(Ordering::Acquire), 1);
}
