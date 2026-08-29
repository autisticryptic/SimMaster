//! Canonical IMS IP-family connection plan.
//!
//! Historically the IPv4/IPv6 strategy was scattered across four decision
//! points that each spoke a different vocabulary and reached their own verdict:
//!   - the AT P-CSCF probe order (`pcscf::ordered_pdp_types`, `IPV4V6/IPV6/IP`),
//!   - the ModemManager bearer attempt/fallback (`bearer.rs`, `ipv4v6/ipv4/ipv6`,
//!     which ignored the configured preference and always fell back v4-first),
//!   - the IPv6 WDS preflight (hardcoded v6), and
//!   - the per-family SIP loop (`live.rs`, ordering local addresses).
//!
//! This module is the single source of truth. `ImsConnectionPlan` turns the
//! configured [`VolteIpFamilyPreference`] into one ordered plan; each consumer
//! projects it into its own vocabulary through the `IpFamily`/`IpType`
//! converters. Failure signals that used to be recognised by ad-hoc substring
//! matching are consolidated into [`FailureClass`].

use crate::platform::config::VolteIpFamilyPreference;

use super::errors::{code, VolteError};

/// A single IP address family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpFamily {
    Ipv4,
    Ipv6,
}

impl IpFamily {
    /// Family of an already-bound local address.
    pub fn of(addr: std::net::IpAddr) -> Self {
        if addr.is_ipv6() {
            IpFamily::Ipv6
        } else {
            IpFamily::Ipv4
        }
    }

    /// Runtime/status label (matches the `ip_family` strings already on the wire).
    pub fn as_str(self) -> &'static str {
        match self {
            IpFamily::Ipv4 => "ipv4",
            IpFamily::Ipv6 => "ipv6",
        }
    }
}

/// A bearer/PDP request type: either dual-stack or a single family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpType {
    Ipv4v6,
    Ipv4,
    Ipv6,
}

impl IpType {
    /// ModemManager `--create-bearer=ip-type=` / `--wds-start-network` vocabulary.
    pub fn as_mm_str(self) -> &'static str {
        match self {
            IpType::Ipv4v6 => "ipv4v6",
            IpType::Ipv4 => "ipv4",
            IpType::Ipv6 => "ipv6",
        }
    }

    /// 3GPP `+CGDCONT` / AT PDP-type vocabulary.
    pub fn as_pdp_str(self) -> &'static str {
        match self {
            IpType::Ipv4v6 => "IPV4V6",
            IpType::Ipv4 => "IP",
            IpType::Ipv6 => "IPV6",
        }
    }

    /// Parse the ModemManager vocabulary back into a type (for the bearer
    /// observe callback, which still carries strings).
    pub fn from_mm_str(value: &str) -> Option<Self> {
        match value {
            "ipv4v6" => Some(IpType::Ipv4v6),
            "ipv4" => Some(IpType::Ipv4),
            "ipv6" => Some(IpType::Ipv6),
            _ => None,
        }
    }
}

/// How a bearer/discovery failure should steer the plan. Replaces the scattered
/// `Ipv6OnlyAllowed`/`Ipv4OnlyAllowed`/`prefix-unavailable` substring checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// The network rejected dual-stack and requires IPv6 only.
    NetworkForcedIpv6,
    /// The network rejected dual-stack and requires IPv4 only.
    NetworkForcedIpv4,
    /// A family was requested but no prefix/address was delivered.
    PrefixUnavailable,
    /// The bearer came up but not in a family the plan admits.
    FamilyUnsupported,
    /// P-CSCF discovery/registration failed for the current family.
    PcscfFailed,
    /// The baseband refused the IMS session in a way that means it is wedged, or
    /// that the QMI control port is already owned by ModemManager.
    ///
    /// Retrying this is actively harmful: repeatedly re-issuing PDP activation
    /// against a wedged modem can escalate to a subsystem restart and take the
    /// whole device down. Callers must abandon the attempt batch, not back off
    /// and try again.
    BasebandWedged,
    /// Anything else — treated as fatal for family fallback.
    Other,
}

