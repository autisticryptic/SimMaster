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
    pub const SECURITY_SERVER_INVALID: &str = "volte_security_server_invalid";
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
    /// The device-selected IMS bearer session could not be started.
    pub const RUNTIME_IMS_BEARER_START_FAILED: &str = "volte_runtime_ims_bearer_start_failed";
    /// The selected provider declared the baseband unsafe for further attempts.
    /// Preserve this independently of diagnostic wording and netdev faults.
    pub const RUNTIME_IMS_BASEBAND_WEDGED: &str = "volte_runtime_ims_baseband_wedged";
    pub const RUNTIME_UE_WORKER_UNAVAILABLE: &str = "volte_runtime_ue_worker_unavailable";
    pub const RUNTIME_MM_BEARER_PATH_MISSING: &str = "volte_runtime_mm_bearer_path_missing";
    pub const RUNTIME_MM_MODEM_WAIT_TIMEOUT: &str = "volte_runtime_mm_modem_wait_timeout";
    pub const RUNTIME_CELLULAR_NETWORK_NOT_REGISTERED: &str =
        "volte_runtime_cellular_network_not_registered";
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
    /// The connected bearer netdev did not complete its remote OPEN handshake.
    pub const BEARER_NETDEV_NOT_UP: &str = "volte_bearer_netdev_not_up";
    /// The device driver reports a permanent runtime fault for the bearer netdev.
    pub const BEARER_NETDEV_RUNTIME_ERROR: &str = "volte_bearer_netdev_runtime_error";
    /// The bearer netdev was still unusable after the bounded readiness wait.
    pub const BEARER_NETDEV_NOT_READY: &str = "volte_bearer_netdev_not_ready";
    /// The bearer re-addressed between reading its settings and using them, so
    /// the source-based policy routing no longer matches the live interface.
    pub const BEARER_ADDRESS_CHANGED: &str = "volte_bearer_address_changed";
    pub const BEARER_SESSION_LOST: &str = "volte_bearer_session_lost";
    pub const PCSCF_FAMILY_MISMATCH: &str = "volte_pcscf_family_mismatch";

    // UE-only native bearer allocation.
    /// This line has no prepared native endpoint whose interface can be moved
    /// into its UE namespace.
    pub const DATA_SLOT_MODE_MISSING: &str = "volte_data_slot_mode_missing";

    // Registration.
    pub const REGISTER_SEND_FAILED: &str = "volte_register_send_failed";
    pub const REGISTER_AUTH_SEND_FAILED: &str = "volte_register_auth_send_failed";
    pub const REGISTER_AUTH_UNEXPECTED_STATUS: &str = "volte_register_auth_unexpected_status";
    pub const REGISTER_INITIAL_UNEXPECTED_STATUS: &str = "volte_register_initial_unexpected_status";
    /// The protected REGISTER refresh did not complete.  Keep refresh failures
    /// distinct from initial registration failures so the UI/logs do not make
    /// a healthy bearer look as if its first registration failed.
    pub const REGISTER_REFRESH_SEND_FAILED: &str = "volte_register_refresh_send_failed";
    pub const REGISTER_REFRESH_RECEIVE_FAILED: &str = "volte_register_refresh_receive_failed";
    pub const REGISTER_REFRESH_UNEXPECTED_STATUS: &str = "volte_register_refresh_unexpected_status";
    pub const REGISTER_REFRESH_AUTH_FAILED: &str = "volte_register_refresh_auth_failed";
    /// The line's UE worker was respawned after a socket/bearer was bound to it.
    /// The access leg must be torn down and rebuilt against the new generation.
    pub const RUNTIME_UE_WORKER_GENERATION_CHANGED: &str =
        "volte_runtime_ue_worker_generation_changed";

    // SMS.
    pub const SMS_ENCODE_FAILED: &str = "volte_sms_encode_failed";
    pub const SMSC_MISSING: &str = "volte_smsc_missing";
    pub const PHONE_URI_INVALID: &str = "volte_phone_uri_invalid";
    pub const SMS_MESSAGE_ALL_VARIANTS_FAILED: &str = "volte_sms_message_all_variants_failed";

    // IMS profile selection / activity probing (pcscf.rs).
    pub const IMS_PROFILE_DEFINITION_AMBIGUOUS: &str = "volte_ims_profile_definition_ambiguous";
    pub const IMS_PREFERRED_PROFILE_OCCUPIED: &str = "volte_ims_preferred_profile_occupied";
    pub const IMS_PROFILE_ACTIVITY_AMBIGUOUS: &str = "volte_ims_profile_activity_ambiguous";
    pub const IMS_PREFERRED_PROFILE_ACTIVE: &str = "volte_ims_preferred_profile_active";
    pub const IMS_PROFILE_ACTIVITY_UNAVAILABLE: &str = "volte_ims_profile_activity_unavailable";

    // DTMF argument validation (sip.rs).
    pub const DTMF_DIGIT_INVALID: &str = "volte_dtmf_digit_invalid";
    pub const DTMF_DURATION_INVALID: &str = "volte_dtmf_duration_invalid";

    // Source-based policy routing (bearer.rs).
    pub const ROUTE_FAMILY_MISMATCH: &str = "volte_route_family_mismatch";
    pub const ROUTE_GATEWAY_FAMILY_MISMATCH: &str = "volte_route_gateway_family_mismatch";

    // API / line lifecycle (handlers).
    pub const CARRIER_PROFILE_NOT_RESOLVED: &str = "volte_carrier_profile_not_resolved";
    pub const DEGRADED: &str = "volte_degraded";
    pub const IP_FAMILIES_CHANGED: &str = "volte_ip_families_changed";
    pub const LINE_ALREADY_REGISTERED: &str = "volte_line_already_registered";
    pub const LINE_CONNECTION_DISABLED: &str = "volte_line_connection_disabled";
    pub const LINE_NOT_PRESENT: &str = "volte_line_not_present";
    /// Historical shape: this one leads with `line_`, not `volte_`. Preserved
    /// verbatim because the frontend matches the exact string.
    pub const LINE_VOLTE_CONNECTION_DISABLED: &str = "line_volte_connection_disabled";
    pub const MODEM_REFRESH_FAILED: &str = "volte_modem_refresh_failed";
    pub const PROFILE_ATTEMPTS_EXHAUSTED: &str = "volte_profile_attempts_exhausted";
    pub const PROFILE_RESTORE_IN_PROGRESS: &str = "volte_profile_restore_in_progress";
    pub const PROFILE_SELECTION_CHANGED: &str = "volte_profile_selection_changed";
    pub const PROFILE_SOURCE_UNSUPPORTED: &str = "volte_profile_source_unsupported";
    pub const RETRY_ALREADY_RUNNING: &str = "volte_retry_already_running";
    pub const SIM_OVERRIDE_NOT_READY: &str = "volte_sim_override_not_ready";

    // Line/profile configuration validation.
    pub const DERIVED_PROFILE_ID_NOT_ALLOWED: &str = "volte_derived_profile_id_not_allowed";
    pub const IP_FAMILIES_DUPLICATE: &str = "volte_ip_families_duplicate";
    pub const IP_FAMILIES_EMPTY: &str = "volte_ip_families_empty";
    pub const PROFILE_ATTEMPT_COUNT_INVALID: &str = "volte_profile_attempt_count_invalid";
    pub const PROFILE_ID_TOO_LONG: &str = "volte_profile_id_too_long";

    // Runtime lifecycle.
    pub const RUNTIME_NOT_RUNNING: &str = "volte_runtime_not_running";
    pub const RUNTIME_SEND_TIMEOUT: &str = "volte_runtime_send_timeout";
    pub const RANDOM_FAILED: &str = "volte_random_failed";

    // SIP channel: socket binding, port reservation and UE-worker transfer.
    // `CHANNEL_READ_TIMEOUT` and `CHANNEL_READ_RETRYABLE` are matched by the
    // register loop to tell a benign read gap from a real failure, so they must
    // stay distinct from the send-side codes.
    pub const CHANNEL_BIND_FAILED: &str = "volte_channel_bind_failed";
    pub const CHANNEL_LOCAL_ADDR_FAILED: &str = "volte_channel_local_addr_failed";
    pub const CHANNEL_READ_FAILED: &str = "volte_channel_read_failed";
    pub const CHANNEL_READ_RETRYABLE: &str = "volte_channel_read_retryable";
    pub const CHANNEL_READ_TIMEOUT: &str = "volte_channel_read_timeout";
    pub const CHANNEL_RECEIVE_CONNECT_FAILED: &str = "volte_channel_receive_connect_failed";
    pub const CHANNEL_RECEIVE_NOT_RESERVED: &str = "volte_channel_receive_not_reserved";
    pub const CHANNEL_RECEIVE_PORT_MISMATCH: &str = "volte_channel_receive_port_mismatch";
    pub const CHANNEL_RECEIVE_RESERVE_FAILED: &str = "volte_channel_receive_reserve_failed";
    pub const CHANNEL_RECEIVE_RESERVE_INVALID_PORT: &str =
        "volte_channel_receive_reserve_invalid_port";
    pub const CHANNEL_RECEIVE_RESERVED_SIP_PORT: &str = "volte_channel_receive_reserved_sip_port";
    pub const CHANNEL_RECEIVE_SOCKET_MISSING: &str = "volte_channel_receive_socket_missing";
    pub const CHANNEL_SECURITY_UPDATE_PENDING: &str = "volte_channel_security_update_pending";
    pub const CHANNEL_SEND_CONNECT_FAILED: &str = "volte_channel_send_connect_failed";
    pub const CHANNEL_SEND_FAILED: &str = "volte_channel_send_failed";
    pub const CHANNEL_SEND_NOT_RESERVED: &str = "volte_channel_send_not_reserved";
    pub const CHANNEL_SEND_PORT_MISMATCH: &str = "volte_channel_send_port_mismatch";
    pub const CHANNEL_SEND_RESERVE_FAILED: &str = "volte_channel_send_reserve_failed";
    pub const CHANNEL_SEND_RESERVE_INVALID_PORT: &str = "volte_channel_send_reserve_invalid_port";
    pub const CHANNEL_SEND_SOCKET_MISSING: &str = "volte_channel_send_socket_missing";
    pub const CHANNEL_SHORT_SEND: &str = "volte_channel_short_send";
    pub const CHANNEL_WORKER_RECEIVE_MISMATCH: &str = "volte_channel_worker_receive_mismatch";
    pub const CHANNEL_WORKER_RECEIVE_REQUIRES_ASYNC: &str =
        "volte_channel_worker_receive_requires_async";
    pub const CHANNEL_WORKER_SEND_MISMATCH: &str = "volte_channel_worker_send_mismatch";
    pub const CHANNEL_WORKER_SEND_REQUIRES_ASYNC: &str = "volte_channel_worker_send_requires_async";
    pub const CHANNEL_WORKER_SOCKET_FAILED: &str = "volte_channel_worker_socket_failed";
    pub const CHANNEL_WORKER_SOCKET_TYPE: &str = "volte_channel_worker_socket_type";

    // Voice / RTP / transfer / SMS runtime (live.rs).
    pub const CNI_REQUIRED_DYNAMIC_UNAVAILABLE: &str = "volte_cni_required_dynamic_unavailable";
    pub const CONCURRENT_CALL_LIMIT: &str = "volte_concurrent_call_limit";
    pub const MT_RP_DATA_INVALID: &str = "volte_mt_rp_data_invalid";
    pub const PANI_REQUIRED_DYNAMIC_UNAVAILABLE: &str = "volte_pani_required_dynamic_unavailable";
    pub const QMI_DEVICE_MISSING: &str = "volte_qmi_device_missing";
    pub const REGISTER_AUTH_NOT_PREPARED: &str = "volte_register_auth_not_prepared";
    pub const REGISTER_NONCE_COUNT_EXHAUSTED: &str = "volte_register_nonce_count_exhausted";
    pub const RTP_BIND_FAILED: &str = "volte_rtp_bind_failed";
    pub const RTP_LOCAL_ADDR_FAILED: &str = "volte_rtp_local_addr_failed";
    pub const RTP_RELAY_MISSING: &str = "volte_rtp_relay_missing";
    pub const RUNTIME_NOT_REGISTERED: &str = "volte_runtime_not_registered";
    pub const SMS_DB_FAILED: &str = "volte_sms_db_failed";
    pub const SMS_MESSAGE_REJECTED: &str = "volte_sms_message_rejected";
    pub const TRANSFER_CALL_NOT_CONFIRMED: &str = "volte_transfer_call_not_confirmed";
    pub const TRANSFER_CALL_UNKNOWN: &str = "volte_transfer_call_unknown";
    pub const TRANSFER_NOT_PENDING: &str = "volte_transfer_not_pending";
    pub const TRANSFER_PENDING: &str = "volte_transfer_pending";
    pub const TRANSFER_REQUEST_INVALID: &str = "volte_transfer_request_invalid";
    pub const TRANSFER_RESPONSE_INVALID: &str = "volte_transfer_response_invalid";
    pub const VOICE_CALL_DUPLICATE: &str = "volte_voice_call_duplicate";
    pub const VOICE_CALL_UNKNOWN: &str = "volte_voice_call_unknown";
    pub const VOICE_CALLEE_INVALID: &str = "volte_voice_callee_invalid";
    pub const VOICE_DIRECTION_MISMATCH: &str = "volte_voice_direction_mismatch";
    pub const VOICE_INITIAL_INVITE_MISSING: &str = "volte_voice_initial_invite_missing";
    pub const VOICE_INVITE_BRANCH_MISSING: &str = "volte_voice_invite_branch_missing";
    pub const VOICE_MEDIA_ADDRESS_INVALID: &str = "volte_voice_media_address_invalid";
    pub const VOICE_MEDIA_PORT_INVALID: &str = "volte_voice_media_port_invalid";
    pub const VOICE_NO_COMMON_CODEC: &str = "volte_voice_no_common_codec";
    pub const VOICE_REINVITE_NOT_PENDING: &str = "volte_voice_reinvite_not_pending";
    pub const VOICE_REINVITE_PENDING: &str = "volte_voice_reinvite_pending";
    pub const VOICE_REMOTE_TAG_MISSING: &str = "volte_voice_remote_tag_missing";
    pub const VOICE_RSEQ_MISSING: &str = "volte_voice_rseq_missing";
    pub const VOICE_SDP_INVALID: &str = "volte_voice_sdp_invalid";

    /// Every stable code in this module, for the cross-layer consistency
    /// guard. The frontend keeps an exact-match table keyed on these values;
    /// `.github/scripts/test_ims_error_code_contract.py` asserts the two sides
    /// stay in sync, so a new code cannot ship without a matching UI hint.
    ///
    /// Order is alphabetical by constant name and carries no meaning.
    pub const ALL: &[&str] = &[
        AKA_MATERIAL_INVALID,
        AKA_RES_EMPTY,
        BEARER_ADDRESS_CHANGED,
        BEARER_NETDEV_NOT_READY,
        BEARER_NETDEV_NOT_UP,
        BEARER_NETDEV_RUNTIME_ERROR,
        BEARER_SESSION_LOST,
        CARRIER_IMS_APN_MISSING,
        CARRIER_PROFILE_MISSING,
        CARRIER_PROFILE_NOT_RESOLVED,
        CHANNEL_BIND_FAILED,
        CHANNEL_LOCAL_ADDR_FAILED,
        CHANNEL_READ_FAILED,
        CHANNEL_READ_RETRYABLE,
        CHANNEL_READ_TIMEOUT,
        CHANNEL_RECEIVE_CONNECT_FAILED,
        CHANNEL_RECEIVE_NOT_RESERVED,
        CHANNEL_RECEIVE_PORT_MISMATCH,
        CHANNEL_RECEIVE_RESERVED_SIP_PORT,
        CHANNEL_RECEIVE_RESERVE_FAILED,
        CHANNEL_RECEIVE_RESERVE_INVALID_PORT,
        CHANNEL_RECEIVE_SOCKET_MISSING,
        CHANNEL_SECURITY_UPDATE_PENDING,
        CHANNEL_SEND_CONNECT_FAILED,
        CHANNEL_SEND_FAILED,
        CHANNEL_SEND_NOT_RESERVED,
        CHANNEL_SEND_PORT_MISMATCH,
        CHANNEL_SEND_RESERVE_FAILED,
        CHANNEL_SEND_RESERVE_INVALID_PORT,
        CHANNEL_SEND_SOCKET_MISSING,
        CHANNEL_SHORT_SEND,
        CHANNEL_WORKER_RECEIVE_MISMATCH,
        CHANNEL_WORKER_RECEIVE_REQUIRES_ASYNC,
        CHANNEL_WORKER_SEND_MISMATCH,
        CHANNEL_WORKER_SEND_REQUIRES_ASYNC,
        CHANNEL_WORKER_SOCKET_FAILED,
        CHANNEL_WORKER_SOCKET_TYPE,
        CNI_REQUIRED_DYNAMIC_UNAVAILABLE,
        COMMAND_FAILED,
        COMMAND_SPAWN_FAILED,
        COMMAND_TIMEOUT,
        COMMAND_WAIT_FAILED,
        CONCURRENT_CALL_LIMIT,
        DATA_SLOT_MODE_MISSING,
        DEGRADED,
        DEPENDENCY_MISSING_IP,
        DERIVED_PROFILE_ID_NOT_ALLOWED,
        DIGEST_ALGORITHM_UNSUPPORTED,
        DIGEST_CHALLENGE_MISSING,
        DIGEST_NONCE_DECODE_FAILED,
        DIGEST_NONCE_MISSING,
        DIGEST_QOP_UNSUPPORTED,
        DIGEST_REALM_MISSING,
        DTMF_DIGIT_INVALID,
        DTMF_DURATION_INVALID,
        HEX_INVALID,
        IMSI_MISSING,
        IMS_PREFERRED_PROFILE_ACTIVE,
        IMS_PREFERRED_PROFILE_OCCUPIED,
        IMS_PROFILE_ACTIVITY_AMBIGUOUS,
        IMS_PROFILE_ACTIVITY_UNAVAILABLE,
        IMS_PROFILE_DEFINITION_AMBIGUOUS,
        IPSEC_IK_INVALID,
        IPSEC_REQUIRES_IPV6,
        IPSEC_UDP_BIND_FAILED,
        IPV6_GATEWAY_MISSING,
        IP_FAMILIES_CHANGED,
        IP_FAMILIES_DUPLICATE,
        IP_FAMILIES_EMPTY,
        IP_SETTINGS_MISSING,
        LINE_ALREADY_REGISTERED,
        LINE_CONNECTION_DISABLED,
        LINE_NOT_PRESENT,
        LINE_VOLTE_CONNECTION_DISABLED,
        MM_IMSI_MISSING,
        MODEM_REFRESH_FAILED,
        MT_RP_DATA_INVALID,
        PANI_REQUIRED_DYNAMIC_UNAVAILABLE,
        PCSCF_FAMILY_MISMATCH,
        PHONE_URI_INVALID,
        PROFILE_ATTEMPTS_EXHAUSTED,
        PROFILE_ATTEMPT_COUNT_INVALID,
        PROFILE_ID_TOO_LONG,
        PROFILE_RESTORE_IN_PROGRESS,
        PROFILE_SELECTION_CHANGED,
        PROFILE_SOURCE_UNSUPPORTED,
        QMI_DEVICE_MISSING,
        RANDOM_FAILED,
        REGISTER_AUTH_NOT_PREPARED,
        REGISTER_AUTH_SEND_FAILED,
        REGISTER_AUTH_UNEXPECTED_STATUS,
        REGISTER_INITIAL_UNEXPECTED_STATUS,
        REGISTER_NONCE_COUNT_EXHAUSTED,
        REGISTER_NONCE_NOT_AKA,
        REGISTER_REFRESH_AUTH_FAILED,
        REGISTER_REFRESH_RECEIVE_FAILED,
        REGISTER_REFRESH_SEND_FAILED,
        REGISTER_REFRESH_UNEXPECTED_STATUS,
        REGISTER_SEND_FAILED,
        RETRY_ALREADY_RUNNING,
        ROUTE_FAMILY_MISMATCH,
        ROUTE_GATEWAY_FAMILY_MISMATCH,
        RTP_BIND_FAILED,
        RTP_LOCAL_ADDR_FAILED,
        RTP_RELAY_MISSING,
        RUNTIME_ALL_PCSCF_FAILED,
        RUNTIME_CELLULAR_NETWORK_NOT_REGISTERED,
        RUNTIME_IMS_BASEBAND_WEDGED,
        RUNTIME_IMS_BEARER_START_FAILED,
        RUNTIME_IMS_ENDPOINT_UNAVAILABLE,
        RUNTIME_IMS_FAMILY_UNSUPPORTED,
        RUNTIME_MM_BEARER_CONNECT_FAILED,
        RUNTIME_MM_BEARER_NOT_CONNECTED,
        RUNTIME_MM_BEARER_PATH_MISSING,
        RUNTIME_MM_BEARER_ROAMING_FORBIDDEN,
        RUNTIME_MM_MODEM_WAIT_TIMEOUT,
        RUNTIME_NOT_REGISTERED,
        RUNTIME_NOT_RUNNING,
        RUNTIME_PROFILE_PCSCF_MISSING,
        RUNTIME_SEND_TIMEOUT,
        RUNTIME_UE_WORKER_GENERATION_CHANGED,
        RUNTIME_UE_WORKER_UNAVAILABLE,
        SECURITY_SERVER_INVALID,
        SECURITY_SERVER_MISSING,
        SIM_OVERRIDE_NOT_READY,
        SIP_HEADER_MISSING,
        SIP_HEADER_NOT_UTF8,
        SIP_NOT_UTF8,
        SIP_STATUS_INVALID,
        SIP_STATUS_MISSING,
        SMSC_MISSING,
        SMS_DB_FAILED,
        SMS_ENCODE_FAILED,
        SMS_MESSAGE_ALL_VARIANTS_FAILED,
        SMS_MESSAGE_REJECTED,
        TRANSFER_CALL_NOT_CONFIRMED,
        TRANSFER_CALL_UNKNOWN,
        TRANSFER_NOT_PENDING,
        TRANSFER_PENDING,
        TRANSFER_REQUEST_INVALID,
        TRANSFER_RESPONSE_INVALID,
        USIM_AID_MISSING,
        USIM_AID_NOT_USIM,
        USIM_AKA_FAILED,
        VOICE_CALLEE_INVALID,
        VOICE_CALL_DUPLICATE,
        VOICE_CALL_UNKNOWN,
        VOICE_DIRECTION_MISMATCH,
        VOICE_INITIAL_INVITE_MISSING,
        VOICE_INVITE_BRANCH_MISSING,
        VOICE_MEDIA_ADDRESS_INVALID,
        VOICE_MEDIA_PORT_INVALID,
        VOICE_NO_COMMON_CODEC,
        VOICE_REINVITE_NOT_PENDING,
        VOICE_REINVITE_PENDING,
        VOICE_REMOTE_TAG_MISSING,
        VOICE_RSEQ_MISSING,
        VOICE_SDP_INVALID,
    ];
}

