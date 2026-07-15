use chrono::NaiveDate;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EpdgPolicy {
    pub host: &'static str,
    pub port: u16,
    pub apn: Option<&'static str>,
    pub ip_stack: &'static str,
    pub dns_server: Option<&'static str>,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RegisterPolicy {
    pub supported_header: &'static str,
    pub include_pani_authenticated: bool,
    pub strict_security_server_offer: bool,
    pub enable_initial_reject_fallback: bool,
    pub use_plain_digest_placeholder: bool,
    pub require_sec_agree_headers: bool,
    pub security_client_mechanisms: &'static [&'static str],
    pub live_header_variant_set: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ImsPolicy {
    pub domain: &'static str,
    pub realm: &'static str,
    pub registrar: Option<&'static str>,
    pub pcscf: Option<&'static str>,
    pub transport: &'static str,
    pub local_port: u16,
    pub user_agent: &'static str,
    pub identity_source: &'static str,
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

/// Voice-calling policy for a carrier: which legs are usable, the media codec
/// preference, and AMR framing parameters. This mirrors [`SmsPolicy`] and drives
/// the voice state machine + SDP offer builder in `voice.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct VoicePolicy {
    /// Whether the self-authored VoWiFi voice leg (INVITE/SDP/AMR/RTP over
    /// ePDG) is enabled for this carrier.
    pub vowifi_enabled: bool,
    /// Whether the operator (AT + USB-Audio) fallback leg may be attempted.
    /// The leg is only used at runtime when USB-Audio is actually present.
    pub carrier_fallback_enabled: bool,
    /// Preferred audio codecs in priority order. Tokens: "amr", "amr-wb",
    /// "pcmu", "pcma".
    pub preferred_codecs: &'static [&'static str],
    /// Whether AMR payloads should be offered octet-aligned (`octet-align=1`).
    pub amr_octet_align: bool,
    /// Packetization time (ms) advertised in the SDP offer.
    pub ptime_ms: u16,
    /// Whether an outward standard SIP endpoint may be exposed per SIM (the
    /// external Asterisk/Linphone integration seam). Off by default.
    pub sip_endpoint_exposed: bool,
}