/// Signatures of a baseband that has stopped accepting IMS session setup.
///
/// `interface-in-use-config-match` is QMI telling us the data interface is
/// already claimed — on this platform that is ModemManager holding the primary
/// QMI port. The generic ModemManager "internal error" on an IMS bearer connect
/// is the same condition surfaced one layer up. `endpoint hangup` is the QMI
/// control channel itself going away.
///
/// Two phrasings of the same refusal reach this function, and only one of them
/// is ModemManager's. The native path shells out to `qmicli`, which renders the
/// identical failure as `QMI protocol error (14): 'CallFailed'` and
/// `verbose call end reason (2,201): [internal] error` — no space in
/// `CallFailed`, brackets around `internal`. Matching only ModemManager's
/// spelling classified the native failure as retryable, which is how the
/// firmware crash loop got started.
///
/// `client id not released` deliberately does **not** appear below, and must not
/// be added. It reads like a leak, but it is `qmicli` acknowledging the
/// `--client-no-release-cid` flag that every secondary-QMI invocation passes on
/// purpose (the CID has to outlive the process that started the call). Because
/// stderr is folded into the error text, the notice rides along on *every*
/// secondary-QMI failure whatever the cause — so matching it classified all of
/// them as wedged, and a transient refusal abandoned the batch instead of
/// retrying. On hardware that cost the data path ~70 s on every start: the
/// first attempt lost a PDN race against ModemManager's own `simple connect`
/// (which logs the very same `'CallFailed'` for itself), and recovery had to
/// wait for the next watchdog pass.
///
/// What separates the two is the call-end reason, not the CID notice. The crash
/// signature carries `[internal] error`; PDN contention carries
/// `generic-unspecified`. Only the former is a baseband that has stopped
/// accepting sessions.
fn is_baseband_wedge(lowercased: &str) -> bool {
    let call_failed = lowercased.contains("call failed") || lowercased.contains("callfailed");
    let internal_error =
        lowercased.contains("internal error") || lowercased.contains("[internal] error");

    lowercased.contains("interface-in-use-config-match")
        || lowercased.contains("endpoint hangup")
        || lowercased.contains("mobileequipment.unknown")
        || lowercased.contains(code::BEARER_NETDEV_RUNTIME_ERROR)
        || (call_failed && internal_error)
}

impl FailureClass {
    /// Classify a raw ModemManager error/detail string (bearer layer).
    pub fn from_details(details: &str) -> Self {
        let error = details.to_ascii_lowercase();
        if error.contains("ipv6onlyallowed")
            || error.contains("ipv6-only-allowed")
            || error.contains("only ipv6 allowed")
        {
            FailureClass::NetworkForcedIpv6
        } else if error.contains("ipv4onlyallowed")
            || error.contains("ipv4-only-allowed")
            || error.contains("only ipv4 allowed")
        {
            FailureClass::NetworkForcedIpv4
        } else if error.contains("prefix-unavailable") {
            FailureClass::PrefixUnavailable
        } else if error.contains(code::BEARER_NETDEV_RUNTIME_ERROR) {
            FailureClass::BasebandWedged
        } else if error.contains(code::BEARER_NETDEV_NOT_UP)
            || error.contains(code::BEARER_NETDEV_NOT_READY)
        {
            // Interface bring-up races are recoverable. Only the kernel's
            // latched runtime-PM error is a confirmed permanent bam-dmux wedge.
            FailureClass::Other
        } else if is_baseband_wedge(&error) {
            FailureClass::BasebandWedged
        } else {
            FailureClass::Other
        }
    }

