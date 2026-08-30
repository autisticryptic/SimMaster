//! Which IMS access leg may hold a registration, and how each leg identifies
//! its flow.
//!
//! # Why this module exists
//!
//! SimAdmin can bring up two IMS access legs for one subscription: VoLTE over
//! the cellular IMS bearer, and VoWiFi over the ePDG. Both were independent
//! booleans, so both could register the *same* IMPU at the same time, to two
//! different P-CSCFs, with no coordination and no observable rule about which
//! one a terminating call would reach.
//!
//! # What the specifications actually require
//!
//! GSMA IR.51 (IMS profile for voice, video and SMS over untrusted Wi-Fi)
//! describes **one** IMS registration that follows the access, not two
//! concurrent ones:
//!
//! * §2.2.1: when the PDN connection to the IMS well-known APN moves between
//!   Wi-Fi and cellular, the UE must "initiate re-registration procedure as
//!   specified in 3GPP TS 24.229, section 5.1.1.4", update `P-Access-Network-Info`
//!   and update the `g.3gpp.accesstype` media feature tag. Re-registration of
//!   the existing registration — not an additional registration.
//! * §4.8 / §5.1: "After the UE has discovered the P-CSCF and registered to IMS,
//!   the UE must use this P-CSCF as long as the IMS registration is valid",
//!   explicitly including when the IMS APN is handed over between Wi-Fi and
//!   LTE. One registration, one P-CSCF, across the access change.
//! * IR.51 says nothing about `reg-id` at all, because in its model there is
//!   only ever one flow to identify.
//!
//! RFC 5626 does allow one UA to hold several simultaneous flows, but it is
//! strict about how they are told apart (§6): a binding is keyed on the
//! **(AOR, instance-id, reg-id)** triple. Consequently (§4.2.1) a UA with more
//! than one simultaneous registration MUST use a `reg-id` "distinct from other
//! `reg-id` parameters used in other registrations that use the same
//! `+sip.instance` parameter and AOR" — and if the pair repeats, the registrar
//! "replaces the old Contact URI and flow information" (§3.2) instead of
//! keeping both.
//!
//! # Consequences for this codebase
//!
//! Two rules fall directly out of the above, and [`ImsAccess::reg_id`] plus
//! [`decide`] implement them:
//!
//! 1. Each access leg gets its own fixed `reg-id`. Sharing one stable
//!    `+sip.instance` across the legs (which RFC 5626 §4.1 requires, since the
//!    instance id names the *UE*) is only safe when the `reg-id` differs;
//!    otherwise the second leg to register silently replaces the first leg's
//!    binding while our own runtime still believes it is registered. The values
//!    are constants so they repeat across restarts, as §4.2.1 requires, letting
//!    a stale binding be replaced rather than accumulate.
//! 2. Both legs stay registered by default, and a single-registration model is
//!    available as an explicit opt-in. The default is *not* IR.51's model, and
//!    that is a deliberate, documented divergence — see below.
//!
//! # Why the default keeps both legs registered
//!
//! IR.51 describes one registration following the access, but this project
//! already made the opposite choice for a concrete reason, recorded in
//! `services::orchestrator::ims_access`: tearing down the cellular leg when
//! Wi-Fi calling comes up means a Wi-Fi drop leaves the line with **no** voice
//! path until a full re-registration completes, and the voice router has nothing
//! to fall back to because only one leg is ever registered. That module encodes
//! "a path may be `Registered` while the other path is also `Registered`" as an
//! invariant with tests to match.
//!
//! Switching the default to single-registration would silently reintroduce that
//! fault, so this module keeps [`ImsAccessPreference::Concurrent`] as the
//! default and makes the IR.51 model selectable per line. RFC 5626 §4.2.1
//! permits simultaneous flows outright, so the concurrent default is standards-
//! legal; what it gives up is IR.51's guarantee about *which* binding a
//! terminating call reaches, which is why the choice is exposed rather than
//! hard-coded.
//!
//! This module decides *registration*. It is deliberately separate from
//! `voice_path` priority, which orders **originating** call routing over legs
//! that are already registered, and from
//! `services::orchestrator::ims_access`, which *describes* observed access
//! state. This one decides what is *permitted* to register.

use serde::{Deserialize, Serialize};

/// One IMS access leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImsAccess {
    /// VoLTE over the cellular IMS bearer.
    Cellular,
    /// VoWiFi over untrusted WLAN via the ePDG.
    Wlan,
}

