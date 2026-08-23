//! VoLTE data-slot allocation: which QMI endpoint carries IMS and which carries
//! normal mobile data.
//!
//! # The allocation is fixed, not derived
//!
//! **IMS always registers through the port ModemManager owns (qmi0), and DATA6
//! always carries user data.** Only one allocation is ever reported:
//! `IMS allocated to primary qmi0; DATA6 is reserved for data`, with the
//! data-path intent serialized as `secondary_qmi_data`, or `independent_wwan1`
//! for a VoLTE-only line that reserves no data slot.
//!
//! This deliberately drops the endpoint-swapping behaviour the module used to
//! copy from beta8, where an ordinary-data session already active on qmi0 moved
//! IMS onto DATA6 instead. That allocation cannot work in this project:
//! `secondary-qmi-init` binds DATA6 and then holds its character device open for
//! the whole boot, so a second WDS client on the same device fails with
//! `Client ID not released`, and each retry strands another client until the
//! modem's DHCP manager faults (`dhcp_client_mgr.c:263`). It was measured
//! crashing the baseband roughly every 50-120 s — see
//! `docs/QCM410_BAM_DMUX_MODEM_CRASH.md` §10.
//!
//! So a busy qmi0 is not an allocation input any more; it is a precondition to
//! fix. See [`DataSlotMode::requires_primary_data_release`].
//!
//! This is pure logic (no IO), so the selection and its conflict rules are fully
//! unit-tested without a modem. The IO that acts on the chosen mode lives in
//! `live.rs` / `native_bearer.rs` / `secondary_qmi.rs`.

use super::errors::{code, VolteError};

/// What the second endpoint is reserved for. IMS is on qmi0 in both variants --
/// that is the invariant this module enforces -- so the only distinction is
/// whether a data slot is reserved alongside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSlotMode {
    /// IMS uses primary qmi0 and ordinary data uses DATA6.
    PrimaryImsSecondaryData,
    /// VoLTE-only line: no separate data exit is reserved, reported as
    /// `independent_wwan1`.
    PrimaryImsOnly,
}

impl DataSlotMode {
    /// The human-readable allocation message logged for this mode.
    ///
    /// Both variants report the same allocation: IMS on qmi0 is an invariant
    /// here, and the variants differ only in whether a data slot is reserved.
    pub fn allocation_message(self) -> &'static str {
        "IMS allocated to primary qmi0; DATA6 is reserved for data"
    }

    /// The data-path token reported for this allocation.
    pub fn as_str(self) -> &'static str {
        match self {
            DataSlotMode::PrimaryImsOnly => "independent_wwan1",
            DataSlotMode::PrimaryImsSecondaryData => "secondary_qmi_data",
        }
    }

    /// Whether IMS must be established through ModemManager on primary qmi0.
    ///
    /// Always true: IMS on DATA6 is not a supported allocation. Kept as a
    /// predicate because the runtime branches on it to decide whether to drive
    /// a native QMI bearer, and that branch must stay readable.
    pub fn ims_on_primary(self) -> bool {
        true
    }

    /// True when a distinct data slot (DATA6) is reserved alongside IMS.
    pub fn reserves_data_slot(self) -> bool {
        matches!(self, DataSlotMode::PrimaryImsSecondaryData)
    }

    /// True when user data has to be released from qmi0 before this allocation
    /// actually holds.
    ///
    /// The allocation is valid either way -- IMS goes on qmi0 regardless. But an
    /// ordinary ModemManager data bearer sitting on qmi0 deactivates the IMS
    /// bearer on that same port on this firmware, so the caller must release it
    /// and let user data come back up on DATA6 where it belongs.
    ///
    /// This is what replaced moving IMS onto DATA6: same trigger, but it fixes
    /// the misplaced bearer instead of putting IMS on an endpoint that is held
    /// open elsewhere.
    pub fn requires_primary_data_release(self, primary_data_active: bool) -> bool {
        primary_data_active && self.reserves_data_slot()
    }
}

/// The runtime inputs beta8 feeds into the slot decision:
/// whether the line asked for a proxied data exit, and whether each endpoint
/// currently has an active data session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DataSlotInputs {
    /// The line wants a mobile-data exit in addition to VoLTE.
    pub data_requested: bool,
    /// A data session is already active on the primary port.
    pub primary_data_active: bool,
    /// A data session is already active on the secondary (DATA6) endpoint.
    pub secondary_data_active: bool,
    /// A usable secondary QMI endpoint (DATA6) exists for this baseband.
    pub secondary_endpoint_available: bool,
}

