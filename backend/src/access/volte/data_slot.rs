//! VoLTE data-slot allocation: which QMI endpoint carries IMS and which carries
//! normal mobile data.
//!
//! # Why this exists (beta2 alignment)
//!
//! beta2 does not hardcode "IMS on the primary port". It computes an explicit
//! *data-slot mode* at connect time (`src/volte.rs:1676-1687`) from three inputs
//! — whether data was requested, whether the primary data session is active, and
//! whether a secondary (DATA6) session is active — and reports one of two
//! allocations:
//!   - `IMS allocated to primary qmi0; DATA6 is reserved for data`
//!   - `IMS allocated to DATA6; primary qmi0 is reserved for data`
//! with the config-level intent serialized as one of `independent_wwan1`,
//! `secondary_qmi_data`, or `both_data_slots_active`, and the failure cases
//! `volte_data_slot_mode_missing` / `volte_data_slot_conflict`.
//!
//! Real-hardware testing (2026-07-27, Maxis 50212) established the decisive fact
//! that constrains this: **only "IMS on the primary port via qmi-proxy, DATA6
//! reserved for data" actually works** for the full IMS flow — the primary port
//! is the one endpoint where a WDS client id survives across `qmicli`
//! invocations (start-network → get-current-settings → P-CSCF). DATA6 can only
//! single-shot one command; it cannot reuse a CID, so it cannot host the IMS
//! multi-step flow. This module therefore *selects* the allocation exactly the
//! way beta2 does, but the only viable IMS-bearing endpoint is the primary port.
//!
//! This is pure logic (no IO), so the selection and its conflict rules are fully
//! unit-tested without a modem. The IO that acts on the chosen mode lives in
//! `live.rs` / `native_bearer.rs` / `secondary_qmi.rs`.

use super::errors::{code, VolteError};

/// Which physical QMI endpoint the IMS bearer is allocated to, and what the
/// other endpoint is reserved for. Mirrors beta2's two reported allocations plus
/// the "data not requested" case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSlotMode {
    /// IMS on the primary control port (via `qmi-proxy`); the secondary DATA6
    /// endpoint is reserved for normal mobile data. This is beta2's
    /// `IMS allocated to primary qmi0; DATA6 is reserved for data` and the only
    /// allocation that carries the full IMS flow on the reference firmware.
    PrimaryImsSecondaryData,
    /// IMS on the secondary DATA6 endpoint; the primary port is reserved for
    /// data. beta2's `IMS allocated to DATA6; primary qmi0 is reserved for data`.
    /// Retained for parity, but DATA6 cannot reuse a CID across `qmicli`
    /// invocations, so the multi-step IMS flow cannot run here on the reference
    /// firmware (`select` never picks it without an explicit request).
    SecondaryImsPrimaryData,
    /// IMS on the primary port and no separate data slot is requested — the
    /// common case when the line only wants VoLTE and not a proxied data exit.
    PrimaryImsOnly,
}

impl DataSlotMode {
    /// The human-readable allocation message beta2 logs for this mode.
    pub fn allocation_message(self) -> &'static str {
        match self {
            DataSlotMode::PrimaryImsSecondaryData | DataSlotMode::PrimaryImsOnly => {
                "IMS allocated to primary qmi0; DATA6 is reserved for data"
            }
            DataSlotMode::SecondaryImsPrimaryData => {
                "IMS allocated to DATA6; primary qmi0 is reserved for data"
            }
        }
    }

    /// The config-intent token beta2 serializes this allocation as. Each of the
    /// three beta2 tokens maps to exactly one mode, so the mapping round-trips.
    pub fn as_str(self) -> &'static str {
        match self {
            DataSlotMode::PrimaryImsOnly => "independent_wwan1",
            DataSlotMode::PrimaryImsSecondaryData => "secondary_qmi_data",
            DataSlotMode::SecondaryImsPrimaryData => "both_data_slots_active",
        }
    }

    /// True when this mode puts the IMS bearer on the primary control port. Every
    /// viable mode on the reference firmware does; `live.rs` uses this to decide
    /// whether the native primary-port path is the one to drive.
    pub fn ims_on_primary(self) -> bool {
        matches!(
            self,
            DataSlotMode::PrimaryImsSecondaryData | DataSlotMode::PrimaryImsOnly
        )
    }

    /// True when a distinct data slot (DATA6) is reserved alongside IMS.
    pub fn reserves_data_slot(self) -> bool {
        matches!(
            self,
            DataSlotMode::PrimaryImsSecondaryData | DataSlotMode::SecondaryImsPrimaryData
        )
    }
}

/// The runtime inputs beta2 feeds into the slot decision (`volte.rs:1676`):
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

/// Select the data-slot allocation the way beta2 does.
///
/// Rules (from `sub_58E0C4` / `volte.rs:1676-1687`, constrained by the hardware
/// finding that only primary-port IMS carries the full flow):
///   - No data requested → IMS owns the primary port, nothing else reserved
///     (`PrimaryImsOnly`).
///   - Data requested and a secondary DATA6 endpoint is available → IMS stays on
///     the primary port and DATA6 is reserved for data
///     (`PrimaryImsSecondaryData`).
///   - Data requested but *both* endpoints already carry active data sessions →
///     `volte_data_slot_conflict`: there is no free endpoint to place IMS on
///     without disturbing a live data session.
///   - Data requested but no secondary endpoint exists to offload data onto →
///     `volte_data_slot_mode_missing`: the allocation cannot be satisfied.
pub fn select_data_slot_mode(inputs: DataSlotInputs) -> Result<DataSlotMode, VolteError> {
    if !inputs.data_requested {
        // VoLTE-only line: IMS on the primary port, no data slot to juggle.
        return Ok(DataSlotMode::PrimaryImsOnly);
    }

    // Data is requested alongside IMS. IMS must land on the primary port (the
    // only endpoint that can host the multi-step flow), so data has to go to
    // DATA6. If both endpoints are already busy there is nowhere to put IMS.
    if inputs.primary_data_active && inputs.secondary_data_active {
        return Err(VolteError::with_detail(
            code::DATA_SLOT_CONFLICT,
            "both_data_slots_active",
        ));
    }

    if !inputs.secondary_endpoint_available {
        // There is no DATA6 endpoint to offload data onto, so IMS-plus-data
        // cannot be allocated. The caller decides whether to drop the data exit
        // or refuse to bring up VoLTE.
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

    #[test]
    fn data_plus_ims_reserves_data6_and_keeps_ims_on_primary() {
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
    }

    #[test]
    fn both_slots_busy_is_a_conflict() {
        let error = select_data_slot_mode(DataSlotInputs {
            data_requested: true,
            secondary_endpoint_available: true,
            primary_data_active: true,
            secondary_data_active: true,
        })
        .unwrap_err();
        assert_eq!(error.code(), code::DATA_SLOT_CONFLICT);
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

    #[test]
    fn secondary_ims_mode_reports_the_other_allocation_message() {
        // Not selected by `select` on the reference firmware, but the mapping must
        // stay correct for parity/logging.
        let mode = DataSlotMode::SecondaryImsPrimaryData;
        assert_eq!(
            mode.allocation_message(),
            "IMS allocated to DATA6; primary qmi0 is reserved for data"
        );
        assert_eq!(mode.as_str(), "both_data_slots_active");
        assert!(!mode.ims_on_primary());
        assert!(mode.reserves_data_slot());
    }
}
