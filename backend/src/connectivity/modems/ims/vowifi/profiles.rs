#[cfg(test)]
use chrono::NaiveDate;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;

use crate::connectivity::core::access_network::AccessIdentityPolicy;
#[cfg(test)]
use crate::connectivity::core::voice::AudioCodec;

/// REGISTER `Expires` most carriers accept.
pub const DEFAULT_REGISTER_EXPIRES_SECONDS: u32 = 3600;
/// Access type reported in `P-Access-Network-Info` for Wi-Fi calling.
pub const DEFAULT_ACCESS_NETWORK_INFO: &str = "IEEE-802.11";
/// Status codes that mean "the network is busy or unavailable, try again".
pub const DEFAULT_TEMPORARY_STATUS_CODES: &[u16] = &[408, 429, 500, 502, 503, 504];
/// Status codes that mean "this SIM will not be allowed to register".
pub const DEFAULT_FORBIDDEN_STATUS_CODES: &[u16] = &[403];
/// Status codes that should trigger the initial-reject fallback REGISTER.
pub const DEFAULT_INITIAL_REJECT_FALLBACK_STATUS_CODES: &[u16] = &[400, 403, 500];
/// Delay before retrying after a temporary rejection.
pub const DEFAULT_TEMPORARY_RETRY_SECONDS: u16 = 60;
/// SIP TCP keepalive that keeps NAT bindings alive on the ePDG path.
pub const DEFAULT_IMS_TCP_KEEPALIVE_SECONDS: u16 = 30;
/// SIP OPTIONS ping interval used to confirm the registration is still live.
pub const DEFAULT_IMS_OPTIONS_PING_INTERVAL_SECONDS: u16 = 45;

#[cfg(test)]
const TEST_ALLOW_METHODS: &str =
    "INVITE,ACK,CANCEL,BYE,UPDATE,PRACK,MESSAGE,REFER,NOTIFY,INFO,OPTIONS";

#[cfg(test)]
const TEST_VISITED_NETWORK_HEADER: &str = "\"legacy-test-profile\"";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CarrierProfileMeta {
    pub profile_id: &'static str,
    pub mcc: &'static str,
    pub mnc: &'static str,
    pub mnc_len: u8,
    pub plmn: &'static str,
    pub country_iso2: &'static str,
    pub brand: &'static str,
    pub operator_legal_name: &'static str,
    pub aliases: &'static [&'static str],
    pub source_refs: &'static [&'static str],
    pub last_verified: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProfileIdentityPolicy {
    pub device_model_hint: &'static str,
    pub spoof_imei: bool,
    /// Whether a device identity (IMEI) is presented during IKE_AUTH. Some
    /// carriers refuse the exchange when the identity is missing or unknown, so
    /// this has to be settable per carrier.
    pub device_identity_enabled: bool,
    /// The IMEI to present. `None` means "use the modem's own IMEI".
    pub device_identity_imei: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EpdgPolicy {
    pub host: &'static str,
    pub port: u16,
    pub apn: Option<&'static str>,
    pub ip_stack: &'static str,
    /// Primary DNS override. Kept as the first entry of `dns_servers`; retained
    /// as its own field because several call sites want a single answer.
    pub dns_server: Option<&'static str>,
    /// Ordered DNS servers to try when resolving the ePDG. Later entries are
    /// used when an earlier one does not answer — if the ePDG FQDN cannot be
    /// resolved there is no connection at all, so a single server is a real
    /// single point of failure.
    pub dns_servers: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Ikev2Policy {
    pub nat_keepalive_seconds: u16,
    pub dpd_interval_seconds: u16,
    pub reauth_interval_seconds: Option<u16>,
    pub ike_proposals: &'static [&'static str],
    pub esp_proposals: &'static [&'static str],
    pub aka_challenge_mode: &'static str,
    pub include_epdg_idr: bool,
    /// Optional RFC822 IDi template used for EAP-AKA. Public PLMNs may omit
    /// this and use the standards-derived permanent NAI. Private PLMNs (MCC
    /// 999) must provide their deployment's real template because a public
    /// `3gppnetwork.org` realm cannot be inferred for them.
    ///
    /// Supported runtime placeholders: `{imsi}`, `{mcc}`, `{mnc}`, `{mnc3}`,
    /// `{plmn}`, `{epdg_fqdn}`, `{ims_domain}`, and `{ims_realm}`.
    pub identity_template: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RegisterPolicy {
    pub supported_header: &'static str,
    pub request_uri_policy: &'static str,
    pub include_pani_initial: bool,
    pub include_pani_authenticated: bool,
    /// `none`, `aka_empty`, `digest_empty`, or `implementation_variant`.
    pub initial_authorization: &'static str,
    pub include_mmtel_features: bool,
    pub include_route_header: bool,
    pub include_visited_network: bool,
    pub include_p_preferred_identity: bool,
    pub visited_network_header: Option<&'static str>,
    pub allow_methods: Option<&'static str>,
    pub strict_security_server_offer: bool,
    pub enable_initial_reject_fallback: bool,
    pub use_plain_digest_placeholder: bool,
    /// Legacy two-state sec-agree switch. `sec_agree_mode` supersedes it and is
    /// what the runtime consults; this stays so existing profiles keep working.
    pub require_sec_agree_headers: bool,
    pub proxy_require_sec_agree_headers: bool,
    /// `auto` (follow the challenge), `required` (always send Security-Client /
    /// Security-Verify) or `disabled`. A sec-agree mismatch makes REGISTER fail
    /// outright, and carriers genuinely differ, so this must be settable.
    pub sec_agree_mode: &'static str,
    pub security_client_mechanisms: &'static [&'static str],
    pub live_header_variant_set: &'static str,
    /// Value of the REGISTER `Expires` header. Some carriers reject the common
    /// 3600 default and demand their own value.
    pub expires_seconds: u32,
    /// Base/static value of `P-Access-Network-Info`, e.g. `IEEE-802.11` or
    /// `3GPP-E-UTRAN-FDD`. The source policy below decides whether this value,
    /// a real serving-cell identity, or no header is emitted.
    pub access_network_info: &'static str,
    pub pani_identity_policy: AccessIdentityPolicy,
    /// Optional static `Cellular-Network-Info` value. This is deliberately
    /// separate from the WLAN PANI so VoWiFi never reuses a Wi-Fi string as a
    /// cellular identity.
    pub cellular_network_info: Option<&'static str>,
    pub cni_identity_policy: AccessIdentityPolicy,
    /// `android_default` or `legacy` — controls the shape of the Contact header.
    pub contact_mode: &'static str,
    /// Order of Contact header parameters. Empty means "use the built-in order
    /// for `contact_mode`".
    pub contact_param_order: &'static [&'static str],
    /// Always add `+sip.instance="<urn:uuid:...>"` and the access leg's `reg-id`
    /// to the REGISTER Contact (RFC 5626). The value is per access leg
    /// (`ImsAccess::reg_id`), never a literal `1`, because both legs share one
    /// instance id and a binding is keyed on (AOR, instance-id, reg-id).
    /// Mirrors `sip.common.register.always_add_sip_instance`.
    pub always_add_sip_instance: bool,
    /// Send a `Cellular-Network-Info` header with the REGISTER. Mirrors
    /// `sip.common.register.enable_cellular_network_info`.
    pub enable_cellular_network_info: bool,
    /// SIP status codes treated as retryable-later rather than fatal.
    pub temporary_status_codes: &'static [u16],
    /// SIP status codes that mean "stop, this will never succeed".
    pub forbidden_status_codes: &'static [u16],
    /// Status codes that should trigger the initial-reject fallback path.
    pub initial_reject_fallback_status_codes: &'static [u16],
    /// How long to wait before retrying after a temporary rejection.
    pub temporary_retry_seconds: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ImsPolicy {
    pub domain: &'static str,
    pub realm: &'static str,
    pub registrar: Option<&'static str>,
    pub pcscf: Option<&'static str>,
    /// `tcp`, `udp`, or `auto` to follow whatever the P-CSCF offers.
    pub transport: &'static str,
    pub local_port: u16,
    pub user_agent: &'static str,
    pub identity_source: &'static str,
    /// TCP keepalive for the SIP control channel. Without it, NAT along the
    /// path silently drops the registration and inbound calls stop arriving.
    /// Zero disables.
    pub tcp_keepalive_seconds: u16,
    /// Interval for SIP OPTIONS pings that keep the registration fresh. Zero
    /// disables.
    pub options_ping_interval_seconds: u16,
    pub register: RegisterPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SmsPolicy {
    pub receiver_transport: &'static str,
    pub smsc_auth_required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct E911Policy {
    pub enabled: bool,
    pub provider: Option<&'static str>,
    pub entitlement_url: Option<&'static str>,
    pub websheet_host_policy: Option<&'static str>,
}

/// Carrier-owned supplementary-service transport policy. XCAP endpoints are
/// deliberately opt-in: guessing a public URL or reusing an IMS registrar
/// would send subscriber state to the wrong service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct UtPolicy {
    pub enabled: bool,
    pub xcap_root: Option<&'static str>,
    pub document_selector: Option<&'static str>,
    pub namespace: Option<&'static str>,
    /// Currently `digest_aka` or `none`. Unsupported values are rejected
    /// while importing a catalog profile rather than at request time.
    pub authentication: &'static str,
    /// Partial XCAP writes are opt-in and require an explicit selector for the
    /// document being changed. Missing selectors retain full-document PUT.
    pub partial_update: bool,
    pub call_waiting_selector: Option<&'static str>,
    pub diversion_rule_selector: Option<&'static str>,
    pub oip_selector: Option<&'static str>,
    pub oir_selector: Option<&'static str>,
    /// TLS is always certificate and hostname verified. Catalog policy may
    /// narrow the protocol range or replace public roots with a carrier CA.
    pub tls_min_version: &'static str,
    pub tls_max_version: &'static str,
    pub tls_builtin_roots: bool,
    pub tls_additional_ca_pem: Option<&'static str>,
}

#[cfg(test)]
pub const DEFAULT_UT_POLICY: UtPolicy = UtPolicy {
    enabled: false,
    xcap_root: None,
    document_selector: None,
    namespace: None,
    authentication: "none",
    partial_update: false,
    call_waiting_selector: None,
    diversion_rule_selector: None,
    oip_selector: None,
    oir_selector: None,
    tls_min_version: "1.2",
    tls_max_version: "1.3",
    tls_builtin_roots: true,
    tls_additional_ca_pem: None,
};

/// Voice-calling policy for a carrier: which legs are usable, the media codec
/// preference, and AMR framing parameters. This mirrors [`SmsPolicy`] and drives
/// the voice state machine + SDP offer builder in `voice.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct VoiceCodecPolicy {
    pub codec: &'static str,
    pub payload_type: Option<u8>,
    pub sample_rate: Option<u32>,
    pub fmtp: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct VoicePolicy {
    /// Whether the self-authored VoWiFi voice leg (INVITE/SDP/AMR/RTP over
    /// ePDG) is enabled for this carrier.
    pub vowifi_enabled: bool,
    /// Whether the operator (AT + USB-Audio) fallback leg may be attempted.
    /// The leg is only used at runtime when USB-Audio is actually present.
    pub carrier_fallback_enabled: bool,
    /// Preferred audio codecs in priority order. Tokens: "evs", "amr",
    /// "amr-wb", "pcmu", "pcma".
    pub preferred_codecs: &'static [&'static str],
    /// Carrier-supplied payload type, sample-rate and fmtp policy. An empty
    /// slice keeps the legacy token-only offer behavior.
    pub codec_policies: &'static [VoiceCodecPolicy],
    /// Whether AMR payloads should be offered octet-aligned (`octet-align=1`).
    pub amr_octet_align: bool,
    /// Packetization time (ms) advertised in the SDP offer.
    pub ptime_ms: u16,
    /// Whether an outward standard SIP endpoint may be exposed per SIM (the
    /// external Asterisk/Linphone integration seam). Off by default.
    pub sip_endpoint_exposed: bool,
    /// Carrier fallback when the SIM does not expose EF-MBDN/AT+CSVM.
    pub voicemail_number: Option<&'static str>,
}

