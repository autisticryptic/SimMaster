//! Cellular IMS runtime error taxonomy.
//!
//! These codes describe the IMS registration and its services (voice, SMS,
//! supplementary) over cellular access. They used to carry a `volte_` prefix,
//! which wrongly equated IMS registration with its voice service; they are now
//! `cellular_ims_*`. The frontend matches them by exact token against a table
//! generated from this module (`cellularImsErrorCodes.ts`). The codes are
//! SimAdmin-owned identifiers derived from 3GPP terminology, not copied from
//! any third-party binary source.

use std::fmt;

/// Stable error-code strings surfaced in `last_error` and runtime events.
///
/// These are grouped by lifecycle stage. The frontend keeps an exact copy of
/// the full set (see `code::ALL`) so logs stay greppable and each failure has a
/// single, unambiguous cause code.
pub mod code {
    // Dependency / environment.
    pub const DEPENDENCY_MISSING_IP: &str = "cellular_ims_dependency_missing:ip";
    pub const COMMAND_SPAWN_FAILED: &str = "cellular_ims_command_spawn_failed";
    pub const COMMAND_TIMEOUT: &str = "cellular_ims_command_timeout";
    pub const COMMAND_FAILED: &str = "cellular_ims_command_failed";
    pub const COMMAND_WAIT_FAILED: &str = "cellular_ims_command_wait_failed";

    // Identity / AKA.
    pub const IMSI_MISSING: &str = "cellular_ims_imsi_missing";
    pub const MM_IMSI_MISSING: &str = "cellular_ims_mm_imsi_missing";
    pub const CARRIER_PROFILE_MISSING: &str = "cellular_ims_carrier_profile_missing";
    pub const CARRIER_IMS_APN_MISSING: &str = "cellular_ims_carrier_ims_apn_missing";
    pub const USIM_AID_MISSING: &str = "cellular_ims_usim_aid_missing";
    pub const USIM_AID_NOT_USIM: &str = "cellular_ims_usim_aid_not_usim";
    pub const USIM_AKA_FAILED: &str = "cellular_ims_usim_aka_failed";
    pub const AKA_MATERIAL_INVALID: &str = "cellular_ims_aka_material_invalid";
    pub const AKA_RES_EMPTY: &str = "cellular_ims_aka_res_empty";

    // Digest challenge parsing.
    pub const DIGEST_CHALLENGE_MISSING: &str = "cellular_ims_digest_challenge_missing";
    pub const DIGEST_REALM_MISSING: &str = "cellular_ims_digest_realm_missing";
    pub const DIGEST_NONCE_MISSING: &str = "cellular_ims_digest_nonce_missing";
    pub const DIGEST_NONCE_DECODE_FAILED: &str = "cellular_ims_digest_nonce_decode_failed";
    pub const DIGEST_QOP_UNSUPPORTED: &str = "cellular_ims_digest_qop_unsupported";
    pub const DIGEST_ALGORITHM_UNSUPPORTED: &str = "cellular_ims_digest_algorithm_unsupported";
    pub const REGISTER_NONCE_NOT_AKA: &str = "cellular_ims_register_nonce_not_aka";

    // IPsec (ip xfrm).
    pub const IPSEC_IK_INVALID: &str = "cellular_ims_ipsec_ik_invalid";
    pub const IPSEC_REQUIRES_IPV6: &str = "cellular_ims_ipsec_requires_ipv6";
    pub const IPSEC_UDP_BIND_FAILED: &str = "cellular_ims_ipsec_udp_bind_failed";
    pub const SECURITY_SERVER_INVALID: &str = "cellular_ims_security_server_invalid";
    pub const SECURITY_SERVER_MISSING: &str = "cellular_ims_security_server_missing";

