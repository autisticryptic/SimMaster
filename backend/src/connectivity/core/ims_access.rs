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
    /// Request concurrency when supported. Otherwise retain the valid existing
    /// registration; prefer cellular on cold start, with WLAN as fallback.
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
    #[default]
    ClientIncomplete,
    /// A fully capable client has not established network support on the
    /// active registration (including the first-hop outbound procedure).
    NotNegotiated,
    /// Complete local implementation AND successful outbound negotiation.
    Negotiated,
}

/// Do not change this by adding a Supported token or by observing an unrelated
/// Require header. UDP STUN/flow timers and flow recovery must be implemented
/// and negotiated before enabling the concurrent branch.
pub const CURRENT_CONCURRENT_SUPPORT: ConcurrentRegistrationSupport =
    ConcurrentRegistrationSupport::ClientIncomplete;

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
                if inputs.concurrent_support == ConcurrentRegistrationSupport::Negotiated {
                    ImsAccessDecision::both("ims_access_concurrent_negotiated")
                } else if inputs.wlan_registered && !inputs.cellular_registered {
                    // Do not flap back from a working fallback just because
                    // the primary modem has reappeared.
                    ImsAccessDecision::wlan_only("ims_access_single_preserve_wlan")
                } else {
                    // Also resolves a legacy, unconfirmed dual registration
                    // deterministically. Reconciliation defers teardown in calls.
                    ImsAccessDecision::cellular_only("ims_access_single_cellular")
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
        assert!(!decide(both(ImsAccessPreference::Concurrent)).wlan_registers);
    }

    #[test]
    fn concurrent_intent_is_preserved_but_default_cold_start_is_single() {
        assert_eq!(
            ImsAccessPreference::default(),
            ImsAccessPreference::Concurrent
        );
        assert_eq!(
            CURRENT_CONCURRENT_SUPPORT,
            ConcurrentRegistrationSupport::ClientIncomplete
        );
        let d = decide(both(ImsAccessPreference::default()));
        assert!(d.cellular_registers && !d.wlan_registers);
        assert_eq!(d.effective_mode(), "single_registration");
    }

    #[test]
    fn concurrency_requires_both_client_support_and_network_negotiation() {
        for support in [
            ConcurrentRegistrationSupport::ClientIncomplete,
            ConcurrentRegistrationSupport::NotNegotiated,
            ConcurrentRegistrationSupport::Negotiated,
        ] {
            let mut i = both(ImsAccessPreference::Concurrent);
            i.concurrent_support = support;
            let d = decide(i);
            assert!(d.cellular_registers);
            assert_eq!(
                d.wlan_registers,
                support == ConcurrentRegistrationSupport::Negotiated
            );
        }
    }

    #[test]
    fn enabled_wlan_does_not_block_cellular_refresh_even_during_retry() {
        let mut i = both(ImsAccessPreference::Concurrent);
        i.cellular_registered = true;
        i.cellular_available = false;
        let d = decide(i);
        assert!(d.permits(ImsAccess::Cellular));
        assert!(!d.permits(ImsAccess::Wlan));
        assert!(d.legs_to_release(true, false).is_empty());
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
    fn exhausted_cellular_allows_wlan_but_pending_attempt_is_not_registration() {
        let mut i = both(ImsAccessPreference::Concurrent);
        i.cellular_available = false;
        assert!(decide(i).wlan_registers);
        i.cellular_available = true;
        assert!(!decide(i).wlan_registers);
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
    fn resolves_legacy_unconfirmed_dual_registration_without_disabling_intents() {
        let mut i = both(ImsAccessPreference::Concurrent);
        i.cellular_registered = true;
        i.wlan_registered = true;
        assert_eq!(decide(i).legs_to_release(true, true), vec![ImsAccess::Wlan]);
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
            });
            assert!(!(d.cellular_registers && d.wlan_registers), "{bits}");
        }
    }
}