/// A sensible default voice policy: VoWiFi leg on, carrier fallback allowed
/// (gated by USB-Audio at runtime), AMR + AMR-WB + G.711 offered, no outward
/// SIP endpoint. Used only by the explicit profiles compiled for tests.
#[cfg(test)]
pub const DEFAULT_VOICE_POLICY: VoicePolicy = VoicePolicy {
    vowifi_enabled: true,
    carrier_fallback_enabled: true,
    preferred_codecs: &["amr-wb", "amr", "pcmu", "pcma"],
    codec_policies: &[],
    amr_octet_align: false,
    ptime_ms: 20,
    sip_endpoint_exposed: false,
    voicemail_number: None,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CarrierProfile {
    pub meta: CarrierProfileMeta,
    pub identity: ProfileIdentityPolicy,
    pub epdg: EpdgPolicy,
    pub ikev2: Ikev2Policy,
    pub ims: ImsPolicy,
    pub sms: SmsPolicy,
    pub voice: VoicePolicy,
    pub e911: E911Policy,
    pub ut: UtPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarrierMatch {
    pub profile: &'static CarrierProfile,
    pub matched_prefix: String,
}

/// Access leg for a conservative profile derived only from public 3GPP naming
/// rules. LTE and Wi-Fi use different identifiers and P-Access-Network-Info
/// values so a generated profile can never cross the two registration paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Standard3gppAccess {
    LteEpc,
    WifiEpdg,
}

impl Standard3gppAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LteEpc => "lte",
            Self::WifiEpdg => "vowifi",
        }
    }

    fn access_network_info(self) -> &'static str {
        match self {
            Self::LteEpc => "3GPP-E-UTRAN-FDD",
            Self::WifiEpdg => DEFAULT_ACCESS_NETWORK_INFO,
        }
    }
}

#[cfg(test)]
pub static GB_EE_23433: CarrierProfile = CarrierProfile {
    meta: CarrierProfileMeta {
        profile_id: "gb_ee_23433",
        mcc: "234",
        mnc: "33",
        mnc_len: 2,
        plmn: "23433",
        country_iso2: "gb",
        brand: "EE",
        operator_legal_name: "EE Limited",
        aliases: &["Orange UK", "Everything Everywhere"],
        source_refs: &[
            "https://www.itu.int/",
            "https://www.gsma.com/",
            "3GPP 3GPPnetwork domain rules",
            "SimAdmin stage-1 black-box evidence (2026-06-17)",
        ],
        last_verified: "2026-06-17",
    },
    identity: ProfileIdentityPolicy {
        device_model_hint: "rmx3366",
        spoof_imei: false,
        device_identity_enabled: false,
        device_identity_imei: None,
    },
    epdg: EpdgPolicy {
        host: "epdg.epc.mnc033.mcc234.pub.3gppnetwork.org",
        port: 500,
        apn: Some("ims"),
        ip_stack: "ipv6",
        dns_server: None,
        dns_servers: &[],
    },
    ikev2: Ikev2Policy {
        nat_keepalive_seconds: 20,
        dpd_interval_seconds: 600,
        reauth_interval_seconds: None,
        ike_proposals: &[
            "aes128-sha256-modp2048",
            "aes128-sha256-prfsha1-modp2048",
            "aes128-sha1-modp2048",
            "aes256-sha256-prfsha1-modp2048",
            "aes256-sha256-modp2048",
            "aes128-sha256-prfsha1-modp1024",
            "aes128-sha256-modp1024",
            "aes128-sha1-modp1024",
            "aes256-sha256-prfsha1-modp1024",
            "aes256-sha1-modp1024",
            "aes256-sha512-prfsha512-modp2048",
            "aes256-sha512-prfsha512-modp1024",
        ],
        esp_proposals: &["aes128-sha256", "aes128-sha1", "aes256-sha512"],
        aka_challenge_mode: "standard",
        include_epdg_idr: true,
        identity_template: None,
    },
    ims: ImsPolicy {
        domain: "ims.mnc033.mcc234.3gppnetwork.org",
        realm: "ims.mnc033.mcc234.3gppnetwork.org",
        registrar: None,
        pcscf: None,
        transport: "tcp",
        local_port: 5060,
        user_agent: "SimAdmin VoWiFi",
        identity_source: "carrier_device_model",
        tcp_keepalive_seconds: DEFAULT_IMS_TCP_KEEPALIVE_SECONDS,
        options_ping_interval_seconds: DEFAULT_IMS_OPTIONS_PING_INTERVAL_SECONDS,
        register: RegisterPolicy {
            supported_header: "path,sec-agree,gruu",
            request_uri_policy: "home_domain",
            include_pani_initial: true,
            include_pani_authenticated: true,
            initial_authorization: "aka_empty",
            include_mmtel_features: true,
            include_route_header: true,
            include_visited_network: true,
            include_p_preferred_identity: true,
            visited_network_header: Some(TEST_VISITED_NETWORK_HEADER),
            allow_methods: Some(TEST_ALLOW_METHODS),
            strict_security_server_offer: true,
            enable_initial_reject_fallback: false,
            use_plain_digest_placeholder: false,
            require_sec_agree_headers: false,
            proxy_require_sec_agree_headers: false,
            sec_agree_mode: "auto",
            expires_seconds: DEFAULT_REGISTER_EXPIRES_SECONDS,
            access_network_info: DEFAULT_ACCESS_NETWORK_INFO,
            pani_identity_policy: AccessIdentityPolicy::Static,
            cellular_network_info: None,
            cni_identity_policy: AccessIdentityPolicy::Omit,
            contact_mode: "android_default",
            contact_param_order: &[],
            temporary_status_codes: DEFAULT_TEMPORARY_STATUS_CODES,
            forbidden_status_codes: DEFAULT_FORBIDDEN_STATUS_CODES,
            initial_reject_fallback_status_codes: DEFAULT_INITIAL_REJECT_FALLBACK_STATUS_CODES,
            temporary_retry_seconds: DEFAULT_TEMPORARY_RETRY_SECONDS,
            always_add_sip_instance: false,
            enable_cellular_network_info: false,
            security_client_mechanisms: &["hmac-sha-1-96/aes-cbc/esp/trans"],
            live_header_variant_set: "ee_ims_features",
        },
    },
    sms: SmsPolicy {
        receiver_transport: "tcp",
        smsc_auth_required: false,
    },
    voice: DEFAULT_VOICE_POLICY,
    e911: E911Policy {
        enabled: false,
        provider: None,
        entitlement_url: None,
        websheet_host_policy: None,
    },
    ut: DEFAULT_UT_POLICY,
};