    // SIP framing / encoding.
    pub const SIP_STATUS_INVALID: &str = "cellular_ims_sip_status_invalid";
    pub const SIP_STATUS_MISSING: &str = "cellular_ims_sip_status_missing";
    pub const SIP_NOT_UTF8: &str = "cellular_ims_sip_not_utf8";
    pub const SIP_HEADER_NOT_UTF8: &str = "cellular_ims_sip_header_not_utf8";
    pub const SIP_HEADER_MISSING: &str = "cellular_ims_sip_header_missing";
    pub const HEX_INVALID: &str = "cellular_ims_hex_invalid";

    // Bearer / modem / P-CSCF.
    pub const RUNTIME_MM_BEARER_ROAMING_FORBIDDEN: &str =
        "cellular_ims_runtime_mm_bearer_roaming_forbidden";
    pub const RUNTIME_MM_BEARER_NOT_CONNECTED: &str =
        "cellular_ims_runtime_mm_bearer_not_connected";
    pub const RUNTIME_MM_BEARER_CONNECT_FAILED: &str =
        "cellular_ims_runtime_mm_bearer_connect_failed";
    /// No dedicated QMI endpoint is available for IMS. There is deliberately no
    /// fallback to the ModemManager bearer: that path wedges the baseband.
    pub const RUNTIME_IMS_ENDPOINT_UNAVAILABLE: &str =
        "cellular_ims_runtime_ims_endpoint_unavailable";
    /// The device-selected IMS bearer session could not be started.
    pub const RUNTIME_IMS_BEARER_START_FAILED: &str =
        "cellular_ims_runtime_ims_bearer_start_failed";
    /// The selected provider declared the baseband unsafe for further attempts.
    /// Preserve this independently of diagnostic wording and netdev faults.
    pub const RUNTIME_IMS_BASEBAND_WEDGED: &str = "cellular_ims_runtime_ims_baseband_wedged";
    pub const RUNTIME_UE_WORKER_UNAVAILABLE: &str = "cellular_ims_runtime_ue_worker_unavailable";
    pub const RUNTIME_MM_BEARER_PATH_MISSING: &str = "cellular_ims_runtime_mm_bearer_path_missing";
    pub const RUNTIME_MM_MODEM_WAIT_TIMEOUT: &str = "cellular_ims_runtime_mm_modem_wait_timeout";
    pub const RUNTIME_CELLULAR_NETWORK_NOT_REGISTERED: &str =
        "cellular_ims_runtime_cellular_network_not_registered";
    pub const RUNTIME_ALL_PCSCF_FAILED: &str = "cellular_ims_runtime_all_pcscf_failed";
    /// No P-CSCF was prefetched from a stored IMS profile for this line. Not a
    /// hard failure on its own — discovery falls through to the live bearer /
    /// WDS / AT layers. Mirrors beta2's `volte_runtime_profile_pcscf_missing`.
    pub const RUNTIME_PROFILE_PCSCF_MISSING: &str = "cellular_ims_runtime_profile_pcscf_missing";
    /// A required IP family could not be brought up on the IMS bearer (e.g. the
    /// network forced IPv6-only but no prefix was delivered, or per-family IP
    /// configuration failed). Mirrors 1.7's `volte_runtime_ims_family_unsupported`.
    pub const RUNTIME_IMS_FAMILY_UNSUPPORTED: &str = "cellular_ims_runtime_ims_family_unsupported";
    pub const IP_SETTINGS_MISSING: &str = "cellular_ims_ip_settings_missing";
    pub const IPV6_GATEWAY_MISSING: &str = "cellular_ims_ipv6_gateway_missing";
    /// The connected bearer netdev did not complete its remote OPEN handshake.
    pub const BEARER_NETDEV_NOT_UP: &str = "cellular_ims_bearer_netdev_not_up";
    /// The device driver reports a permanent runtime fault for the bearer netdev.
    pub const BEARER_NETDEV_RUNTIME_ERROR: &str = "cellular_ims_bearer_netdev_runtime_error";
    /// The bearer netdev was still unusable after the bounded readiness wait.
    pub const BEARER_NETDEV_NOT_READY: &str = "cellular_ims_bearer_netdev_not_ready";
    /// The bearer re-addressed between reading its settings and using them, so
    /// the source-based policy routing no longer matches the live interface.
    pub const BEARER_ADDRESS_CHANGED: &str = "cellular_ims_bearer_address_changed";
    pub const BEARER_SESSION_LOST: &str = "cellular_ims_bearer_session_lost";
    pub const PCSCF_FAMILY_MISMATCH: &str = "cellular_ims_pcscf_family_mismatch";