    /// Classify a structured [`VolteError`] surfaced during the per-family SIP
    /// loop (was `live::should_try_next_family`).
    pub fn from_error(error: &VolteError) -> Self {
        match error.code() {
            code::BEARER_NETDEV_RUNTIME_ERROR => FailureClass::BasebandWedged,
            code::REGISTER_INITIAL_UNEXPECTED_STATUS
                if error
                    .detail()
                    .is_some_and(|detail| detail.contains("sip_status=")) =>
            {
                FailureClass::Other
            }
            code::RUNTIME_ALL_PCSCF_FAILED
            | code::PCSCF_FAMILY_MISMATCH
            | code::IPSEC_UDP_BIND_FAILED
            | code::REGISTER_INITIAL_UNEXPECTED_STATUS
            | code::COMMAND_FAILED => FailureClass::PcscfFailed,
            code::RUNTIME_IMS_FAMILY_UNSUPPORTED => FailureClass::FamilyUnsupported,
            _ => FailureClass::Other,
        }
    }

    /// Whether it is unsafe to retry this failure against the same baseband.
    pub fn is_unsafe_to_retry(self) -> bool {
        matches!(self, FailureClass::BasebandWedged)
    }

    /// If the failure forces a single family, which one.
    pub fn forced_family(self) -> Option<IpFamily> {
        match self {
            FailureClass::NetworkForcedIpv6 => Some(IpFamily::Ipv6),
            FailureClass::NetworkForcedIpv4 => Some(IpFamily::Ipv4),
            _ => None,
        }
    }

    /// Whether the per-family SIP loop should try the next family on this error.
    pub fn is_retryable_family(self) -> bool {
        matches!(self, FailureClass::PcscfFailed)
    }
}

/// Best-effort legacy label for a resolved single-family order.
///
/// `ImsConnectionPlan::preference` predates the per-line ordered list and is now
/// only a reporting/diagnostic value — the runtime drives off `bearer_attempts`
/// and `pcscf_order`. A custom list that has no exact legacy equivalent (e.g.
/// single families before dual-stack) maps to the closest `*First`/`*Only` label.
fn preference_for_pcscf_order(pcscf_order: &[IpFamily]) -> VolteIpFamilyPreference {
    match pcscf_order {
        [IpFamily::Ipv6, IpFamily::Ipv4, ..] => VolteIpFamilyPreference::Ipv6First,
        [IpFamily::Ipv4, IpFamily::Ipv6, ..] => VolteIpFamilyPreference::Ipv4First,
        [IpFamily::Ipv6] => VolteIpFamilyPreference::Ipv6Only,
        [IpFamily::Ipv4] => VolteIpFamilyPreference::Ipv4Only,
        _ => VolteIpFamilyPreference::default(),
    }
}

/// A resolved, ordered plan for one IMS connection attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImsConnectionPlan {
    preference: VolteIpFamilyPreference,
    /// Ordered bearer/PDP attempts. Dual-stack leads for the `*First` presets,
    /// but a per-line list may place it anywhere or omit it.
    bearer_attempts: Vec<IpType>,
    /// Ordered single families for P-CSCF probing and SIP local-address offer.
    pcscf_order: Vec<IpFamily>,
}

impl ImsConnectionPlan {
    /// Build the plan from the configured preference.
    pub fn from_preference(preference: VolteIpFamilyPreference) -> Self {
        let (bearer_attempts, pcscf_order) = match preference {
            VolteIpFamilyPreference::Ipv6First => (
                vec![IpType::Ipv4v6, IpType::Ipv6, IpType::Ipv4],
                vec![IpFamily::Ipv6, IpFamily::Ipv4],
            ),
            VolteIpFamilyPreference::Ipv4First => (
                vec![IpType::Ipv4v6, IpType::Ipv4, IpType::Ipv6],
                vec![IpFamily::Ipv4, IpFamily::Ipv6],
            ),
            VolteIpFamilyPreference::Ipv6Only => (vec![IpType::Ipv6], vec![IpFamily::Ipv6]),
            VolteIpFamilyPreference::Ipv4Only => (vec![IpType::Ipv4], vec![IpFamily::Ipv4]),
        };
        Self {
            preference,
            bearer_attempts,
            pcscf_order,
        }
    }

