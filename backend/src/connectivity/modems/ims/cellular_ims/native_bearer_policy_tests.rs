//! Production strategy tests: requirements cannot disappear between families.
use super::*;
use crate::hardware::devices::{
    ims_bearer_policy::{ImsApnAuthentication, ImsApnAuthMethod, ImsBearerRequirements, PcoRequestPolicy},
    transport::{CapabilitySupport, ImsBearerCapabilities, TransportFuture},
};
use std::{collections::VecDeque, sync::Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Sent {
    apn: String,
    profile: Option<u32>,
    cid: u8,
    families: Vec<u8>,
    roaming: bool,
    sim: Option<(String, u8)>,
    requirements: ImsBearerRequirements,
}
struct Provider {
    supported: bool,
    sent: Mutex<Vec<Sent>>,
    outcomes: Mutex<VecDeque<(ImsBearerErrorKind, ImsBearerFailureHint)>>,
}
struct Handle;
impl ImsBearerHandle for Handle {
    fn check_liveness(&mut self) -> Result<(), ImsBearerError> { Ok(()) }
    fn release(self: Box<Self>) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>> {
        Box::pin(async {})
    }
}
impl ImsBearerTransport for Provider {
    fn ims_bearer_capabilities(&self) -> ImsBearerCapabilities {
        ImsBearerCapabilities { pap: if self.supported { CapabilitySupport::Supported }
            else { CapabilitySupport::Unknown }, ..Default::default() }
    }
    fn endpoint_available(&self, _: &str) -> bool { true }
    fn establish_ims_bearer<'a>(&'a self, _: &'a str, _: &'a str, _: &'a str,
        _: Option<u32>, _: u8, _: &'a [u8], _: bool, _: Option<(&'a str, u8)>,
    ) -> TransportFuture<'a, Result<(ImsBearerInfo, Box<dyn ImsBearerHandle + Send>), ImsBearerError>> {
        panic!("strategy must use requirement-aware provider seam")
    }
    fn establish_ims_bearer_with_requirements<'a>(&'a self, _: &'a str, _: &'a str, apn: &'a str,
        profile: Option<u32>, cid: u8, families: &'a [u8], roaming: bool,
        sim: Option<(&'a str, u8)>, requirements: &'a ImsBearerRequirements,
    ) -> TransportFuture<'a, Result<(ImsBearerInfo, Box<dyn ImsBearerHandle + Send>), ImsBearerError>> {
        Box::pin(async move {
            self.validate_ims_bearer_requirements(requirements)?;
            self.sent.lock().unwrap().push(Sent { apn: apn.into(), profile, cid,
                families: families.to_vec(), roaming, sim: sim.map(|(id, slot)| (id.into(), slot)),
                requirements: requirements.clone() });
            if let Some((kind, hint)) = self.outcomes.lock().unwrap().pop_front() {
                return Err(ImsBearerError { kind, hint, detail: "fixture_refusal".into() });
            }
            Ok((ImsBearerInfo { interface: "fixture0".into(), ip_type: "ipv4".into(),
                ipv4_address: Some("192.0.2.2".parse().unwrap()), ipv4_prefix: Some(32),
                ..Default::default() }, Box::new(Handle) as Box<dyn ImsBearerHandle + Send>))
        })
    }
}
fn request() -> BearerRequest {
    let mut request = BearerRequest::for_apn("ims.example", false);
    request.profile_id = Some(7);
    request.requirements.authentication = ImsApnAuthentication { method: ImsApnAuthMethod::Pap,
        username: Some("test-user".into()), password: Some("test-password".into()) };
    request
}
fn provider(supported: bool, outcomes: Vec<(ImsBearerErrorKind, ImsBearerFailureHint)>) -> Provider {
    Provider { supported, sent: Mutex::new(Vec::new()), outcomes: Mutex::new(outcomes.into()) }
}
async fn establish(provider: &Provider, request: &BearerRequest) -> Result<NativeImsBearer, CellularImsError> {
    establish_native_ims_bearer(provider, "/dev/fixture", "fixture", request,
        &ImsConnectionPlan::default(), Some(&("8900000000000000001".into(), 1))).await
}

#[tokio::test]
async fn capability_or_credential_rejection_precedes_all_family_attempts() {
    let provider = provider(false, vec![]);
    let mut request = request();
    assert_eq!(establish(&provider, &request).await.err().unwrap().code(), code::BEARER_REQUIREMENTS_UNSUPPORTED);
    request.requirements.authentication.password = None;
    assert_eq!(establish(&provider, &request).await.err().unwrap().code(), code::BEARER_REQUIREMENTS_INVALID);
    assert!(provider.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn actual_family_loop_preserves_auth_apn_pin_and_sim() {
    let provider = provider(true, vec![(ImsBearerErrorKind::SessionStartFailed, ImsBearerFailureHint::None); 2]);
    let request = request();
    let bearer = establish(&provider, &request).await.unwrap();
    assert_eq!(bearer.connection.ip_type, "ipv4");
    let sent = provider.sent.lock().unwrap();
    assert_eq!(sent.iter().map(|s| s.families.clone()).collect::<Vec<_>>(), vec![vec![6,4],vec![6],vec![4]]);
    for value in sent.iter() {
        assert_eq!(value.requirements, request.requirements);
        assert_eq!(value.apn, request.apn);
        assert_eq!(value.profile, Some(7));
        assert_eq!(value.cid, 7);
        assert_eq!(value.sim, Some(("8900000000000000001".into(), 1)));
        assert!(!value.roaming);
    }
}

#[tokio::test]
async fn forced_family_keeps_requirements_and_valid_single_grant() {
    let provider = provider(true, vec![(ImsBearerErrorKind::SessionStartFailed, ImsBearerFailureHint::NetworkForcedIpv4)]);
    let request = request();
    establish(&provider, &request).await.unwrap();
    let sent = provider.sent.lock().unwrap();
    assert_eq!(sent.iter().map(|s| s.families.clone()).collect::<Vec<_>>(), vec![vec![6,4],vec![4]]);
    assert!(sent.iter().all(|s| s.requirements == request.requirements && s.profile == Some(7)));
}

#[tokio::test]
async fn late_provider_policy_refusal_does_not_reopen_family_loop() {
    let provider = provider(true, vec![(ImsBearerErrorKind::RequirementsUnsupported, ImsBearerFailureHint::None)]);
    let error = establish(&provider, &request()).await.err().unwrap();
    assert_eq!(error.code(), code::BEARER_REQUIREMENTS_UNSUPPORTED);
    assert_eq!(provider.sent.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn explicit_pco_policy_never_becomes_reporting_or_apn_only() {
    let provider = provider(true, vec![]);
    let mut request = request();
    request.requirements.pcscf_via_pco = PcoRequestPolicy::Required;
    assert_eq!(establish(&provider, &request).await.err().unwrap().code(), code::BEARER_REQUIREMENTS_UNSUPPORTED);
    assert!(provider.sent.lock().unwrap().is_empty());
}