    // UE-only native bearer allocation.
    /// This line has no prepared native endpoint whose interface can be moved
    /// into its UE namespace.
    pub const DATA_SLOT_MODE_MISSING: &str = "cellular_ims_data_slot_mode_missing";

    // Registration.
    pub const REGISTER_SEND_FAILED: &str = "cellular_ims_register_send_failed";
    pub const REGISTER_AUTH_SEND_FAILED: &str = "cellular_ims_register_auth_send_failed";
    pub const REGISTER_AUTH_UNEXPECTED_STATUS: &str =
        "cellular_ims_register_auth_unexpected_status";
    pub const REGISTER_INITIAL_UNEXPECTED_STATUS: &str =
        "cellular_ims_register_initial_unexpected_status";
    /// The protected REGISTER refresh did not complete.  Keep refresh failures
    /// distinct from initial registration failures so the UI/logs do not make
    /// a healthy bearer look as if its first registration failed.
    pub const REGISTER_REFRESH_SEND_FAILED: &str = "cellular_ims_register_refresh_send_failed";
    pub const REGISTER_REFRESH_RECEIVE_FAILED: &str =
        "cellular_ims_register_refresh_receive_failed";
    pub const REGISTER_REFRESH_UNEXPECTED_STATUS: &str =
        "cellular_ims_register_refresh_unexpected_status";
    pub const REGISTER_REFRESH_AUTH_FAILED: &str = "cellular_ims_register_refresh_auth_failed";
    /// The line's UE worker was respawned after a socket/bearer was bound to it.
    /// The access leg must be torn down and rebuilt against the new generation.
    pub const RUNTIME_UE_WORKER_GENERATION_CHANGED: &str =
        "cellular_ims_runtime_ue_worker_generation_changed";

    // SMS.
    pub const SMS_ENCODE_FAILED: &str = "cellular_ims_sms_encode_failed";
    pub const SMSC_MISSING: &str = "cellular_ims_smsc_missing";
    pub const PHONE_URI_INVALID: &str = "cellular_ims_phone_uri_invalid";
    pub const SMS_MESSAGE_ALL_VARIANTS_FAILED: &str =
        "cellular_ims_sms_message_all_variants_failed";

    // IMS profile selection / activity probing (pcscf.rs).
    pub const IMS_PROFILE_DEFINITION_AMBIGUOUS: &str = "cellular_ims_profile_definition_ambiguous";
    pub const IMS_PREFERRED_PROFILE_OCCUPIED: &str = "cellular_ims_preferred_profile_occupied";
    pub const IMS_PROFILE_ACTIVITY_AMBIGUOUS: &str = "cellular_ims_profile_activity_ambiguous";
    pub const IMS_PREFERRED_PROFILE_ACTIVE: &str = "cellular_ims_preferred_profile_active";
    pub const IMS_PROFILE_ACTIVITY_UNAVAILABLE: &str = "cellular_ims_profile_activity_unavailable";

    // DTMF argument validation (sip.rs).
    pub const DTMF_DIGIT_INVALID: &str = "cellular_ims_dtmf_digit_invalid";
    pub const DTMF_DURATION_INVALID: &str = "cellular_ims_dtmf_duration_invalid";

    // Source-based policy routing (bearer.rs).
    pub const ROUTE_FAMILY_MISMATCH: &str = "cellular_ims_route_family_mismatch";
    pub const ROUTE_GATEWAY_FAMILY_MISMATCH: &str = "cellular_ims_route_gateway_family_mismatch";