/// Select the data-slot allocation.
///
/// This project fixes the allocation rather than deriving it from which endpoint
/// happens to be busy: **IMS always registers through the port ModemManager owns
/// (qmi0), and DATA6 always carries user data.** There is therefore no endpoint
/// to choose and the only remaining question is whether the allocation can be
/// satisfied at all:
///
///   - No data requested → VoLTE-only line, no data slot to reserve
///     (`PrimaryImsOnly`).
///   - Data requested but no secondary endpoint exists →
///     `volte_data_slot_mode_missing`. Putting user data on qmi0 next to the IMS
///     bearer is not a fallback: on this firmware an ordinary ModemManager
///     bearer deactivates the IMS bearer on the same port.
///   - Otherwise → `PrimaryImsSecondaryData`.
///
/// An ordinary-data session already active on qmi0 does **not** move IMS to
/// DATA6. It means user data is on the wrong endpoint and has to be released so
/// it can be re-established on DATA6 — see
/// [`DataSlotMode::requires_primary_data_release`]. Flipping IMS onto DATA6
/// instead is what the allocator used to do, and it cannot work here: DATA6's
/// character device is held open for the whole boot by `secondary-qmi-init`, so
/// a second WDS client on it fails with `Client ID not released` and every retry
/// strands another client until the modem's DHCP manager faults. See
/// `docs/QCM410_BAM_DMUX_MODEM_CRASH.md` §10.
pub fn select_data_slot_mode(inputs: DataSlotInputs) -> Result<DataSlotMode, VolteError> {
    if !inputs.data_requested {
        // VoLTE-only line: IMS on the primary port, no data slot to juggle.
        return Ok(DataSlotMode::PrimaryImsOnly);
    }

    if !inputs.secondary_endpoint_available {
        return Err(VolteError::new(code::DATA_SLOT_MODE_MISSING));
    }

    Ok(DataSlotMode::PrimaryImsSecondaryData)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volte_only_line_keeps_ims_on_primary_with_no_data_slot() {
        let mode = select_data_slot_mode(DataSlotInputs {
            data_requested: false,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(mode, DataSlotMode::PrimaryImsOnly);
        assert!(mode.ims_on_primary());
        assert!(!mode.reserves_data_slot());
        assert_eq!(
            mode.allocation_message(),
            "IMS allocated to primary qmi0; DATA6 is reserved for data"
        );
        assert_eq!(mode.as_str(), "independent_wwan1");
    }

    /// The whole point of this module: a data session already on qmi0 used to
    /// move IMS onto DATA6, which crashed the baseband. It now keeps IMS on qmi0
    /// and flags the misplaced bearer for release instead.
    #[test]
    fn primary_data_never_moves_ims_to_data6() {
        let mode = select_data_slot_mode(DataSlotInputs {
            data_requested: true,
            secondary_endpoint_available: true,
            primary_data_active: true,
            secondary_data_active: false,
        })
        .unwrap();
        assert_eq!(mode, DataSlotMode::PrimaryImsSecondaryData);
        assert!(mode.ims_on_primary());
        assert!(mode.reserves_data_slot());
        assert_eq!(mode.as_str(), "secondary_qmi_data");
        assert!(
            mode.requires_primary_data_release(true),
            "user data on qmi0 has to be released so IMS can own that port"
        );
    }

    #[test]
    fn secondary_data_keeps_ims_on_primary() {
        let mode = select_data_slot_mode(DataSlotInputs {
            data_requested: true,
            secondary_endpoint_available: true,
            primary_data_active: false,
            secondary_data_active: true,
        })
        .unwrap();
        assert_eq!(mode, DataSlotMode::PrimaryImsSecondaryData);
        assert!(mode.ims_on_primary());
        assert_eq!(mode.as_str(), "secondary_qmi_data");
        assert!(!mode.requires_primary_data_release(false));
    }

    #[test]
    fn requested_data_without_an_active_bearer_reserves_data6() {
        let mode = select_data_slot_mode(DataSlotInputs {
            data_requested: true,
            secondary_endpoint_available: true,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(mode, DataSlotMode::PrimaryImsSecondaryData);
        assert!(mode.ims_on_primary());
    }

    /// Data on both endpoints used to be a hard conflict, because the old design
    /// needed a free endpoint to put IMS on. IMS now always uses qmi0, so this is
    /// just user data in two places: keep the allocation and release the qmi0 one.
    #[test]
    fn both_slots_busy_releases_the_primary_instead_of_failing() {
        let mode = select_data_slot_mode(DataSlotInputs {
            data_requested: true,
            secondary_endpoint_available: true,
            primary_data_active: true,
            secondary_data_active: true,
        })
        .unwrap();
        assert_eq!(mode, DataSlotMode::PrimaryImsSecondaryData);
        assert!(mode.requires_primary_data_release(true));
    }

    #[test]
    fn data_requested_without_a_secondary_endpoint_is_missing() {
        let error = select_data_slot_mode(DataSlotInputs {
            data_requested: true,
            secondary_endpoint_available: false,
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!(error.code(), code::DATA_SLOT_MODE_MISSING);
    }

    /// IMS on qmi0 is an invariant, so every reachable mode must report it --
    /// no input combination may produce the DATA6 allocation message.
    #[test]
    fn every_allocation_keeps_ims_on_the_modemmanager_port() {
        for data_requested in [false, true] {
            for primary_data_active in [false, true] {
                for secondary_data_active in [false, true] {
                    let inputs = DataSlotInputs {
                        data_requested,
                        primary_data_active,
                        secondary_data_active,
                        secondary_endpoint_available: true,
                    };
                    let mode = select_data_slot_mode(inputs).expect("satisfiable");
                    assert!(mode.ims_on_primary(), "{inputs:?}");
                    assert_eq!(
                        mode.allocation_message(),
                        "IMS allocated to primary qmi0; DATA6 is reserved for data",
                        "{inputs:?}"
                    );
                }
            }
        }
    }

    /// A VoLTE-only line reserves no data slot, so there is no misplaced bearer
    /// to release even if ModemManager happens to have one.
    #[test]
    fn a_volte_only_line_never_asks_to_release_primary_data() {
        assert!(!DataSlotMode::PrimaryImsOnly.requires_primary_data_release(true));
    }
}