    /// Build the plan from an explicit, ordered attempt list (the per-line,
    /// web-editable form). The list order is honored literally: dual-stack
    /// (`Ipv4v6`) is just another entry, so a line can try single families before
    /// dual-stack, or omit dual-stack entirely. `pcscf_order` follows the same
    /// order with dual-stack projected onto its two single families. An empty
    /// list falls back to the default preference so a misconfigured line still
    /// connects.
    ///
    /// `[v4v6, v4, v6]` == `Ipv4First`, `[v4v6, v6, v4]` == `Ipv6First`,
    /// `[v4]` == `Ipv4Only`, `[v6]` == `Ipv6Only` — a strict superset of
    /// [`VolteIpFamilyPreference`].
    pub fn from_families(families: &[crate::platform::config::VolteIpFamily]) -> Self {
        use crate::platform::config::VolteIpFamily;
        if families.is_empty() {
            return Self::from_preference(VolteIpFamilyPreference::default());
        }
        // Bearer attempts follow the list literally — dual-stack sits wherever the
        // operator put it.
        let mut bearer_attempts: Vec<IpType> = Vec::with_capacity(families.len());
        for family in families {
            let ip_type = match family {
                VolteIpFamily::Ipv4v6 => IpType::Ipv4v6,
                VolteIpFamily::Ipv4 => IpType::Ipv4,
                VolteIpFamily::Ipv6 => IpType::Ipv6,
            };
            if !bearer_attempts.contains(&ip_type) {
                bearer_attempts.push(ip_type);
            }
        }

        // P-CSCF probing and the SIP local-address offer are per single family, and
        // their priority comes from the *explicitly listed* single families. Taking
        // it from dual-stack instead would fix the order at v4-then-v6 and silently
        // override an operator who asked for v6 first.
        let mut pcscf_order: Vec<IpFamily> = Vec::with_capacity(2);
        for family in families {
            let single = match family {
                VolteIpFamily::Ipv4 => IpFamily::Ipv4,
                VolteIpFamily::Ipv6 => IpFamily::Ipv6,
                VolteIpFamily::Ipv4v6 => continue,
            };
            if !pcscf_order.contains(&single) {
                pcscf_order.push(single);
            }
        }
        // Dual-stack can still deliver a family the list never named on its own
        // (including the dual-stack-only case, where it names none). Append those
        // after the explicit ones so stated priority always wins.
        if families.contains(&VolteIpFamily::Ipv4v6) {
            for single in [IpFamily::Ipv4, IpFamily::Ipv6] {
                if !pcscf_order.contains(&single) {
                    pcscf_order.push(single);
                }
            }
        }

        Self {
            preference: preference_for_pcscf_order(&pcscf_order),
            bearer_attempts,
            pcscf_order,
        }
    }

    /// Apply the LTE access row's `ip_family` as a hint only. A catalog row is
    /// not allowed to force a single-family connection: the standard fallback
    /// still keeps the other families available when the network rejects the
    /// hinted one. Callers should use this only when the line has not supplied
    /// an explicit family order.
    pub fn with_catalog_ip_stack_hint(self, ip_stack: &str) -> Self {
        match ip_stack.trim().to_ascii_lowercase().as_str() {
            "ipv4" => Self::from_preference(VolteIpFamilyPreference::Ipv4First),
            "ipv6" => Self::from_preference(VolteIpFamilyPreference::Ipv6First),
            _ => self,
        }
    }

    pub fn preference(&self) -> VolteIpFamilyPreference {
        self.preference
    }

    /// Every bearer/PDP attempt, in the configured order. Consumers that can act
    /// on dual-stack and single families uniformly should walk this rather than
    /// assuming dual-stack comes first — a per-line list may order it anywhere.
    pub fn bearer_attempts(&self) -> &[IpType] {
        &self.bearer_attempts
    }

    /// The first bearer attempt (dual-stack unless the preference is single-only).
    pub fn initial_bearer_attempt(&self) -> IpType {
        self.bearer_attempts[0]
    }