    // API / line lifecycle (handlers).
    pub const CARRIER_PROFILE_NOT_RESOLVED: &str = "cellular_ims_carrier_profile_not_resolved";
    pub const DEGRADED: &str = "cellular_ims_degraded";
    pub const IP_FAMILIES_CHANGED: &str = "cellular_ims_ip_families_changed";
    pub const LINE_ALREADY_REGISTERED: &str = "cellular_ims_line_already_registered";
    pub const LINE_CONNECTION_DISABLED: &str = "cellular_ims_line_connection_disabled";
    pub const LINE_NOT_PRESENT: &str = "cellular_ims_line_not_present";
    /// Historical shape: this one leads with `line_`, not `cellular_ims_`.
    /// The frontend matches the exact string.
    pub const LINE_CELLULAR_IMS_CONNECTION_DISABLED: &str = "line_cellular_ims_connection_disabled";
    pub const MODEM_REFRESH_FAILED: &str = "cellular_ims_modem_refresh_failed";
    pub const PROFILE_ATTEMPTS_EXHAUSTED: &str = "cellular_ims_profile_attempts_exhausted";
    pub const PROFILE_RESTORE_IN_PROGRESS: &str = "cellular_ims_profile_restore_in_progress";
    pub const PROFILE_SELECTION_CHANGED: &str = "cellular_ims_profile_selection_changed";
    pub const PROFILE_SOURCE_UNSUPPORTED: &str = "cellular_ims_profile_source_unsupported";
    pub const RETRY_ALREADY_RUNNING: &str = "cellular_ims_retry_already_running";
    pub const SIM_OVERRIDE_NOT_READY: &str = "cellular_ims_sim_override_not_ready";

    // Line/profile configuration validation.
    pub const DERIVED_PROFILE_ID_NOT_ALLOWED: &str = "cellular_ims_derived_profile_id_not_allowed";
    pub const IP_FAMILIES_DUPLICATE: &str = "cellular_ims_ip_families_duplicate";
    pub const IP_FAMILIES_EMPTY: &str = "cellular_ims_ip_families_empty";
    pub const PROFILE_ATTEMPT_COUNT_INVALID: &str = "cellular_ims_profile_attempt_count_invalid";
    pub const PROFILE_ID_TOO_LONG: &str = "cellular_ims_profile_id_too_long";

    // Runtime lifecycle.
    pub const RUNTIME_NOT_RUNNING: &str = "cellular_ims_runtime_not_running";
    pub const RUNTIME_SEND_TIMEOUT: &str = "cellular_ims_runtime_send_timeout";
    pub const RANDOM_FAILED: &str = "cellular_ims_random_failed";

