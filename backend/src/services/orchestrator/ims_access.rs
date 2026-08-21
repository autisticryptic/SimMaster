//! Layered IMS access-path state.
//!
//! # Why this module exists
//!
//! IMS, the access paths that reach it, and the access chosen to carry voice are
//! four different things. Collapsing them into one `ims_registered` flag (or into
//! "VoLTE mode" vs "VoWiFi mode") produces two concrete bugs:
//!
//!   * bringing up Wi-Fi calling tears down a perfectly good LTE registration, so
//!     a Wi-Fi drop leaves the line with no voice path at all until a full
//!     re-registration completes;
//!   * the voice router has nothing to fall back to, because only one leg is ever
//!     registered at a time.
//!
//! 3GPP models these separately (TS 23.402 access selection, TS 24.229 IMS
//! registration), and so does this module:
//!
//! ```text
//! IMS
//!├── Registration state          -> ImsRegistrationState (a set, never a bool)
//! │
//! ├── 3GPP access                 -> ThreeGppAccess
//! │   ├── LTE/5G connectivity        radio_available / bearer_up
//! │   ├── P-CSCF                     path.pcscf
//! │   └── IMS access state           path.stage
//! │
//! └── Non-3GPP access             -> NonThreeGppAccess
//!     ├── Wi-Fi + IKEv2/IPsec        ike_ready / child_sa_ready / esp_ready
//!     ├── ePDG                       epdg_ready
//!     ├── P-CSCF                     path.pcscf
//!     └── IMS access state           path.stage
//! ```
//!
//! and the access that carries voice right now is a *selection over* those paths
//! ([`VoiceAccessSelection`]), driven by the operator/user policy
//! ([`VoicePathPolicy`]) rather than by whichever leg happened to connect last.
//!
//! # Invariants this encodes
//!
//!   * A path may be `Registered` while the other path is also `Registered`.
//!     Both legs coexisting is the normal case, not a conflict.
//!   * Changing the voice access does **not** change [`ImsRegistrationState`].
//!     A VoLTE -> VoWiFi switch in idle is a re-selection, not a re-registration.
//!   * A path going down only clears *its own* entry in `registered_over`.
//!
//! # Scope
//!
//! Everything here is pure decision logic over plain data, so it runs under
//! `cargo test` on any host. Mapping live VoLTE/VoWiFi runtimes onto the
//! [`ThreeGppObservation`] / [`NonThreeGppObservation`] inputs is the caller's
//! job.
//!
//! In-call media continuity (IMS service continuity / SRVCC) is deliberately
//! **not** modeled here. Moving an established call between accesses is a
//! different state machine from idle access selection, and conflating the two is
//! what this module exists to prevent. The voice router already pins each call to
//! the access that answered it; idle re-selection must never disturb that.

use serde::{Deserialize, Serialize};

use crate::hardware::devices::transport::{BearerDomain, PduSessionInfo, QosFlowInfo, ThreeGppRat};
use crate::platform::config::{AccessPathKind, VoicePathPolicy};

use super::voice_router::{plan_voice_route, RejectedVoiceLeg, VoiceLegReadiness};

/// Access family as defined by TS 23.402: 3GPP (LTE/5G) or non-3GPP (Wi-Fi via
/// ePDG). The two families reach the same IMS core over different paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessFamily {
    ThreeGpp,
    NonThreeGpp,
}

/// How far one access path has progressed toward carrying IMS traffic.
///
/// The ladder is shared by both families so the UI and the router can compare
/// them, while the family-specific detail (bearer vs ePDG/IKE) stays on
/// [`ThreeGppAccess`] / [`NonThreeGppAccess`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessPathStage {
    /// The user has not enabled this access path.
    Disabled,
    /// Enabled, but no usable IP transport toward the IMS core.
    Down,
    /// IP transport exists (LTE bearer up, or IPsec tunnel to the ePDG up).
    TransportUp,
    /// Transport is up and a P-CSCF is known, so REGISTER can be attempted.
    SignalingReady,
    /// REGISTER succeeded over this access.
    Registered,
    /// Impaired. `degraded_reason` on the path carries the detail.
    Degraded,
}

impl AccessPathStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Down => "down",
            Self::TransportUp => "transport_up",
            Self::SignalingReady => "signaling_ready",
            Self::Registered => "registered",
            Self::Degraded => "degraded",
        }
    }

    /// Whether this stage can carry SIP signaling for a new request.
    pub const fn can_signal(self) -> bool {
        matches!(self, Self::Registered)
    }
}

