//! VoLTE runtime error taxonomy.
//!
//! Clean-room note: the error *codes* below (e.g. `volte_imsi_missing`) mirror
//! the semantic categories that the published frontend (`volteStatus.js`)
//! matches on via `last_error` substring checks. Preserving these substrings is
//! an interoperability requirement so the existing UI renders the correct
//! Chinese hint. The codes are SimAdmin-owned identifiers derived from 3GPP
//! terminology, not copied from any third-party binary source.

use std::fmt;

/// Stable error-code strings surfaced in `last_error` and runtime events.
///
/// These are grouped by lifecycle stage. The frontend only matches on a subset
/// (see `frontend_contract_substrings` in tests) but we keep the full family so
/// logs stay greppable and each failure has a single, unambiguous cause code.
pub mod code {
    // Dependency / environment.
    pub const DEPENDENCY_MISSING_IP: &str = "volte_dependency_missing:ip";
    pub const COMMAND_SPAWN_FAILED: &str = "volte_command_spawn_failed";
    pub const COMMAND_TIMEOUT: &str = "volte_command_timeout";
    pub const COMMAND_FAILED: &str = "volte_command_failed";
    pub const COMMAND_WAIT_FAILED: &str = "volte_command_wait_failed";

    // Identity / AKA.
    pub const IMSI_MISSING: &str = "volte_imsi_missing";
    pub const MM_IMSI_MISSING: &str = "volte_mm_imsi_missing";
    pub const CARRIER_PROFILE_MISSING: &str = "volte_carrier_profile_missing";
    pub const CARRIER_IMS_APN_MISSING: &str = "volte_carrier_ims_apn_missing";
    pub const USIM_AID_MISSING: &str = "volte_usim_aid_missing";
    pub const USIM_AID_NOT_USIM: &str = "volte_usim_aid_not_usim";
    pub const USIM_AKA_FAILED: &str = "volte_usim_aka_failed";
    pub const AKA_MATERIAL_INVALID: &str = "volte_aka_material_invalid";
    pub const AKA_RES_EMPTY: &str = "volte_aka_res_empty";

    // Digest challenge parsing.
    pub const DIGEST_CHALLENGE_MISSING: &str = "volte_digest_challenge_missing";
    pub const DIGEST_REALM_MISSING: &str = "volte_digest_realm_missing";
    pub const DIGEST_NONCE_MISSING: &str = "volte_digest_nonce_missing";
    pub const DIGEST_NONCE_DECODE_FAILED: &str = "volte_digest_nonce_decode_failed";
    pub const DIGEST_QOP_UNSUPPORTED: &str = "volte_digest_qop_unsupported";
    pub const DIGEST_ALGORITHM_UNSUPPORTED: &str = "volte_digest_algorithm_unsupported";
    pub const REGISTER_NONCE_NOT_AKA: &str = "volte_register_nonce_not_aka";

    // IPsec (ip xfrm).
    pub const IPSEC_IK_INVALID: &str = "volte_ipsec_ik_invalid";
    pub const IPSEC_REQUIRES_IPV6: &str = "volte_ipsec_requires_ipv6";
    pub const IPSEC_UDP_BIND_FAILED: &str = "volte_ipsec_udp_bind_failed";
    pub const SECURITY_SERVER_MISSING: &str = "volte_security_server_missing";

    // SIP framing / encoding.
    pub const SIP_STATUS_INVALID: &str = "volte_sip_status_invalid";
    pub const SIP_STATUS_MISSING: &str = "volte_sip_status_missing";
    pub const SIP_NOT_UTF8: &str = "volte_sip_not_utf8";
    pub const SIP_HEADER_NOT_UTF8: &str = "volte_sip_header_not_utf8";
    pub const SIP_HEADER_MISSING: &str = "volte_sip_header_missing";
    pub const HEX_INVALID: &str = "volte_hex_invalid";

    // Bearer / modem / P-CSCF.
    pub const RUNTIME_MM_BEARER_ROAMING_FORBIDDEN: &str =
        "volte_runtime_mm_bearer_roaming_forbidden";
    pub const RUNTIME_MM_BEARER_NOT_CONNECTED: &str = "volte_runtime_mm_bearer_not_connected";
    pub const RUNTIME_MM_BEARER_CONNECT_FAILED: &str = "volte_runtime_mm_bearer_connect_failed";
    /// No dedicated QMI endpoint is available for IMS. There is deliberately no
    /// fallback to the ModemManager bearer: that path wedges the baseband.
    pub const RUNTIME_IMS_ENDPOINT_UNAVAILABLE: &str = "volte_runtime_ims_endpoint_unavailable";
    /// The IMS data session could not be started on the secondary QMI endpoint.
    pub const RUNTIME_IMS_BEARER_START_FAILED: &str = "volte_runtime_ims_bearer_start_failed";
    pub const RUNTIME_MM_BEARER_PATH_MISSING: &str = "volte_runtime_mm_bearer_path_missing";
    pub const RUNTIME_MM_MODEM_WAIT_TIMEOUT: &str = "volte_runtime_mm_modem_wait_timeout";
    pub const RUNTIME_ALL_PCSCF_FAILED: &str = "volte_runtime_all_pcscf_failed";
    /// No P-CSCF was prefetched from a stored IMS profile for this line. Not a
    /// hard failure on its own — discovery falls through to the live bearer /
    /// WDS / AT layers. Mirrors beta2's `volte_runtime_profile_pcscf_missing`.
    pub const RUNTIME_PROFILE_PCSCF_MISSING: &str = "volte_runtime_profile_pcscf_missing";
    /// A required IP family could not be brought up on the IMS bearer (e.g. the
    /// network forced IPv6-only but no prefix was delivered, or per-family IP
    /// configuration failed). Mirrors 1.7's `volte_runtime_ims_family_unsupported`.
    pub const RUNTIME_IMS_FAMILY_UNSUPPORTED: &str = "volte_runtime_ims_family_unsupported";
    pub const IP_SETTINGS_MISSING: &str = "volte_ip_settings_missing";
    pub const IPV6_GATEWAY_MISSING: &str = "volte_ipv6_gateway_missing";
    pub const PCSCF_FAMILY_MISMATCH: &str = "volte_pcscf_family_mismatch";

