use super::*;
use crate::hardware::devices::{
    ims_bearer_policy::{ImsApnAuthentication, ImsApnAuthMethod, ImsBearerRequirements},
    transport::{ImsBearerCapabilities, ImsBearerError, ImsBearerHandle, ImsBearerInfo, TransportFuture},
};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Preflight(AtomicUsize);
impl ImsBearerTransport for Preflight {
    fn ims_bearer_capabilities(&self) -> ImsBearerCapabilities {
        self.0.fetch_add(1, Ordering::AcqRel);
        ImsBearerCapabilities::retained_mm()
    }
    fn endpoint_available(&self, _: &str) -> bool { panic!("preflight must not inspect endpoint") }
    fn establish_ims_bearer<'a>(&'a self, _: &'a str, _: &'a str, _: &'a str,
        _: Option<u32>, _: u8, _: &'a [u8], _: bool, _: Option<(&'a str, u8)>,
    ) -> TransportFuture<'a, Result<(ImsBearerInfo, Box<dyn ImsBearerHandle + Send>), ImsBearerError>> {
        panic!("preflight must not dispatch bearer")
    }
}
fn profile() -> CarrierProfile {
    *crate::connectivity::modems::ims::vowifi::profiles::derive_standard_3gpp_profile("001", "01",
        crate::connectivity::modems::ims::vowifi::profiles::Standard3gppAccess::LteEpc).unwrap()
}
fn requirements() -> ImsBearerRequirements {
    ImsBearerRequirements { authentication: ImsApnAuthentication { method: ImsApnAuthMethod::Pap,
        username: Some("fixture-user".into()), password: Some("fixture-secret".into()) }, ..Default::default() }
}
#[test]
fn live_preparation_gate_rejects_unsupported_auth_before_profile_or_activation() {
    let mut profile = profile();
    profile.ims.bearer = Box::leak(Box::new(requirements()));
    let effective = crate::connectivity::modems::ims::effective_profile::resolve_effective_ims_profile(&profile, None);
    let transport = Preflight(AtomicUsize::new(0));
    let error = validate_bearer_profile_requirements(Some(&transport), &effective).unwrap_err();
    assert_eq!(error.code(), code::BEARER_REQUIREMENTS_UNSUPPORTED);
    assert_eq!(transport.0.load(Ordering::Acquire), 1);
    assert!(!format!("{error:?}").contains("fixture-secret"));
}
#[test]
fn live_preparation_gate_checks_apn_credential_binding_before_driver() {
    let mut profile = profile();
    profile.ims.bearer = Box::leak(Box::new(requirements()));
    let mut override_ = SimOverride::default();
    override_.ims_cellular.apn = Some("different.example".into());
    let effective = crate::connectivity::modems::ims::effective_profile::resolve_effective_ims_profile(&profile, Some(&override_));
    let transport = Preflight(AtomicUsize::new(0));
    let error = validate_bearer_profile_requirements(Some(&transport), &effective).unwrap_err();
    assert_eq!(error.code(), code::BEARER_REQUIREMENTS_INVALID);
    assert_eq!(transport.0.load(Ordering::Acquire), 0);
}
#[test]
fn live_preparation_gate_leaves_legacy_derived_requirements_unchanged() {
    let profile = profile();
    let effective = crate::connectivity::modems::ims::effective_profile::resolve_effective_ims_profile(&profile, None);
    let transport = Preflight(AtomicUsize::new(0));
    validate_bearer_profile_requirements(Some(&transport), &effective).unwrap();
    assert_eq!(transport.0.load(Ordering::Acquire), 1);
}
