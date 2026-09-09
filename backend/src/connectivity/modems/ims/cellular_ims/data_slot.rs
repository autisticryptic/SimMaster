//! UE-only VoLTE bearer allocation.
//!
//! The coordinator receives endpoint facts from the selected device driver. On
//! QCA410 that means IMS on primary qmi0 and ordinary data on the project-created
//! DATA6 endpoint; other device drivers may provide a different layout.

use super::errors::{code, CellularImsError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSlotMode {
    /// Native IMS is the only cellular bearer requested for the line.
    UeNativeIms,
    /// Native IMS and native cellular data are both requested inside the UE
    /// namespace. The hardware drivers must resolve distinct usable netdevs.
    UeNativeImsWithData,
}

impl DataSlotMode {
    pub fn allocation_message(self) -> &'static str {
        match self {
            Self::UeNativeIms => "native IMS allocated inside the UE namespace",
            Self::UeNativeImsWithData => {
                "native IMS and cellular data allocated inside the UE namespace"
            }
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::UeNativeIms => "ue_native_ims",
            Self::UeNativeImsWithData => "ue_native_ims_with_data",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DataSlotInputs {
    /// The selected device's native IMS control/data endpoint is ready.
    pub ims_endpoint_available: bool,
    pub data_requested: bool,
    /// The selected device's independent ordinary-data endpoint is ready.
    pub data_endpoint_available: bool,
}

pub fn select_data_slot_mode(inputs: DataSlotInputs) -> Result<DataSlotMode, CellularImsError> {
    if !inputs.ims_endpoint_available || (inputs.data_requested && !inputs.data_endpoint_available)
    {
        return Err(CellularImsError::new(code::DATA_SLOT_MODE_MISSING));
    }

    Ok(if inputs.data_requested {
        DataSlotMode::UeNativeImsWithData
    } else {
        DataSlotMode::UeNativeIms
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_ims_does_not_require_data6_when_data_is_disabled() {
        let mode = select_data_slot_mode(DataSlotInputs {
            ims_endpoint_available: true,
            data_requested: false,
            data_endpoint_available: false,
        })
        .unwrap();
        assert_eq!(mode, DataSlotMode::UeNativeIms);
    }

    #[test]
    fn ordinary_data_requires_data6() {
        let error = select_data_slot_mode(DataSlotInputs {
            ims_endpoint_available: true,
            data_requested: true,
            data_endpoint_available: false,
        })
        .unwrap_err();
        assert_eq!(error.code(), code::DATA_SLOT_MODE_MISSING);
    }

    #[test]
    fn cellular_ims_only_uses_native_ims_in_the_ue_namespace() {
        let mode = select_data_slot_mode(DataSlotInputs {
            ims_endpoint_available: true,
            data_requested: false,
            data_endpoint_available: false,
        })
        .unwrap();
        assert_eq!(mode, DataSlotMode::UeNativeIms);
        assert_eq!(mode.as_str(), "ue_native_ims");
    }

    #[test]
    fn data_and_ims_remain_ue_native() {
        let mode = select_data_slot_mode(DataSlotInputs {
            ims_endpoint_available: true,
            data_requested: true,
            data_endpoint_available: true,
        })
        .unwrap();
        assert_eq!(mode, DataSlotMode::UeNativeImsWithData);
        assert_eq!(mode.as_str(), "ue_native_ims_with_data");
    }

    #[test]
    fn ims_endpoint_is_required_for_every_native_ims_line() {
        let error = select_data_slot_mode(DataSlotInputs {
            ims_endpoint_available: false,
            data_requested: false,
            data_endpoint_available: false,
        })
        .unwrap_err();
        assert_eq!(error.code(), code::DATA_SLOT_MODE_MISSING);
    }
}