/// State common to every IMS access path, independent of family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImsAccessPath {
    pub kind: AccessPathKind,
    pub family: AccessFamily,
    /// Whether the user enabled this access. Kept separate from `stage` so the
    /// UI can distinguish "off" from "on but failing".
    pub configured: bool,
    pub stage: AccessPathStage,
    /// Whether IMS registration is live over *this* access specifically.
    pub registered: bool,
    /// P-CSCF bound to this access. Each access discovers its own; they are
    /// routinely different addresses and must not be shared.
    pub pcscf: Option<String>,
    pub degraded_reason: Option<String>,
}

/// Observed facts about the 3GPP access path: LTE/5G -> EPC/5GC -> P-CSCF -> IMS.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThreeGppObservation {
    /// `volte_connection_enabled` for the line.
    pub configured: bool,
    /// Radio usable: modem present and not in airplane mode.
    pub radio_available: bool,
    /// IMS APN bearer established.
    pub bearer_up: bool,
    /// A SIP transport toward the P-CSCF is established (IPsec SA or, where the
    /// carrier permits it, plain UDP).
    pub signaling_ready: bool,
    pub pcscf: Option<String>,
    pub registered: bool,
    /// `ipsec` / `udp` / `none`, purely informational.
    pub registration_mode: Option<String>,
    pub degraded_reason: Option<String>,
    /// Whether the per-line media gateway (Asterisk trunk backend) can carry a
    /// call over this access right now.
    pub media_gateway_ready: bool,
    /// RAT and packet-core metadata reported by the bearer provider. Unknown
    /// values are intentionally retained instead of inferring VoNR from a 5G
    /// cell or from the modem's advertised radio modes.
    pub rat: ThreeGppRat,
    pub bearer_domain: BearerDomain,
    pub pdu_session: Option<PduSessionInfo>,
    pub qos_flows: Vec<QosFlowInfo>,
    /// `None` means the provider cannot determine VoNR capability. `Some(true)`
    /// is only a capability signal; registration and media readiness are still
    /// evaluated independently.
    pub vonr_capable: Option<bool>,
}

/// Observed facts about the non-3GPP access path:
/// Wi-Fi -> IKEv2/IPsec -> ePDG -> EPC/5GC -> P-CSCF -> IMS.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NonThreeGppObservation {
    /// `vowifi.enabled` for the line.
    pub configured: bool,
    /// ePDG selected for this line, when one has been resolved.
    pub epdg_host: Option<String>,
    pub epdg_ready: bool,
    pub ike_ready: bool,
    pub child_sa_ready: bool,
    pub esp_ready: bool,
    pub pcscf: Option<String>,
    pub registered: bool,
    pub degraded_reason: Option<String>,
    pub media_gateway_ready: bool,
}

impl ThreeGppObservation {
    fn stage(&self) -> AccessPathStage {
        if !self.configured {
            return AccessPathStage::Disabled;
        }
        if !self.radio_available {
            return AccessPathStage::Down;
        }
        if self.degraded_reason.is_some() {
            return AccessPathStage::Degraded;
        }
        if self.registered && self.bearer_up && self.signaling_ready {
            return AccessPathStage::Registered;
        }
        if self.bearer_up && self.signaling_ready && self.pcscf.is_some() {
            return AccessPathStage::SignalingReady;
        }
        if self.bearer_up && self.radio_available {
            return AccessPathStage::TransportUp;
        }
        AccessPathStage::Down
    }
}

impl NonThreeGppObservation {
    /// The IPsec tunnel to the ePDG is the non-3GPP transport. All four legs of
    /// it must be up before the path can carry SIP.
    pub fn tunnel_up(&self) -> bool {
        self.epdg_ready && self.ike_ready && self.child_sa_ready && self.esp_ready
    }

    fn stage(&self) -> AccessPathStage {
        if !self.configured {
            return AccessPathStage::Disabled;
        }
        let tunnel_up = self.tunnel_up();
        if !tunnel_up {
            // A teardown can leave the previous error in the persisted
            // snapshot. Without a complete ePDG/IKE/IPsec transport the path
            // is down, not degraded and never registered.
            return AccessPathStage::Down;
        }
        if self.degraded_reason.is_some() {
            return AccessPathStage::Degraded;
        }
        if self.registered {
            return AccessPathStage::Registered;
        }
        if self.pcscf.is_some() {
            return AccessPathStage::SignalingReady;
        }
        AccessPathStage::TransportUp
    }
}