impl ImsAccess {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cellular => "cellular",
            Self::Wlan => "wlan",
        }
    }

    /// RFC 5626 `reg-id` for this leg's flow.
    ///
    /// Fixed per access so the same value recurs after a restart (§4.2.1) and
    /// so the two legs never collide on the (AOR, instance-id, reg-id) key
    /// (§6). Both are non-zero and far below 2^31, as §10 requires.
    pub const fn reg_id(self) -> u32 {
        match self {
            Self::Cellular => 1,
            Self::Wlan => 2,
        }
    }
}

/// Which leg should hold the registration when both are configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ImsAccessPreference {
    /// Keep both legs registered at once, each with its own `reg-id`.
    ///
    /// This is the default, and deliberately so: it is what this project
    /// already did, and `services::orchestrator::ims_access` encodes it as an
    /// invariant ("a path may be `Registered` while the other path is also
    /// `Registered`"). Tearing the cellular leg down when Wi-Fi comes up was a
    /// fixed bug — a Wi-Fi drop then left the line with no voice path until a
    /// full re-registration completed, and the voice router lost its fallback.
    ///
    /// RFC 5626 permits this explicitly, provided each flow carries a distinct
    /// `reg-id` (§4.2.1), which [`ImsAccess::reg_id`] guarantees. The cost is
    /// that terminating-call delivery across two bindings is a carrier TAS
    /// decision this project can neither observe nor control.
    #[default]
    Concurrent,
    /// GSMA IR.51's single-registration model, preferring Wi-Fi: register over
    /// WLAN whenever it is usable, otherwise over cellular. Opt-in, because it
    /// gives up the live fallback the concurrent model provides.
    WlanPreferred,
    /// Single-registration model preferring the cellular leg. Opt-in for the
    /// same reason.
    CellularPreferred,
}

impl ImsAccessPreference {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WlanPreferred => "wlan_preferred",
            Self::CellularPreferred => "cellular_preferred",
            Self::Concurrent => "concurrent",
        }
    }
}

/// Everything the decision depends on. Callers pass what they have observed;
/// this module performs no I/O so the rule stays testable and auditable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImsAccessInputs {
    /// The line's VoLTE connection is switched on.
    pub cellular_enabled: bool,
    /// The line's VoWiFi connection is switched on.
    pub wlan_enabled: bool,
    /// A modem and IMS bearer are actually usable right now.
    pub cellular_available: bool,
    /// An ePDG path is actually usable right now.
    pub wlan_available: bool,
    /// The presented device identity is a user-supplied IMEI rather than the
    /// modem's own. See [`decide`] for why this excludes the cellular leg.
    pub device_identity_spoofed: bool,
    pub preference: ImsAccessPreference,
}

impl Default for ImsAccessInputs {
    fn default() -> Self {
        Self {
            cellular_enabled: false,
            wlan_enabled: false,
            cellular_available: false,
            wlan_available: false,
            device_identity_spoofed: false,
            preference: ImsAccessPreference::default(),
        }
    }
}

/// Which legs may register, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ImsAccessDecision {
    pub cellular_registers: bool,
    pub wlan_registers: bool,
    /// Stable machine code naming the rule that produced this outcome, so the
    /// decision is reportable instead of inferred from behaviour.
    pub code: &'static str,
}

impl ImsAccessDecision {
    const fn none(code: &'static str) -> Self {
        Self {
            cellular_registers: false,
            wlan_registers: false,
            code,
        }
    }

    const fn cellular_only(code: &'static str) -> Self {
        Self {
            cellular_registers: true,
            wlan_registers: false,
            code,
        }
    }

