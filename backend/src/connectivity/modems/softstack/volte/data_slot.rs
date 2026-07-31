//! VoLTE data-slot allocation: which QMI endpoint carries IMS and which carries
//! normal mobile data.
//!
//! # Why this exists (beta8 alignment)
//!
//! beta8 does not hardcode "IMS on the primary port". It computes an explicit
//! *data-slot mode* at connect time (`src/volte.rs:1676-1687`) from three inputs
//! — whether data was requested, whether the primary data session is active, and
//! whether a secondary (DATA6) session is active — and reports one of two
//! allocations:
//!   - `IMS allocated to primary qmi0; DATA6 is reserved for data`
//!   - `IMS allocated to DATA6; primary qmi0 is reserved for data`
//! with the data-path intent serialized as `independent_wwan1` or
//! `secondary_qmi_data`. `both_data_slots_active` is a conflict detail, not a
//! valid allocation, and is returned with
//! `volte_data_slot_mode_missing` / `volte_data_slot_conflict`.
//!
//! The selected mode is operational, not just a status token: IMS uses primary
//! qmi0 when ordinary data is on DATA6, and IMS uses DATA6 when ordinary data is
//! already active on qmi0. Starting both ordinary-data bearers is a conflict.
//!
//! This is pure logic (no IO), so the selection and its conflict rules are fully
//! unit-tested without a modem. The IO that acts on the chosen mode lives in
//! `live.rs` / `native_bearer.rs` / `secondary_qmi.rs`.

use super::errors::{code, VolteError};

/// Which physical QMI endpoint the IMS bearer is allocated to, and what the
/// other endpoint is reserved for. Mirrors beta8's two reported allocations plus
/// the "data not requested" case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSlotMode {
    /// IMS uses primary qmi0 and ordinary data uses DATA6.
    PrimaryImsSecondaryData,
    /// IMS uses DATA6 and ordinary data remains on primary qmi0.
    SecondaryImsPrimaryData,
    /// VoLTE-only line: no separate data exit is reserved. beta8 reports this as
    /// `independent_wwan1`.
    PrimaryImsOnly,
}

impl DataSlotMode {
    /// The human-readable allocation message beta8 logs for this mode.
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

    /// The data-path token beta8 reports for this allocation.
    pub fn as_str(self) -> &'static str {
        match self {
            DataSlotMode::PrimaryImsOnly => "independent_wwan1",
            DataSlotMode::PrimaryImsSecondaryData => "secondary_qmi_data",
            DataSlotMode::SecondaryImsPrimaryData => "independent_wwan1",
        }
    }

    /// Whether IMS must be established through ModemManager on primary qmi0.
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

/// Select the data-slot allocation the way beta8 does.
///
/// Rules (allocation branch at `0x5A8928` in the beta8 binary):
///   - No data requested → VoLTE-only line, no separate data slot (`PrimaryImsOnly`,
///     token `independent_wwan1`).
///   - Data requested but *both* endpoints already carry active data sessions →
///     `volte_data_slot_conflict`: there is no free endpoint left.
///   - Data requested but no secondary endpoint exists →
///     `volte_data_slot_mode_missing`: the allocation cannot be satisfied.
///   - Primary data active → IMS on DATA6 (`SecondaryImsPrimaryData`).
///   - Otherwise → IMS on qmi0 and ordinary data on DATA6
///     (`PrimaryImsSecondaryData`).
pub fn select_data_slot_mode(inputs: DataSlotInputs) -> Result<DataSlotMode, VolteError> {
    if !inputs.data_requested {
        // VoLTE-only line: IMS on the primary port, no data slot to juggle.
        return Ok(DataSlotMode::PrimaryImsOnly);
    }

    if inputs.primary_data_active && inputs.secondary_data_active {
        return Err(VolteError::with_detail(
            code::DATA_SLOT_CONFLICT,
            "both_data_slots_active",
        ));
    }

    if !inputs.secondary_endpoint_available {
        return Err(VolteError::new(code::DATA_SLOT_MODE_MISSING));
    }

    if inputs.primary_data_active {
        Ok(DataSlotMode::SecondaryImsPrimaryData)
    } else {
        Ok(DataSlotMode::PrimaryImsSecondaryData)
    }
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
    fn primary_data_moves_ims_to_data6() {
        let mode = select_data_slot_mode(DataSlotInputs {
            data_requested: true,
            secondary_endpoint_available: true,
            primary_data_active: true,
            secondary_data_active: false,
        })
        .unwrap();
        assert_eq!(mode, DataSlotMode::SecondaryImsPrimaryData);
        assert!(!mode.ims_on_primary());
        assert!(mode.reserves_data_slot());
        assert_eq!(mode.as_str(), "independent_wwan1");
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
        let mode = DataSlotMode::SecondaryImsPrimaryData;
        assert_eq!(
            mode.allocation_message(),
            "IMS allocated to DATA6; primary qmi0 is reserved for data"
        );
        assert_eq!(mode.as_str(), "independent_wwan1");
        assert!(!mode.ims_on_primary());
        assert!(mode.reserves_data_slot());
    }
}