/// 3GPP access path plus its family-specific detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThreeGppAccess {
    #[serde(flatten)]
    pub path: ImsAccessPath,
    pub radio_available: bool,
    pub bearer_up: bool,
    pub registration_mode: Option<String>,
    pub rat: ThreeGppRat,
    pub bearer_domain: BearerDomain,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pdu_session: Option<PduSessionInfo>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub qos_flows: Vec<QosFlowInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vonr_capable: Option<bool>,
}

/// Non-3GPP access path plus its ePDG/IKE/IPsec detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NonThreeGppAccess {
    #[serde(flatten)]
    pub path: ImsAccessPath,
    pub epdg_host: Option<String>,
    pub epdg_ready: bool,
    pub ike_ready: bool,
    pub child_sa_ready: bool,
    pub esp_ready: bool,
    /// Convenience: all four IPsec/ePDG legs are up.
    pub tunnel_up: bool,
}

/// IMS registration state, expressed as the set of accesses currently holding a
/// live registration.
///
/// This is deliberately a set and not a boolean. A UE registered over both the
/// 3GPP and the non-3GPP access is a normal, supported state; a single flag
/// cannot represent it, and code that uses one inevitably starts tearing one
/// registration down to keep the flag meaningful.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ImsRegistrationState {
    pub registered_over: Vec<AccessPathKind>,
}

impl ImsRegistrationState {
    /// Whether the line has an IMS registration over *any* access.
    pub fn is_registered(&self) -> bool {
        !self.registered_over.is_empty()
    }

    pub fn is_registered_over(&self, kind: AccessPathKind) -> bool {
        self.registered_over.contains(&kind)
    }
}

/// Which registered access voice would use right now, and why the others were
/// not chosen.
///
/// `active` changing does **not** imply any registration changed — that is the
/// whole point of separating this from [`ImsRegistrationState`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VoiceAccessSelection {
    /// Highest-priority access that can carry voice now, if any.
    pub active: Option<AccessPathKind>,
    /// All eligible accesses in policy order. Anything past the first is a
    /// live fallback target, which only exists because both legs stay registered.
    pub candidates: Vec<AccessPathKind>,
    pub rejected: Vec<RejectedVoiceLeg>,
    pub gateway_mode: bool,
}

/// Complete IMS view for one line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImsSubsystemState {
    pub line_id: String,
    pub registration: ImsRegistrationState,
    pub three_gpp: ThreeGppAccess,
    pub non_three_gpp: NonThreeGppAccess,
    pub voice: VoiceAccessSelection,
}

impl ImsSubsystemState {
    pub fn build(
        line_id: impl Into<String>,
        policy: &VoicePathPolicy,
        three_gpp: &ThreeGppObservation,
        non_three_gpp: &NonThreeGppObservation,
    ) -> Self {
        let three_gpp_stage = three_gpp.stage();
        let three_gpp_path = ImsAccessPath {
            kind: AccessPathKind::Volte,
            family: AccessFamily::ThreeGpp,
            configured: three_gpp.configured,
            stage: three_gpp_stage,
            registered: matches!(three_gpp_stage, AccessPathStage::Registered),
            pcscf: three_gpp.pcscf.clone(),
            degraded_reason: three_gpp.degraded_reason.clone(),
        };
        let non_three_gpp_stage = non_three_gpp.stage();
        let non_three_gpp_path = ImsAccessPath {
            kind: AccessPathKind::Vowifi,
            family: AccessFamily::NonThreeGpp,
            configured: non_three_gpp.configured,
            stage: non_three_gpp_stage,
            registered: matches!(non_three_gpp_stage, AccessPathStage::Registered),
            pcscf: non_three_gpp.pcscf.clone(),
            degraded_reason: non_three_gpp.degraded_reason.clone(),
        };

        // Registration order follows the enum's canonical order rather than the
        // voice policy: this is a description of IMS state, not a preference.
        let mut registered_over = Vec::new();
        if non_three_gpp_path.registered {
            registered_over.push(AccessPathKind::Vowifi);
        }
        if three_gpp_path.registered {
            registered_over.push(AccessPathKind::Volte);
        }

        let plan = plan_voice_route(
            policy,
            &[
                VoiceLegReadiness {
                    kind: AccessPathKind::Vowifi,
                    feature_enabled: non_three_gpp_path.configured,
                    registered: non_three_gpp_path.registered,
                    media_gateway_ready: non_three_gpp.media_gateway_ready,
                },
                VoiceLegReadiness {
                    kind: AccessPathKind::Volte,
                    feature_enabled: three_gpp_path.configured,
                    registered: three_gpp_path.registered,
                    media_gateway_ready: three_gpp.media_gateway_ready,
                },
            ],
        );

        Self {
            line_id: line_id.into(),
            registration: ImsRegistrationState { registered_over },
            three_gpp: ThreeGppAccess {
                path: three_gpp_path,
                radio_available: three_gpp.radio_available,
                bearer_up: three_gpp.bearer_up,
                registration_mode: three_gpp.registration_mode.clone(),
                rat: three_gpp.rat,
                bearer_domain: three_gpp.bearer_domain,
                pdu_session: three_gpp.pdu_session.clone(),
                qos_flows: three_gpp.qos_flows.clone(),
                vonr_capable: three_gpp.vonr_capable,
            },
            non_three_gpp: NonThreeGppAccess {
                path: non_three_gpp_path,
                epdg_host: non_three_gpp.epdg_host.clone(),
                epdg_ready: non_three_gpp.epdg_ready,
                ike_ready: non_three_gpp.ike_ready,
                child_sa_ready: non_three_gpp.child_sa_ready,
                esp_ready: non_three_gpp.esp_ready,
                tunnel_up: non_three_gpp.tunnel_up(),
            },
            voice: VoiceAccessSelection {
                active: plan.candidates.first().copied(),
                candidates: plan.candidates,
                rejected: plan.rejected,
                gateway_mode: plan.gateway_mode,
            },
        }
    }
}