/// A sensible default voice policy: VoWiFi leg on, carrier fallback allowed
/// (gated by USB-Audio at runtime), AMR + AMR-WB + G.711 offered, no outward
/// SIP endpoint. Used by every builtin and dynamic profile until tuned.
pub const DEFAULT_VOICE_POLICY: VoicePolicy = VoicePolicy {
    vowifi_enabled: true,
    carrier_fallback_enabled: true,
    preferred_codecs: &["amr-wb", "amr", "pcmu", "pcma"],
    amr_octet_align: false,
    ptime_ms: 20,
    sip_endpoint_exposed: false,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarrierMatch {
    pub profile: &'static CarrierProfile,
    pub matched_prefix: String,
}

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
    },
    epdg: EpdgPolicy {
        host: "epdg.epc.mnc033.mcc234.pub.3gppnetwork.org",
        port: 500,
        apn: Some("ims"),
        ip_stack: "ipv6",
        dns_server: None,
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
        register: RegisterPolicy {
            supported_header: "path,sec-agree,gruu",
            include_pani_authenticated: true,
            strict_security_server_offer: true,
            enable_initial_reject_fallback: false,
            use_plain_digest_placeholder: false,
            require_sec_agree_headers: false,
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
};

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
    },
    epdg: EpdgPolicy {
        host: "epdg.epc.mnc004.mcc204.pub.3gppnetwork.org",
        port: 500,
        apn: Some("ims"),
        ip_stack: "ipv4v6",
        dns_server: None,
    },
    ikev2: Ikev2Policy {
        nat_keepalive_seconds: 20,
        dpd_interval_seconds: 600,
        reauth_interval_seconds: None,
        ike_proposals: &["aes256-sha256-prfsha512-modp2048"],
        esp_proposals: &["aes256-sha256"],
        aka_challenge_mode: "standard",
        include_epdg_idr: true,
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
        register: RegisterPolicy {
            supported_header: "path,sec-agree,gruu",
            include_pani_authenticated: true,
            strict_security_server_offer: true,
            enable_initial_reject_fallback: false,
            use_plain_digest_placeholder: false,
            require_sec_agree_headers: true,
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
};

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
    },
    epdg: EpdgPolicy {
        host: "epdg.epc.mnc260.mcc310.pub.3gppnetwork.org",
        port: 500,
        apn: Some("ims"),
        ip_stack: "ipv4v6",
        dns_server: None,
    },
    ikev2: Ikev2Policy {
        nat_keepalive_seconds: 20,
        dpd_interval_seconds: 600,
        reauth_interval_seconds: None,
        ike_proposals: &["aes128-sha256-modp2048"],
        esp_proposals: &["aes128-sha256", "aes128-sha1"],
        aka_challenge_mode: "standard",
        include_epdg_idr: true,
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
        register: RegisterPolicy {
            supported_header: "path,sec-agree,gruu",
            include_pani_authenticated: true,
            strict_security_server_offer: true,
            enable_initial_reject_fallback: false,
            use_plain_digest_placeholder: false,
            require_sec_agree_headers: true,
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
};

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
    },
    epdg: EpdgPolicy {
        host: "epdg.epc.att.net",
        port: 500,
        apn: Some("ims"),
        ip_stack: "ipv4v6",
        dns_server: None,
    },
    ikev2: Ikev2Policy {
        nat_keepalive_seconds: 20,
        dpd_interval_seconds: 600,
        reauth_interval_seconds: None,
        ike_proposals: &["aes128-sha256-modp2048"],
        esp_proposals: &["aes128-sha256"],
        aka_challenge_mode: "standard",
        include_epdg_idr: true,
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
        register: RegisterPolicy {
            supported_header: "path,sec-agree,gruu",
            include_pani_authenticated: true,
            strict_security_server_offer: true,
            enable_initial_reject_fallback: true,
            use_plain_digest_placeholder: false,
            require_sec_agree_headers: true,
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
};

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
    },
    epdg: EpdgPolicy {
        host: "epdg.epc.mnc007.mcc262.pub.3gppnetwork.org",
        port: 500,
        apn: Some("ims"),
        ip_stack: "ipv4v6",
        dns_server: None,
    },
    ikev2: Ikev2Policy {
        nat_keepalive_seconds: 20,
        dpd_interval_seconds: 600,
        reauth_interval_seconds: None,
        ike_proposals: &["aes256-sha256-prfsha1-modp2048"],
        esp_proposals: &["aes256-sha256"],
        aka_challenge_mode: "standard",
        include_epdg_idr: true,
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
        register: RegisterPolicy {
            supported_header: "path,sec-agree,gruu",
            include_pani_authenticated: true,
            strict_security_server_offer: true,
            enable_initial_reject_fallback: true,
            use_plain_digest_placeholder: false,
            require_sec_agree_headers: true,
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
};

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
    },
    epdg: EpdgPolicy {
        host: "epdg.epc.mnc005.mcc530.pub.3gppnetwork.spark.co.nz",
        port: 500,
        apn: Some("ims"),
        ip_stack: "ipv4v6",
        dns_server: None,
    },
    ikev2: Ikev2Policy {
        nat_keepalive_seconds: 20,
        dpd_interval_seconds: 600,
        reauth_interval_seconds: None,
        ike_proposals: &["aes256-sha256-prfsha256-modp2048"],
        esp_proposals: &["aes256-sha256"],
        aka_challenge_mode: "standard",
        include_epdg_idr: true,
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
        register: RegisterPolicy {
            supported_header: "path,sec-agree,gruu",
            include_pani_authenticated: true,
            strict_security_server_offer: true,
            enable_initial_reject_fallback: false,
            use_plain_digest_placeholder: false,
            require_sec_agree_headers: true,
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
};

pub static BUILTIN_PROFILES: &[CarrierProfile] = &[
    GB_EE_23433,
    NL_VODAFONE_20404,
    US_TMOBILE_310260,
    US_ATT_310410,
    DE_O2_26207,
    NZ_SPARK_53005,
];

static DYNAMIC_PROFILES: OnceLock<Mutex<HashMap<String, &'static CarrierProfile>>> =
    OnceLock::new();

/// 动态生成标准的 3GPP 运营商配置，并将其转化为静态生命周期的引用
pub fn generate_standard_3gpp_profile(
    mcc: &str,
    mnc: &str,
    mnc_len: u8,
) -> &'static CarrierProfile {
    let plmn = format!("{}{}", mcc, mnc);
    let cache = DYNAMIC_PROFILES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap();

    if let Some(profile) = guard.get(&plmn) {
        return profile;
    }

    // 格式化补全 MNC（标准 3GPP 域名中，MNC 必须固定补齐为 3 位，例如 15 需补为 015）
    let padded_mnc = format!("{:0>3}", mnc);
    let epdg_host = Box::leak(
        format!("epdg.epc.mnc{}.mcc{}.pub.3gppnetwork.org", padded_mnc, mcc).into_boxed_str(),
    );
    let ims_domain =
        Box::leak(format!("ims.mnc{}.mcc{}.3gppnetwork.org", padded_mnc, mcc).into_boxed_str());
    let profile_id = Box::leak(format!("dynamic_3gpp_{}", plmn).into_boxed_str());

    let profile = CarrierProfile {
        meta: CarrierProfileMeta {
            profile_id,
            mcc: Box::leak(mcc.to_string().into_boxed_str()),
            mnc: Box::leak(mnc.to_string().into_boxed_str()),
            mnc_len,
            plmn: Box::leak(plmn.clone().into_boxed_str()),
            country_iso2: "unknown",
            brand: "Standard 3GPP",
            operator_legal_name: "Generic 3GPP Carrier",
            aliases: &[],
            source_refs: &["Generated dynamically via 3GPP fallback rules"],
            last_verified: "2026-06-24",
        },
        identity: ProfileIdentityPolicy {
            device_model_hint: "generic_android_class",
            spoof_imei: false,
        },
        epdg: EpdgPolicy {
            host: epdg_host,
            port: 500,
            apn: Some("ims"),
            ip_stack: "ipv4v6",
            dns_server: None,
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
        },
        ims: ImsPolicy {
            domain: ims_domain,
            realm: ims_domain,
            registrar: None,
            pcscf: None,
            transport: "tcp",
            local_port: 5060,
            user_agent: "SimAdmin VoWiFi",
            identity_source: "isim",
            register: RegisterPolicy {
                supported_header: "path,sec-agree,gruu",
                include_pani_authenticated: true,
                strict_security_server_offer: false,
                enable_initial_reject_fallback: true,
                use_plain_digest_placeholder: false,
                require_sec_agree_headers: true,
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
    };

    let static_profile = Box::leak(Box::new(profile));
    guard.insert(plmn, static_profile);
    static_profile
}

pub fn resolve_by_imsi(imsi: &str) -> Option<CarrierMatch> {
    // 1. 尝试匹配内置预设
    if let Some(matched) = resolve_builtin_by_imsi(imsi) {
        return Some(matched);
    }

    // 2. 尝试动态生成 3GPP 标准预设
    let digits = imsi.trim();
    if digits.len() >= 5 && digits.chars().all(|c| c.is_ascii_digit()) {
        let mcc = &digits[..3];
        let mnc_len = if digits.starts_with("310") || digits.starts_with("405") {
            3
        } else {
            2
        };
        if digits.len() >= 3 + mnc_len {
            let mnc = &digits[3..3 + mnc_len];
            let profile = generate_standard_3gpp_profile(mcc, mnc, mnc_len as u8);
            return Some(CarrierMatch {
                profile,
                matched_prefix: format!("{}{}", mcc, mnc),
            });
        }
    }
    None
}

fn resolve_builtin_by_imsi(imsi: &str) -> Option<CarrierMatch> {
    BUILTIN_PROFILES.iter().find_map(|profile| {
        if profile.meta.mcc.is_empty() || profile.meta.mnc.is_empty() {
            return None;
        }
        let prefix_len = 3 + profile.meta.mnc_len as usize;
        let digits = imsi.trim();
        if digits.len() < prefix_len || !digits.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let prefix = &digits[..prefix_len];
        let expected = format!("{}{}", profile.meta.mcc, profile.meta.mnc);
        if prefix == expected {
            Some(CarrierMatch {
                profile,
                matched_prefix: expected,
            })
        } else {
            None
        }
    })
}

pub fn resolve_by_plmn(mcc: &str, mnc: &str) -> Option<&'static CarrierProfile> {
    // 1. 尝试匹配内置预设
    if let Some(profile) = BUILTIN_PROFILES
        .iter()
        .find(|profile| profile.meta.mcc == mcc && profile.meta.mnc == mnc)
    {
        return Some(profile);
    }

    // 2. 尝试动态生成 3GPP 预设
    if mcc.len() == 3
        && mcc.chars().all(|c| c.is_ascii_digit())
        && !mnc.is_empty()
        && mnc.chars().all(|c| c.is_ascii_digit())
    {
        return Some(generate_standard_3gpp_profile(mcc, mnc, mnc.len() as u8));
    }

    None
}

pub fn resolve_by_profile_id(profile_id: &str) -> Option<&'static CarrierProfile> {
    let normalized = profile_id.trim();
    if normalized.is_empty() {
        return None;
    }

    // 1. 尝试匹配内置预设
    if let Some(profile) = BUILTIN_PROFILES
        .iter()
        .find(|profile| profile.meta.profile_id == normalized)
    {
        return Some(profile);
    }

    // 2. 尝试解析动态预设 ID
    if let Some(plmn) = normalized.strip_prefix("dynamic_3gpp_") {
        if plmn.len() >= 5 && plmn.len() <= 6 && plmn.chars().all(|c| c.is_ascii_digit()) {
            let mcc = &plmn[..3];
            let mnc = &plmn[3..];
            return Some(generate_standard_3gpp_profile(mcc, mnc, mnc.len() as u8));
        }
    }

    None
}

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
            .all(|codec| matches!(*codec, "amr" | "amr-wb" | "pcmu" | "pcma"))
        {
            return Err(format!(
                "voice.preferred_codecs has unknown codec in {}",
                profile.meta.profile_id
            ));
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
        validate_builtin_profiles().expect("builtin profiles should validate");
    }

    #[test]
    fn resolves_gb_ee_profile_by_imsi_prefix() {
        let match_result = resolve_by_imsi("234331234567890").expect("should match");
        assert_eq!(match_result.profile.meta.profile_id, "gb_ee_23433");
        assert_eq!(match_result.matched_prefix, "23433");
    }

    #[test]
    fn resolves_nl_vodafone_profile_by_plmn() {
        let profile = resolve_by_plmn("204", "04").expect("should match");
        assert_eq!(profile.meta.profile_id, "nl_vodafone_20404");
    }

    #[test]
    fn resolves_profile_by_clean_room_profile_id() {
        let profile = resolve_by_profile_id("nl_vodafone_20404").expect("should match");
        assert_eq!(profile.meta.plmn, "20404");
        assert!(resolve_by_profile_id(" CTEUK_23433 ").is_none());
    }

    #[test]
    fn gb_ee_prioritizes_observed_successful_ike_proposal() {
        assert_eq!(GB_EE_23433.ikev2.ike_proposals[0], "aes128-sha256-modp2048");
        assert!(GB_EE_23433
            .ikev2
            .ike_proposals
            .contains(&"aes128-sha256-prfsha1-modp2048"));
    }

    #[test]
    fn resolves_dynamic_3gpp_profile_by_imsi_and_plmn() {
        // 测试通过未内置的 Telekom DE (262-01) IMSI 动态解析
        let match_result = resolve_by_imsi("262011234567890").expect("should resolve dynamically");
        assert_eq!(match_result.profile.meta.profile_id, "dynamic_3gpp_26201");
        assert_eq!(
            match_result.profile.epdg.host,
            "epdg.epc.mnc001.mcc262.pub.3gppnetwork.org"
        );
        assert_eq!(
            match_result.profile.ims.domain,
            "ims.mnc001.mcc262.3gppnetwork.org"
        );

        // 测试通过 PLMN 动态解析
        let profile = resolve_by_plmn("262", "01").expect("should resolve dynamically");
        assert_eq!(profile.meta.profile_id, "dynamic_3gpp_26201");

        // 测试通过 Profile ID 动态解析
        let profile_by_id =
            resolve_by_profile_id("dynamic_3gpp_26201").expect("should resolve dynamically");
        assert_eq!(profile_by_id.meta.plmn, "26201");
    }
}
