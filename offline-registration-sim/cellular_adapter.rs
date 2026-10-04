use super::*;
use crate::connectivity::core::offline_sim::WireBuilder;

pub(crate) fn builder(
    profile: &'static CarrierProfile,
    identity: ImsIdentity,
    route: ImsRoute,
) -> Box<dyn WireBuilder> {
    Box::new(Builder {
        profile,
        identity,
        route,
        variants: register_variants(profile),
        index: 0,
        security_verify: None,
    })
}
struct Builder {
    profile: &'static CarrierProfile,
    identity: ImsIdentity,
    route: ImsRoute,
    variants: Vec<CellularImsRegisterVariant>,
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
        let security = variant.security_client_offer.build(SecAgree {
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
        self.security_verify = select_security_server(self.profile, &sip::header_values(frame,"Security-Server"))?
            .map(|selected| selected.verify);
        Ok(())
    }
    fn advance(&mut self, failure: &RegisterFailure) -> bool {
        if failure.auth_rounds != 0 {
            return false;
        }
        if let Some(next) = next_dynamic_register_variant_with_roaming(
            self.profile,
            self.variants[self.index],
            failure,
            false,
        ) {
            self.variants.insert(self.index + 1, next);
            self.index += 1;
            return true;
        }
        if pre_authentication_variant_failure(failure) && self.index + 1 < self.variants.len() {
            self.index += 1;
            return true;
        }
        false
    }
    fn label(&self) -> &'static str {
        self.variants[self.index].label
    }
}
