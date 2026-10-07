use super::*;
use crate::connectivity::core::offline_sim::WireBuilder;

pub(crate) fn builder(
    profile: &'static CarrierProfile,
    identity: ImsIdentity,
    route: ImsRoute,
) -> Box<dyn WireBuilder> {
    let variants = register_variants(profile);
    let fallback = register_fallback::RegisterFallbackState::new(variants[0]);
    let mut history = register_fallback::RegisterCandidateHistory::default();
    history.record(variants[0], fallback.requires_protection(profile));
    Box::new(Builder {
        profile,
        identity,
        route,
        variants,
        fallback,
        history,
        index: 0,
        security_verify: None,
    })
}
struct Builder {
    profile: &'static CarrierProfile,
    identity: ImsIdentity,
    route: ImsRoute,
    variants: Vec<CellularImsRegisterVariant>,
    fallback: register_fallback::RegisterFallbackState,
    history: register_fallback::RegisterCandidateHistory,
    index: usize,
    security_verify: Option<String>,
}
impl WireBuilder for Builder {
    fn build(&self, cseq: u32, expires: u32, authorization: Option<&str>) -> Vec<u8> {
        let variant = self.variants[self.index];
        let initial = (authorization.is_none()
            && variant.authorization == CellularImsInitialAuthorization::UriFirstEmptyAka)
            .then(|| {
                crate::connectivity::core::digest_aka::build_initial_authorization_header_uri_first(
                    &self.identity.private_user,
                    self.profile.ims.realm,
                    &sip::register_request_uri(self.profile, &self.route),
                )
            });
        let enabled = self.profile.ims.register.sec_agree_mode != "disabled";
        let security = variant.build_security_offer(SecAgree {
            spi_c:10001,spi_s:10002,port_c:5062,port_s:5063,
        },self.profile).expect("valid real client-offer builder");
        sip::build_register_from_profile(
            self.profile,
            if authorization.is_some() {
                sip::RegisterPhase::Authenticated
            } else {
                sip::RegisterPhase::Initial
            },
            &self.identity,
            &self.route,
            &RequestIds {
                call_id: "offline-register@fixture.invalid".into(),
                from_tag: "fixture".into(),
                cseq,
            },
            expires,
            authorization.or(initial.as_deref()),
            enabled.then_some(security.as_str()),
            if enabled && authorization.is_some() { self.security_verify.as_deref() } else { None },
            "urn:uuid:00000000-0000-4000-8000-000000000001",
            variant.policy,
        )
    }
    fn accept_security_challenge(&mut self, frame: &[u8]) -> Result<(), ImsError> {
        let mechanism = self.variants[self.index].security_mechanism;
        self.security_verify = select_security_server_for_mechanism(self.profile, &sip::header_values(frame,"Security-Server"), mechanism)?
            .map(|selected| selected.verify);
        if (mechanism.is_some() || self.fallback.requires_protection(self.profile)) && self.security_verify.is_none() {
            return Err(ImsError::new(code::SECURITY_SERVER_MISSING));
        }
        Ok(())
    }
    fn accept_success(&self, authenticated: bool) -> Result<(), ImsError> {
        if (self.variants[self.index].security_mechanism.is_some() || self.fallback.requires_protection(self.profile))
            && (!authenticated || self.security_verify.is_none()) {
            return Err(ImsError::new(code::SECURITY_SERVER_MISSING));
        }
        Ok(())
    }
    fn advance(&mut self, failure: &RegisterFailure) -> bool {
        if failure.auth_rounds != 0 {
            return false;
        }
        let current = match self.fallback.observe(self.profile, self.variants[self.index], failure) {
            Ok(current) => current,
            Err(_) => return false,
        };
        match security_hint::decide(self.profile, current, failure) {
            security_hint::Decision::Retry(next) => {
                if !self.history.record(next, self.fallback.requires_protection(self.profile)) {
                    return false;
                }
                self.variants.insert(self.index + 1, next);
                self.index += 1;
                return true;
            }
            security_hint::Decision::Stop(_) => return false,
            security_hint::Decision::NotApplicable => {}
        }
        if let Some(next) = next_dynamic_register_variant_with_roaming(
            self.profile,
            current,
            failure,
            false,
        ) {
            if self.history.record(next, self.fallback.requires_protection(self.profile)) {
                self.variants.insert(self.index + 1, next);
                self.index += 1;
                return true;
            }
        }
        if pre_authentication_variant_failure(failure) {
            while self.index + 1 < self.variants.len() {
                self.index += 1;
                self.variants[self.index] = self.fallback.apply(self.variants[self.index]);
                if self.history.record(self.variants[self.index], self.fallback.requires_protection(self.profile)) {
                    return true;
                }
            }
        }
        false
    }
    fn label(&self) -> &'static str {
        self.variants[self.index].label
    }
}
