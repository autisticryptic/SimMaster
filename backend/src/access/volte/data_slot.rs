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
//! with the data-path intent serialized as `independent_wwan1` or
//! `secondary_qmi_data`. `both_data_slots_active` is a conflict detail, not a
//! valid allocation, and is returned with
//! `volte_data_slot_mode_missing` / `volte_data_slot_conflict`.
//!
//! IDA on beta2 settled which allocation is real: the binary logs
//! `Native VoLTE secondary QMI IMS WDS bearer started` (`volte.rs:1976`) and
//! reads the IMS P-CSCF/IP from `AT+CGCONTRDP`, not from
//! `--wds-get-current-settings` (which lives only on the data path,
//! `secondary_qmi_data.rs`). Because P-CSCF comes from AT, the IMS session is a
//! *single* `--wds-start-network` with no CID to reuse — so it runs on the
//! **secondary DATA6 endpoint**, leaving the primary QMI port to ModemManager.
//! Running a second data session on the primary port is what produced
//! `verbose call end reason (2,201): [internal] error` in the field logs. This
//! module still *selects* the allocation the way beta2 does; the IMS-bearing
//! endpoint is the secondary one.
//!
//! This is pure logic (no IO), so the selection and its conflict rules are fully
//! unit-tested without a modem. The IO that acts on the chosen mode lives in
//! `live.rs` / `native_bearer.rs` / `secondary_qmi.rs`.

use super::errors::{code, VolteError};

/// Which physical QMI endpoint the IMS bearer is allocated to, and what the
/// other endpoint is reserved for. Mirrors beta2's two reported allocations plus
/// the "data not requested" case.
///
/// NOTE: these variant names predate the beta2-aligned bearer path and are kept
/// only because their `as_str()` tokens (`secondary_qmi_data` / `independent_wwan1`)
/// are the exact data-path strings beta2 reports. The IMS bearer itself now always
/// runs on the secondary QMI endpoint (see `native_bearer.rs`); this mode only
/// selects what the *data* exit is reserved on and drives the reported token — it
/// no longer gates which endpoint carries IMS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSlotMode {
    /// A separate mobile-data exit is reserved alongside VoLTE. beta2 reports this
    /// as `secondary_qmi_data`.
    PrimaryImsSecondaryData,
    /// Parity variant for beta2's alternate allocation string; not selected by
    /// `select_data_slot_mode`.
    SecondaryImsPrimaryData,
    /// VoLTE-only line: no separate data exit is reserved. beta2 reports this as
    /// `independent_wwan1`.
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

    /// The data-path token beta2 reports for this allocation.
    pub fn as_str(self) -> &'static str {
        match self {
            DataSlotMode::PrimaryImsOnly => "independent_wwan1",
            DataSlotMode::PrimaryImsSecondaryData => "secondary_qmi_data",
            DataSlotMode::SecondaryImsPrimaryData => "independent_wwan1",
        }
    }

    /// Legacy predicate retained for the data-slot unit tests. IMS no longer runs
    /// on the primary port under any mode, so `live.rs` no longer gates on this.
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
/// Rules (from `sub_58E0C4` / `volte.rs:1676-1687`). IMS runs on the secondary
/// DATA6 endpoint; the reported allocation token is what beta2 logs, and it is
/// preserved verbatim for parity even though the endpoint roles are now settled:
///   - No data requested → VoLTE-only line, no separate data slot (`PrimaryImsOnly`,
///     token `independent_wwan1`).
///   - Data requested and a secondary DATA6 endpoint is available →
///     `PrimaryImsSecondaryData` (token `secondary_qmi_data`).
///   - Data requested but *both* endpoints already carry active data sessions →
///     `volte_data_slot_conflict`: there is no free endpoint left.
///   - Data requested but no secondary endpoint exists →
///     `volte_data_slot_mode_missing`: the allocation cannot be satisfied.
pub fn select_data_slot_mode(inputs: DataSlotInputs) -> Result<DataSlotMode, VolteError> {
    if !inputs.data_requested {
        // VoLTE-only line: IMS on the primary port, no data slot to juggle.
        return Ok(DataSlotMode::PrimaryImsOnly);
    }

    // Data is requested alongside IMS. IMS takes the secondary DATA6 endpoint
    // (single-shot WDS), so a data exit would need the primary port. If both
    // endpoints are already busy there is nowhere left to place a new session.
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
        assert_eq!(mode.as_str(), "independent_wwan1");
        assert!(!mode.ims_on_primary());
        assert!(mode.reserves_data_slot());
    }
}