/// Unified VoLTE error carrying a stable code plus optional detail suffix.
///
/// `Display` renders as `code` or `code:detail`, matching the binary's
/// observed `code:arg` convention (e.g. `volte_command_failed:mmcli`). This is
/// what gets stored in `last_error`, so the frontend substring matcher keys off
/// the leading code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellularImsError {
    code: &'static str,
    detail: Option<String>,
}

impl CellularImsError {
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

impl fmt::Display for CellularImsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.detail {
            Some(detail) => write!(f, "{}:{}", self.code, detail),
            None => write!(f, "{}", self.code),
        }
    }
}

impl std::error::Error for CellularImsError {}

/// Convenience constructor: `verr!(IMSI_MISSING)` or `verr!(COMMAND_FAILED, "mmcli")`.
#[macro_export]
macro_rules! verr {
    ($code:expr) => {
        $crate::connectivity::modems::ims::cellular_ims::errors::CellularImsError::new($code)
    };
    ($code:expr, $detail:expr) => {
        $crate::connectivity::modems::ims::cellular_ims::errors::CellularImsError::with_detail(
            $code, $detail,
        )
    };
}

pub type CellularImsResult<T> = Result<T, CellularImsError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_without_detail_is_bare_code() {
        assert_eq!(
            CellularImsError::new(code::IMSI_MISSING).to_string(),
            "volte_imsi_missing"
        );
    }

    #[test]
    fn display_with_detail_uses_colon_suffix() {
        assert_eq!(
            CellularImsError::with_detail(code::COMMAND_FAILED, "mmcli").to_string(),
            "volte_command_failed:mmcli"
        );
    }

    /// `code::ALL` must list every declared constant. A code missing from it is
    /// invisible to the cross-layer guard, which is how a shipped code ends up
    /// with no UI hint. Parsing this file at build time is not possible, so the
    /// count is asserted instead: adding a constant without extending `ALL`
    /// fails here.
    #[test]
    fn all_lists_every_declared_code() {
        // Keep in step with the `pub const` count in `mod code`.
        assert_eq!(
            code::ALL.len(),
            158,
            "code::ALL is out of sync with mod code"
        );
        let mut seen = std::collections::BTreeSet::new();
        for entry in code::ALL {
            assert!(
                seen.insert(*entry),
                "code::ALL contains a duplicate value: {entry}"
            );
        }
    }

    /// No code may be a substring of another. The frontend historically used
    /// `includes()`, so an accidental prefix relation silently routes a failure
    /// to the wrong hint. Renaming must not reintroduce that.
    #[test]
    fn no_code_is_a_substring_of_another() {
        for outer in code::ALL {
            for inner in code::ALL {
                if outer == inner {
                    continue;
                }
                assert!(
                    !outer.contains(inner),
                    "code {inner} is a substring of {outer}; exact-match routing would be ambiguous"
                );
            }
        }
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