#[cfg(test)]
pub static NL_VODAFONE_20404: CarrierProfile = CarrierProfile {
    meta: CarrierProfileMeta {
        profile_id: "nl_vodafone_20404",
        mcc: "204",
        mnc: "04",
        mnc_len: 2,
        plmn: "20404",
        country_iso2: "nl",
        brand: "Vodafone",
        operator_legal_name: "Vodafone Libertel B.V.",
        aliases: &["vodafone NL"],
        source_refs: &[
            "https://www.itu.int/",
            "https://www.gsma.com/",
            "public carrier interop matrix",
        ],
        last_verified: "2026-06-17",
    },
    identity: ProfileIdentityPolicy {
        device_model_hint: "generic_android_class",
        spoof_imei: false,
        device_identity_enabled: false,
        device_identity_imei: None,
    },
    epdg: EpdgPolicy {
        host: "epdg.epc.mnc004.mcc204.pub.3gppnetwork.org",
        port: 500,
        apn: Some("ims"),
        ip_stack: "ipv4v6",
        dns_server: None,
        dns_servers: &[],
    },
    ikev2: Ikev2Policy {
        nat_keepalive_seconds: 20,
        dpd_interval_seconds: 600,
        reauth_interval_seconds: None,
        ike_proposals: &["aes256-sha256-prfsha512-modp2048"],
        esp_proposals: &["aes256-sha256"],
        aka_challenge_mode: "standard",
        include_epdg_idr: true,
        identity_template: None,
    },
    ims: ImsPolicy {
        domain: "ims.mnc004.mcc204.3gppnetwork.org",
        realm: "ims.mnc004.mcc204.3gppnetwork.org",
        registrar: None,
        pcscf: None,
        transport: "tcp",
        local_port: 5060,
        user_agent: "SimAdmin VoWiFi",
        identity_source: "isim",
        tcp_keepalive_seconds: DEFAULT_IMS_TCP_KEEPALIVE_SECONDS,
        options_ping_interval_seconds: DEFAULT_IMS_OPTIONS_PING_INTERVAL_SECONDS,
        register: RegisterPolicy {
            supported_header: "path,sec-agree,gruu",
            request_uri_policy: "home_domain",
            include_pani_initial: true,
            include_pani_authenticated: true,
            initial_authorization: "aka_empty",
            include_mmtel_features: true,
            include_route_header: true,
            include_visited_network: true,
            include_p_preferred_identity: true,
            visited_network_header: Some(TEST_VISITED_NETWORK_HEADER),
            allow_methods: Some(TEST_ALLOW_METHODS),
            strict_security_server_offer: true,
            enable_initial_reject_fallback: false,
            use_plain_digest_placeholder: false,
            require_sec_agree_headers: true,
            proxy_require_sec_agree_headers: true,
            sec_agree_mode: "auto",
            expires_seconds: DEFAULT_REGISTER_EXPIRES_SECONDS,
            access_network_info: DEFAULT_ACCESS_NETWORK_INFO,
            pani_identity_policy: AccessIdentityPolicy::Static,
            cellular_network_info: None,
            cni_identity_policy: AccessIdentityPolicy::Omit,
            contact_mode: "android_default",
            contact_param_order: &[],
            temporary_status_codes: DEFAULT_TEMPORARY_STATUS_CODES,
            forbidden_status_codes: DEFAULT_FORBIDDEN_STATUS_CODES,
            initial_reject_fallback_status_codes: DEFAULT_INITIAL_REJECT_FALLBACK_STATUS_CODES,
            temporary_retry_seconds: DEFAULT_TEMPORARY_RETRY_SECONDS,
            always_add_sip_instance: false,
            enable_cellular_network_info: false,
            security_client_mechanisms: &["hmac-sha-1-96/aes-cbc/esp/trans"],
            live_header_variant_set: "standard_ims_features",
        },
    },
    sms: SmsPolicy {
        receiver_transport: "tcp",
        smsc_auth_required: false,
    },
    voice: DEFAULT_VOICE_POLICY,
    e911: E911Policy {
        enabled: false,
        provider: None,
        entitlement_url: None,
        websheet_host_policy: None,
    },
    ut: DEFAULT_UT_POLICY,
};

#[cfg(test)]
#[cfg(test)]
pub static US_TMOBILE_310260: CarrierProfile = CarrierProfile {
    meta: CarrierProfileMeta {
        profile_id: "us_tmobile_310260",
        mcc: "310",
        mnc: "260",
        mnc_len: 3,
        plmn: "310260",
        country_iso2: "us",
        brand: "T-Mobile",
        operator_legal_name: "T-Mobile USA, Inc.",
        aliases: &["T-Mobile US"],
        source_refs: &["https://www.itu.int/", "https://www.gsma.com/"],
        last_verified: "2026-06-17",
    },
    identity: ProfileIdentityPolicy {
        device_model_hint: "generic_android_class",
        spoof_imei: false,
        device_identity_enabled: false,
        device_identity_imei: None,
    },
    epdg: EpdgPolicy {
        host: "epdg.epc.mnc260.mcc310.pub.3gppnetwork.org",
        port: 500,
        apn: Some("ims"),
        ip_stack: "ipv4v6",
        dns_server: None,
        dns_servers: &[],
    },
    ikev2: Ikev2Policy {
        nat_keepalive_seconds: 20,
        dpd_interval_seconds: 600,
        reauth_interval_seconds: None,
        ike_proposals: &["aes128-sha256-modp2048"],
        esp_proposals: &["aes128-sha256", "aes128-sha1"],
        aka_challenge_mode: "standard",
        include_epdg_idr: true,
        identity_template: None,
    },
    ims: ImsPolicy {
        domain: "ims.mnc260.mcc310.3gppnetwork.org",
        realm: "ims.mnc260.mcc310.3gppnetwork.org",
        registrar: None,
        pcscf: None,
        transport: "tcp",
        local_port: 5060,
        user_agent: "SimAdmin VoWiFi",
        identity_source: "isim",
        tcp_keepalive_seconds: DEFAULT_IMS_TCP_KEEPALIVE_SECONDS,
        options_ping_interval_seconds: DEFAULT_IMS_OPTIONS_PING_INTERVAL_SECONDS,
        register: RegisterPolicy {
            supported_header: "path,sec-agree,gruu",
            request_uri_policy: "home_domain",
            include_pani_initial: true,
            include_pani_authenticated: true,
            initial_authorization: "aka_empty",
            include_mmtel_features: true,
            include_route_header: true,
            include_visited_network: true,
            include_p_preferred_identity: true,
            visited_network_header: Some(TEST_VISITED_NETWORK_HEADER),
            allow_methods: Some(TEST_ALLOW_METHODS),
            strict_security_server_offer: true,
            enable_initial_reject_fallback: false,
            use_plain_digest_placeholder: false,
            require_sec_agree_headers: true,
            proxy_require_sec_agree_headers: true,
            sec_agree_mode: "auto",
            expires_seconds: DEFAULT_REGISTER_EXPIRES_SECONDS,
            access_network_info: DEFAULT_ACCESS_NETWORK_INFO,
            pani_identity_policy: AccessIdentityPolicy::Static,
            cellular_network_info: None,
            cni_identity_policy: AccessIdentityPolicy::Omit,
            contact_mode: "android_default",
            contact_param_order: &[],
            temporary_status_codes: DEFAULT_TEMPORARY_STATUS_CODES,
            forbidden_status_codes: DEFAULT_FORBIDDEN_STATUS_CODES,
            initial_reject_fallback_status_codes: DEFAULT_INITIAL_REJECT_FALLBACK_STATUS_CODES,
            temporary_retry_seconds: DEFAULT_TEMPORARY_RETRY_SECONDS,
            always_add_sip_instance: false,
            enable_cellular_network_info: false,
            security_client_mechanisms: &["hmac-sha-1-96/aes-cbc/esp/trans"],
            live_header_variant_set: "standard_ims_features",
        },
    },
    sms: SmsPolicy {
        receiver_transport: "tcp",
        smsc_auth_required: false,
    },
    voice: DEFAULT_VOICE_POLICY,
    e911: E911Policy {
        enabled: true,
        provider: Some("tmobile_entitlement"),
        entitlement_url: Some("https://eas3.msg.t-mobile.com/"),
        websheet_host_policy: Some("public_https"),
    },
    ut: DEFAULT_UT_POLICY,
};

