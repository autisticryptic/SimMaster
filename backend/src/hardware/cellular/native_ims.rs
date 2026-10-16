//! Identity-free modem-native observations. Never host SIP readiness or a lease.

use serde::Serialize;

pub const NATIVE_IMS_SCHEMA: &str = "ec20_ce_fag_v0803";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeImsRegistration {
    /// The qualified getter can return initialized zero for an absent TLV.
    UnknownOrUnreported,
    Registering,
    Registered,
    Limited,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeImsService {
    UnknownOrUnreported,
    Limited,
    Available,
    Unknown,
}

/// Sequential firmware getter samples, NOT an atomic snapshot. No technology,
/// SIM application/AID, host registration, calling or media readiness is proved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct NativeImsSample {
    pub schema: &'static str,
    /// Reported flag only: an EFS read failure may also produce false.
    pub registration_reporting: bool,
    /// Reported flag only, not proof of persistent configuration.
    pub service_reporting: bool,
    pub registration: NativeImsRegistration,
    pub sms: NativeImsService,
    pub voice: NativeImsService,
    /// The last firmware field is UT, not video.
    pub ut: NativeImsService,
}

impl Default for NativeImsSample {
    fn default() -> Self {
        Self {
            schema: NATIVE_IMS_SCHEMA,
            registration_reporting: false,
            service_reporting: false,
            registration: NativeImsRegistration::Unknown,
            sms: NativeImsService::Unknown,
            voice: NativeImsService::Unknown,
            ut: NativeImsService::Unknown,
        }
    }
}

/// Whitelisted outcomes only. Never carries AT text, device paths or identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeImsProbeError {
    Unsupported,
    Busy,
    BindingChanged,
    Unavailable,
    MalformedResponse,
    FirmwareNotQualified,
}

impl NativeImsProbeError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unsupported => "native_ims_unsupported",
            Self::Busy => "native_ims_busy",
            Self::BindingChanged => "native_ims_binding_changed",
            Self::Unavailable => "native_ims_unavailable",
            Self::MalformedResponse => "native_ims_malformed_response",
            Self::FirmwareNotQualified => "native_ims_firmware_not_qualified",
        }
    }
}

impl std::fmt::Display for NativeImsProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for NativeImsProbeError {}
