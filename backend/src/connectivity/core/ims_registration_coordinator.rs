//! Per-line registration admission shared by every IMS entry point.
//!
//! Lock order: transition_lock -> admission write (briefly) -> access/bearer
//! locks. REGISTER holds an admission read permit through its exchange, so a
//! policy transition cannot remove its path mid-exchange. Never hold an
//! admission write guard during teardown or connection. Refresh of the existing
//! VoLTE session uses its session mutex and is NOT gated on the other enabled
//! intent. The transition path disconnects it only for a real access switch.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, RwLock, Weak},
    time::{Duration, Instant},
};

use serde::Serialize;
use tokio::sync::{Mutex as AsyncMutex, MutexGuard, RwLock as AsyncRwLock, RwLockReadGuard};

use super::{
    ims_access::{
        ConcurrentRegistrationSupport, ImsAccess, ImsAccessDecision, ImsAccessPreference,
        CURRENT_CONCURRENT_SUPPORT,
    },
    outbound::FlowLease,
    register_response::RegisterArtifacts,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct OutboundResponseEvidence {
    /// Passive evidence from the most recent successful REGISTER on this leg.
    /// Not itself proof of a complete outbound negotiation/implementation.
    pub require_outbound: bool,
    pub flow_timer_seconds: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImsRegistrationPolicyStatus {
    pub requested: ImsAccessPreference,
    pub effective: &'static str,
    pub concurrent_support: ConcurrentRegistrationSupport,
    pub desired: ImsAccessDecision,
    pub applied: ImsAccessDecision,
    pub switch_deferred_for_call: bool,
    pub cellular_last_response: Option<OutboundResponseEvidence>,
    pub wlan_last_response: Option<OutboundResponseEvidence>,
}

struct Observation {
    applied: ImsAccessDecision,
    switch_deferred_for_call: bool,
    cellular: Option<OutboundResponseEvidence>,
    wlan: Option<OutboundResponseEvidence>,
    cellular_flow: Weak<FlowLease>,
    wlan_flow: Weak<FlowLease>,
    recovery: [(u32, Option<Instant>); 2],
    outbound_rejected_until: [Option<Instant>; 2],
}

pub struct ImsRegistrationCoordinator {
    /// Serializes LTE/WLAN bring-up and policy reconciliation for ONE line.
    pub transition_lock: AsyncMutex<()>,
    admission: AsyncRwLock<ImsAccessDecision>,
    register_lock: AsyncMutex<()>,
    observation: RwLock<Observation>,
}

impl Default for ImsRegistrationCoordinator {
    fn default() -> Self {
        let initial = ImsAccessDecision::none("ims_access_not_reconciled");
        Self {
            transition_lock: AsyncMutex::new(()),
            admission: AsyncRwLock::new(initial),
            register_lock: AsyncMutex::new(()),
            observation: RwLock::new(Observation {
                applied: initial,
                switch_deferred_for_call: false,
                cellular: None,
                wlan: None,
                cellular_flow: Weak::new(),
                wlan_flow: Weak::new(),
                recovery: [(0, None); 2],
                outbound_rejected_until: [None; 2],
            }),
        }
    }
}

/// REGISTER exchanges are serialized per line. This prevents two callers
/// using the same just-negotiated snapshot to start competing first flows.
/// The read permit still drains before an access teardown.
pub struct RegistrationPermit<'a> {
    _admission: RwLockReadGuard<'a, ImsAccessDecision>,
    _register: MutexGuard<'a, ()>,
}

impl ImsRegistrationCoordinator {
    /// Caller owns transition_lock. Waits for an already admitted REGISTER;
    /// future attempts then see the new decision before touching the network.
    pub async fn publish(&self, decision: ImsAccessDecision) {
        *self.admission.write().await = decision;
        let mut observation = self.observation.write().unwrap_or_else(|e| e.into_inner());
        observation.applied = decision;
        observation.switch_deferred_for_call = false;
    }

    /// Explicit stop/config-reset paths also drain REGISTER before teardown.
    /// Caller owns transition_lock. Never authorize the other access here.
    pub async fn park(&self, access: ImsAccess) {
        let mut admission = self.admission.write().await;
        match access {
            ImsAccess::Cellular => admission.cellular_registers = false,
            ImsAccess::Wlan => admission.wlan_registers = false,
        }
        admission.code = "ims_access_registration_parked";
        let mut observation = self.observation.write().unwrap_or_else(|e| e.into_inner());
        observation.applied = *admission;
        observation.switch_deferred_for_call = false;
    }

    /// Hold the returned permit until the REGISTER transaction finishes.
    /// A line never reconciled by its owner fails closed, including SMS/voice
    /// helpers and diagnostic stages which bypass the HTTP restore workflow.
    pub async fn admit(&self, access: ImsAccess) -> Result<RegistrationPermit<'_>, &'static str> {
        let register = self.register_lock.lock().await;
        let decision = self.admission.read().await;
        if !decision.permits(access) {
            return Err("ims_access_registration_parked");
        }
        // Backoff gates flow creation, never refresh of a still-owned binding.
        let existing = {
            let o = self.observation.read().unwrap_or_else(|e| e.into_inner());
            match access {
                ImsAccess::Cellular => &o.cellular_flow,
                ImsAccess::Wlan => &o.wlan_flow,
            }
            .upgrade()
            .is_some_and(|flow| flow.live())
        };
        if !existing && !self.recovery_ready(access) {
            return Err("ims_outbound_recovery_backoff");
        }
        if !existing && !self.flow_creation_ready(access) {
            return Err("ims_outbound_additional_flow_not_supported");
        }
        if !existing
            && decision.cellular_registers
            && decision.wlan_registers
            && self.concurrent_support() != ConcurrentRegistrationSupport::Negotiated
        {
            return Err("ims_access_registration_parked");
        }
        Ok(RegistrationPermit {
            _admission: decision,
            _register: register,
        })
    }

    pub fn attach_flow(&self, access: ImsAccess, flow: Weak<FlowLease>) {
        let mut observation = self.observation.write().unwrap_or_else(|e| e.into_inner());
        match access {
            ImsAccess::Cellular => observation.cellular_flow = flow,
            ImsAccess::Wlan => observation.wlan_flow = flow,
        }
    }

    pub fn binding_instance(&self, access: ImsAccess) -> Option<String> {
        let o = self.observation.read().unwrap_or_else(|e| e.into_inner());
        let flow = match access {
            ImsAccess::Cellular => &o.cellular_flow,
            ImsAccess::Wlan => &o.wlan_flow,
        };
        flow.upgrade()
            .filter(|flow| flow.live())
            .map(|flow| flow.instance.clone())
    }

    /// Different carrier profiles may choose UUID vs IMEI formatting. Once
    /// one access owns a binding, the other must use that same UA instance,
    /// rather than accidentally create a second device identity for this SIM.
    /// Callers hold the per-line REGISTER permit during initial bring-up.
    pub fn registration_instance(&self, proposed: &str) -> String {
        self.binding_instance(ImsAccess::Wlan)
            .or_else(|| self.binding_instance(ImsAccess::Cellular))
            .unwrap_or_else(|| proposed.to_string())
    }

    pub fn concurrent_support(&self) -> ConcurrentRegistrationSupport {
        let observation = self.observation.read().unwrap_or_else(|e| e.into_inner());
        support_from_observation(&observation)
    }

    /// Remember explicit peer refusal without invalidating the healthy primary.
    /// Cool down capability probing so every reconciliation cannot repeat a
    /// rejected secondary REGISTER. A later network change can be re-probed.
    pub fn reject_outbound(&self, access: ImsAccess) {
        self.observation
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .outbound_rejected_until[flow_index(access)] =
            Some(Instant::now() + Duration::from_secs(1800));
    }

    pub fn may_offer_outbound(&self, access: ImsAccess) -> bool {
        self.observation
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .outbound_rejected_until[flow_index(access)]
        .is_none_or(|until| Instant::now() >= until)
    }

    pub fn additional_flow_requires_outbound(&self, access: ImsAccess) -> bool {
        let o = self.observation.read().unwrap_or_else(|e| e.into_inner());
        // Admission is an intent, not proof a second binding exists. Require
        // outbound whenever the opposite binding is still owned/unexpired,
        // including a temporarily unproven flow waiting for its next pong.
        match access {
            ImsAccess::Cellular => o.wlan_flow.upgrade().is_some_and(|l| l.live()),
            ImsAccess::Wlan => o.cellular_flow.upgrade().is_some_and(|l| l.live()),
        }
    }

    pub fn instance_matches(&self, instance: &str) -> bool {
        let o = self.observation.read().unwrap_or_else(|e| e.into_inner());
        [&o.cellular_flow, &o.wlan_flow]
            .iter()
            .filter_map(|l| l.upgrade())
            .filter(|l| l.live())
            .all(|l| l.instance == instance)
    }

    pub fn invalidate_outbound_flows(&self) {
        let o = self.observation.read().unwrap_or_else(|e| e.into_inner());
        for lease in [&o.cellular_flow, &o.wlan_flow]
            .iter()
            .filter_map(|l| l.upgrade())
        {
            lease.invalidate();
        }
    }

    pub fn flow_failed(&self, access: ImsAccess) {
        let mut o = self.observation.write().unwrap_or_else(|e| e.into_inner());
        let another = match access {
            ImsAccess::Cellular => o.wlan_flow.upgrade().is_some_and(|l| l.proven()),
            ImsAccess::Wlan => o.cellular_flow.upgrade().is_some_and(|l| l.proven()),
        };
        let (failures, retry_at) = &mut o.recovery[flow_index(access)];
        *failures = failures.saturating_add(1);
        // RFC 5626 4.5: different bases with/without a surviving flow, capped
        // exponential backoff and fresh 50%-100% jitter for every recovery.
        let base = if another { 90u64 } else { 30u64 };
        let upper = (base * (1u64 << (*failures).min(6))).min(1800);
        let delay = Duration::from_secs_f64(
            upper as f64 * (0.5 + super::outbound::random_fraction() * 0.5),
        );
        *retry_at = Some(Instant::now() + delay);
    }

    pub fn flow_healthy(&self, access: ImsAccess) {
        self.observation
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .recovery[flow_index(access)] = (0, None);
    }

    pub fn recovery_ready(&self, access: ImsAccess) -> bool {
        self.observation
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .recovery[flow_index(access)]
        .1
        .is_none_or(|deadline| Instant::now() >= deadline)
    }

    /// Refusal/backoff affects only creation of this access's new flow. A
    /// healthy opposite access remains registered and continues to refresh.
    /// With no surviving flow, a cooled-down outbound offer may still fall
    /// back to a legacy SINGLE registration; never do that for a second flow.
    pub fn flow_creation_ready(&self, access: ImsAccess) -> bool {
        self.recovery_ready(access)
            && (!self.additional_flow_requires_outbound(access) || self.may_offer_outbound(access))
    }

    pub fn defer_switch_for_call(&self) {
        self.observation
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .switch_deferred_for_call = true;
    }

    pub fn observe_response(&self, access: ImsAccess, artifacts: &RegisterArtifacts) {
        let evidence = Some(OutboundResponseEvidence {
            require_outbound: artifacts.outbound_required,
            flow_timer_seconds: artifacts.flow_timer_seconds,
        });
        let mut observation = self.observation.write().unwrap_or_else(|e| e.into_inner());
        match access {
            ImsAccess::Cellular => observation.cellular = evidence,
            ImsAccess::Wlan => observation.wlan = evidence,
        }
    }

    /// Read-only and non-async: a status query never initiates registration or
    /// waits for a 32-second SIP transaction to report its effective policy.
    pub fn status(
        &self,
        requested: ImsAccessPreference,
        desired: ImsAccessDecision,
    ) -> ImsRegistrationPolicyStatus {
        let observation = self.observation.read().unwrap_or_else(|e| e.into_inner());
        ImsRegistrationPolicyStatus {
            requested,
            effective: observation.applied.effective_mode(),
            concurrent_support: support_from_observation(&observation),
            desired,
            applied: observation.applied,
            switch_deferred_for_call: observation.switch_deferred_for_call,
            cellular_last_response: observation.cellular,
            wlan_last_response: observation.wlan,
        }
    }
}

fn flow_index(access: ImsAccess) -> usize {
    match access {
        ImsAccess::Cellular => 0,
        ImsAccess::Wlan => 1,
    }
}

fn support_from_observation(o: &Observation) -> ConcurrentRegistrationSupport {
    // A secondary P-CSCF refusing outbound says nothing about the established
    // primary's negotiation. Demoting both here made reconciliation tear down
    // the working registration after a failed secondary attempt.
    if [&o.cellular_flow, &o.wlan_flow]
        .iter()
        .filter_map(|l| l.upgrade())
        .any(|l| l.proven())
    {
        ConcurrentRegistrationSupport::Negotiated
    } else {
        CURRENT_CONCURRENT_SUPPORT
    }
}

/// The map lock is only held to look up/allocate a per-line object, never for
/// network I/O. LineRuntime owns a strong reference; old removed lines can be
/// reclaimed. Live adapters resolve the SAME object without depending on API
/// state or a process-global IMS access preference.
pub fn for_line(line_id: &str) -> Arc<ImsRegistrationCoordinator> {
    static LINES: OnceLock<Mutex<HashMap<String, Weak<ImsRegistrationCoordinator>>>> =
        OnceLock::new();
    let mut lines = LINES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(existing) = lines.get(line_id).and_then(Weak::upgrade) {
        return existing;
    }
    lines.retain(|_, coordinator| coordinator.strong_count() > 0);
    let coordinator = Arc::new(ImsRegistrationCoordinator::default());
    lines.insert(line_id.to_string(), Arc::downgrade(&coordinator));
    coordinator
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fails_closed_until_owner_reconciles_and_parks_only_opposite_access() {
        let coordinator = ImsRegistrationCoordinator::default();
        assert!(coordinator.admit(ImsAccess::Wlan).await.is_err());
        assert!(coordinator.admit(ImsAccess::Cellular).await.is_err());
        coordinator
            .publish(ImsAccessDecision::cellular_only("selected"))
            .await;
        assert!(coordinator.admit(ImsAccess::Cellular).await.is_ok());
        assert!(coordinator.admit(ImsAccess::Wlan).await.is_err());
    }

    #[tokio::test]
    async fn in_flight_register_finishes_before_switch_can_change_admission() {
        let coordinator = Arc::new(ImsRegistrationCoordinator::default());
        coordinator
            .publish(ImsAccessDecision::cellular_only("selected"))
            .await;
        let permit = coordinator.admit(ImsAccess::Cellular).await.unwrap();
        assert!(coordinator.admission.try_write().is_err());
        let other = Arc::clone(&coordinator);
        let change = tokio::spawn(async move {
            other.publish(ImsAccessDecision::wlan_only("switch")).await;
        });
        tokio::task::yield_now().await;
        assert!(!change.is_finished());
        drop(permit);
        change.await.unwrap();
        assert!(coordinator.admit(ImsAccess::Cellular).await.is_err());
        assert!(coordinator.admit(ImsAccess::Wlan).await.is_ok());
    }

    #[tokio::test]
    async fn call_deferral_preserves_applied_registration_and_exposes_pending_switch() {
        let coordinator = ImsRegistrationCoordinator::default();
        let old = ImsAccessDecision::cellular_only("old");
        let desired = ImsAccessDecision::wlan_only("new");
        coordinator.publish(old).await;
        coordinator.defer_switch_for_call();
        let status = coordinator.status(ImsAccessPreference::WlanPreferred, desired);
        assert_eq!(status.applied, old);
        assert_eq!(status.desired, desired);
        assert!(status.switch_deferred_for_call);
        assert!(coordinator.admit(ImsAccess::Wlan).await.is_err());
        // A protected refresh on the selected leg remains legal during a call.
        assert!(coordinator.admit(ImsAccess::Cellular).await.is_ok());
        coordinator.publish(desired).await;
        assert!(
            !coordinator
                .status(ImsAccessPreference::WlanPreferred, desired)
                .switch_deferred_for_call
        );
    }

    #[tokio::test]
    async fn transition_lock_is_shared_between_accesses_but_not_between_lines() {
        let line = for_line("test-coordinator-line-one");
        let same = for_line("test-coordinator-line-one");
        let other = for_line("test-coordinator-line-two");
        assert!(Arc::ptr_eq(&line, &same));
        let _guard = line.transition_lock.lock().await;
        assert!(same.transition_lock.try_lock().is_err());
        assert!(other.transition_lock.try_lock().is_ok());
        line.publish(ImsAccessDecision::cellular_only("selected"))
            .await;
        assert!(other.admit(ImsAccess::Cellular).await.is_err());
    }

    #[tokio::test]
    async fn explicit_stop_cannot_authorize_an_unregistered_opposite_access() {
        let coordinator = ImsRegistrationCoordinator::default();
        coordinator
            .publish(ImsAccessDecision::wlan_only("selected"))
            .await;
        coordinator.park(ImsAccess::Wlan).await;
        assert!(coordinator.admit(ImsAccess::Wlan).await.is_err());
        assert!(coordinator.admit(ImsAccess::Cellular).await.is_err());
    }

    #[test]
    fn response_evidence_alone_never_enables_concurrency() {
        let coordinator = ImsRegistrationCoordinator::default();
        coordinator.observe_response(
            ImsAccess::Wlan,
            &RegisterArtifacts {
                outbound_required: true,
                flow_timer_seconds: Some(25),
                ..Default::default()
            },
        );
        let status = coordinator.status(
            ImsAccessPreference::Concurrent,
            ImsAccessDecision::cellular_only("selected"),
        );
        assert_eq!(
            status.concurrent_support,
            ConcurrentRegistrationSupport::NotNegotiated
        );
        assert_eq!(status.wlan_last_response.unwrap().require_outbound, true);
        assert!(status.cellular_last_response.is_none());
        assert_eq!(status.effective, "none"); // evidence is not admission
    }
}
