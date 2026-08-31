//! UE-only VoLTE bearer allocation.
//!
//! IMS never uses a ModemManager bearer in the host network namespace. A line
//! must have a prepared native QMI endpoint whose data-plane interface can be
//! moved into that line's UE namespace. Ordinary cellular data, when enabled,
//! follows the same rule and uses its own native session/interface.

use super::errors::{code, VolteError};

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
    pub data_requested: bool,
    /// A native endpoint prepared for this exact baseband is mandatory even
    /// when ordinary data is disabled, because IMS itself uses that endpoint.
    pub native_endpoint_available: bool,
}

pub fn select_data_slot_mode(inputs: DataSlotInputs) -> Result<DataSlotMode, VolteError> {
    if !inputs.native_endpoint_available {
        return Err(VolteError::new(code::DATA_SLOT_MODE_MISSING));
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
    fn native_endpoint_is_required_for_every_volte_line() {
        for data_requested in [false, true] {
            let error = select_data_slot_mode(DataSlotInputs {
                data_requested,
                native_endpoint_available: false,
            })
            .unwrap_err();
            assert_eq!(error.code(), code::DATA_SLOT_MODE_MISSING);
        }
    }

    #[test]
    fn volte_only_uses_native_ims_in_the_ue_namespace() {
        let mode = select_data_slot_mode(DataSlotInputs {
            data_requested: false,
            native_endpoint_available: true,
        })
        .unwrap();
        assert_eq!(mode, DataSlotMode::UeNativeIms);
        assert_eq!(mode.as_str(), "ue_native_ims");
    }

    #[test]
    fn data_and_ims_remain_ue_native() {
        let mode = select_data_slot_mode(DataSlotInputs {
            data_requested: true,
            native_endpoint_available: true,
        })
        .unwrap();
        assert_eq!(mode, DataSlotMode::UeNativeImsWithData);
        assert_eq!(mode.as_str(), "ue_native_ims_with_data");
    }
}