/// An "everything off" state, used only to satisfy the `T: Default` bound on the
/// API error constructor. Built through [`ImsSubsystemState::build`] so it can
/// never drift out of sync with a real snapshot.
impl Default for ImsSubsystemState {
    fn default() -> Self {
        Self::build(
            String::new(),
            &VoicePathPolicy::default(),
            &ThreeGppObservation::default(),
            &NonThreeGppObservation::default(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fully-up 3GPP access.
    fn lte_up() -> ThreeGppObservation {
        ThreeGppObservation {
            configured: true,
            radio_available: true,
            bearer_up: true,
            signaling_ready: true,
            pcscf: Some("10.1.1.1".into()),
            registered: true,
            registration_mode: Some("ipsec".into()),
            degraded_reason: None,
            media_gateway_ready: true,
            rat: ThreeGppRat::Lte,
            bearer_domain: BearerDomain::Eps,
            pdu_session: None,
            qos_flows: Vec::new(),
            vonr_capable: Some(false),
        }
    }

    /// A fully-up non-3GPP access.
    fn wifi_up() -> NonThreeGppObservation {
        NonThreeGppObservation {
            configured: true,
            epdg_host: Some("epdg.example".into()),
            epdg_ready: true,
            ike_ready: true,
            child_sa_ready: true,
            esp_ready: true,
            pcscf: Some("172.20.1.1".into()),
            registered: true,
            degraded_reason: None,
            media_gateway_ready: true,
        }
    }

    /// Configured but nothing established.
    fn wifi_down() -> NonThreeGppObservation {
        NonThreeGppObservation {
            configured: true,
            ..Default::default()
        }
    }

    fn lte_down() -> ThreeGppObservation {
        ThreeGppObservation {
            configured: true,
            ..Default::default()
        }
    }

    fn build_state(
        three_gpp: &ThreeGppObservation,
        non_three_gpp: &NonThreeGppObservation,
    ) -> ImsSubsystemState {
        ImsSubsystemState::build(
            "line-1",
            &VoicePathPolicy::default(),
            three_gpp,
            non_three_gpp,
        )
    }

    // --- the four operator scenarios ------------------------------------

    #[test]
    fn lte_only_carries_voice_on_volte() {
        let state = build_state(&lte_up(), &wifi_down());

        assert_eq!(
            state.registration.registered_over,
            vec![AccessPathKind::Volte]
        );
        assert_eq!(state.voice.active, Some(AccessPathKind::Volte));
        assert_eq!(state.three_gpp.path.stage, AccessPathStage::Registered);
        assert_eq!(state.non_three_gpp.path.stage, AccessPathStage::Down);
    }

    #[test]
    fn both_available_keeps_both_registered_and_policy_picks_voice() {
        let state = build_state(&lte_up(), &wifi_up());

        // Both legs stay registered. This is the invariant the old
        // "tear down VoLTE when VoWiFi connects" logic violated.
        assert_eq!(
            state.registration.registered_over,
            vec![AccessPathKind::Vowifi, AccessPathKind::Volte]
        );
        assert!(state.registration.is_registered_over(AccessPathKind::Volte));
        assert!(state
            .registration
            .is_registered_over(AccessPathKind::Vowifi));

        // Default policy prefers VoWiFi, but VoLTE remains a live fallback.
        assert_eq!(state.voice.active, Some(AccessPathKind::Vowifi));
        assert_eq!(
            state.voice.candidates,
            vec![AccessPathKind::Vowifi, AccessPathKind::Volte]
        );
    }

    #[test]
    fn wifi_only_carries_voice_on_vowifi() {
        let state = build_state(&lte_down(), &wifi_up());

        assert_eq!(
            state.registration.registered_over,
            vec![AccessPathKind::Vowifi]
        );
        assert_eq!(state.voice.active, Some(AccessPathKind::Vowifi));
        assert_eq!(state.non_three_gpp.path.stage, AccessPathStage::Registered);
    }

    #[test]
    fn losing_wifi_falls_back_to_volte_without_touching_its_registration() {
        let both = build_state(&lte_up(), &wifi_up());
        assert_eq!(both.voice.active, Some(AccessPathKind::Vowifi));

        // Wi-Fi drops. Only the non-3GPP entry disappears; the 3GPP
        // registration is untouched and immediately carries voice.
        let after = build_state(&lte_up(), &wifi_down());

        assert_eq!(
            after.registration.registered_over,
            vec![AccessPathKind::Volte]
        );
        assert_eq!(after.voice.active, Some(AccessPathKind::Volte));
        assert_eq!(after.three_gpp.path.stage, AccessPathStage::Registered);
        assert_eq!(
            both.three_gpp.path, after.three_gpp.path,
            "losing the non-3GPP access must not change the 3GPP path state"
        );
    }

    // --- separation invariants ------------------------------------------

    #[test]
    fn voice_selection_change_does_not_change_registration() {
        let volte_first = VoicePathPolicy {
            priority: vec![
                crate::platform::config::PathLayerConfig {
                    kind: AccessPathKind::Volte,
                    enabled: true,
                },
                crate::platform::config::PathLayerConfig {
                    kind: AccessPathKind::Vowifi,
                    enabled: true,
                },
            ],
            gateway_mode: true,
        };

        let default_policy = build_state(&lte_up(), &wifi_up());
        let swapped = ImsSubsystemState::build("line-1", &volte_first, &lte_up(), &wifi_up());

        assert_eq!(default_policy.voice.active, Some(AccessPathKind::Vowifi));
        assert_eq!(swapped.voice.active, Some(AccessPathKind::Volte));
        assert_eq!(
            default_policy.registration, swapped.registration,
            "changing which access carries voice is not a re-registration"
        );
    }

    #[test]
    fn disabled_access_is_distinguishable_from_a_failing_one() {
        let disabled = ThreeGppObservation {
            configured: false,
            ..lte_up()
        };
        let state = build_state(&disabled, &wifi_up());

        assert_eq!(state.three_gpp.path.stage, AccessPathStage::Disabled);
        assert!(!state.three_gpp.path.registered);
        assert!(!state.registration.is_registered_over(AccessPathKind::Volte));

        let failing = ThreeGppObservation {
            registered: false,
            degraded_reason: Some("bearer_lost".into()),
            ..lte_up()
        };
        let state = build_state(&failing, &wifi_up());
        assert_eq!(state.three_gpp.path.stage, AccessPathStage::Degraded);
    }

    #[test]
    fn tunnel_must_be_complete_before_the_path_can_signal() {
        let half_up = NonThreeGppObservation {
            configured: true,
            epdg_ready: true,
            ike_ready: true,
            child_sa_ready: true,
            esp_ready: false,
            pcscf: Some("172.20.1.1".into()),
            ..Default::default()
        };
        assert!(!half_up.tunnel_up());

        let state = build_state(&lte_up(), &half_up);
        assert_eq!(state.non_three_gpp.path.stage, AccessPathStage::Down);
        assert_eq!(state.voice.active, Some(AccessPathKind::Volte));
    }

    #[test]
    fn stale_three_gpp_registration_is_cleared_when_radio_is_unavailable() {
        let stale = ThreeGppObservation {
            radio_available: false,
            ..lte_up()
        };

        let state = build_state(&stale, &wifi_up());

        assert_eq!(state.three_gpp.path.stage, AccessPathStage::Down);
        assert!(!state.three_gpp.path.registered);
        assert!(!state.registration.is_registered_over(AccessPathKind::Volte));
        assert_eq!(state.voice.active, Some(AccessPathKind::Vowifi));
    }

    #[test]
    fn stale_non_three_gpp_registration_is_cleared_when_tunnel_is_incomplete() {
        let stale = NonThreeGppObservation {
            esp_ready: false,
            ..wifi_up()
        };

        let state = build_state(&lte_up(), &stale);

        assert_eq!(state.non_three_gpp.path.stage, AccessPathStage::Down);
        assert!(!state.non_three_gpp.path.registered);
        assert!(!state
            .registration
            .is_registered_over(AccessPathKind::Vowifi));
        assert_eq!(state.voice.active, Some(AccessPathKind::Volte));
    }

    #[test]
    fn stale_non_three_gpp_error_is_not_a_degraded_path_after_teardown() {
        let stale = NonThreeGppObservation {
            configured: true,
            degraded_reason: Some("old_ipsec_error".into()),
            ..Default::default()
        };

        let state = build_state(&lte_down(), &stale);

        assert_eq!(state.non_three_gpp.path.stage, AccessPathStage::Down);
        assert_eq!(
            state.non_three_gpp.path.degraded_reason.as_deref(),
            Some("old_ipsec_error")
        );
    }

    #[test]
    fn no_registration_anywhere_leaves_voice_unrouted() {
        let state = build_state(&lte_down(), &wifi_down());

        assert!(!state.registration.is_registered());
        assert_eq!(state.voice.active, None);
        assert!(state.voice.candidates.is_empty());
        assert_eq!(state.voice.rejected.len(), 2);
    }

    #[test]
    fn a_registered_access_without_media_gateway_is_not_a_voice_candidate() {
        let no_media = ThreeGppObservation {
            media_gateway_ready: false,
            ..lte_up()
        };
        let state = build_state(&no_media, &wifi_down());

        // Still registered for SMS/signaling...
        assert!(state.registration.is_registered_over(AccessPathKind::Volte));
        // ...but cannot carry voice in gateway mode.
        assert_eq!(state.voice.active, None);
    }

    #[test]
    fn five_gs_metadata_survives_projection_without_changing_access_identity() {
        let observation = ThreeGppObservation {
            rat: ThreeGppRat::NrSa,
            bearer_domain: BearerDomain::FiveGs,
            pdu_session: Some(PduSessionInfo {
                session_id: Some(7),
                dnn: Some("ims".into()),
                s_nssai: Some("1-010203".into()),
                ssc_mode: Some(1),
            }),
            qos_flows: vec![QosFlowInfo {
                qfi: Some(5),
                five_qi: Some(1),
                ..Default::default()
            }],
            vonr_capable: Some(true),
            ..lte_up()
        };

        let state = build_state(&observation, &wifi_down());

        // NR reaches the same 3GPP IMS access. It is metadata on that access,
        // not a second IMS account/path and not an automatic readiness claim.
        assert_eq!(state.three_gpp.path.family, AccessFamily::ThreeGpp);
        assert_eq!(state.three_gpp.rat, ThreeGppRat::NrSa);
        assert_eq!(state.three_gpp.bearer_domain, BearerDomain::FiveGs);
        assert_eq!(state.three_gpp.pdu_session, observation.pdu_session);
        assert_eq!(state.three_gpp.qos_flows, observation.qos_flows);
        assert_eq!(state.three_gpp.vonr_capable, Some(true));
    }

    #[test]
    fn absent_provider_metadata_remains_unknown_instead_of_claiming_vonr() {
        let observation = ThreeGppObservation {
            configured: true,
            ..Default::default()
        };

        let state = build_state(&observation, &wifi_down());

        assert_eq!(state.three_gpp.rat, ThreeGppRat::Unknown);
        assert_eq!(state.three_gpp.bearer_domain, BearerDomain::Unknown);
        assert!(state.three_gpp.pdu_session.is_none());
        assert!(state.three_gpp.qos_flows.is_empty());
        assert_eq!(state.three_gpp.vonr_capable, None);
    }
}
