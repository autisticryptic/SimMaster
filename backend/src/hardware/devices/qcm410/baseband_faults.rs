//! Qualcomm 410 (MSM8916-class) baseband fault observations.
//!
//! Implements [`BasebandFaultPolicy`] for this SoC. Everything here is knowledge
//! about *this* hardware and its 2022-vintage modem firmware; nothing in the
//! generic IMS path should need to know any of it.
//!
//! The one fault this platform can observe from outside the firmware is the
//! bam-dmux runtime-PM latch. Full diagnosis, including the experiments that
//! ruled out EFS corruption, image damage and reflashing, is in
//! `docs/QCM410_BAM_DMUX_MODEM_CRASH.md`; the short version:
//!
//! A cold-boot race between the mainline `qcom_bam_dmux` probe and firmware
//! still initialising its Data Services Memory pool crashes the modem DSP with
//! `smd_dsm_memcpy.c:297`. remoteproc recovers the DSP, but the Linux-side
//! driver latches its runtime-PM state at `error` and a firmware restart does
//! not clear it. Every subsequent netdev OPEN is answered `EINVAL`.
//!
//! Consequences that shape this file:
//!
//! * **Observe, never act.** `power/control=on` only helps *before* the latch,
//!   and `remoteproc stop` resets the entire SoC — so there is no in-process
//!   recovery to offer, and pretending otherwise would make things worse.
//! * **Refusing is correct, not over-strict.** A manual `ip link set up` fails
//!   with `EINVAL` too. Retrying only hammers firmware that cannot answer.
//! * **Read through the interface's own device link.** The owning platform
//!   device is `4080000.remoteproc:bam-dmux` on this board, but reading
//!   `/sys/class/net/<if>/device/power/runtime_status` follows whatever device
//!   actually owns the netdev, so it stays correct if the address changes and
//!   never hard-codes a board path.

use super::super::baseband_faults::{BasebandFault, BasebandFaultPolicy};

/// Sysfs path exposing the runtime-PM state of the device owning a netdev.
fn runtime_status_path(interface: &str) -> String {
    format!("/sys/class/net/{interface}/device/power/runtime_status")
}

/// Whether a runtime-PM status string is the latched error state.
///
/// Trimmed and case-insensitive: sysfs yields a trailing newline, and matching
/// it exactly is the kind of punctuation detail that has already cost this
/// project a misclassified baseband wedge.
fn runtime_status_is_error(status: &str) -> bool {
    status.trim().eq_ignore_ascii_case("error")
}

pub struct Qcm410BasebandFaults;

impl BasebandFaultPolicy for Qcm410BasebandFaults {
    fn platform(&self) -> &'static str {
        "qcm410"
    }

    fn inspect_data_interface(&self, interface: &str) -> BasebandFault {
        match std::fs::read_to_string(runtime_status_path(interface)) {
            Ok(status) if runtime_status_is_error(&status) => BasebandFault::DataPathLatched,
            // An unreadable path is not a fault: the netdev may not exist yet,
            // or may not expose runtime PM at all. Reporting a fault here would
            // block bring-up on a healthy interface.
            _ => BasebandFault::None,
        }
    }

    fn fault_note(&self, fault: BasebandFault) -> Option<&'static str> {
        match fault {
            BasebandFault::DataPathLatched => Some(
                "bam-dmux runtime_status=error is latched on the Linux driver; \
                 a firmware restart does not clear it and every OPEN returns EINVAL",
            ),
            BasebandFault::None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latched_status_is_recognised_with_trailing_whitespace_and_any_case() {
        // sysfs returns "error\n"; a previous bug in this project came down to
        // exactly this kind of punctuation mismatch.
        assert!(runtime_status_is_error("error\n"));
        assert!(runtime_status_is_error(" ERROR "));
        assert!(runtime_status_is_error("Error"));
    }

    #[test]
    fn healthy_runtime_states_are_not_faults() {
        // "suspended" is the healthy idle state on this platform, per the crash
        // doc's own verification commands.
        assert!(!runtime_status_is_error("suspended\n"));
        assert!(!runtime_status_is_error("active\n"));
        assert!(!runtime_status_is_error(""));
    }

    #[test]
    fn a_missing_sysfs_path_is_not_reported_as_a_fault() {
        // Absence of evidence must not become evidence of a fault, or this
        // policy turns into a gate on interfaces that were merely not created
        // yet.
        let policy = Qcm410BasebandFaults;
        assert_eq!(
            policy.inspect_data_interface("simadmin-nonexistent-netdev"),
            BasebandFault::None
        );
    }

    #[test]
    fn the_status_path_follows_the_netdev_device_link_not_a_board_path() {
        // Reading through the interface's own device symlink is what keeps this
        // correct when the owning remoteproc address differs.
        assert_eq!(
            runtime_status_path("wwan0"),
            "/sys/class/net/wwan0/device/power/runtime_status"
        );
    }

    #[test]
    fn the_latch_carries_an_explanatory_note_for_error_details() {
        let policy = Qcm410BasebandFaults;
        let note = policy
            .fault_note(BasebandFault::DataPathLatched)
            .expect("the latch is this platform's documented fault");
        assert!(note.contains("EINVAL"));
        assert!(policy.fault_note(BasebandFault::None).is_none());
    }
}