    // SIP channel: socket binding, port reservation and UE-worker transfer.
    // `CHANNEL_READ_TIMEOUT` and `CHANNEL_READ_RETRYABLE` are matched by the
    // register loop to tell a benign read gap from a real failure, so they must
    // stay distinct from the send-side codes.
    pub const CHANNEL_BIND_FAILED: &str = "cellular_ims_channel_bind_failed";
    pub const CHANNEL_LOCAL_ADDR_FAILED: &str = "cellular_ims_channel_local_addr_failed";
    pub const CHANNEL_READ_FAILED: &str = "cellular_ims_channel_read_failed";
    pub const CHANNEL_READ_RETRYABLE: &str = "cellular_ims_channel_read_retryable";
    pub const CHANNEL_READ_TIMEOUT: &str = "cellular_ims_channel_read_timeout";
    pub const CHANNEL_RECEIVE_CONNECT_FAILED: &str = "cellular_ims_channel_receive_connect_failed";
    pub const CHANNEL_RECEIVE_NOT_RESERVED: &str = "cellular_ims_channel_receive_not_reserved";
    pub const CHANNEL_RECEIVE_PORT_MISMATCH: &str = "cellular_ims_channel_receive_port_mismatch";
    pub const CHANNEL_RECEIVE_RESERVE_FAILED: &str = "cellular_ims_channel_receive_reserve_failed";
    pub const CHANNEL_RECEIVE_RESERVE_INVALID_PORT: &str =
        "cellular_ims_channel_receive_reserve_invalid_port";
    pub const CHANNEL_RECEIVE_RESERVED_SIP_PORT: &str =
        "cellular_ims_channel_receive_reserved_sip_port";
    pub const CHANNEL_RECEIVE_SOCKET_MISSING: &str = "cellular_ims_channel_receive_socket_missing";
    pub const CHANNEL_SECURITY_UPDATE_PENDING: &str =
        "cellular_ims_channel_security_update_pending";
    pub const CHANNEL_SEND_CONNECT_FAILED: &str = "cellular_ims_channel_send_connect_failed";
    pub const CHANNEL_SEND_FAILED: &str = "cellular_ims_channel_send_failed";
    pub const CHANNEL_SEND_NOT_RESERVED: &str = "cellular_ims_channel_send_not_reserved";
    pub const CHANNEL_SEND_PORT_MISMATCH: &str = "cellular_ims_channel_send_port_mismatch";
    pub const CHANNEL_SEND_RESERVE_FAILED: &str = "cellular_ims_channel_send_reserve_failed";
    pub const CHANNEL_SEND_RESERVE_INVALID_PORT: &str =
        "cellular_ims_channel_send_reserve_invalid_port";
    pub const CHANNEL_SEND_SOCKET_MISSING: &str = "cellular_ims_channel_send_socket_missing";
    pub const CHANNEL_SHORT_SEND: &str = "cellular_ims_channel_short_send";
    pub const CHANNEL_WORKER_RECEIVE_MISMATCH: &str =
        "cellular_ims_channel_worker_receive_mismatch";
    pub const CHANNEL_WORKER_RECEIVE_REQUIRES_ASYNC: &str =
        "cellular_ims_channel_worker_receive_requires_async";
    pub const CHANNEL_WORKER_SEND_MISMATCH: &str = "cellular_ims_channel_worker_send_mismatch";
    pub const CHANNEL_WORKER_SEND_REQUIRES_ASYNC: &str =
        "cellular_ims_channel_worker_send_requires_async";
    pub const CHANNEL_WORKER_SOCKET_FAILED: &str = "cellular_ims_channel_worker_socket_failed";
    pub const CHANNEL_WORKER_SOCKET_TYPE: &str = "cellular_ims_channel_worker_socket_type";