#[cfg(test)]
#[cfg(test)]
pub static US_ATT_310410: CarrierProfile = CarrierProfile {
    meta: CarrierProfileMeta {
        profile_id: "us_att_310410",
        mcc: "310",
        mnc: "410",
        mnc_len: 3,
        plmn: "310410",
        country_iso2: "us",
        brand: "AT&T",
        operator_legal_name: "AT&T Mobility LLC",
        aliases: &["AT&T", "AT&T MVNO path"],
        source_refs: &["https://www.itu.int/", "https://www.gsma.com/"],
        last_verified: "2026-06-17",
    },
    identity: ProfileIdentityPolicy {
        device_model_hint: "generic_android_class",
        spoof_imei: false,
        device_identity_enabled: false,
        device_identity_imei: None,
    },
    epdg: EpdgPolicy {
        host: "epdg.epc.att.net",
        port: 500,
        apn: Some("ims"),
        ip_stack: "ipv4v6",
        dns_server: None,
        dns_servers: &[],
    },
    ikev2: Ikev2Policy {
        nat_keepalive_seconds: 20,
        dpd_interval_seconds: 600,
        reauth_interval_seconds: None,
        ike_proposals: &["aes128-sha256-modp2048"],
        esp_proposals: &["aes128-sha256"],
        aka_challenge_mode: "standard",
        include_epdg_idr: true,
        identity_template: None,
    },
    ims: ImsPolicy {
        domain: "ims.mnc410.mcc310.3gppnetwork.org",
        realm: "ims.mnc410.mcc310.3gppnetwork.org",
        registrar: None,
        pcscf: None,
        transport: "tcp",
        local_port: 5060,
        user_agent: "SimAdmin VoWiFi",
        identity_source: "isim",
        tcp_keepalive_seconds: DEFAULT_IMS_TCP_KEEPALIVE_SECONDS,
        options_ping_interval_seconds: DEFAULT_IMS_OPTIONS_PING_INTERVAL_SECONDS,
        register: RegisterPolicy {
            supported_header: "path,sec-agree,gruu",
            request_uri_policy: "home_domain",
            include_pani_initial: true,
            include_pani_authenticated: true,
            initial_authorization: "aka_empty",
            include_mmtel_features: true,
            include_route_header: true,
            include_visited_network: true,
            include_p_preferred_identity: true,
            visited_network_header: Some(TEST_VISITED_NETWORK_HEADER),
            allow_methods: Some(TEST_ALLOW_METHODS),
            strict_security_server_offer: true,
            enable_initial_reject_fallback: true,
            use_plain_digest_placeholder: false,
            require_sec_agree_headers: true,
            proxy_require_sec_agree_headers: true,
            sec_agree_mode: "auto",
            expires_seconds: DEFAULT_REGISTER_EXPIRES_SECONDS,
            access_network_info: DEFAULT_ACCESS_NETWORK_INFO,
            pani_identity_policy: AccessIdentityPolicy::Static,
            cellular_network_info: None,
            cni_identity_policy: AccessIdentityPolicy::Omit,
            contact_mode: "android_default",
            contact_param_order: &[],
            temporary_status_codes: DEFAULT_TEMPORARY_STATUS_CODES,
            forbidden_status_codes: DEFAULT_FORBIDDEN_STATUS_CODES,
            initial_reject_fallback_status_codes: DEFAULT_INITIAL_REJECT_FALLBACK_STATUS_CODES,
            temporary_retry_seconds: DEFAULT_TEMPORARY_RETRY_SECONDS,
            always_add_sip_instance: false,
            enable_cellular_network_info: false,
            security_client_mechanisms: &["hmac-sha-1-96/aes-cbc/esp/trans"],
            live_header_variant_set: "standard_ims_features",
        },
    },
    sms: SmsPolicy {
        receiver_transport: "tcp",
        smsc_auth_required: false,
    },
    voice: DEFAULT_VOICE_POLICY,
    e911: E911Policy {
        enabled: true,
        provider: Some("att_entitlement"),
        entitlement_url: Some("https://sentitlement2.mobile.att.net/"),
        websheet_host_policy: Some("public_https"),
    },
    ut: DEFAULT_UT_POLICY,
};

#[cfg(test)]
#[cfg(test)]
pub static DE_O2_26207: CarrierProfile = CarrierProfile {
    meta: CarrierProfileMeta {
        profile_id: "de_o2_26207",
        mcc: "262",
        mnc: "07",
        mnc_len: 2,
        plmn: "26207",
        country_iso2: "de",
        brand: "O2",
        operator_legal_name: "Telefonica Germany GmbH & Co. OHG",
        aliases: &["O2 Germany", "Telefonica"],
        source_refs: &["https://www.itu.int/", "https://www.gsma.com/"],
        last_verified: "2026-06-17",
    },
    identity: ProfileIdentityPolicy {
        device_model_hint: "iphone15,4_like",
        spoof_imei: false,
        device_identity_enabled: false,
        device_identity_imei: None,
    },
    epdg: EpdgPolicy {
        host: "epdg.epc.mnc007.mcc262.pub.3gppnetwork.org",
        port: 500,
        apn: Some("ims"),
        ip_stack: "ipv4v6",
        dns_server: None,
        dns_servers: &[],
    },
    ikev2: Ikev2Policy {
        nat_keepalive_seconds: 20,
        dpd_interval_seconds: 600,
        reauth_interval_seconds: None,
        ike_proposals: &["aes256-sha256-prfsha1-modp2048"],
        esp_proposals: &["aes256-sha256"],
        aka_challenge_mode: "standard",
        include_epdg_idr: true,
        identity_template: None,
    },
    ims: ImsPolicy {
        domain: "ims.mnc007.mcc262.3gppnetwork.org",
        realm: "ims.mnc007.mcc262.3gppnetwork.org",
        registrar: None,
        pcscf: None,
        transport: "tcp",
        local_port: 5060,
        user_agent: "SimAdmin VoWiFi",
        identity_source: "isim",
        tcp_keepalive_seconds: DEFAULT_IMS_TCP_KEEPALIVE_SECONDS,
        options_ping_interval_seconds: DEFAULT_IMS_OPTIONS_PING_INTERVAL_SECONDS,
        register: RegisterPolicy {
            supported_header: "path,sec-agree,gruu",
            request_uri_policy: "home_domain",
            include_pani_initial: true,
            include_pani_authenticated: true,
            initial_authorization: "aka_empty",
            include_mmtel_features: true,
            include_route_header: true,
            include_visited_network: true,
            include_p_preferred_identity: true,
            visited_network_header: Some(TEST_VISITED_NETWORK_HEADER),
            allow_methods: Some(TEST_ALLOW_METHODS),
            strict_security_server_offer: true,
            enable_initial_reject_fallback: true,
            use_plain_digest_placeholder: false,
            require_sec_agree_headers: true,
            proxy_require_sec_agree_headers: true,
            sec_agree_mode: "auto",
            expires_seconds: DEFAULT_REGISTER_EXPIRES_SECONDS,
            access_network_info: DEFAULT_ACCESS_NETWORK_INFO,
            pani_identity_policy: AccessIdentityPolicy::Static,
            cellular_network_info: None,
            cni_identity_policy: AccessIdentityPolicy::Omit,
            contact_mode: "android_default",
            contact_param_order: &[],
            temporary_status_codes: DEFAULT_TEMPORARY_STATUS_CODES,
            forbidden_status_codes: DEFAULT_FORBIDDEN_STATUS_CODES,
            initial_reject_fallback_status_codes: DEFAULT_INITIAL_REJECT_FALLBACK_STATUS_CODES,
            temporary_retry_seconds: DEFAULT_TEMPORARY_RETRY_SECONDS,
            always_add_sip_instance: false,
            enable_cellular_network_info: false,
            security_client_mechanisms: &["hmac-sha-1-96/aes-cbc/esp/trans"],
            live_header_variant_set: "standard_ims_features",
        },
    },
    sms: SmsPolicy {
        receiver_transport: "tcp",
        smsc_auth_required: false,
    },
    voice: DEFAULT_VOICE_POLICY,
    e911: E911Policy {
        enabled: false,
        provider: None,
        entitlement_url: None,
        websheet_host_policy: None,
    },
    ut: DEFAULT_UT_POLICY,
};

#[cfg(test)]
#[cfg(test)]
pub static NZ_SPARK_53005: CarrierProfile = CarrierProfile {
    meta: CarrierProfileMeta {
        profile_id: "nz_spark_53005",
        mcc: "530",
        mnc: "05",
        mnc_len: 2,
        plmn: "53005",
        country_iso2: "nz",
        brand: "Spark",
        operator_legal_name: "Spark New Zealand Trading Limited",
        aliases: &["Spark NZ"],
        source_refs: &["https://www.itu.int/", "https://www.gsma.com/"],
        last_verified: "2026-06-17",
    },
    identity: ProfileIdentityPolicy {
        device_model_hint: "iphone15,4_like",
        spoof_imei: false,
        device_identity_enabled: false,
        device_identity_imei: None,
    },
    epdg: EpdgPolicy {
        host: "epdg.epc.mnc005.mcc530.pub.3gppnetwork.spark.co.nz",
        port: 500,
        apn: Some("ims"),
        ip_stack: "ipv4v6",
        dns_server: None,
        dns_servers: &[],
    },
    ikev2: Ikev2Policy {
        nat_keepalive_seconds: 20,
        dpd_interval_seconds: 600,
        reauth_interval_seconds: None,
        ike_proposals: &["aes256-sha256-prfsha256-modp2048"],
        esp_proposals: &["aes256-sha256"],
        aka_challenge_mode: "standard",
        include_epdg_idr: true,
        identity_template: None,
    },
    ims: ImsPolicy {
        domain: "ims.mnc005.mcc530.3gppnetwork.org",
        realm: "ims.mnc005.mcc530.3gppnetwork.org",
        registrar: None,
        pcscf: None,
        transport: "tcp",
        local_port: 5060,
        user_agent: "SimAdmin VoWiFi",
        identity_source: "isim",
        tcp_keepalive_seconds: DEFAULT_IMS_TCP_KEEPALIVE_SECONDS,
        options_ping_interval_seconds: DEFAULT_IMS_OPTIONS_PING_INTERVAL_SECONDS,
        register: RegisterPolicy {
            supported_header: "path,sec-agree,gruu",
            request_uri_policy: "home_domain",
            include_pani_initial: true,
            include_pani_authenticated: true,
            initial_authorization: "aka_empty",
            include_mmtel_features: true,
            include_route_header: true,
            include_visited_network: true,
            include_p_preferred_identity: true,
            visited_network_header: Some(TEST_VISITED_NETWORK_HEADER),
            allow_methods: Some(TEST_ALLOW_METHODS),
            strict_security_server_offer: true,
            enable_initial_reject_fallback: false,
            use_plain_digest_placeholder: false,
            require_sec_agree_headers: true,
            proxy_require_sec_agree_headers: true,
            sec_agree_mode: "auto",
            expires_seconds: DEFAULT_REGISTER_EXPIRES_SECONDS,
            access_network_info: DEFAULT_ACCESS_NETWORK_INFO,
            pani_identity_policy: AccessIdentityPolicy::Static,
            cellular_network_info: None,
            cni_identity_policy: AccessIdentityPolicy::Omit,
            contact_mode: "android_default",
            contact_param_order: &[],
            temporary_status_codes: DEFAULT_TEMPORARY_STATUS_CODES,
            forbidden_status_codes: DEFAULT_FORBIDDEN_STATUS_CODES,
            initial_reject_fallback_status_codes: DEFAULT_INITIAL_REJECT_FALLBACK_STATUS_CODES,
            temporary_retry_seconds: DEFAULT_TEMPORARY_RETRY_SECONDS,
            always_add_sip_instance: false,
            enable_cellular_network_info: false,
            security_client_mechanisms: &["hmac-sha-1-96/aes-cbc/esp/trans"],
            live_header_variant_set: "standard_ims_features",
        },
    },
    sms: SmsPolicy {
        receiver_transport: "tcp",
        smsc_auth_required: false,
    },
    voice: DEFAULT_VOICE_POLICY,
    e911: E911Policy {
        enabled: false,
        provider: None,
        entitlement_url: None,
        websheet_host_policy: None,
    },
    ut: DEFAULT_UT_POLICY,
};

