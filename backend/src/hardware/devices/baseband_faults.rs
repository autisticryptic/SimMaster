//! Per-hardware baseband fault mitigations.
//!
//! Some basebands have firmware defects that a portable IMS/data path must work
//! around. Those workarounds are *platform* knowledge, not IMS knowledge: a
//! Qualcomm 410 latches its bam-dmux runtime-PM state at `error` and refuses
//! every subsequent netdev OPEN with `EINVAL`, while another modem may fail in
//! an entirely different way or not at all.
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
//! return the new policy from [`fault_policy_for`]. Implement nothing else:
//! [`GenericBasebandFaults`] is the correct behaviour for a baseband with no
//! known firmware defect, so a platform that needs no mitigation should not have
//! such a file at all.
//!
//! # What belongs here, and what does not
//!
//! A mitigation belongs here when it is a workaround for *hardware or firmware*
//! behaviour. It does not belong here when it is a bug in SimAdmin. The 410's
//! two documented crashes divide exactly along that line
//! (`docs/QCM410_BAM_DMUX_MODEM_CRASH.md`):
//!
//! * `smd_dsm_memcpy.c:297` — a cold-boot race between the mainline
//!   `qcom_bam_dmux` probe and 2022-vintage firmware still initialising its Data
//!   Services Memory pool. It happens before SimAdmin does anything, it latches
//!   `bam-dmux` runtime-PM at `error`, and the only correct response from us is
//!   to *observe and refuse*: the kernel answers `EINVAL` to OPEN, so retrying
//!   just hammers the firmware. That observation is platform-specific, so it
//!   lives here.
//! * `dhcp_client_mgr.c:263` — SimAdmin's own unbounded retry loop exhausting a
//!   small WDS client pool. Nothing platform-specific mitigates that; it was
//!   fixed by classifying the failure as unsafe to retry. It does **not** belong
//!   here, and adding a "mitigation" for it would hide the real fix.

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
/// cycle a baseband: on the 410 those actions are documented as either
/// ineffective (`power/control=on` after the latch) or far too broad
/// (`remoteproc stop` resets the whole SoC). Recovery policy belongs to the
/// caller, which has the session context to decide.
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
    match kind {
        super::DeviceKind::Qcm410 => &super::qcm410::baseband_faults::Qcm410BasebandFaults,
        super::DeviceKind::Unknown => &GenericBasebandFaults,
    }
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