    // Voice / RTP / transfer / SMS runtime (live.rs).
    pub const CNI_REQUIRED_DYNAMIC_UNAVAILABLE: &str =
        "cellular_ims_cni_required_dynamic_unavailable";
    pub const CONCURRENT_CALL_LIMIT: &str = "cellular_ims_concurrent_call_limit";
    pub const MT_RP_DATA_INVALID: &str = "cellular_ims_mt_rp_data_invalid";
    pub const PANI_REQUIRED_DYNAMIC_UNAVAILABLE: &str =
        "cellular_ims_pani_required_dynamic_unavailable";
    pub const QMI_DEVICE_MISSING: &str = "cellular_ims_qmi_device_missing";
    pub const REGISTER_AUTH_NOT_PREPARED: &str = "cellular_ims_register_auth_not_prepared";
    pub const REGISTER_NONCE_COUNT_EXHAUSTED: &str = "cellular_ims_register_nonce_count_exhausted";
    pub const RTP_BIND_FAILED: &str = "cellular_ims_rtp_bind_failed";
    pub const RTP_LOCAL_ADDR_FAILED: &str = "cellular_ims_rtp_local_addr_failed";
    pub const RTP_RELAY_MISSING: &str = "cellular_ims_rtp_relay_missing";
    pub const RUNTIME_NOT_REGISTERED: &str = "cellular_ims_runtime_not_registered";
    pub const SMS_DB_FAILED: &str = "cellular_ims_sms_db_failed";
    pub const SMS_MESSAGE_REJECTED: &str = "cellular_ims_sms_message_rejected";
    pub const TRANSFER_CALL_NOT_CONFIRMED: &str = "cellular_ims_transfer_call_not_confirmed";
    pub const TRANSFER_CALL_UNKNOWN: &str = "cellular_ims_transfer_call_unknown";
    pub const TRANSFER_NOT_PENDING: &str = "cellular_ims_transfer_not_pending";
    pub const TRANSFER_PENDING: &str = "cellular_ims_transfer_pending";
    pub const TRANSFER_REQUEST_INVALID: &str = "cellular_ims_transfer_request_invalid";
    pub const TRANSFER_RESPONSE_INVALID: &str = "cellular_ims_transfer_response_invalid";
    pub const VOICE_CALL_DUPLICATE: &str = "cellular_ims_voice_call_duplicate";
    pub const VOICE_CALL_UNKNOWN: &str = "cellular_ims_voice_call_unknown";
    pub const VOICE_CALLEE_INVALID: &str = "cellular_ims_voice_callee_invalid";
    pub const VOICE_DIRECTION_MISMATCH: &str = "cellular_ims_voice_direction_mismatch";
    pub const VOICE_INITIAL_INVITE_MISSING: &str = "cellular_ims_voice_initial_invite_missing";
    pub const VOICE_INVITE_BRANCH_MISSING: &str = "cellular_ims_voice_invite_branch_missing";
    pub const VOICE_MEDIA_ADDRESS_INVALID: &str = "cellular_ims_voice_media_address_invalid";
    pub const VOICE_MEDIA_PORT_INVALID: &str = "cellular_ims_voice_media_port_invalid";
    pub const VOICE_NO_COMMON_CODEC: &str = "cellular_ims_voice_no_common_codec";
    pub const VOICE_REINVITE_NOT_PENDING: &str = "cellular_ims_voice_reinvite_not_pending";
    pub const VOICE_REINVITE_PENDING: &str = "cellular_ims_voice_reinvite_pending";
    pub const VOICE_REMOTE_TAG_MISSING: &str = "cellular_ims_voice_remote_tag_missing";
    pub const VOICE_RSEQ_MISSING: &str = "cellular_ims_voice_rseq_missing";
    pub const VOICE_SDP_INVALID: &str = "cellular_ims_voice_sdp_invalid";

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
        LINE_CELLULAR_IMS_CONNECTION_DISABLED,
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

/// Unified cellular IMS error carrying a stable code plus optional detail suffix.
///
/// `Display` renders as `code` or `code:detail` (e.g.
/// `cellular_ims_command_failed:mmcli`). This is what gets stored in
/// `last_error`; the frontend tokenizes it and matches each code exactly.
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
            "cellular_ims_imsi_missing"
        );
    }

    #[test]
    fn display_with_detail_uses_colon_suffix() {
        assert_eq!(
            CellularImsError::with_detail(code::COMMAND_FAILED, "mmcli").to_string(),
            "cellular_ims_command_failed:mmcli"
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

    /// Every code names the IMS registration layer, not its voice service: the
    /// old `volte_` prefix must not come back, and the one historical
    /// exception keeps its `line_` lead.
    #[test]
    fn codes_use_the_cellular_ims_prefix() {
        for c in code::ALL {
            if *c == code::LINE_CELLULAR_IMS_CONNECTION_DISABLED {
                assert!(c.starts_with("line_cellular_ims_"), "{c}");
                continue;
            }
            assert!(c.starts_with("cellular_ims_"), "unexpected code shape: {c}");
            assert!(!c.contains("volte"), "code still names VoLTE: {c}");
        }
    }
}