    const fn wlan_only(code: &'static str) -> Self {
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

    /// Whether this leg is permitted to hold an IMS registration.
    pub const fn permits(&self, access: ImsAccess) -> bool {
        match access {
            ImsAccess::Cellular => self.cellular_registers,
            ImsAccess::Wlan => self.wlan_registers,
        }
    }

    /// Legs that must be torn down to reach this decision, given what is up.
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
}

/// Decide which access legs may register IMS for one line.
///
/// # The spoofed-identity rule
///
/// When the presented device identity is a user-supplied IMEI, the cellular leg
/// is excluded regardless of preference. The baseband has already attached to
/// that same operator's EPS/5GS core with its own real IMEISV, so a cellular IMS
/// registration presenting a different identity is contradicted by the attach
/// that carries it — the two identities reach one network. Over untrusted WLAN
/// the ePDG sees only what we present in IKE_AUTH and `+sip.instance`, so the
/// WLAN leg is the only one where a presented identity is coherent. With
/// spoofing off, both legs present the identical identity and the ordinary
/// preference applies.
pub fn decide(inputs: ImsAccessInputs) -> ImsAccessDecision {
    let cellular_usable = inputs.cellular_enabled && inputs.cellular_available;
    let wlan_usable = inputs.wlan_enabled && inputs.wlan_available;

    if inputs.device_identity_spoofed {
        // The cellular leg cannot carry a presented identity coherently, so it
        // stays down even when it is enabled and available.
        return if wlan_usable {
            ImsAccessDecision::wlan_only("ims_access_wlan_only_spoofed_device_identity")
        } else {
            ImsAccessDecision::none("ims_access_none_spoofed_identity_requires_wlan")
        };
    }

    match inputs.preference {
        ImsAccessPreference::Concurrent => match (cellular_usable, wlan_usable) {
            (true, true) => ImsAccessDecision::both("ims_access_concurrent_both_legs"),
            (true, false) => ImsAccessDecision::cellular_only("ims_access_cellular_only_available"),
            (false, true) => ImsAccessDecision::wlan_only("ims_access_wlan_only_available"),
            (false, false) => ImsAccessDecision::none("ims_access_none_available"),
        },
        ImsAccessPreference::WlanPreferred => {
            if wlan_usable {
                ImsAccessDecision::wlan_only("ims_access_wlan_preferred")
            } else if cellular_usable {
                ImsAccessDecision::cellular_only("ims_access_cellular_fallback")
            } else {
                ImsAccessDecision::none("ims_access_none_available")
            }
        }
        ImsAccessPreference::CellularPreferred => {
            if cellular_usable {
                ImsAccessDecision::cellular_only("ims_access_cellular_preferred")
            } else if wlan_usable {
                ImsAccessDecision::wlan_only("ims_access_wlan_fallback")
            } else {
                ImsAccessDecision::none("ims_access_none_available")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both_up(preference: ImsAccessPreference) -> ImsAccessInputs {
        ImsAccessInputs {
            cellular_enabled: true,
            wlan_enabled: true,
            cellular_available: true,
            wlan_available: true,
            device_identity_spoofed: false,
            preference,
        }
    }

    #[test]
    fn reg_ids_differ_per_access_so_bindings_cannot_replace_each_other() {
        // RFC 5626 §6 keys a binding on (AOR, instance-id, reg-id). Both legs
        // now share one instance id, so identical reg-ids would make the second
        // registration replace the first -- silently, while our runtime still
        // reported both as registered.
        assert_ne!(ImsAccess::Cellular.reg_id(), ImsAccess::Wlan.reg_id());
        // §10: non-zero and below 2^31.
        for access in [ImsAccess::Cellular, ImsAccess::Wlan] {
            assert_ne!(access.reg_id(), 0);
            assert!(access.reg_id() < 1 << 31);
        }
    }

    #[test]
    fn reg_ids_are_constant_across_restarts() {
        // §4.2.1 requires the same sequence of reg-id values after a reboot, so
        // a stale binding is replaced rather than left beside the new one.
        assert_eq!(ImsAccess::Cellular.reg_id(), 1);
        assert_eq!(ImsAccess::Wlan.reg_id(), 2);
    }

    #[test]
    fn default_preference_keeps_both_legs_registered() {
        // Regression guard. `services::orchestrator::ims_access` documents
        // coexistence as an invariant, and tearing the cellular leg down when
        // Wi-Fi comes up was a fixed bug: a Wi-Fi drop then left the line with
        // no voice path and the router with no fallback. Introducing a
        // single-registration default here would reintroduce exactly that, so
        // the default must stay Concurrent.
        assert_eq!(
            ImsAccessPreference::default(),
            ImsAccessPreference::Concurrent
        );
        let decision = decide(both_up(ImsAccessPreference::default()));
        assert!(decision.cellular_registers);
        assert!(decision.wlan_registers);
        assert_eq!(decision.code, "ims_access_concurrent_both_legs");
    }

    #[test]
    fn wlan_preferred_falls_back_to_cellular_when_wlan_is_unusable() {
        let mut inputs = both_up(ImsAccessPreference::WlanPreferred);
        inputs.wlan_available = false;
        let decision = decide(inputs);
        assert!(decision.cellular_registers);
        assert!(!decision.wlan_registers);
        assert_eq!(decision.code, "ims_access_cellular_fallback");

        // Disabled counts the same as unavailable.
        let mut disabled = both_up(ImsAccessPreference::WlanPreferred);
        disabled.wlan_enabled = false;
        assert!(decide(disabled).cellular_registers);
    }

    #[test]
    fn cellular_preferred_is_the_mirror_image() {
        let decision = decide(both_up(ImsAccessPreference::CellularPreferred));
        assert!(decision.cellular_registers);
        assert!(!decision.wlan_registers);
        assert_eq!(decision.code, "ims_access_cellular_preferred");

        let mut inputs = both_up(ImsAccessPreference::CellularPreferred);
        inputs.cellular_available = false;
        let fallback = decide(inputs);
        assert!(fallback.wlan_registers);
        assert!(!fallback.cellular_registers);
        assert_eq!(fallback.code, "ims_access_wlan_fallback");
    }

    #[test]
    fn concurrent_registers_both_legs_and_is_the_default() {
        // Guard the existing invariant in services::orchestrator::ims_access:
        // both legs registered at once is the normal case. A default that tore
        // the cellular leg down when Wi-Fi came up would reintroduce the fixed
        // bug where a Wi-Fi drop left the line with no voice path at all.
        assert_eq!(
            ImsAccessPreference::default(),
            ImsAccessPreference::Concurrent
        );
        let decision = decide(both_up(ImsAccessPreference::Concurrent));
        assert!(decision.cellular_registers && decision.wlan_registers);
        assert_eq!(decision.code, "ims_access_concurrent_both_legs");
    }

    #[test]
    fn single_registration_modes_are_opt_in_only() {
        // Both IR.51-style modes must be reachable, but neither may become the
        // default by accident: they give up the live fallback leg.
        for preference in [
            ImsAccessPreference::WlanPreferred,
            ImsAccessPreference::CellularPreferred,
        ] {
            assert_ne!(ImsAccessPreference::default(), preference);
            let decision = decide(both_up(preference));
            assert!(
                decision.cellular_registers ^ decision.wlan_registers,
                "{preference:?} must register exactly one leg"
            );
        }
    }

    #[test]
    fn spoofed_device_identity_keeps_only_the_wlan_leg() {
        // The baseband already attached with its real IMEISV to the same
        // operator, so a cellular IMS leg presenting a different identity is
        // contradicted by its own attach.
        for preference in [
            ImsAccessPreference::WlanPreferred,
            ImsAccessPreference::CellularPreferred,
            ImsAccessPreference::Concurrent,
        ] {
            let mut inputs = both_up(preference);
            inputs.device_identity_spoofed = true;
            let decision = decide(inputs);
            assert!(
                !decision.cellular_registers,
                "cellular must stay down under {preference:?}"
            );
            assert!(decision.wlan_registers);
            assert_eq!(
                decision.code,
                "ims_access_wlan_only_spoofed_device_identity"
            );
        }
    }

    #[test]
    fn spoofing_without_a_usable_wlan_leg_registers_nothing() {
        // Deliberately not a silent fallback to cellular: that would present
        // the real identity after the user asked for a different one.
        let mut inputs = both_up(ImsAccessPreference::CellularPreferred);
        inputs.device_identity_spoofed = true;
        inputs.wlan_available = false;
        let decision = decide(inputs);
        assert!(!decision.cellular_registers && !decision.wlan_registers);
        assert_eq!(
            decision.code,
            "ims_access_none_spoofed_identity_requires_wlan"
        );
    }

    #[test]
    fn spoofing_off_leaves_both_legs_on_equal_footing() {
        // With no disguise the legs present the identical identity, so the
        // ordinary preference decides and cellular is reachable again.
        let mut inputs = both_up(ImsAccessPreference::CellularPreferred);
        inputs.device_identity_spoofed = false;
        assert!(decide(inputs).cellular_registers);
    }

    #[test]
    fn legs_to_release_names_only_what_is_up_and_no_longer_permitted() {
        let decision = decide(both_up(ImsAccessPreference::WlanPreferred));
        // Cellular was up from a previous decision and must now be torn down.
        assert_eq!(
            decision.legs_to_release(true, true),
            vec![ImsAccess::Cellular]
        );
        // Nothing to do when only the permitted leg is up.
        assert!(decision.legs_to_release(false, true).is_empty());
        // A leg that is already down is not released again.
        assert!(decision.legs_to_release(false, false).is_empty());
    }

    #[test]
    fn permits_matches_the_boolean_fields() {
        let decision = decide(both_up(ImsAccessPreference::WlanPreferred));
        assert!(decision.permits(ImsAccess::Wlan));
        assert!(!decision.permits(ImsAccess::Cellular));
    }

    #[test]
    fn nothing_enabled_registers_nothing() {
        let decision = decide(ImsAccessInputs::default());
        assert!(!decision.cellular_registers && !decision.wlan_registers);
        assert_eq!(decision.code, "ims_access_none_available");
    }
}