#[cfg(test)]
pub static BUILTIN_PROFILES: &[CarrierProfile] = &[
    GB_EE_23433,
    NL_VODAFONE_20404,
    US_TMOBILE_310260,
    US_ATT_310410,
    DE_O2_26207,
    NZ_SPARK_53005,
];

static DERIVED_PROFILES: OnceLock<Mutex<HashMap<String, &'static CarrierProfile>>> =
    OnceLock::new();

fn standard_public_plmn<'a>(mcc: &'a str, mnc: &str) -> Option<(&'a str, String)> {
    let mcc = mcc.trim();
    let mnc = mnc.trim();
    if mcc.len() != 3
        || !matches!(mnc.len(), 2 | 3)
        || !mcc.bytes().all(|byte| byte.is_ascii_digit())
        || !mnc.bytes().all(|byte| byte.is_ascii_digit())
        // 3GPP reserves MCC 999 for private networks. Publishing an Internet
        // fallback for it would turn a private deployment into a public-DNS
        // guess, so those profiles must come from the user/catalog/UICC/modem.
        || mcc == "999"
    {
        return None;
    }
    Some((mcc, format!("{:0>3}", mnc)))
}

/// Standard IMS home-network domain from 3GPP TS 23.003.
pub fn standard_ims_home_domain(mcc: &str, mnc: &str) -> Option<String> {
    let (mcc, padded_mnc) = standard_public_plmn(mcc, mnc)?;
    Some(format!("ims.mnc{padded_mnc}.mcc{mcc}.3gppnetwork.org"))
}

/// Standard operator-identifier ePDG FQDN from 3GPP TS 23.003.
pub fn standard_operator_epdg_fqdn(mcc: &str, mnc: &str) -> Option<String> {
    let (mcc, padded_mnc) = standard_public_plmn(mcc, mnc)?;
    Some(format!(
        "epdg.epc.mnc{padded_mnc}.mcc{mcc}.pub.3gppnetwork.org"
    ))
}

/// Parse a standard operator-identifier ePDG FQDN and return its MCC plus
/// three-digit MNC encoding. Only the public 3GPP operator form is accepted;
/// private/extension domains and emergency `sos.*` names are rejected.
pub fn parse_standard_operator_epdg_fqdn(host: &str) -> Option<(String, String)> {
    let labels = host
        .trim()
        .trim_end_matches('.')
        .split('.')
        .map(|label| label.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if labels.len() != 7
        || labels[0] != "epdg"
        || labels[1] != "epc"
        || labels[4] != "pub"
        || labels[5] != "3gppnetwork"
        || labels[6] != "org"
    {
        return None;
    }
    let mnc = labels[2].strip_prefix("mnc")?;
    let mcc = labels[3].strip_prefix("mcc")?;
    if mcc.len() != 3
        || mnc.len() != 3
        || !mcc.bytes().all(|byte| byte.is_ascii_digit())
        || !mnc.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let (_, padded_mnc) = standard_public_plmn(mcc, mnc)?;
    Some((mcc.to_string(), padded_mnc))
}

/// Standard EAP-AKA NAI realm from 3GPP TS 23.003.
pub fn standard_epc_nai_realm(mcc: &str, mnc: &str) -> Option<String> {
    let (mcc, padded_mnc) = standard_public_plmn(mcc, mnc)?;
    Some(format!("nai.epc.mnc{padded_mnc}.mcc{mcc}.3gppnetwork.org"))
}

/// Visited-country discovery FQDN from 3GPP TS 23.003.
pub fn standard_visited_country_epdg_fqdn(mcc: &str) -> Option<String> {
    let (mcc, _) = standard_public_plmn(mcc, "00")?;
    Some(format!(
        "epdg.epc.mcc{mcc}.visited-country.pub.3gppnetwork.org"
    ))
}

/// Standard tracking-area ePDG FQDN from 3GPP TS 23.003.
///
/// This only formats a name. Callers must not use it merely because a TAC is
/// available: TS 24.302 requires operator/UICC selection information to choose
/// the location-based format.
pub fn standard_tai_epdg_fqdn(mcc: &str, mnc: &str, tac: u32, technology: &str) -> Option<String> {
    let (mcc, padded_mnc) = standard_public_plmn(mcc, mnc)?;
    match technology.trim().to_ascii_lowercase().as_str() {
        "lte" if tac <= 0xffff => Some(format!(
            "tac-lb{:02x}.tac-hb{:02x}.tac.epdg.epc.mnc{padded_mnc}.mcc{mcc}.pub.3gppnetwork.org",
            tac & 0xff,
            (tac >> 8) & 0xff,
        )),
        "nr" if tac <= 0xff_ffff => Some(format!(
            "tac-lb{:02x}.tac-mb{:02x}.tac-hb{:02x}.5gstac.epdg.epc.mnc{padded_mnc}.mcc{mcc}.pub.3gppnetwork.org",
            tac & 0xff,
            (tac >> 8) & 0xff,
            (tac >> 16) & 0xff,
        )),
        _ => None,
    }
}

/// Generate a conservative profile from public 3GPP naming rules.
///
/// This is an explicitly unverified last resort. It derives only standard
/// domains and a portable IMS registration envelope: stable flow identity,
/// access-type PANI and MMTEL capability for voice. On untrusted Wi-Fi, CNI is
/// enabled only as a capability gate and is emitted only when a real serving-cell
/// snapshot exists. Initial empty Authorization, visited-network identity and
/// mandatory sec-agree remain disabled until a database/catalog profile opts in
/// or the network challenges the UE.
pub fn derive_standard_3gpp_profile(
    mcc: &str,
    mnc: &str,
    access: Standard3gppAccess,
) -> Option<&'static CarrierProfile> {
    let mcc = mcc.trim();
    let mnc = mnc.trim();
    let epdg_host = standard_operator_epdg_fqdn(mcc, mnc)?;
    let ims_domain = standard_ims_home_domain(mcc, mnc)?;

    let plmn = format!("{}{}", mcc, mnc);
    let cache_key = format!("{}:{plmn}", access.as_str());
    let cache = DERIVED_PROFILES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    if let Some(profile) = guard.get(&cache_key) {
        return Some(*profile);
    }

    let epdg_host = Box::leak(epdg_host.into_boxed_str());
    let ims_domain = Box::leak(ims_domain.into_boxed_str());
    let profile_id =
        Box::leak(format!("derived_3gpp_{}_{}", access.as_str(), plmn).into_boxed_str());
    let source_refs: &'static [&'static str] = match access {
        Standard3gppAccess::LteEpc => {
            &["Unverified standard-derived LTE fallback; not present as a ready database profile"]
        }
        Standard3gppAccess::WifiEpdg => &[
            "Unverified standard-derived Wi-Fi fallback; not present as a ready database profile",
        ],
    };

    let profile = CarrierProfile {
        meta: CarrierProfileMeta {
            profile_id,
            mcc: Box::leak(mcc.to_string().into_boxed_str()),
            mnc: Box::leak(mnc.to_string().into_boxed_str()),
            mnc_len: mnc.len() as u8,
            plmn: Box::leak(plmn.clone().into_boxed_str()),
            country_iso2: "unknown",
            brand: "Standard 3GPP",
            operator_legal_name: "Generic 3GPP Carrier",
            aliases: &[],
            source_refs,
            last_verified: "2026-08-19",
        },
        identity: ProfileIdentityPolicy {
            device_model_hint: "generic_android_class",
            spoof_imei: false,
            device_identity_enabled: false,
            device_identity_imei: None,
        },
        epdg: EpdgPolicy {
            host: epdg_host,
            port: 500,
            apn: Some("ims"),
            ip_stack: "ipv4v6",
            dns_server: None,
            dns_servers: &[],
        },
        ikev2: Ikev2Policy {
            nat_keepalive_seconds: 20,
            dpd_interval_seconds: 600,
            reauth_interval_seconds: None,
            ike_proposals: &[
                "aes256-sha256-prfsha512-modp2048",
                "aes256-sha512-prfsha512-modp2048",
                "aes256-sha256-prfsha256-modp2048",
                "aes256-sha256-prfsha1-modp2048",
                "aes128-sha256-prfsha1-modp2048",
                "aes128-sha256-prfsha256-modp2048",
                "aes128-sha256-modp2048",
                "aes128-sha256-modp1024",
                "aes128-sha1-modp1024",
                "aes256-sha1-modp1024",
                "aes256-sha256-prfsha1-modp1024",
            ],
            esp_proposals: &[
                "aes256-sha256",
                "aes128-sha256",
                "aes256-sha512",
                "aes128-sha1",
            ],
            aka_challenge_mode: "standard",
            include_epdg_idr: true,
            identity_template: None,
        },
        ims: ImsPolicy {
            domain: ims_domain,
            realm: ims_domain,
            registrar: None,
            pcscf: None,
            // UDP is the most interoperable IMS fallback transport. A catalog
            // profile may still override this explicitly (for networks that
            // require TCP), but an unknown operator should not be forced onto
            // TCP before the P-CSCF has expressed that requirement.
            transport: "udp",
            local_port: 5060,
            user_agent: "SimAdmin IMS",
            identity_source: "isim",
            tcp_keepalive_seconds: DEFAULT_IMS_TCP_KEEPALIVE_SECONDS,
            options_ping_interval_seconds: DEFAULT_IMS_OPTIONS_PING_INTERVAL_SECONDS,
            register: RegisterPolicy {
                supported_header: "path,sec-agree,gruu",
                request_uri_policy: "home_domain",
                include_pani_initial: true,
                include_pani_authenticated: true,
                initial_authorization: "none",
                // Voice-capable fallback registrations advertise MMTEL/audio.
                // A carrier-specific database profile can deliberately select
                // an SMS-only Contact (as observed with some IPCC profiles).
                include_mmtel_features: true,
                include_route_header: false,
                include_visited_network: false,
                include_p_preferred_identity: true,
                visited_network_header: None,
                allow_methods: None,
                strict_security_server_offer: false,
                enable_initial_reject_fallback: false,
                use_plain_digest_placeholder: false,
                require_sec_agree_headers: false,
                proxy_require_sec_agree_headers: false,
                sec_agree_mode: "auto",
                expires_seconds: DEFAULT_REGISTER_EXPIRES_SECONDS,
                access_network_info: access.access_network_info(),
                pani_identity_policy: match access {
                    Standard3gppAccess::LteEpc => AccessIdentityPolicy::DynamicIfKnown,
                    Standard3gppAccess::WifiEpdg => AccessIdentityPolicy::Static,
                },
                cellular_network_info: None,
                cni_identity_policy: match access {
                    Standard3gppAccess::LteEpc => AccessIdentityPolicy::Omit,
                    Standard3gppAccess::WifiEpdg => AccessIdentityPolicy::DynamicIfKnown,
                },
                contact_mode: "standard",
                contact_param_order: match access {
                    Standard3gppAccess::LteEpc => &[
                        "+g.3gpp.mid-call",
                        "+g.3gpp.srvcc-alerting",
                        "+g.3gpp.ps2cs-srvcc-orig-pre-alerting",
                    ],
                    Standard3gppAccess::WifiEpdg => &[],
                },
                temporary_status_codes: DEFAULT_TEMPORARY_STATUS_CODES,
                forbidden_status_codes: DEFAULT_FORBIDDEN_STATUS_CODES,
                initial_reject_fallback_status_codes: DEFAULT_INITIAL_REJECT_FALLBACK_STATUS_CODES,
                temporary_retry_seconds: DEFAULT_TEMPORARY_RETRY_SECONDS,
                always_add_sip_instance: true,
                enable_cellular_network_info: matches!(access, Standard3gppAccess::WifiEpdg),
                security_client_mechanisms: &["hmac-sha-1-96/aes-cbc/esp/trans"],
                live_header_variant_set: "standard_3gpp_conservative",
            },
        },
        sms: SmsPolicy {
            receiver_transport: "tcp",
            smsc_auth_required: false,
        },
        voice: VoicePolicy {
            vowifi_enabled: matches!(access, Standard3gppAccess::WifiEpdg),
            carrier_fallback_enabled: true,
            preferred_codecs: &["amr-wb", "amr", "pcmu", "pcma"],
            codec_policies: &[],
            amr_octet_align: false,
            ptime_ms: 20,
            sip_endpoint_exposed: false,
            voicemail_number: None,
        },
        e911: E911Policy {
            enabled: false,
            provider: None,
            entitlement_url: None,
            websheet_host_policy: None,
        },
        ut: UtPolicy {
            enabled: false,
            xcap_root: None,
            document_selector: None,
            namespace: None,
            authentication: "none",
            partial_update: false,
            call_waiting_selector: None,
            diversion_rule_selector: None,
            oip_selector: None,
            oir_selector: None,
            tls_min_version: "1.2",
            tls_max_version: "1.3",
            tls_builtin_roots: true,
            tls_additional_ca_pem: None,
        },
    };

    let static_profile: &'static CarrierProfile = Box::leak(Box::new(profile));
    guard.insert(cache_key, static_profile);
    Some(static_profile)
}