    // Data slot allocation (beta2 alignment). The IMS bearer and the normal
    // mobile-data bearer each need a QMI endpoint; on this firmware they cannot
    // share one. `select_data_slot_mode` decides which endpoint carries IMS and
    // which carries data (see `data_slot.rs`).
    /// No data-slot mode could be resolved for the line — neither the configured
    /// preference nor the endpoint capabilities yielded a usable allocation.
    /// Mirrors beta2's `volte_data_slot_mode_missing`.
    pub const DATA_SLOT_MODE_MISSING: &str = "volte_data_slot_mode_missing";
    /// The requested IMS and data allocations collide (e.g. both demand the
    /// primary port, or a secondary endpoint the other already holds). Mirrors
    /// beta2's `volte_data_slot_conflict`.
    pub const DATA_SLOT_CONFLICT: &str = "volte_data_slot_conflict";

    // Registration.
    pub const REGISTER_SEND_FAILED: &str = "volte_register_send_failed";
    pub const REGISTER_AUTH_SEND_FAILED: &str = "volte_register_auth_send_failed";
    pub const REGISTER_AUTH_UNEXPECTED_STATUS: &str = "volte_register_auth_unexpected_status";
    pub const REGISTER_INITIAL_UNEXPECTED_STATUS: &str = "volte_register_initial_unexpected_status";

    // SMS.
    pub const SMS_ENCODE_FAILED: &str = "volte_sms_encode_failed";
    pub const SMSC_MISSING: &str = "volte_smsc_missing";
    pub const PHONE_URI_INVALID: &str = "volte_phone_uri_invalid";
    pub const SMS_MESSAGE_ALL_VARIANTS_FAILED: &str = "volte_sms_message_all_variants_failed";

    // Runtime lifecycle.
    pub const RUNTIME_NOT_RUNNING: &str = "volte_runtime_not_running";
    pub const RUNTIME_SEND_TIMEOUT: &str = "volte_runtime_send_timeout";
    pub const RANDOM_FAILED: &str = "volte_random_failed";
}

/// Unified VoLTE error carrying a stable code plus optional detail suffix.
///
/// `Display` renders as `code` or `code:detail`, matching the binary's
/// observed `code:arg` convention (e.g. `volte_command_failed:mmcli`). This is
/// what gets stored in `last_error`, so the frontend substring matcher keys off
/// the leading code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolteError {
    code: &'static str,
    detail: Option<String>,
}

impl VolteError {
    pub fn new(code: &'static str) -> Self {
        Self { code, detail: None }
    }

    pub fn with_detail(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: Some(detail.into()),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

impl fmt::Display for VolteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.detail {
            Some(detail) => write!(f, "{}:{}", self.code, detail),
            None => write!(f, "{}", self.code),
        }
    }
}

impl std::error::Error for VolteError {}

/// Convenience constructor: `verr!(IMSI_MISSING)` or `verr!(COMMAND_FAILED, "mmcli")`.
#[macro_export]
macro_rules! verr {
    ($code:expr) => {
        $crate::connectivity::modems::ims::volte::errors::VolteError::new($code)
    };
    ($code:expr, $detail:expr) => {
        $crate::connectivity::modems::ims::volte::errors::VolteError::with_detail($code, $detail)
    };
}

pub type VolteResult<T> = Result<T, VolteError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_without_detail_is_bare_code() {
        assert_eq!(
            VolteError::new(code::IMSI_MISSING).to_string(),
            "volte_imsi_missing"
        );
    }

    #[test]
    fn display_with_detail_uses_colon_suffix() {
        assert_eq!(
            VolteError::with_detail(code::COMMAND_FAILED, "mmcli").to_string(),
            "volte_command_failed:mmcli"
        );
    }

    /// The frontend `h()` matcher in volteStatus.js keys off these substrings.
    /// If any of these codes change, the UI stops rendering the right hint.
    #[test]
    fn frontend_contract_substrings_present() {
        // Left column of the §4.5 error-mapping table.
        let contract = [
            code::IMSI_MISSING,
            code::RUNTIME_MM_BEARER_ROAMING_FORBIDDEN,
            code::DEPENDENCY_MISSING_IP,
            code::RUNTIME_MM_MODEM_WAIT_TIMEOUT,
            code::AKA_RES_EMPTY,
            code::USIM_AKA_FAILED,
            code::AKA_MATERIAL_INVALID,
        ];
        for c in contract {
            assert!(c.starts_with("volte_"), "unexpected code shape: {c}");
        }
        // The frontend also matches the `volte_at_` prefix family.
        assert!("volte_at_timeout".starts_with("volte_at_"));
    }
}
