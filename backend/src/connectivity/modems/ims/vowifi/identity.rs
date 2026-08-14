use serde::Serialize;

use crate::{
    hardware::cellular::modem_manager::SimIdentity, services::system::system_event::mask_identifier,
};

/// SIM identity held by the local VoWiFi runtime.
///
/// Raw IMSI/ICCID are intentionally private. They may be used for local profile
/// matching, but public API responses must go through `masked()`.
#[derive(Clone)]
pub struct VowifiSimIdentity {
    iccid: String,
    imsi: String,
    operator_id: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct MaskedSimIdentity {
    pub present: bool,
    pub iccid: String,
    pub imsi: String,
    pub operator_id: String,
}

impl VowifiSimIdentity {
    pub fn from_modem(identity: &SimIdentity) -> Self {
        Self {
            iccid: identity.iccid.trim().to_string(),
            imsi: identity.imsi.trim().to_string(),
            operator_id: identity.operator_id.trim().to_string(),
        }
    }

    pub fn present(&self) -> bool {
        !self.imsi.is_empty() || !self.iccid.is_empty() || !self.operator_id.is_empty()
    }

    pub fn imsi(&self) -> &str {
        &self.imsi
    }

    pub fn operator_id(&self) -> &str {
        &self.operator_id
    }

    /// Build the identity used for carrier/profile matching when a line has a
    /// configured VoWiFi IMSI presentation override. The real SIM identity is
    /// still retained by the caller for QMI/UIM AKA authentication.
    pub fn with_presented_imsi(&self, imsi: impl Into<String>) -> Self {
        Self {
            iccid: self.iccid.clone(),
            imsi: imsi.into(),
            // A modem's current operator may describe the serving network and
            // conflict with a spoofed home IMSI. Force matching from IMSI.
            operator_id: String::new(),
        }
    }

    pub fn masked(&self) -> MaskedSimIdentity {
        MaskedSimIdentity {
            present: self.present(),
            iccid: mask_identifier(&self.iccid),
            imsi: mask_identifier(&self.imsi),
            operator_id: self.operator_id.clone(),
        }
    }
}