pub fn is_standard_derived_profile(profile: &CarrierProfile) -> bool {
    profile.meta.profile_id.starts_with("derived_3gpp_")
}

fn derived_profile_by_id(profile_id: &str) -> Option<&'static CarrierProfile> {
    let guard = DERIVED_PROFILES
        .get()?
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard
        .values()
        .find(|profile| profile.meta.profile_id == profile_id)
        .copied()
}

fn derive_standard_match(
    imsi: &str,
    home_plmn: Option<&str>,
    access: Standard3gppAccess,
) -> Option<CarrierMatch> {
    let digits = imsi.trim();
    if digits.len() < 5 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let plmn = home_plmn
        .map(str::trim)
        .filter(|plmn| {
            matches!(plmn.len(), 5 | 6)
                && plmn.bytes().all(|byte| byte.is_ascii_digit())
                && digits.starts_with(*plmn)
        })
        .map(str::to_string)?;
    let profile = derive_standard_3gpp_profile(&plmn[..3], &plmn[3..], access)?;
    Some(CarrierMatch {
        profile,
        matched_prefix: plmn,
    })
}

#[cfg(test)]
pub fn generate_standard_3gpp_profile(
    mcc: &str,
    mnc: &str,
    _mnc_len: u8,
) -> &'static CarrierProfile {
    derive_standard_3gpp_profile(mcc, mnc, Standard3gppAccess::WifiEpdg)
        .expect("test MCC/MNC must be valid")
}

/// Profiles published from the database, keyed by PLMN and by profile id.
///
/// The live matching path (`resolve_by_imsi` / `resolve_by_plmn`) is a pure
/// function used from modules that have no database handle. Rather than thread
/// one through the whole VoWiFi stack, `ProfileStore` publishes its rows here so
/// an operator's edit actually takes effect on the next connection instead of
/// only showing up in the API.
struct PublishedProfileMatch {
    match_prefix: String,
    profile: &'static CarrierProfile,
}

struct ProfileOverrides {
    by_plmn: HashMap<String, &'static CarrierProfile>,
    by_id: HashMap<String, &'static CarrierProfile>,
    imsi_matches: Vec<PublishedProfileMatch>,
}
static DB_OVERRIDES: OnceLock<std::sync::RwLock<ProfileOverrides>> = OnceLock::new();
static AMBIGUOUS_PLMN_PREFIXES: OnceLock<std::sync::RwLock<std::collections::HashSet<String>>> =
    OnceLock::new();

#[cfg(test)]
static PROFILE_RESOLVER_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) struct ProfileResolverTestGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for ProfileResolverTestGuard {
    fn drop(&mut self) {
        publish_database_profiles(&[]);
        publish_ambiguous_plmn_prefixes(&[]);
    }
}

#[cfg(test)]
pub(crate) fn profile_resolver_test_guard() -> ProfileResolverTestGuard {
    let lock = PROFILE_RESOLVER_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    publish_database_profiles(&[]);
    publish_ambiguous_plmn_prefixes(&[]);
    ProfileResolverTestGuard { _lock: lock }
}

fn overrides() -> &'static std::sync::RwLock<ProfileOverrides> {
    DB_OVERRIDES.get_or_init(|| {
        std::sync::RwLock::new(ProfileOverrides {
            by_plmn: HashMap::new(),
            by_id: HashMap::new(),
            imsi_matches: Vec::new(),
        })
    })
}