    /// Single-family bearer attempts to try after the initial one fails, honoring
    /// the configured preference order. Empty for `*Only` modes (no fallback).
    pub fn single_family_fallbacks(&self) -> Vec<IpType> {
        self.bearer_attempts.iter().copied().skip(1).collect()
    }

    /// Resolve the bearer fallbacks after a dual-stack failure: a network-forced
    /// family collapses to just that family; otherwise the preference-ordered
    /// single families are used.
    pub fn bearer_fallbacks_after(&self, class: FailureClass) -> Vec<IpType> {
        match class.forced_family() {
            Some(IpFamily::Ipv6) => vec![IpType::Ipv6],
            Some(IpFamily::Ipv4) => vec![IpType::Ipv4],
            None => self.single_family_fallbacks(),
        }
    }

    /// AT `+CGDCONT` PDP types to probe, in plan order.
    pub fn pdp_types(&self) -> Vec<&'static str> {
        self.bearer_attempts
            .iter()
            .map(|ip_type| ip_type.as_pdp_str())
            .collect()
    }

    /// Single families for P-CSCF/SIP, in plan order.
    pub fn pcscf_order(&self) -> &[IpFamily] {
        &self.pcscf_order
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    /// The exact detail string produced on the reference device. Retrying this
    /// is what escalates a wedged modem into a subsystem restart, so it must be
    /// classified as unsafe rather than as a generic failure.
    #[test]
    fn real_device_bearer_internal_error_is_classified_as_wedged() {
        let detail = "volte_command_failed:mmcli:1:-b /org/freedesktop/ModemManager1/Bearer/4 \
             --connect:error: couldn't connect the bearer: \
             'GDBus.Error:org.freedesktop.ModemManager1.Error.MobileEquipment.Unknown: \
             Unknown error: Call failed: internal error: error'";
        let class = FailureClass::from_details(detail);
        assert_eq!(class, FailureClass::BasebandWedged);
        assert!(class.is_unsafe_to_retry());
    }

    #[test]
    fn qmi_interface_contention_and_endpoint_hangup_are_wedge_signatures() {
        for detail in [
            "QMI protocol error (14): CallFailed - interface-in-use-config-match",
            "CID allocation failed in the CTL client: endpoint hangup",
        ] {
            assert_eq!(
                FailureClass::from_details(detail),
                FailureClass::BasebandWedged,
                "{detail}"
            );
        }
    }

    /// Family negotiation and prefix problems are normal and must stay
    /// retryable, otherwise a recoverable case would be turned into a hard stop.
    #[test]
    fn recoverable_failures_are_not_treated_as_wedged() {
        for (detail, expected) in [
            ("[3gpp] ipv4-only-allowed", FailureClass::NetworkForcedIpv4),
            ("ipv6-only-allowed", FailureClass::NetworkForcedIpv6),
            ("prefix-unavailable", FailureClass::PrefixUnavailable),
            ("operation-failed", FailureClass::Other),
        ] {
            let class = FailureClass::from_details(detail);
            assert_eq!(class, expected, "{detail}");
            assert!(!class.is_unsafe_to_retry(), "{detail}");
        }
    }

    #[test]
    fn dual_stack_default_leads_with_ipv4v6() {
        let plan = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv6First);
        assert_eq!(plan.initial_bearer_attempt(), IpType::Ipv4v6);
        // Fallback honors preference (ipv6 before ipv4), unlike the old always-v4-first.
        assert_eq!(
            plan.single_family_fallbacks(),
            vec![IpType::Ipv6, IpType::Ipv4]
        );
        let plan4 = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv4First);
        assert_eq!(
            plan4.single_family_fallbacks(),
            vec![IpType::Ipv4, IpType::Ipv6]
        );
    }

    #[test]
    fn network_forced_family_collapses_fallback() {
        let plan = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv6First);
        assert_eq!(
            plan.bearer_fallbacks_after(FailureClass::NetworkForcedIpv4),
            vec![IpType::Ipv4]
        );
        assert_eq!(
            plan.bearer_fallbacks_after(FailureClass::NetworkForcedIpv6),
            vec![IpType::Ipv6]
        );
    }

    #[test]
    fn unclear_failure_uses_preference_order() {
        let plan = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv4First);
        assert_eq!(
            plan.bearer_fallbacks_after(FailureClass::PrefixUnavailable),
            vec![IpType::Ipv4, IpType::Ipv6]
        );
    }

    #[test]
    fn catalog_ip_stack_hint_reorders_only_single_family_fallbacks() {
        let base = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv4First);
        assert_eq!(
            base.clone().with_catalog_ip_stack_hint("ipv6").pdp_types(),
            vec!["IPV4V6", "IPV6", "IP"]
        );
        assert_eq!(
            base.clone().with_catalog_ip_stack_hint("ipv4").pdp_types(),
            vec!["IPV4V6", "IP", "IPV6"]
        );
        assert_eq!(
            base.clone()
                .with_catalog_ip_stack_hint("ipv4v6")
                .pdp_types(),
            base.pdp_types()
        );
    }

    /// The per-line ordered list reproduces every legacy preset exactly, so
    /// switching a line to a custom list cannot silently change its behaviour.
    #[test]
    fn from_families_reproduces_the_legacy_presets() {
        use crate::platform::config::VolteIpFamily as F;
        for (families, preference) in [
            (
                vec![F::Ipv4v6, F::Ipv4, F::Ipv6],
                VolteIpFamilyPreference::Ipv4First,
            ),
            (
                vec![F::Ipv4v6, F::Ipv6, F::Ipv4],
                VolteIpFamilyPreference::Ipv6First,
            ),
            (vec![F::Ipv4], VolteIpFamilyPreference::Ipv4Only),
            (vec![F::Ipv6], VolteIpFamilyPreference::Ipv6Only),
        ] {
            let from_list = ImsConnectionPlan::from_families(&families);
            let from_preset = ImsConnectionPlan::from_preference(preference);
            assert_eq!(
                from_list.pdp_types(),
                from_preset.pdp_types(),
                "{families:?} should match {preference:?}"
            );
            assert_eq!(
                from_list.pcscf_order(),
                from_preset.pcscf_order(),
                "{families:?} should match {preference:?}"
            );
        }
    }

    /// Dual-stack is an ordinary orderable entry: it may come after a single
    /// family, or be omitted entirely. This is what the legacy presets could not
    /// express.
    #[test]
    fn from_families_honors_dual_stack_position_and_omission() {
        use crate::platform::config::VolteIpFamily as F;

        // Single family first, dual-stack as the fallback.
        let v4_then_dual = ImsConnectionPlan::from_families(&[F::Ipv4, F::Ipv4v6]);
        assert_eq!(v4_then_dual.initial_bearer_attempt(), IpType::Ipv4);
        assert_eq!(v4_then_dual.single_family_fallbacks(), vec![IpType::Ipv4v6]);

        // Dual-stack omitted: only single families are ever requested.
        let no_dual = ImsConnectionPlan::from_families(&[F::Ipv6, F::Ipv4]);
        assert_eq!(no_dual.pdp_types(), vec!["IPV6", "IP"]);
        assert_eq!(no_dual.pcscf_order(), &[IpFamily::Ipv6, IpFamily::Ipv4]);

        // Dual-stack only: no single-family bearer attempt, but P-CSCF still needs
        // a single-family probe order.
        let dual_only = ImsConnectionPlan::from_families(&[F::Ipv4v6]);
        assert_eq!(dual_only.pdp_types(), vec!["IPV4V6"]);
        assert_eq!(dual_only.pcscf_order(), &[IpFamily::Ipv4, IpFamily::Ipv6]);

        // An empty list is a misconfiguration and falls back to the default.
        let empty = ImsConnectionPlan::from_families(&[]);
        assert_eq!(
            empty.pdp_types(),
            ImsConnectionPlan::from_preference(VolteIpFamilyPreference::default()).pdp_types()
        );
    }

    #[test]
    fn single_only_modes_never_try_the_other_family() {
        let v6 = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv6Only);
        assert_eq!(v6.initial_bearer_attempt(), IpType::Ipv6);
        assert!(v6.single_family_fallbacks().is_empty());
        assert_eq!(v6.pcscf_order(), &[IpFamily::Ipv6]);
        let v4 = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv4Only);
        assert_eq!(v4.pdp_types(), vec!["IP"]);
        assert_eq!(v4.pcscf_order(), &[IpFamily::Ipv4]);
    }

    #[test]
    fn pdp_types_match_legacy_ordered_pdp_types() {
        assert_eq!(
            ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv6First).pdp_types(),
            vec!["IPV4V6", "IPV6", "IP"]
        );
        assert_eq!(
            ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv4First).pdp_types(),
            vec!["IPV4V6", "IP", "IPV6"]
        );
    }

    #[test]
    fn failure_class_from_details_recognizes_forced_families() {
        assert_eq!(
            FailureClass::from_details(
                "org.freedesktop.ModemManager1.Error.MobileEquipment.Ipv6OnlyAllowed"
            ),
            FailureClass::NetworkForcedIpv6
        );
        assert_eq!(
            FailureClass::from_details("only ipv4 allowed"),
            FailureClass::NetworkForcedIpv4
        );
        assert_eq!(
            FailureClass::from_details("ipv6 error: prefix-unavailable"),
            FailureClass::PrefixUnavailable
        );
        assert_eq!(
            FailureClass::from_details("some other failure"),
            FailureClass::Other
        );
    }

    #[test]
    fn family_of_address() {
        assert_eq!(
            IpFamily::of(IpAddr::V6(Ipv6Addr::LOCALHOST)),
            IpFamily::Ipv6
        );
        assert_eq!(
            IpFamily::of(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            IpFamily::Ipv4
        );
    }

    #[test]
    fn retryable_family_only_for_pcscf_class() {
        assert!(FailureClass::PcscfFailed.is_retryable_family());
        assert!(!FailureClass::NetworkForcedIpv6.is_retryable_family());
        assert!(!FailureClass::Other.is_retryable_family());
    }

    /// Verbatim detail from the QCM410, captured while the firmware was crash
    /// looping. Classifying this as retryable is what made the loop. Note the
    /// spellings `'CallFailed'` (no space) and `[internal] error` (bracketed) --
    /// `qmicli` renders the same refusal differently from ModemManager, and
    /// matching only ModemManager's wording let this through.
    ///
    /// The `Client ID not released` line is present here but carries no weight:
    /// what makes this detail unsafe is `[internal] error`. Compare with
    /// `pdn_contention_stays_retryable_despite_the_cid_notice`, which has the
    /// same CID line and must classify the other way.
    #[test]
    fn native_qmi_call_failure_is_unsafe_to_retry() {
        let detail = concat!(
            "secondary_qmi_action_failed:[/dev/wwan0at2] Client ID not released:\n",
            "        Service: 'wds'\n",
            "            CID: '4'\n",
            "[/dev/wwan0at2] couldn't detect transport type of port: unsupported wwan port\n",
            "[/dev/wwan0at2] requested QMI mode but unexpected transport type found\n",
            "error: couldn't start network: QMI protocol error (14): 'CallFailed'\n",
            "call end reason (12): (null)\n",
            "verbose call end reason (2,201): [internal] error",
        );

        let class = FailureClass::from_details(detail);
        assert_eq!(class, FailureClass::BasebandWedged, "detail: {detail}");
        assert!(
            class.is_unsafe_to_retry(),
            "an [internal] error refusal escalates to a subsystem restart if retried"
        );
    }

    /// Verbatim detail from the QCM410, captured on a healthy modem with fault
    /// count 0. The first DATA6 activation after a start loses a PDN race with
    /// ModemManager's own `simple connect` -- which logs the identical
    /// `'CallFailed'` against its own bearer seconds earlier -- and the next
    /// attempt succeeds.
    ///
    /// The `Client ID not released` notice is here too, because every
    /// secondary-QMI call passes `--client-no-release-cid` and stderr is folded
    /// into the error text. Treating that notice as a wedge signal made this
    /// transient failure abandon the batch, costing the data path ~70 s per
    /// start. What distinguishes it from the crash signature is
    /// `generic-unspecified` where that one has `[internal] error`.
    #[test]
    fn pdn_contention_stays_retryable_despite_the_cid_notice() {
        let detail = concat!(
            "secondary_qmi_data_action_failed:[/dev/wwan0at2] Client ID not released: ",
            "Service: 'wds' CID: '4' ",
            "-Warning ** [/dev/wwan0at2] couldn't detect transport type of port: ",
            "unsupported wwan port ",
            "-Warning ** [/dev/wwan0at2] requested QMI mode but unexpected transport type found ",
            "error: couldn't start network: QMI protocol error (14): 'CallFailed' ",
            "call end reason (1): generic-unspecified ",
            "verbose call end reason (49372,1380): [(null)] (null)",
        );

        let class = FailureClass::from_details(detail);
        assert_ne!(
            class,
            FailureClass::BasebandWedged,
            "PDN contention is transient; abandoning the batch strands the data path \
             until the next watchdog pass -- detail: {detail}"
        );
        assert!(!class.is_unsafe_to_retry());
    }

    /// The CID notice must never on its own decide the class, in either
    /// direction: it is present on every secondary-QMI failure whatever the
    /// cause, so it carries no diagnostic information at all.
    #[test]
    fn the_cid_notice_alone_decides_nothing() {
        assert_eq!(
            FailureClass::from_details(
                "secondary_qmi_data_action_failed:[/dev/wwan0at2] Client ID not released: \
                 Service: 'wds' CID: '4'"
            ),
            FailureClass::Other
        );
    }

    /// The ModemManager spelling must keep working -- this is the pairing that
    /// was already covered, and only the pair may trip it.
    #[test]
    fn modemmanager_call_failure_still_needs_both_halves() {
        assert_eq!(
            FailureClass::from_details("Call failed: internal error"),
            FailureClass::BasebandWedged
        );
        // Either half on its own is too generic to abandon a batch over.
        assert_eq!(
            FailureClass::from_details("call failed: no service"),
            FailureClass::Other
        );
        assert_eq!(
            FailureClass::from_details("internal error while reading state"),
            FailureClass::Other
        );
    }

    #[test]
    fn bearer_netdev_errors_have_explicit_retry_safety() {
        let runtime_error =
            VolteError::with_detail(code::BEARER_NETDEV_RUNTIME_ERROR, "interface=wwan0");
        let runtime_class = FailureClass::from_error(&runtime_error);
        assert_eq!(runtime_class, FailureClass::BasebandWedged);
        assert!(!runtime_class.is_retryable_family());
        assert!(runtime_class.is_unsafe_to_retry());

        for code in [code::BEARER_NETDEV_NOT_UP, code::BEARER_NETDEV_NOT_READY] {
            let error = VolteError::with_detail(code, "interface=wwan0");
            let class = FailureClass::from_error(&error);
            assert_eq!(class, FailureClass::Other);
            assert!(!class.is_retryable_family());
            assert!(!class.is_unsafe_to_retry());
        }
    }

    #[test]
    fn terminal_sip_status_is_not_retried_as_an_ip_family_failure() {
        let rejection = VolteError::with_detail(
            code::REGISTER_INITIAL_UNEXPECTED_STATUS,
            "ims_register_initial_unexpected_status:sip_status=400",
        );
        assert_eq!(FailureClass::from_error(&rejection), FailureClass::Other);

        let timeout = VolteError::with_detail(
            code::REGISTER_INITIAL_UNEXPECTED_STATUS,
            "ims_register_initial_receive_failed",
        );
        assert!(FailureClass::from_error(&timeout).is_retryable_family());
    }
}
