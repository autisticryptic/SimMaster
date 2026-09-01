//! Per-hardware baseband fault mitigations.
//!
//! Some basebands have firmware defects that a portable IMS/data path must work
//! around. Those workarounds are *platform* knowledge, not IMS knowledge. Each
//! concrete device driver owns its signatures and explanation; this module
//! contains only the shared contract and the no-op policy.
//!
//! Keeping that knowledge inline in the IMS registration path (as
//! `volte/bearer.rs` used to) has two costs. It makes the generic path assert
//! things that are only true for one SoC, and it gives a new platform nowhere to
//! put its own quirks except by adding another branch to shared code.
//!
//! So the shape here mirrors [`super::transport`]: upper layers ask a trait
//! object what the platform says, and each device directory implements it.
//!
//! # Adding a platform
//!
//! Create `hardware/devices/<platform>/baseband_faults.rs` — a sibling of the
//! 410's, inside that platform's own directory — implement
//! [`BasebandFaultPolicy`] there, add the platform to [`super::DeviceKind`], and
//! return the new policy from its [`super::DeviceDriver`] implementation. Implement nothing else:
//! [`GenericBasebandFaults`] is the correct behaviour for a baseband with no
//! known firmware defect, so a platform that needs no mitigation should not have
//! such a file at all.
//!
//! A mitigation belongs in a concrete driver only when it is a workaround for
//! hardware or firmware behaviour. Application retry bugs and protocol policy
//! remain in their own layers.

use std::fmt;

/// Why a baseband refused to bring an interface up, as far as the platform can
/// tell from outside the firmware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasebandFault {
    /// The platform knows of no fault: the interface simply is not up yet.
    None,
    /// The data-path driver has latched a permanent error state. Further OPEN
    /// attempts are rejected by the kernel, so they must not be retried.
    DataPathLatched,
}

impl BasebandFault {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::DataPathLatched => "data_path_latched",
        }
    }

    /// Whether an interface bring-up may be attempted or retried at all.
    ///
    /// A latched data path answers `EINVAL` to every OPEN, so retrying cannot
    /// succeed and does reach the firmware. `None` must permit the attempt:
    /// "no known fault" is not evidence of a fault.
    pub fn permits_bring_up(self) -> bool {
        matches!(self, Self::None)
    }
}

impl fmt::Display for BasebandFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What one hardware platform knows about its baseband's failure modes.
///
/// Implementations must only *observe*. Nothing here may reset, rebind or power
/// cycle a baseband. Recovery policy belongs to the caller, which has the
/// session context to decide.
pub trait BasebandFaultPolicy: Send + Sync {
    /// Stable identifier for logs and runtime snapshots.
    fn platform(&self) -> &'static str;

    /// Inspect a data-path interface before an administrative bring-up.
    fn inspect_data_interface(&self, interface: &str) -> BasebandFault;

    /// Human-readable note naming the documented fault, for error details.
    ///
    /// Returns `None` when the platform has nothing to add.
    fn fault_note(&self, fault: BasebandFault) -> Option<&'static str> {
        let _ = fault;
        None
    }
}

/// A baseband with no known firmware defect.
///
/// Deliberately reports [`BasebandFault::None`] rather than guessing: inventing
/// a fault would turn this into a gate that blocks a healthy platform.
pub struct GenericBasebandFaults;

impl BasebandFaultPolicy for GenericBasebandFaults {
    fn platform(&self) -> &'static str {
        "generic"
    }

    fn inspect_data_interface(&self, interface: &str) -> BasebandFault {
        let _ = interface;
        BasebandFault::None
    }
}

/// Resolve the fault policy for the running platform.
pub fn fault_policy_for(kind: super::DeviceKind) -> &'static dyn BasebandFaultPolicy {
    super::baseband_fault_policy(kind)
}

/// Resolve the fault policy by detecting the platform.
pub fn detected_fault_policy() -> &'static dyn BasebandFaultPolicy {
    fault_policy_for(super::detect_device_kind())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_platform_never_reports_a_fault_it_cannot_observe() {
        // A platform with no dedicated driver must not block bring-up: an
        // invented fault would be a gate, and this whole module exists to keep
        // platform quirks from gating generic paths.
        let policy = fault_policy_for(super::super::DeviceKind::Unknown);
        assert_eq!(policy.platform(), "generic");
        assert_eq!(policy.inspect_data_interface("wwan0"), BasebandFault::None);
        assert!(policy.inspect_data_interface("wwan0").permits_bring_up());
    }

    #[test]
    fn a_latched_data_path_forbids_bring_up_and_no_fault_permits_it() {
        assert!(!BasebandFault::DataPathLatched.permits_bring_up());
        assert!(BasebandFault::None.permits_bring_up());
    }

    #[test]
    fn qcm410_is_dispatched_to_its_own_policy() {
        let policy = fault_policy_for(super::super::DeviceKind::Qcm410);
        assert_eq!(policy.platform(), "qcm410");
        assert!(policy.fault_note(BasebandFault::DataPathLatched).is_some());
    }
}
