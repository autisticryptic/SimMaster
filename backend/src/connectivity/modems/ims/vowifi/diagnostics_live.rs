//! Passive WiFi registration evidence. Scope tokens and weak runtime handles
//! prevent late work for a reset/replaced registration from rewriting its view.
use super::*;
use crate::connectivity::core::registration_diagnostics as facts;
use std::sync::Weak;
use super::super::runtime::VowifiRuntime;

struct ProducerOwner { line: String, runtime: Option<Weak<VowifiRuntime>>, generation: u64 }
tokio::task_local! { static PRODUCER_OWNER: ProducerOwner; }

pub(super) async fn run_with_owner<F: Future>(owner: &VowifiRuntime, generation: u64, work: F) -> F::Output {
    let runtime = super::reg_event_live::runtime_for_line(owner.line_id())
        .filter(|runtime| owner.same_diagnostics_owner(runtime));
    let context = ProducerOwner { line: owner.line_id().to_owned(), runtime: runtime.as_ref().map(Arc::downgrade), generation };
    PRODUCER_OWNER.scope(context, work).await
}

fn producer_runtime(line: &str) -> Option<(Arc<VowifiRuntime>, Option<u64>)> {
    if let Ok(context) = PRODUCER_OWNER.try_with(|owner| {
        (owner.line == line).then(|| owner.runtime.as_ref().and_then(Weak::upgrade)
            .map(|runtime| (runtime, Some(owner.generation)))).flatten()
    }) { return context; }
    super::reg_event_live::runtime_for_line(line).map(|runtime| (runtime, None))
}

