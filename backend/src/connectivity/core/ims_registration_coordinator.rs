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
};

use serde::Serialize;
use tokio::sync::{Mutex as AsyncMutex, RwLock as AsyncRwLock, RwLockReadGuard};

use super::{
    ims_access::{
        ConcurrentRegistrationSupport, ImsAccess, ImsAccessDecision, ImsAccessPreference,
        CURRENT_CONCURRENT_SUPPORT,
    },
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
}

pub struct ImsRegistrationCoordinator {
    /// Serializes LTE/WLAN bring-up and policy reconciliation for ONE line.
    pub transition_lock: AsyncMutex<()>,
    admission: AsyncRwLock<ImsAccessDecision>,
    observation: RwLock<Observation>,
}

impl Default for ImsRegistrationCoordinator {
    fn default() -> Self {
        let initial = ImsAccessDecision::none("ims_access_not_reconciled");
        Self {
            transition_lock: AsyncMutex::new(()),
            admission: AsyncRwLock::new(initial),
            observation: RwLock::new(Observation {
                applied: initial,
                switch_deferred_for_call: false,
                cellular: None,
                wlan: None,
            }),
        }
    }
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
    pub async fn admit(
        &self,
        access: ImsAccess,
    ) -> Result<RwLockReadGuard<'_, ImsAccessDecision>, &'static str> {
        let decision = self.admission.read().await;
        if decision.permits(access) {
            Ok(decision)
        } else {
            Err("ims_access_registration_parked")
        }
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
            concurrent_support: CURRENT_CONCURRENT_SUPPORT,
            desired,
            applied: observation.applied,
            switch_deferred_for_call: observation.switch_deferred_for_call,
            cellular_last_response: observation.cellular,
            wlan_last_response: observation.wlan,
        }
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
    fn response_evidence_never_enables_incomplete_local_outbound() {
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
            ConcurrentRegistrationSupport::ClientIncomplete
        );
        assert_eq!(status.wlan_last_response.unwrap().require_outbound, true);
        assert!(status.cellular_last_response.is_none());
        assert_eq!(status.effective, "none"); // evidence is not admission
    }
}
