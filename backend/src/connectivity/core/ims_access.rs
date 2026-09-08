//! Registration access selection, not originating-call routing.
//!
//! Distinct reg-id values alone do NOT authorize concurrent IMS registrations.
//! TS 24.229 5.1.1.2.1(f) requires a successful response with Require: outbound
//! after an outbound-capable REGISTER before an additional flow is registered.
//! Without that negotiation the new contact can replace the old one. RFC 5626
//! also requires flow maintenance (including STUN for UDP, 4.4.2); merely adding
//! Supported: outbound is not an implementation of that protocol.
//!
//! Keep the user's preference and enabled intents, but coordinate a single
//! registration until both the client and the network support multiple flows.
//! This is break-before-make selection/fallback, NOT seamless IR.51 handover.
//! A valid selected registration always remains eligible for protected refresh.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImsAccess {
    Cellular,
    Wlan,
}

impl ImsAccess {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cellular => "cellular",
            Self::Wlan => "wlan",
        }
    }

    /// Stable, distinct flow identifiers. Necessary but not sufficient for
    /// RFC 5626: the registrar may ignore these without outbound negotiation.
    pub const fn reg_id(self) -> u32 {
        match self {
            Self::Cellular => 1,
            Self::Wlan => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ImsAccessPreference {
    /// Request concurrency when supported. Otherwise prefer WLAN, with
    /// cellular as the bounded-recovery fallback (including existing profiles).
    #[default]
    Concurrent,
    WlanPreferred,
    CellularPreferred,
}

impl ImsAccessPreference {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Concurrent => "concurrent",
            Self::WlanPreferred => "wlan_preferred",
            Self::CellularPreferred => "cellular_preferred",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrentRegistrationSupport {
    /// The current client lacks complete RFC 5626 flow maintenance. This says
    /// nothing about whether an operator supports it with a capable client.
    ClientIncomplete,
    /// No live binding has established or declined network support yet.
    #[default]
    NotNegotiated,
    /// An owned successful registration offered outbound, but its response
    /// did not accept it. Scoped to the CURRENT flow, not a permanent carrier
    /// blacklist or an inference from a timeout.
    NotSupported,
    /// An owned live binding negotiated outbound. Transport validation is
    /// checked separately before creating an additional flow; a delayed pong
    /// must not erase negotiated capability and tear down existing bindings.
    Negotiated,
}

/// Bootstrap state only. Runtime authorization comes from an owned live flow
/// with matching Contact, Require/Path negotiation and maintained transport
/// (acknowledged keepalive, or the TS 24.229 no-NAT logical-flow exemption).
pub const CURRENT_CONCURRENT_SUPPORT: ConcurrentRegistrationSupport =
    ConcurrentRegistrationSupport::NotNegotiated;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImsAccessInputs {
    pub cellular_enabled: bool,
    pub wlan_enabled: bool,
    /// Eligible to attempt registration; exhausted recovery is not available.
    pub cellular_available: bool,
    pub wlan_available: bool,
    /// Valid lease, NOT the proactive refresh deadline or a pending attempt.
    pub cellular_registered: bool,
    pub wlan_registered: bool,
    pub device_identity_spoofed: bool,
    pub preference: ImsAccessPreference,
    pub concurrent_support: ConcurrentRegistrationSupport,
    /// An additional flow was rejected or failed negotiation validation.
    /// This does not revoke an existing primary's negotiated capability/lease.
    pub multiple_registration_blocked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ImsAccessDecision {
    pub cellular_registers: bool,
    pub wlan_registers: bool,
    pub code: &'static str,
}

impl ImsAccessDecision {
    pub const fn none(code: &'static str) -> Self {
        Self {
            cellular_registers: false,
            wlan_registers: false,
            code,
        }
    }

    pub const fn cellular_only(code: &'static str) -> Self {
        Self {
            cellular_registers: true,
            wlan_registers: false,
            code,
        }
    }

    pub const fn wlan_only(code: &'static str) -> Self {
        Self {
            cellular_registers: false,
            wlan_registers: true,
            code,
        }
    }

    const fn both(code: &'static str) -> Self {
        Self {
            cellular_registers: true,
            wlan_registers: true,
            code,
        }
    }

    pub const fn permits(&self, access: ImsAccess) -> bool {
        match access {
            ImsAccess::Cellular => self.cellular_registers,
            ImsAccess::Wlan => self.wlan_registers,
        }
    }

    pub fn legs_to_release(&self, cellular_up: bool, wlan_up: bool) -> Vec<ImsAccess> {
        let mut release = Vec::new();
        if cellular_up && !self.cellular_registers {
            release.push(ImsAccess::Cellular);
        }
        if wlan_up && !self.wlan_registers {
            release.push(ImsAccess::Wlan);
        }
        release
    }

    pub const fn effective_mode(&self) -> &'static str {
        match (self.cellular_registers, self.wlan_registers) {
            (true, true) => "concurrent",
            (true, false) | (false, true) => "single_registration",
            (false, false) => "none",
        }
    }
}

pub fn decide(inputs: ImsAccessInputs) -> ImsAccessDecision {
    // A refresh retry must not disqualify an unexpired registration. The
    // opposite access being enabled/available is not evidence of lease loss.
    let cellular =
        inputs.cellular_enabled && (inputs.cellular_available || inputs.cellular_registered);
    let wlan = inputs.wlan_enabled && (inputs.wlan_available || inputs.wlan_registered);
    // Preserve the existing device-identity rule: a spoofed identity must not
    // leak back to the cellular attachment using the modem's real identity.
    if inputs.device_identity_spoofed {
        return if wlan {
            ImsAccessDecision::wlan_only("ims_access_wlan_only_spoofed_device_identity")
        } else {
            ImsAccessDecision::none("ims_access_none_spoofed_identity_requires_wlan")
        };
    }
    match inputs.preference {
        ImsAccessPreference::Concurrent => match (cellular, wlan) {
            (true, true) => {
                if inputs.concurrent_support == ConcurrentRegistrationSupport::Negotiated
                    && !inputs.multiple_registration_blocked
                {
                    ImsAccessDecision::both("ims_access_concurrent_negotiated")
                } else if inputs.multiple_registration_blocked {
                    ImsAccessDecision::wlan_only("ims_access_single_wlan_multi_flow_blocked")
                } else {
                    // Concurrency is a capability request, NOT cellular-first
                    // preference. The coordinator defers this switch in calls;
                    // exhausted WLAN recovery removes WLAN eligibility above.
                    ImsAccessDecision::wlan_only("ims_access_single_wlan_preferred")
                }
            }
            (true, false) => ImsAccessDecision::cellular_only("ims_access_cellular_only_available"),
            (false, true) => ImsAccessDecision::wlan_only("ims_access_wlan_only_available"),
            (false, false) => ImsAccessDecision::none("ims_access_none_available"),
        },
        ImsAccessPreference::WlanPreferred => {
            if wlan {
                ImsAccessDecision::wlan_only("ims_access_wlan_preferred")
            } else if cellular {
                ImsAccessDecision::cellular_only("ims_access_cellular_fallback")
            } else {
                ImsAccessDecision::none("ims_access_none_available")
            }
        }
        ImsAccessPreference::CellularPreferred => {
            if cellular {
                ImsAccessDecision::cellular_only("ims_access_cellular_preferred")
            } else if wlan {
                ImsAccessDecision::wlan_only("ims_access_wlan_fallback")
            } else {
                ImsAccessDecision::none("ims_access_none_available")
            }
        }
    }
}

/// Conservatively protect incoming/ringing/dialing and unknown states as well
/// as established/held calls. This is access teardown, not just media rebuild.
pub fn call_blocks_access_switch(state: &str) -> bool {
    !matches!(
        state,
        "terminated"
            | "ended"
            | "failed"
            | "released"
            | "disconnected"
            | "cancelled"
            | "rejected"
            | "completed"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both(preference: ImsAccessPreference) -> ImsAccessInputs {
        ImsAccessInputs {
            cellular_enabled: true,
            wlan_enabled: true,
            cellular_available: true,
            wlan_available: true,
            preference,
            ..Default::default()
        }
    }

    #[test]
    fn reg_ids_are_distinct_stable_and_positive_not_a_capability_gate() {
        assert_eq!(ImsAccess::Cellular.reg_id(), 1);
        assert_eq!(ImsAccess::Wlan.reg_id(), 2);
        let decision = decide(both(ImsAccessPreference::Concurrent));
        assert!(decision.wlan_registers && !decision.cellular_registers);
    }

    #[test]
    fn concurrent_intent_is_preserved_but_default_cold_start_is_single() {
        assert_eq!(
            ImsAccessPreference::default(),
            ImsAccessPreference::Concurrent
        );
        assert_eq!(
            CURRENT_CONCURRENT_SUPPORT,
            ConcurrentRegistrationSupport::NotNegotiated
        );
        let d = decide(both(ImsAccessPreference::default()));
        assert!(d.wlan_registers && !d.cellular_registers);
        assert_eq!(d.effective_mode(), "single_registration");
    }

    #[test]
    fn concurrency_requires_both_client_support_and_network_negotiation() {
        for support in [
            ConcurrentRegistrationSupport::ClientIncomplete,
            ConcurrentRegistrationSupport::NotNegotiated,
            ConcurrentRegistrationSupport::NotSupported,
            ConcurrentRegistrationSupport::Negotiated,
        ] {
            let mut i = both(ImsAccessPreference::Concurrent);
            i.concurrent_support = support;
            let d = decide(i);
            assert!(d.wlan_registers);
            assert_eq!(
                d.cellular_registers,
                support == ConcurrentRegistrationSupport::Negotiated
            );
        }
    }

    #[test]
    fn enabled_wlan_does_not_block_cellular_refresh_even_during_retry() {
        let mut i = both(ImsAccessPreference::Concurrent);
        i.cellular_registered = true;
        i.cellular_available = false;
        i.wlan_available = false; // WLAN retry/backoff must not block the fallback refresh.
        let d = decide(i);
        assert!(d.permits(ImsAccess::Cellular));
        assert!(!d.permits(ImsAccess::Wlan));
        assert!(d.legs_to_release(true, false).is_empty());
    }

    #[test]
    fn negotiated_backup_refresh_stays_admitted_while_wlan_recovers() {
        let mut i = both(ImsAccessPreference::Concurrent);
        i.concurrent_support = ConcurrentRegistrationSupport::Negotiated;
        i.cellular_registered = true;
        i.cellular_available = false;
        let d = decide(i);
        assert!(d.permits(ImsAccess::Cellular) && d.permits(ImsAccess::Wlan));
        assert!(d.legs_to_release(true, false).is_empty());
        i.cellular_enabled = false;
        assert!(!decide(i).cellular_registers);
        i.wlan_enabled = false;
        assert_eq!(decide(i).effective_mode(), "none");
    }

    #[test]
    fn returning_wlan_requests_switch_from_legacy_cellular_not_competing_register() {
        let mut i = both(ImsAccessPreference::Concurrent);
        i.cellular_registered = true;
        let d = decide(i);
        assert!(d.wlan_registers && !d.cellular_registers);
        assert_eq!(d.legs_to_release(true, false), vec![ImsAccess::Cellular]);
    }

    #[test]
    fn valid_wlan_fallback_is_sticky_when_cellular_returns() {
        let mut i = both(ImsAccessPreference::Concurrent);
        i.wlan_registered = true;
        assert!(decide(i).wlan_registers);
        assert!(!decide(i).cellular_registers);
        i.wlan_available = false; // refresh due is not lease expiry
        assert!(decide(i).wlan_registers);
        i.wlan_registered = false;
        assert!(decide(i).cellular_registers);
    }

    #[test]
    fn cellular_return_does_not_preempt_wlan_priority() {
        let mut i = both(ImsAccessPreference::Concurrent);
        i.cellular_available = false;
        assert!(decide(i).wlan_registers);
        i.cellular_available = true;
        assert!(decide(i).wlan_registers);
        assert!(!decide(i).cellular_registers);
    }

    #[test]
    fn explicit_preferences_switch_and_fall_back_symmetrically() {
        let mut i = both(ImsAccessPreference::WlanPreferred);
        i.cellular_registered = true;
        assert_eq!(
            decide(i).legs_to_release(true, false),
            vec![ImsAccess::Cellular]
        );
        i.wlan_available = false;
        assert!(decide(i).cellular_registers);
        i = both(ImsAccessPreference::CellularPreferred);
        i.wlan_registered = true;
        assert_eq!(
            decide(i).legs_to_release(false, true),
            vec![ImsAccess::Wlan]
        );
        i.cellular_available = false;
        assert!(decide(i).wlan_registers);
    }

    #[test]
    fn single_mode_and_multi_flow_refusal_keep_wlan_then_cellular_priority() {
        let mut inputs = both(ImsAccessPreference::WlanPreferred);
        inputs.concurrent_support = ConcurrentRegistrationSupport::Negotiated;
        assert_eq!(decide(inputs).effective_mode(), "single_registration");
        assert!(decide(inputs).wlan_registers);
        inputs.preference = ImsAccessPreference::Concurrent;
        inputs.cellular_registered = true;
        inputs.multiple_registration_blocked = true;
        let fallback = decide(inputs);
        assert!(fallback.wlan_registers && !fallback.cellular_registers);
        assert_eq!(
            fallback.legs_to_release(true, false),
            vec![ImsAccess::Cellular]
        );
        inputs.wlan_available = false;
        let keep_primary = decide(inputs);
        assert!(keep_primary.cellular_registers && !keep_primary.wlan_registers);
        assert!(keep_primary.legs_to_release(true, false).is_empty());
        inputs.cellular_registered = false;
        inputs.cellular_available = false;
        assert_eq!(decide(inputs).effective_mode(), "none");
    }

    #[test]
    fn resolves_legacy_unconfirmed_dual_registration_without_disabling_intents() {
        let mut i = both(ImsAccessPreference::Concurrent);
        i.cellular_registered = true;
        i.wlan_registered = true;
        assert_eq!(
            decide(i).legs_to_release(true, true),
            vec![ImsAccess::Cellular]
        );
        assert!(i.cellular_enabled && i.wlan_enabled);
    }

    #[test]
    fn disabled_intent_wins_even_over_a_stale_registration_snapshot() {
        let mut i = both(ImsAccessPreference::Concurrent);
        i.cellular_registered = true;
        i.cellular_enabled = false;
        assert!(!decide(i).cellular_registers);
        i.wlan_enabled = false;
        assert_eq!(decide(i).effective_mode(), "none");
    }

    #[test]
    fn spoofed_identity_never_falls_back_to_cellular() {
        for preference in [
            ImsAccessPreference::Concurrent,
            ImsAccessPreference::WlanPreferred,
            ImsAccessPreference::CellularPreferred,
        ] {
            let mut i = both(preference);
            i.device_identity_spoofed = true;
            assert!(!decide(i).cellular_registers);
            assert!(decide(i).wlan_registers);
            i.wlan_available = false;
            assert_eq!(decide(i).effective_mode(), "none");
        }
    }

    #[test]
    fn every_nonterminal_call_state_defers_access_teardown() {
        for state in [
            "active",
            "held",
            "incoming",
            "ringing",
            "dialing",
            "connecting",
            "unknown",
            "",
        ] {
            assert!(call_blocks_access_switch(state), "{state}");
        }
        for state in [
            "terminated",
            "ended",
            "failed",
            "released",
            "disconnected",
            "cancelled",
            "rejected",
            "completed",
        ] {
            assert!(!call_blocks_access_switch(state), "{state}");
        }
    }

    #[test]
    fn unconfirmed_concurrency_never_authorizes_two_legs_for_any_input_combination() {
        for bits in 0_u16..512 {
            let d = decide(ImsAccessInputs {
                cellular_enabled: bits & 1 != 0,
                wlan_enabled: bits & 2 != 0,
                cellular_available: bits & 4 != 0,
                wlan_available: bits & 8 != 0,
                cellular_registered: bits & 16 != 0,
                wlan_registered: bits & 32 != 0,
                device_identity_spoofed: bits & 64 != 0,
                concurrent_support: if bits & 128 != 0 {
                    ConcurrentRegistrationSupport::ClientIncomplete
                } else {
                    ConcurrentRegistrationSupport::NotNegotiated
                },
                preference: ImsAccessPreference::Concurrent,
                multiple_registration_blocked: false,
            });
            assert!(!(d.cellular_registers && d.wlan_registers), "{bits}");
        }
    }
}