#[derive(Clone, Default)]
pub(super) struct Scope {
    runtime: Option<Weak<VowifiRuntime>>,
    token: Option<facts::DiagnosticsToken>,
}
impl std::fmt::Debug for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("RegistrationDiagnosticsScope") }
}
impl Scope {
    pub(super) async fn current_or_begin(line: &str) -> Self {
        let Some((runtime, generation)) = producer_runtime(line) else { return Self::default(); };
        let current = runtime.current_ims_diagnostics_token().await.filter(|token|
            generation.is_none_or(|generation| token.generation == generation));
        let token = match current {
            Some(token) => Some(token), None => match generation {
                Some(generation) => runtime.begin_ims_diagnostics_at(generation).await,
                None => runtime.begin_ims_diagnostics().await,
            },
        };
        Self { runtime: Some(Arc::downgrade(&runtime)), token }
    }
    pub(super) async fn for_profile(line: &str, profile: &'static CarrierProfile) -> Self {
        let Some((runtime, _)) = producer_runtime(line) else { return Self::default(); };
        let current = runtime.snapshot().await;
        if current.diagnostics.facts.profile_id.as_deref() != Some(profile.meta.profile_id) {
            return Self::begin(line, profile).await;
        }
        let scope = Self::current_or_begin(line).await;
        scope.profile(line, profile).await;
        scope
    }
    pub(super) async fn begin(line: &str, profile: &'static CarrierProfile) -> Self {
        let Some((runtime, generation)) = producer_runtime(line) else { return Self::default(); };
        let token = match generation {
            Some(generation) => runtime.begin_ims_diagnostics_at(generation).await,
            None => runtime.begin_ims_diagnostics().await,
        };
        let scope = Self { runtime: Some(Arc::downgrade(&runtime)), token };
        scope.profile(line, profile).await;
        scope
    }
    pub(super) async fn update(&self, change: impl FnOnce(&mut facts::DiagnosticsState)) {
        if let (Some(runtime), Some(token)) = (self.runtime.as_ref().and_then(Weak::upgrade), self.token) {
            runtime.observe_ims_diagnostics(token, change).await;
        }
    }
    pub(super) async fn profile(&self, line: &str, profile: &'static CarrierProfile) {
        let source = profile_source(line, profile);
        self.update(|state| state.profile(profile.meta.profile_id, source, profile.ims.identity_source)).await;
    }
    pub(super) async fn requested_family(&self, value: &str) {
        let family = facts::IpFamily::parse(value);
        self.update(|state| { state.facts.requested_family = family; state.facts.granted_family = facts::IpFamily::Unknown; }).await;
    }
    pub(super) async fn granted_family(&self, addresses: Option<&[IpAddr]>) {
        let granted = addresses.map_or(facts::IpFamily::Unknown, |addresses|
            facts::IpFamily::granted(addresses.iter().any(IpAddr::is_ipv4), addresses.iter().any(IpAddr::is_ipv6)));
        self.update(|state| state.facts.granted_family = granted).await;
    }
    pub(super) async fn selected(&self, selection: &LiveImsSelection, gateway: &TunGatewayRuntime, target: SocketAddr,
        transport: crate::connectivity::core::context::SipTransport) {
        let (identity, domain, realm) = selection.diagnostic_sources();
        let (requested, granted, source) = gateway.diagnostic_families_and_source(target.ip());
        let index = gateway.pcscf_addrs().iter().position(|address| *address == target.ip()).map_or(1, |index| index + 1);
        let count = gateway.pcscf_addrs().len().max(1);
        self.update(|state| {
            state.identity(identity, domain, realm);
            state.facts.requested_family = requested; state.facts.granted_family = granted;
            state.endpoint(source, transport, target, index, count);
        }).await;
    }
    pub(super) async fn offer(&self, request: &[u8], auth: Option<&LiveRefreshAuthorization>) {
        let offer = sip_frame::header_values(request, "Security-Client").join(",");
        let digest = auth.and_then(|auth| facts::DigestAlgorithm::parse(&auth.challenge.algorithm));
        self.update(|state| {
            state.offer((!offer.is_empty()).then_some(offer.as_str()));
            state.facts.digest_algorithm = digest;
        }).await;
    }
    pub(super) async fn accepted(&self, route: crate::connectivity::core::context::ImsRoute,
        verify: Option<&str>, auth: Option<&LiveRefreshAuthorization>, response: &[u8]) {
        let digest = auth.map(|state| state.challenge.algorithm.as_str());
        let rspauth = auth.and_then(|state| state.request_proof.as_ref()).and_then(|proof|
            proof.verify_response(response).ok()).map(|verified| verified.rspauth_verified());
        self.update(|state| {
            state.signaling_route(route);
            state.accepted(route.transport, verify, digest, rspauth);
        }).await;
    }
    pub(super) async fn admission(&self, admission: crate::connectivity::core::register_retry::RetryAdmission) {
        use crate::connectivity::core::register_retry::RetryAdmission;
        self.update(|state| match admission {
            RetryAdmission::Allowed => state.wait_ready(),
            RetryAdmission::Deferred { not_before } => state.wait_until(not_before),
            RetryAdmission::Stopped(_) => state.wait_stopped(),
        }).await;
    }
    pub(super) async fn core_failure(&self, error: &ImsError, fallback: facts::FailureScope) {
        self.update(|state| state.failure_in_scope(error.code(), None, fallback)).await;
    }
    pub(super) async fn failure(&self, error: &LiveStageError, fallback: facts::FailureScope) {
        self.update(|state| {
            state.failure_in_scope(&error.reason, error.register_failure, fallback);
            if terminal_ike_auth_rejection(error) {
                if let Some(failure) = state.facts.failure.as_mut() { failure.scope = facts::FailureScope::Authentication; }
            }
            if matches!(error.reason.as_str(), "vowifi_runtime_ue_worker_generation_changed" | "ike_ue_worker_generation_changed") {
                if let Some(failure) = state.facts.failure.as_mut() { failure.scope = facts::FailureScope::Ownership; }
            }
            if let Some(requested) = error.requested_ip_family { state.facts.requested_family = requested; }
        }).await;
    }
}

pub(super) fn profile_source(line: &str, profile: &'static CarrierProfile) -> facts::FactSource {
    live_profile_selection(line).filter(|selected| std::ptr::eq(selected.profile, profile))
        .map(|selected| facts::FactSource::profile(&selected.source))
        .unwrap_or_else(|| if profiles::is_standard_derived_profile(profile) { facts::FactSource::Derived } else { facts::FactSource::Unknown })
}

pub(super) async fn network_wait(line: &str, deadline: Option<Instant>) {
    let scope = Scope::current_or_begin(line).await;
    scope.update(|state| {
        state.facts.procedure = facts::RegistrationProcedure::RegistrationEvent;
        state.failure_in_scope("ims_reg_event_binding_terminated", None, facts::FailureScope::Binding);
        if let Some(deadline) = deadline { state.wait_until(deadline); } else { state.wait_stopped(); }
    }).await;
}

#[cfg(test)]
#[path = "diagnostics_live_tests.rs"]
mod tests;
