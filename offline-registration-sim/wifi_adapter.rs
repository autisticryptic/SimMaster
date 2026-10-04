use super::*;
use crate::connectivity::core::offline_sim::{WireBuilder, SIM_SECURITY};

pub(crate) fn builder(
    profile: &'static CarrierProfile,
    identity: crate::connectivity::core::context::ImsIdentity,
    route: crate::connectivity::core::context::ImsRoute,
) -> Box<dyn WireBuilder> {
    let mut context = LiveRegisterRequestContext::new_with_target(
        profile,
        LiveImsTarget {
            domain: profile.ims.domain.into(),
            realm: profile.ims.realm.into(),
            registrar: profile.ims.registrar.map(str::to_string),
            pcscf: Vec::new(),
        },
        LiveImsRegisterIdentity {
            shared: identity,
            shape: "offline-fixture",
        },
        route.local_addr,
        route.pcscf_addr.ip(),
    )
    .expect("pure request context");
    context.call_id = "offline-register@fixture.invalid".into();
    context.from_tag = "fixture".into();
    Box::new(Builder {
        profile,
        context,
        variants: live_register_header_variants(profile),
        index: 0,
    })
}
struct Builder {
    profile: &'static CarrierProfile,
    context: LiveRegisterRequestContext,
    variants: Vec<LiveRegisterHeaderVariant>,
    index: usize,
}
impl WireBuilder for Builder {
    fn build(&self, cseq: u32, expires: u32, authorization: Option<&str>) -> Vec<u8> {
        let variant = self.variants[self.index];
        // Same pre-auth 423 rebuild rule as the live authenticator: retain the
        // selected initial authorization even when CSeq is no longer one.
        let initial = authorization
            .is_none()
            .then(|| {
                self.context
                    .build_initial_authorization_header(self.profile, variant)
            })
            .flatten();
        self.context
            .build_register_request_with_expires(
                self.profile,
                variant,
                cseq,
                authorization.or(initial.as_deref()),
                (authorization.is_some() && self.profile.ims.register.sec_agree_mode != "disabled")
                    .then_some(SIM_SECURITY),
                expires,
            )
            .into_bytes()
    }
    fn advance(&mut self, failure: &RegisterFailure) -> bool {
        if failure.auth_rounds != 0 {
            return false;
        }
        let error = map_shared_register_failure(failure);
        if let Some(next) =
            next_dynamic_live_register_variant(self.profile, self.variants[self.index], &error)
        {
            self.variants.insert(self.index + 1, next);
            self.index += 1;
            return true;
        }
        if live_register_error_is_terminal(&error) {
            return false;
        }
        if failure
            .response
            .as_deref()
            .and_then(|r| sip_frame::parse_status(r).ok())
            .is_some_and(
                crate::connectivity::core::register::status_permits_register_variant_fallback,
            )
            && self.index + 1 < self.variants.len()
        {
            self.index += 1;
            return true;
        }
        false
    }
    fn label(&self) -> &'static str {
        self.variants[self.index].label
    }
}