/// Replace the published override set. Called by `ProfileStore` after any change.
#[cfg(test)]
fn publish_database_profiles(profiles: &[&'static CarrierProfile]) {
    let matches = profiles
        .iter()
        .map(|profile| (profile.meta.plmn.to_string(), *profile))
        .collect::<Vec<_>>();
    publish_resolver_profiles(profiles, &matches);
}

/// Publish the complete profile-id index and the narrower set of automatic
/// public-identity matches. Match order is significant for equal prefixes, so
/// callers put local overrides before catalog rules.
pub fn publish_resolver_profiles(
    profiles: &[&'static CarrierProfile],
    matches: &[(String, &'static CarrierProfile)],
) {
    let mut by_plmn = HashMap::new();
    let mut by_id = HashMap::new();
    for profile in profiles {
        by_id.insert(profile.meta.profile_id.to_string(), *profile);
    }
    let mut imsi_matches = Vec::new();
    for (match_prefix, profile) in matches {
        by_plmn
            .entry(profile.meta.plmn.to_string())
            .or_insert(*profile);
        imsi_matches.push(PublishedProfileMatch {
            match_prefix: match_prefix.clone(),
            profile,
        });
    }
    *overrides()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = ProfileOverrides {
        by_plmn,
        by_id,
        imsi_matches,
    };
}

pub fn publish_ambiguous_plmn_prefixes(prefixes: &[String]) {
    let prefixes = prefixes.iter().cloned().collect();
    *AMBIGUOUS_PLMN_PREFIXES
        .get_or_init(|| std::sync::RwLock::new(std::collections::HashSet::new()))
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = prefixes;
}

fn imsi_has_ambiguous_plmn(imsi: &str) -> bool {
    AMBIGUOUS_PLMN_PREFIXES
        .get_or_init(|| std::sync::RwLock::new(std::collections::HashSet::new()))
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .any(|prefix| imsi.starts_with(prefix))
}

/// Snapshot all profiles currently published from database sources. Runtime
/// diagnostics use this instead of compiling a second carrier list into the
/// binary.
pub fn published_database_profiles() -> Vec<&'static CarrierProfile> {
    let profiles = overrides()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .by_id
        .values()
        .copied()
        .collect::<Vec<_>>();
    #[cfg(test)]
    if profiles.is_empty() {
        return BUILTIN_PROFILES.iter().collect();
    }
    profiles
}

fn database_profile_for_plmn(plmn: &str) -> Option<&'static CarrierProfile> {
    overrides()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .by_plmn
        .get(plmn)
        .copied()
}

fn database_profile_for_id(profile_id: &str) -> Option<&'static CarrierProfile> {
    overrides()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .by_id
        .get(profile_id)
        .copied()
}

/// Longest-prefix match of an IMSI against the published database profiles.
/// Longest first so a 3-digit MNC wins over a 2-digit one sharing its prefix.
fn database_profile_for_imsi(imsi: &str) -> Option<CarrierMatch> {
    let digits = imsi.trim();
    if digits.len() < 5 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if imsi_has_ambiguous_plmn(digits) {
        return None;
    }
    let guard = overrides()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut best: Option<CarrierMatch> = None;
    for candidate in &guard.imsi_matches {
        if digits.starts_with(candidate.match_prefix.as_str())
            && best
                .as_ref()
                .is_none_or(|current| current.matched_prefix.len() < candidate.match_prefix.len())
        {
            best = Some(CarrierMatch {
                profile: candidate.profile,
                matched_prefix: candidate.match_prefix.clone(),
            });
        }
    }
    best
}

fn database_profile_for_home_plmn(imsi: &str, plmn: &str) -> Option<CarrierMatch> {
    let guard = overrides()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut best: Option<CarrierMatch> = None;
    for candidate in &guard.imsi_matches {
        if candidate.profile.meta.plmn == plmn
            && imsi.starts_with(&candidate.match_prefix)
            && best
                .as_ref()
                .is_none_or(|current| current.matched_prefix.len() < candidate.match_prefix.len())
        {
            best = Some(CarrierMatch {
                profile: candidate.profile,
                matched_prefix: candidate.match_prefix.clone(),
            });
        }
    }
    best
}

pub fn resolve_by_imsi(imsi: &str) -> Option<CarrierMatch> {
    let resolved = database_profile_for_imsi(imsi);
    #[cfg(test)]
    if resolved.is_none() {
        let digits = imsi.trim();
        return BUILTIN_PROFILES
            .iter()
            .filter(|profile| digits.starts_with(profile.meta.plmn))
            .max_by_key(|profile| profile.meta.plmn.len())
            .map(|profile| CarrierMatch {
                profile,
                matched_prefix: profile.meta.plmn.to_string(),
            });
    }
    resolved
}

pub fn resolve_by_plmn(mcc: &str, mnc: &str) -> Option<&'static CarrierProfile> {
    let plmn = format!("{mcc}{mnc}");
    let resolved = database_profile_for_plmn(&plmn);
    #[cfg(test)]
    if resolved.is_none() {
        return BUILTIN_PROFILES
            .iter()
            .find(|profile| profile.meta.plmn == plmn);
    }
    resolved
}

pub fn resolve_by_profile_id(profile_id: &str) -> Option<&'static CarrierProfile> {
    let normalized = profile_id.trim();
    if normalized.is_empty() {
        return None;
    }

    let resolved =
        database_profile_for_id(normalized).or_else(|| derived_profile_by_id(normalized));
    #[cfg(test)]
    if resolved.is_none() {
        return BUILTIN_PROFILES
            .iter()
            .find(|profile| profile.meta.profile_id == normalized);
    }
    resolved
}

/// Resolve a profile pinned by a SIM override `profile_id`, honoring only profiles
/// actually published from the catalog or local override database.
///
/// A per-SIM pin is an operator's explicit "use exactly this carrier profile"
/// choice, so it must not silently derive a replacement. Returning `None` here
/// means the explicit pin no longer resolves and the line must report that
/// configuration error instead of changing operator policy implicitly.
pub fn resolve_pinned_database_profile(profile_id: &str) -> Option<&'static CarrierProfile> {
    let normalized = profile_id.trim();
    if normalized.is_empty() {
        return None;
    }
    database_profile_for_id(normalized)
}

/// Resolve the carrier profile for one line: an explicit published profile pin
/// wins, then the catalog/local-override public-identity path.
///
/// `pinned_profile_id` comes from the access-specific SIM override. Pins are
/// strict: an invalid or non-ready explicit choice must be fixed by the
/// operator and never silently replaced by an inferred profile.
pub fn resolve_for_line(
    pinned_profile_id: Option<&str>,
    imsi: &str,
    home_plmn: Option<&str>,
) -> Option<CarrierMatch> {
    if let Some(profile_id) = pinned_profile_id {
        return resolve_pinned_database_profile(profile_id).map(|profile| CarrierMatch {
            profile,
            matched_prefix: profile.meta.plmn.to_string(),
        });
    }
    if let Some(plmn) = home_plmn.map(str::trim).filter(|plmn| {
        matches!(plmn.len(), 5 | 6)
            && plmn.bytes().all(|byte| byte.is_ascii_digit())
            && imsi.trim().starts_with(*plmn)
    }) {
        if let Some(profile) = database_profile_for_home_plmn(imsi.trim(), plmn) {
            return Some(profile);
        }
    }
    resolve_by_imsi(imsi)
        .or_else(|| derive_standard_match(imsi, home_plmn, Standard3gppAccess::WifiEpdg))
}

