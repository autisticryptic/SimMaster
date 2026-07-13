use serde::Serialize;

use crate::{modem_manager::SimIdentity, system_event::mask_identifier};

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

#[derive(Debug, Clone, Serialize)]
pub struct MaskedSimIdentity {
    pub present: bool,
    pub iccid: String,
    pub imsi: String,
    pub operator_id: String,
}

impl Default for MaskedSimIdentity {
    fn default() -> Self {
        Self {
            present: false,
            iccid: String::new(),
            imsi: String::new(),
            operator_id: String::new(),
        }
    }
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

    pub fn masked(&self) -> MaskedSimIdentity {
        MaskedSimIdentity {
            present: self.present(),
            iccid: mask_identifier(&self.iccid),
            imsi: mask_identifier(&self.imsi),
            operator_id: self.operator_id.clone(),
        }
    }
}