#[cfg(test)]
pub fn validate_builtin_profiles() -> Result<(), String> {
    for profile in BUILTIN_PROFILES {
        if profile.meta.mcc.len() != 3 || !profile.meta.mcc.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!("invalid mcc in {}", profile.meta.profile_id));
        }
        if profile.meta.mnc.is_empty() || !profile.meta.mnc.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!("invalid mnc in {}", profile.meta.profile_id));
        }
        if profile.meta.mnc_len as usize != profile.meta.mnc.len() {
            return Err(format!("mnc_len mismatch in {}", profile.meta.profile_id));
        }
        if profile.meta.plmn != format!("{}{}", profile.meta.mcc, profile.meta.mnc) {
            return Err(format!("plmn mismatch in {}", profile.meta.profile_id));
        }
        if NaiveDate::parse_from_str(profile.meta.last_verified, "%Y-%m-%d").is_err() {
            return Err(format!(
                "invalid last_verified in {}",
                profile.meta.profile_id
            ));
        }
        if profile.meta.aliases.is_empty() {
            return Err(format!(
                "aliases must not be empty for {}",
                profile.meta.profile_id
            ));
        }
        if profile.meta.source_refs.is_empty() {
            return Err(format!(
                "source_refs must not be empty for {}",
                profile.meta.profile_id
            ));
        }
        if !matches!(
            profile.ims.register.live_header_variant_set,
            "standard_ims_features" | "ee_ims_features"
        ) {
            return Err(format!(
                "unknown live_header_variant_set in {}",
                profile.meta.profile_id
            ));
        }
        if profile.voice.preferred_codecs.is_empty() {
            return Err(format!(
                "voice.preferred_codecs must not be empty for {}",
                profile.meta.profile_id
            ));
        }
        if !profile
            .voice
            .preferred_codecs
            .iter()
            .all(|codec| AudioCodec::from_token(codec).is_some())
        {
            return Err(format!(
                "voice.preferred_codecs has unknown codec in {}",
                profile.meta.profile_id
            ));
        }
        for policy in profile.voice.codec_policies {
            let Some(codec) = AudioCodec::from_token(policy.codec) else {
                return Err(format!(
                    "voice.codec_policies has unknown codec in {}",
                    profile.meta.profile_id
                ));
            };
            if policy
                .sample_rate
                .is_some_and(|sample_rate| sample_rate != codec.clock_rate())
            {
                return Err(format!(
                    "voice.codec_policies has invalid sample rate in {}",
                    profile.meta.profile_id
                ));
            }
            if policy.payload_type.is_some_and(|payload_type| {
                codec
                    .static_payload_type()
                    .is_some_and(|static_type| payload_type != static_type)
                    || (codec.static_payload_type().is_none()
                        && !(96..=127).contains(&payload_type))
            }) {
                return Err(format!(
                    "voice.codec_policies has invalid payload type in {}",
                    profile.meta.profile_id
                ));
            }
        }
        if profile.voice.ptime_ms == 0 {
            return Err(format!(
                "voice.ptime_ms must be non-zero for {}",
                profile.meta.profile_id
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_builtin_profile_metadata() {
        let _resolver_guard = profile_resolver_test_guard();
        validate_builtin_profiles().expect("builtin profiles should validate");
    }

    #[test]
    fn resolves_gb_ee_profile_by_imsi_prefix() {
        let _resolver_guard = profile_resolver_test_guard();
        let match_result = resolve_by_imsi("234331234567890").expect("should match");
        assert_eq!(match_result.profile.meta.profile_id, "gb_ee_23433");
        assert_eq!(match_result.matched_prefix, "23433");
    }

    #[test]
    fn resolves_nl_vodafone_profile_by_plmn() {
        let _resolver_guard = profile_resolver_test_guard();
        let profile = resolve_by_plmn("204", "04").expect("should match");
        assert_eq!(profile.meta.profile_id, "nl_vodafone_20404");
    }

    #[test]
    fn resolves_profile_by_clean_room_profile_id() {
        let _resolver_guard = profile_resolver_test_guard();
        let profile = resolve_by_profile_id("nl_vodafone_20404").expect("should match");
        assert_eq!(profile.meta.plmn, "20404");
        assert!(resolve_by_profile_id(" CTEUK_23433 ").is_none());
    }

    #[test]
    fn gb_ee_prioritizes_observed_successful_ike_proposal() {
        let _resolver_guard = profile_resolver_test_guard();
        assert_eq!(GB_EE_23433.ikev2.ike_proposals[0], "aes128-sha256-modp2048");
        assert!(GB_EE_23433
            .ikev2
            .ike_proposals
            .contains(&"aes128-sha256-prfsha1-modp2048"));
    }

    #[test]
    fn automatic_line_match_derives_access_specific_standard_profile() {
        let _resolver_guard = profile_resolver_test_guard();
        assert!(resolve_by_imsi("262011234567890").is_none());
        assert!(resolve_by_plmn("262", "01").is_none());
        let matched = resolve_for_line(None, "262011234567890", Some("26201"))
            .expect("missing database row should derive a fallback");
        assert_eq!(matched.profile.meta.profile_id, "derived_3gpp_vowifi_26201");
        assert!(!matched.profile.ims.register.include_visited_network);
        assert_eq!(matched.profile.ims.register.visited_network_header, None);
        assert_eq!(
            matched.profile.ims.register.access_network_info,
            "IEEE-802.11"
        );
        assert_eq!(
            resolve_by_profile_id("derived_3gpp_vowifi_26201")
                .map(|profile| profile.meta.profile_id),
            Some("derived_3gpp_vowifi_26201")
        );
        let lte = derive_standard_3gpp_profile("262", "01", Standard3gppAccess::LteEpc)
            .expect("valid LTE fallback");
        assert_eq!(lte.meta.profile_id, "derived_3gpp_lte_26201");
        assert_ne!(lte.meta.profile_id, matched.profile.meta.profile_id);
        assert_eq!(lte.ims.register.access_network_info, "3GPP-E-UTRAN-FDD");
        assert_eq!(lte.ims.transport, "udp");
        assert_eq!(lte.ims.register.sec_agree_mode, "auto");
        assert!(!lte.ims.register.require_sec_agree_headers);
        assert!(!lte.ims.register.proxy_require_sec_agree_headers);
        assert!(lte.ims.register.include_pani_initial);
        assert!(lte.ims.register.include_pani_authenticated);
        assert!(!lte.ims.register.enable_cellular_network_info);
        assert!(lte.ims.register.always_add_sip_instance);
        assert!(lte.ims.register.include_mmtel_features);
        assert!(!lte.ims.register.include_route_header);
        assert_eq!(lte.ims.register.contact_mode, "standard");
        assert_eq!(
            lte.ims.register.contact_param_order,
            &[
                "+g.3gpp.mid-call",
                "+g.3gpp.srvcc-alerting",
                "+g.3gpp.ps2cs-srvcc-orig-pre-alerting",
            ]
        );
        assert_eq!(
            lte.ims.register.live_header_variant_set,
            "standard_3gpp_conservative"
        );
        let wifi = derive_standard_3gpp_profile("502", "12", Standard3gppAccess::WifiEpdg)
            .expect("derive standard Wi-Fi profile");
        assert!(wifi.ims.register.enable_cellular_network_info);
        assert_eq!(wifi.ims.register.access_network_info, "IEEE-802.11");
        assert!(!lte.meta.source_refs[0].contains("legacy-test-profile"));
    }

    #[test]
    fn standard_3gpp_domains_pad_mnc_and_reject_private_or_invalid_plmns() {
        assert_eq!(
            standard_ims_home_domain("502", "12").as_deref(),
            Some("ims.mnc012.mcc502.3gppnetwork.org")
        );
        assert_eq!(
            standard_operator_epdg_fqdn("310", "260").as_deref(),
            Some("epdg.epc.mnc260.mcc310.pub.3gppnetwork.org")
        );
        assert!(standard_operator_epdg_fqdn("99", "01").is_none());
        assert!(standard_operator_epdg_fqdn("310", "2a").is_none());
        assert!(standard_operator_epdg_fqdn("999", "99").is_none());
        assert!(derive_standard_3gpp_profile("999", "99", Standard3gppAccess::WifiEpdg).is_none());
    }

    #[test]
    fn standard_tracking_area_epdg_names_use_3gpp_byte_order() {
        assert_eq!(
            standard_tai_epdg_fqdn("345", "12", 0x0b21, "lte").as_deref(),
            Some("tac-lb21.tac-hb0b.tac.epdg.epc.mnc012.mcc345.pub.3gppnetwork.org")
        );
        assert_eq!(
            standard_tai_epdg_fqdn("345", "12", 0x0b1a21, "nr").as_deref(),
            Some("tac-lb21.tac-mb1a.tac-hb0b.5gstac.epdg.epc.mnc012.mcc345.pub.3gppnetwork.org")
        );
        assert!(standard_tai_epdg_fqdn("345", "12", 0x1_0000, "lte").is_none());
        assert!(standard_tai_epdg_fqdn("345", "12", 0x100_0000, "nr").is_none());
        assert!(standard_tai_epdg_fqdn("345", "12", 1, "wifi").is_none());
    }

    #[test]
    fn published_catalog_match_keeps_its_full_imsi_prefix() {
        let _resolver_guard = profile_resolver_test_guard();
        let matches = vec![("20404123".to_string(), &NL_VODAFONE_20404)];
        publish_resolver_profiles(&[&NL_VODAFONE_20404], &matches);

        let matched = database_profile_for_imsi("204041234567890").expect("prefix match");
        assert_eq!(matched.profile.meta.profile_id, "nl_vodafone_20404");
        assert_eq!(matched.matched_prefix, "20404123");
        assert!(database_profile_for_imsi("204049994567890").is_none());
    }

    #[test]
    fn line_pin_is_strict_while_unpinned_matching_is_automatic() {
        let _resolver_guard = profile_resolver_test_guard();
        // No pin: behaves exactly like automatic IMSI matching.
        let auto = resolve_for_line(None, "234331234567890", None).expect("imsi should match");
        assert_eq!(auto.profile.meta.profile_id, "gb_ee_23433");

        // An explicit pin is an operator decision and must not silently turn
        // into a database or standard-derived automatic match.
        assert!(resolve_for_line(Some("no_such_db_profile"), "234331234567890", None).is_none());
    }

    #[test]
    fn line_pin_does_not_resolve_builtin_or_derived_ids() {
        let _resolver_guard = profile_resolver_test_guard();
        // A built-in id is reachable through automatic matching, but a per-line
        // pin only honors database profiles: it must not silently resolve here.
        assert!(resolve_pinned_database_profile("gb_ee_23433").is_none());
        assert!(resolve_pinned_database_profile("derived_3gpp_vowifi_26201").is_none());
        assert!(resolve_pinned_database_profile("   ").is_none());
    }

    #[test]
    fn line_pin_selects_the_published_database_profile_over_imsi() {
        let _resolver_guard = profile_resolver_test_guard();
        // Publish one database profile, then pin a line whose SIM IMSI would
        // otherwise match a *different* carrier. The pin must win.
        // Use a PLMN no other test's IMSI matches, so publishing into the shared
        // global overlay cannot perturb concurrent `resolve_by_imsi` tests.
        let mut record =
            super::super::profile_record::CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.meta.profile_id = "pin_test_db_profile".to_string();
        record.meta.mcc = "999".to_string();
        record.meta.mnc = "99".to_string();
        record.meta.plmn = "99999".to_string();
        let leaked: &'static CarrierProfile = record.intern();
        publish_database_profiles(&[leaked]);

        // The SIM IMSI (234-33) would auto-match EE, but the pin to the database
        // profile must win.
        let pinned = resolve_for_line(Some("pin_test_db_profile"), "234331234567890", None)
            .expect("pin should resolve");
        assert_eq!(pinned.profile.meta.profile_id, "pin_test_db_profile");

        // Clear the overlay so other tests see a clean slate.
        publish_database_profiles(&[]);
    }
}
