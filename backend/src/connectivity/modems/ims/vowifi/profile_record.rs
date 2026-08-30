//! Owned, serializable mirror of [`CarrierProfile`].
//!
//! The runtime uses `&'static CarrierProfile` everywhere, which is convenient
//! for the hundreds of call sites that pass profiles around but cannot express
//! "loaded from a database at runtime". This module bridges the two: a
//! `CarrierProfileRecord` is a plain owned struct that serde can round-trip to
//! JSON and SQLite, and `intern()` turns one into a `&'static CarrierProfile`.
//!
//! Interning leaks. That is deliberate and bounded: profiles are immutable once
//! resolved, there is at most one per carrier the device has ever seen, and the
//! alternative is threading a lifetime (or an `Arc`) through the entire VoWiFi
//! stack. The intern cache also deduplicates, so re-reading the same profile
//! from the database does not leak again.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::profiles::{
    self, CarrierProfile, CarrierProfileMeta, E911Policy, EpdgPolicy, Ikev2Policy, ImsPolicy,
    ProfileIdentityPolicy, RegisterPolicy, SmsPolicy, UtPolicy, VoiceCodecPolicy, VoicePolicy,
};
use crate::connectivity::core::access_network::AccessIdentityPolicy;
use crate::connectivity::core::voice::AudioCodec;

/// Version of the JSON shape persisted in `custom_carrier_profiles`.
///
/// Version `0` denotes a row written before the field existed. Such rows are
/// normalized by [`CarrierProfileRecord::from_database_json`] using presence
/// checks against the original JSON so an explicit `false` is never confused
/// with a serde default.
pub const CURRENT_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarrierProfileMetaRecord {
    pub profile_id: String,
    pub mcc: String,
    pub mnc: String,
    pub mnc_len: u8,
    pub plmn: String,
    #[serde(default)]
    pub country_iso2: String,
    #[serde(default)]
    pub brand: String,
    #[serde(default)]
    pub operator_legal_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub source_refs: Vec<String>,
    #[serde(default)]
    pub last_verified: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileIdentityPolicyRecord {
    pub device_model_hint: String,
    #[serde(default)]
    pub spoof_imei: bool,
    #[serde(default)]
    pub device_identity_enabled: bool,
    /// `None` means "use the modem's own IMEI".
    #[serde(default)]
    pub device_identity_imei: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpdgPolicyRecord {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub apn: Option<String>,
    pub ip_stack: String,
    #[serde(default)]
    pub dns_server: Option<String>,
    /// Ordered DNS servers tried in turn when resolving the ePDG.
    #[serde(default)]
    pub dns_servers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ikev2PolicyRecord {
    pub nat_keepalive_seconds: u16,
    pub dpd_interval_seconds: u16,
    #[serde(default)]
    pub reauth_interval_seconds: Option<u16>,
    pub ike_proposals: Vec<String>,
    pub esp_proposals: Vec<String>,
    pub aka_challenge_mode: String,
    #[serde(default)]
    pub include_epdg_idr: bool,
    /// Optional RFC822 IDi template. Required for private PLMNs (MCC 999),
    /// where SimAdmin must not invent a public 3GPP NAI realm.
    #[serde(default)]
    pub identity_template: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterPolicyRecord {
    pub supported_header: String,
    #[serde(default = "default_request_uri_policy")]
    pub request_uri_policy: String,
    #[serde(default = "default_true")]
    pub include_pani_initial: bool,
    #[serde(default = "default_true")]
    pub include_pani_authenticated: bool,
    #[serde(default = "default_initial_authorization")]
    pub initial_authorization: String,
    #[serde(default)]
    pub include_mmtel_features: bool,
    #[serde(default)]
    pub include_route_header: bool,
    #[serde(default)]
    pub include_visited_network: bool,
    #[serde(default = "default_true")]
    pub include_p_preferred_identity: bool,
    #[serde(default)]
    pub visited_network_header: Option<String>,
    #[serde(default)]
    pub allow_methods: Option<String>,
    #[serde(default)]
    pub strict_security_server_offer: bool,
    #[serde(default)]
    pub enable_initial_reject_fallback: bool,
    #[serde(default)]
    pub use_plain_digest_placeholder: bool,
    #[serde(default)]
    pub require_sec_agree_headers: bool,
    #[serde(default)]
    pub proxy_require_sec_agree_headers: bool,
    /// `auto` | `required` | `disabled`.
    #[serde(default = "default_sec_agree_mode")]
    pub sec_agree_mode: String,
    pub security_client_mechanisms: Vec<String>,
    pub live_header_variant_set: String,
    #[serde(default = "default_expires_seconds")]
    pub expires_seconds: u32,
    #[serde(default = "default_access_network_info")]
    pub access_network_info: String,
    #[serde(default = "default_static_access_identity_policy")]
    pub pani_identity_policy: AccessIdentityPolicy,
    #[serde(default)]
    pub cellular_network_info: Option<String>,
    #[serde(default = "default_omit_access_identity_policy")]
    pub cni_identity_policy: AccessIdentityPolicy,
    /// `android_default` | `legacy`.
    #[serde(default = "default_contact_mode")]
    pub contact_mode: String,
    #[serde(default)]
    pub contact_param_order: Vec<String>,
    #[serde(default = "default_true")]
    pub always_add_sip_instance: bool,
    #[serde(default)]
    pub enable_cellular_network_info: bool,
    #[serde(default = "default_temporary_status_codes")]
    pub temporary_status_codes: Vec<u16>,
    #[serde(default = "default_forbidden_status_codes")]
    pub forbidden_status_codes: Vec<u16>,
    #[serde(default = "default_initial_reject_fallback_status_codes")]
    pub initial_reject_fallback_status_codes: Vec<u16>,
    #[serde(default = "default_temporary_retry_seconds")]
    pub temporary_retry_seconds: u16,
}

fn default_sec_agree_mode() -> String {
    "auto".to_string()
}

fn default_request_uri_policy() -> String {
    "registrar".to_string()
}

fn default_true() -> bool {
    true
}

fn default_initial_authorization() -> String {
    "none".to_string()
}

fn default_expires_seconds() -> u32 {
    profiles::DEFAULT_REGISTER_EXPIRES_SECONDS
}

fn default_access_network_info() -> String {
    profiles::DEFAULT_ACCESS_NETWORK_INFO.to_string()
}

fn default_static_access_identity_policy() -> AccessIdentityPolicy {
    AccessIdentityPolicy::Static
}

fn default_omit_access_identity_policy() -> AccessIdentityPolicy {
    AccessIdentityPolicy::Omit
}

fn default_contact_mode() -> String {
    "android_default".to_string()
}

fn default_temporary_status_codes() -> Vec<u16> {
    profiles::DEFAULT_TEMPORARY_STATUS_CODES.to_vec()
}

fn default_forbidden_status_codes() -> Vec<u16> {
    profiles::DEFAULT_FORBIDDEN_STATUS_CODES.to_vec()
}

fn default_initial_reject_fallback_status_codes() -> Vec<u16> {
    profiles::DEFAULT_INITIAL_REJECT_FALLBACK_STATUS_CODES.to_vec()
}

fn default_temporary_retry_seconds() -> u16 {
    profiles::DEFAULT_TEMPORARY_RETRY_SECONDS
}

fn default_tcp_keepalive_seconds() -> u16 {
    profiles::DEFAULT_IMS_TCP_KEEPALIVE_SECONDS
}

fn default_options_ping_interval_seconds() -> u16 {
    profiles::DEFAULT_IMS_OPTIONS_PING_INTERVAL_SECONDS
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImsPolicyRecord {
    pub domain: String,
    pub realm: String,
    #[serde(default)]
    pub registrar: Option<String>,
    #[serde(default)]
    pub pcscf: Option<String>,
    pub transport: String,
    pub local_port: u16,
    pub user_agent: String,
    pub identity_source: String,
    /// Zero disables the SIP TCP keepalive.
    #[serde(default = "default_tcp_keepalive_seconds")]
    pub tcp_keepalive_seconds: u16,
    /// Zero disables the SIP OPTIONS ping.
    #[serde(default = "default_options_ping_interval_seconds")]
    pub options_ping_interval_seconds: u16,
    pub register: RegisterPolicyRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmsPolicyRecord {
    pub receiver_transport: String,
    #[serde(default)]
    pub smsc_auth_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceCodecPolicyRecord {
    pub codec: String,
    #[serde(default)]
    pub payload_type: Option<u8>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub fmtp: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoicePolicyRecord {
    #[serde(default)]
    pub vowifi_enabled: bool,
    #[serde(default)]
    pub carrier_fallback_enabled: bool,
    pub preferred_codecs: Vec<String>,
    #[serde(default)]
    pub codec_policies: Vec<VoiceCodecPolicyRecord>,
    #[serde(default)]
    pub amr_octet_align: bool,
    pub ptime_ms: u16,
    #[serde(default)]
    pub sip_endpoint_exposed: bool,
    #[serde(default)]
    pub voicemail_number: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct E911PolicyRecord {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub entitlement_url: Option<String>,
    #[serde(default)]
    pub websheet_host_policy: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UtPolicyRecord {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub xcap_root: Option<String>,
    #[serde(default)]
    pub document_selector: Option<String>,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default = "default_ut_authentication")]
    pub authentication: String,
    #[serde(default)]
    pub partial_update: bool,
    #[serde(default)]
    pub call_waiting_selector: Option<String>,
    #[serde(default)]
    pub diversion_rule_selector: Option<String>,
    #[serde(default)]
    pub oip_selector: Option<String>,
    #[serde(default)]
    pub oir_selector: Option<String>,
    #[serde(default = "default_ut_tls_min_version")]
    pub tls_min_version: String,
    #[serde(default = "default_ut_tls_max_version")]
    pub tls_max_version: String,
    #[serde(default = "default_true")]
    pub tls_builtin_roots: bool,
    #[serde(default)]
    pub tls_additional_ca_pem: Option<String>,
}

fn default_ut_authentication() -> String {
    "none".to_string()
}

fn default_ut_tls_min_version() -> String {
    "1.2".to_string()
}

fn default_ut_tls_max_version() -> String {
    "1.3".to_string()
}

impl Default for UtPolicyRecord {
    fn default() -> Self {
        Self {
            enabled: false,
            xcap_root: None,
            document_selector: None,
            namespace: None,
            authentication: default_ut_authentication(),
            partial_update: false,
            call_waiting_selector: None,
            diversion_rule_selector: None,
            oip_selector: None,
            oir_selector: None,
            tls_min_version: default_ut_tls_min_version(),
            tls_max_version: default_ut_tls_max_version(),
            tls_builtin_roots: true,
            tls_additional_ca_pem: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarrierProfileRecord {
    #[serde(default)]
    pub schema_version: u32,
    pub meta: CarrierProfileMetaRecord,
    pub identity: ProfileIdentityPolicyRecord,
    pub epdg: EpdgPolicyRecord,
    pub ikev2: Ikev2PolicyRecord,
    pub ims: ImsPolicyRecord,
    pub sms: SmsPolicyRecord,
    pub voice: VoicePolicyRecord,
    pub e911: E911PolicyRecord,
    #[serde(default)]
    pub ut: UtPolicyRecord,
}

/// Leak a string into `'static`, deduplicating so repeated interning of the
/// same value does not grow the heap without bound.
fn intern_str(value: &str) -> &'static str {
    static POOL: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let pool = POOL.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = pool.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = guard.get(value) {
        return existing;
    }
    let leaked: &'static str = Box::leak(value.to_string().into_boxed_str());
    guard.insert(value.to_string(), leaked);
    leaked
}

fn intern_opt(value: Option<&String>) -> Option<&'static str> {
    value
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(intern_str)
}

fn intern_u16_list(values: &[u16]) -> &'static [u16] {
    Box::leak(values.to_vec().into_boxed_slice())
}

fn intern_list(values: &[String]) -> &'static [&'static str] {
    let leaked = values
        .iter()
        .map(|value| intern_str(value))
        .collect::<Vec<_>>();
    Box::leak(leaked.into_boxed_slice())
}

fn intern_voice_codec_policies(values: &[VoiceCodecPolicyRecord]) -> &'static [VoiceCodecPolicy] {
    let leaked = values
        .iter()
        .map(|value| VoiceCodecPolicy {
            codec: intern_str(&value.codec),
            payload_type: value.payload_type,
            sample_rate: value.sample_rate,
            fmtp: intern_opt(value.fmtp.as_ref()),
        })
        .collect::<Vec<_>>();
    Box::leak(leaked.into_boxed_slice())
}

impl CarrierProfileRecord {
    /// Parse a database row while preserving the distinction between a field
    /// that was absent in an old schema and a field explicitly set to `false`.
    ///
    /// Runtime code only consumes the normalized [`CarrierProfile`], so this
    /// migration path applies equally to manually-created database profiles,
    /// copied catalog rows and profiles written by older SimAdmin releases.
    pub fn from_database_json(json: &str) -> Result<Self, String> {
        let value = serde_json::from_str::<Value>(json)
            .map_err(|error| format!("carrier_profile_json_invalid:{error}"))?;
        let mut record = serde_json::from_value::<Self>(value.clone())
            .map_err(|error| format!("carrier_profile_json_invalid:{error}"))?;
        record.normalize_legacy_database_record(&value)?;
        record.validate()?;
        Ok(record)
    }

    /// REGISTER switches that a caller must state explicitly.
    ///
    /// These are tri-state in a carrier bundle -- `true`, `false`/`omit`, or
    /// absent meaning "no opinion" -- but this record stores a plain `bool`, so
    /// by the time a body is deserialized the distinction is gone. A stored
    /// database row keeps it because `from_database_json` inspects the raw JSON;
    /// an API caller has no such rescue.
    pub const REQUIRED_REGISTER_SWITCHES: &'static [&'static str] = &[
        "include_pani_initial",
        "include_pani_authenticated",
        "include_route_header",
        "include_p_preferred_identity",
        "always_add_sip_instance",
        "enable_cellular_network_info",
        "require_sec_agree_headers",
        "proxy_require_sec_agree_headers",
    ];

    /// Parse a record submitted through the API, refusing a partial body.
    ///
    /// A PUT replaces the whole resource, so every REGISTER switch must be
    /// stated. Accepting an absent one would let serde's default decide, and
    /// four of these default to `true`: a caller doing read-modify-write that
    /// dropped a field would silently cancel the operator's `omit` and turn a
    /// header back on, on the registration path, with no error.
    ///
    /// Refusing is the same choice the catalog projection makes for an
    /// unrecognised value -- a bad body is an authoring mistake and must be
    /// visible. The error names every missing field so one round trip is enough
    /// to fix the caller.
    pub fn from_api_json(json: &str) -> Result<Self, String> {
        let value = serde_json::from_str::<Value>(json)
            .map_err(|error| format!("carrier_profile_json_invalid:{error}"))?;
        Self::from_api_value(value)
    }

    /// As `from_api_json`, for a body already parsed by the web framework.
    pub fn from_api_value(value: Value) -> Result<Self, String> {
        let register = value.pointer("/ims/register").and_then(Value::as_object);
        let Some(register) = register else {
            return Err("carrier_profile_register_section_missing".to_string());
        };
        let missing = Self::REQUIRED_REGISTER_SWITCHES
            .iter()
            .filter(|field| !register.contains_key(**field))
            .copied()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(format!(
                "carrier_profile_register_switch_missing:{}",
                missing.join(",")
            ));
        }

        let record = serde_json::from_value::<Self>(value)
            .map_err(|error| format!("carrier_profile_json_invalid:{error}"))?;
        record.validate()?;
        Ok(record)
    }

    fn normalize_legacy_database_record(&mut self, source: &Value) -> Result<(), String> {
        if self.schema_version > CURRENT_SCHEMA_VERSION {
            return Err(format!(
                "carrier_profile_schema_unsupported:{}:{}",
                self.schema_version, CURRENT_SCHEMA_VERSION
            ));
        }

        let register = source.pointer("/ims/register");
        let has = |field: &str| {
            register
                .and_then(Value::as_object)
                .is_some_and(|object| object.contains_key(field))
        };

        // Old records inherited these values from serde defaults. Normalize
        // only missing fields; an operator-authored `false`, `disabled`, or
        // `omit` has higher priority and must survive unchanged.
        if !has("always_add_sip_instance") {
            self.ims.register.always_add_sip_instance = true;
        }
        if !has("enable_cellular_network_info") {
            // CNI can expose serving-cell information and is conditionally
            // applicable. Never synthesize it merely because an old row did
            // not know about the switch.
            self.ims.register.enable_cellular_network_info = false;
        }
        if !has("pani_identity_policy") {
            let access_type = self
                .ims
                .register
                .access_network_info
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_uppercase();
            self.ims.register.pani_identity_policy =
                if access_type.starts_with("3GPP-E-UTRAN") || access_type.starts_with("3GPP-NR") {
                    AccessIdentityPolicy::DynamicIfKnown
                } else {
                    AccessIdentityPolicy::Static
                };
        }
        if !has("cni_identity_policy") {
            self.ims.register.cni_identity_policy =
                if self.ims.register.enable_cellular_network_info {
                    AccessIdentityPolicy::DynamicIfKnown
                } else {
                    AccessIdentityPolicy::Omit
                };
        }
        if !has("include_mmtel_features") {
            // Legacy rows predate the explicit capability switch. Treat them
            // as voice-capable so a normal database profile remains eligible
            // for MMTEL terminating service on either LTE or Wi-Fi. An
            // operator-authored SMS-only/IPCC shape must store an explicit
            // `false`, which the presence check above preserves.
            self.ims.register.include_mmtel_features = true;
        }
        if !has("sec_agree_mode") {
            self.ims.register.sec_agree_mode = if self.ims.register.require_sec_agree_headers
                || self.ims.register.proxy_require_sec_agree_headers
            {
                "required".to_string()
            } else if self.ims.register.security_client_mechanisms.is_empty() {
                "disabled".to_string()
            } else {
                "auto".to_string()
            };
        }

        self.schema_version = CURRENT_SCHEMA_VERSION;
        Ok(())
    }

    /// Turn this record into a `&'static CarrierProfile`.
    ///
    /// Repeated calls for the same `profile_id` return the same reference, so
    /// reloading the database does not leak a new profile each time. A changed
    /// record replaces the cached entry — the previously leaked one stays
    /// allocated, which is acceptable because edits are rare and operator-driven.
    pub fn intern(&self) -> &'static CarrierProfile {
        static CACHE: OnceLock<
            Mutex<HashMap<String, (CarrierProfileRecord, &'static CarrierProfile)>>,
        > = OnceLock::new();
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let mut guard = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((cached_record, cached_profile)) = guard.get(&self.meta.profile_id) {
            if cached_record == self {
                return cached_profile;
            }
        }

        let profile: &'static CarrierProfile = Box::leak(Box::new(self.to_profile()));
        guard.insert(self.meta.profile_id.clone(), (self.clone(), profile));
        profile
    }

    fn to_profile(&self) -> CarrierProfile {
        CarrierProfile {
            meta: CarrierProfileMeta {
                profile_id: intern_str(&self.meta.profile_id),
                mcc: intern_str(&self.meta.mcc),
                mnc: intern_str(&self.meta.mnc),
                mnc_len: self.meta.mnc_len,
                plmn: intern_str(&self.meta.plmn),
                country_iso2: intern_str(&self.meta.country_iso2),
                brand: intern_str(&self.meta.brand),
                operator_legal_name: intern_str(&self.meta.operator_legal_name),
                aliases: intern_list(&self.meta.aliases),
                source_refs: intern_list(&self.meta.source_refs),
                last_verified: intern_str(&self.meta.last_verified),
            },
            identity: ProfileIdentityPolicy {
                device_model_hint: intern_str(&self.identity.device_model_hint),
                spoof_imei: self.identity.spoof_imei,
                device_identity_enabled: self.identity.device_identity_enabled,
                device_identity_imei: intern_opt(self.identity.device_identity_imei.as_ref()),
            },
            epdg: EpdgPolicy {
                host: intern_str(&self.epdg.host),
                port: self.epdg.port,
                apn: intern_opt(self.epdg.apn.as_ref()),
                ip_stack: intern_str(&self.epdg.ip_stack),
                // Keep the single-value field in sync with the list so callers
                // that only want one server never disagree with the list.
                dns_server: intern_opt(
                    self.epdg
                        .dns_server
                        .as_ref()
                        .or_else(|| self.epdg.dns_servers.first()),
                ),
                dns_servers: intern_list(&self.epdg.dns_servers),
            },
            ikev2: Ikev2Policy {
                nat_keepalive_seconds: self.ikev2.nat_keepalive_seconds,
                dpd_interval_seconds: self.ikev2.dpd_interval_seconds,
                reauth_interval_seconds: self.ikev2.reauth_interval_seconds,
                ike_proposals: intern_list(&self.ikev2.ike_proposals),
                esp_proposals: intern_list(&self.ikev2.esp_proposals),
                aka_challenge_mode: intern_str(&self.ikev2.aka_challenge_mode),
                include_epdg_idr: self.ikev2.include_epdg_idr,
                identity_template: self.ikev2.identity_template.as_deref().map(intern_str),
            },
            ims: ImsPolicy {
                domain: intern_str(&self.ims.domain),
                realm: intern_str(&self.ims.realm),
                registrar: intern_opt(self.ims.registrar.as_ref()),
                pcscf: intern_opt(self.ims.pcscf.as_ref()),
                transport: intern_str(&self.ims.transport),
                local_port: self.ims.local_port,
                user_agent: intern_str(&self.ims.user_agent),
                identity_source: intern_str(&self.ims.identity_source),
                tcp_keepalive_seconds: self.ims.tcp_keepalive_seconds,
                options_ping_interval_seconds: self.ims.options_ping_interval_seconds,
                register: RegisterPolicy {
                    supported_header: intern_str(&self.ims.register.supported_header),
                    request_uri_policy: intern_str(&self.ims.register.request_uri_policy),
                    include_pani_initial: self.ims.register.include_pani_initial,
                    include_pani_authenticated: self.ims.register.include_pani_authenticated,
                    initial_authorization: intern_str(&self.ims.register.initial_authorization),
                    include_mmtel_features: self.ims.register.include_mmtel_features,
                    include_route_header: self.ims.register.include_route_header,
                    include_visited_network: self.ims.register.include_visited_network,
                    include_p_preferred_identity: self.ims.register.include_p_preferred_identity,
                    visited_network_header: intern_opt(
                        self.ims.register.visited_network_header.as_ref(),
                    ),
                    allow_methods: intern_opt(self.ims.register.allow_methods.as_ref()),
                    strict_security_server_offer: self.ims.register.strict_security_server_offer,
                    enable_initial_reject_fallback: self
                        .ims
                        .register
                        .enable_initial_reject_fallback,
                    use_plain_digest_placeholder: self.ims.register.use_plain_digest_placeholder,
                    require_sec_agree_headers: self.ims.register.require_sec_agree_headers,
                    proxy_require_sec_agree_headers: self
                        .ims
                        .register
                        .proxy_require_sec_agree_headers,
                    sec_agree_mode: intern_str(&self.ims.register.sec_agree_mode),
                    security_client_mechanisms: intern_list(
                        &self.ims.register.security_client_mechanisms,
                    ),
                    live_header_variant_set: intern_str(&self.ims.register.live_header_variant_set),
                    expires_seconds: self.ims.register.expires_seconds,
                    access_network_info: intern_str(&self.ims.register.access_network_info),
                    pani_identity_policy: self.ims.register.pani_identity_policy,
                    cellular_network_info: intern_opt(
                        self.ims.register.cellular_network_info.as_ref(),
                    ),
                    cni_identity_policy: self.ims.register.cni_identity_policy,
                    contact_mode: intern_str(&self.ims.register.contact_mode),
                    contact_param_order: intern_list(&self.ims.register.contact_param_order),
                    always_add_sip_instance: self.ims.register.always_add_sip_instance,
                    enable_cellular_network_info: self.ims.register.enable_cellular_network_info,
                    temporary_status_codes: intern_u16_list(
                        &self.ims.register.temporary_status_codes,
                    ),
                    forbidden_status_codes: intern_u16_list(
                        &self.ims.register.forbidden_status_codes,
                    ),
                    initial_reject_fallback_status_codes: intern_u16_list(
                        &self.ims.register.initial_reject_fallback_status_codes,
                    ),
                    temporary_retry_seconds: self.ims.register.temporary_retry_seconds,
                },
            },
            sms: SmsPolicy {
                receiver_transport: intern_str(&self.sms.receiver_transport),
                smsc_auth_required: self.sms.smsc_auth_required,
            },
            voice: VoicePolicy {
                vowifi_enabled: self.voice.vowifi_enabled,
                carrier_fallback_enabled: self.voice.carrier_fallback_enabled,
                preferred_codecs: intern_list(&self.voice.preferred_codecs),
                codec_policies: intern_voice_codec_policies(&self.voice.codec_policies),
                amr_octet_align: self.voice.amr_octet_align,
                ptime_ms: self.voice.ptime_ms,
                sip_endpoint_exposed: self.voice.sip_endpoint_exposed,
                voicemail_number: intern_opt(self.voice.voicemail_number.as_ref()),
            },
            e911: E911Policy {
                enabled: self.e911.enabled,
                provider: intern_opt(self.e911.provider.as_ref()),
                entitlement_url: intern_opt(self.e911.entitlement_url.as_ref()),
                websheet_host_policy: intern_opt(self.e911.websheet_host_policy.as_ref()),
            },
            ut: UtPolicy {
                enabled: self.ut.enabled,
                xcap_root: intern_opt(self.ut.xcap_root.as_ref()),
                document_selector: intern_opt(self.ut.document_selector.as_ref()),
                namespace: intern_opt(self.ut.namespace.as_ref()),
                authentication: intern_str(&self.ut.authentication),
                partial_update: self.ut.partial_update,
                call_waiting_selector: intern_opt(self.ut.call_waiting_selector.as_ref()),
                diversion_rule_selector: intern_opt(self.ut.diversion_rule_selector.as_ref()),
                oip_selector: intern_opt(self.ut.oip_selector.as_ref()),
                oir_selector: intern_opt(self.ut.oir_selector.as_ref()),
                tls_min_version: intern_str(&self.ut.tls_min_version),
                tls_max_version: intern_str(&self.ut.tls_max_version),
                tls_builtin_roots: self.ut.tls_builtin_roots,
                tls_additional_ca_pem: intern_opt(self.ut.tls_additional_ca_pem.as_ref()),
            },
        }
    }

    /// Snapshot an existing (built-in or derived) profile as an editable record.
    /// This is how the built-ins are seeded into the database on first run.
    pub fn from_profile(profile: &CarrierProfile) -> Self {
        let to_owned_list =
            |values: &'static [&'static str]| values.iter().map(|v| v.to_string()).collect();
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            meta: CarrierProfileMetaRecord {
                profile_id: profile.meta.profile_id.to_string(),
                mcc: profile.meta.mcc.to_string(),
                mnc: profile.meta.mnc.to_string(),
                mnc_len: profile.meta.mnc_len,
                plmn: profile.meta.plmn.to_string(),
                country_iso2: profile.meta.country_iso2.to_string(),
                brand: profile.meta.brand.to_string(),
                operator_legal_name: profile.meta.operator_legal_name.to_string(),
                aliases: to_owned_list(profile.meta.aliases),
                source_refs: to_owned_list(profile.meta.source_refs),
                last_verified: profile.meta.last_verified.to_string(),
            },
            identity: ProfileIdentityPolicyRecord {
                device_model_hint: profile.identity.device_model_hint.to_string(),
                spoof_imei: profile.identity.spoof_imei,
                device_identity_enabled: profile.identity.device_identity_enabled,
                device_identity_imei: profile.identity.device_identity_imei.map(str::to_string),
            },
            epdg: EpdgPolicyRecord {
                host: profile.epdg.host.to_string(),
                port: profile.epdg.port,
                apn: profile.epdg.apn.map(str::to_string),
                ip_stack: profile.epdg.ip_stack.to_string(),
                dns_server: profile.epdg.dns_server.map(str::to_string),
                dns_servers: to_owned_list(profile.epdg.dns_servers),
            },
            ikev2: Ikev2PolicyRecord {
                nat_keepalive_seconds: profile.ikev2.nat_keepalive_seconds,
                dpd_interval_seconds: profile.ikev2.dpd_interval_seconds,
                reauth_interval_seconds: profile.ikev2.reauth_interval_seconds,
                ike_proposals: to_owned_list(profile.ikev2.ike_proposals),
                esp_proposals: to_owned_list(profile.ikev2.esp_proposals),
                aka_challenge_mode: profile.ikev2.aka_challenge_mode.to_string(),
                include_epdg_idr: profile.ikev2.include_epdg_idr,
                identity_template: profile.ikev2.identity_template.map(str::to_string),
            },
            ims: ImsPolicyRecord {
                domain: profile.ims.domain.to_string(),
                realm: profile.ims.realm.to_string(),
                registrar: profile.ims.registrar.map(str::to_string),
                pcscf: profile.ims.pcscf.map(str::to_string),
                transport: profile.ims.transport.to_string(),
                local_port: profile.ims.local_port,
                user_agent: profile.ims.user_agent.to_string(),
                identity_source: profile.ims.identity_source.to_string(),
                tcp_keepalive_seconds: profile.ims.tcp_keepalive_seconds,
                options_ping_interval_seconds: profile.ims.options_ping_interval_seconds,
                register: RegisterPolicyRecord {
                    supported_header: profile.ims.register.supported_header.to_string(),
                    request_uri_policy: profile.ims.register.request_uri_policy.to_string(),
                    include_pani_initial: profile.ims.register.include_pani_initial,
                    include_pani_authenticated: profile.ims.register.include_pani_authenticated,
                    initial_authorization: profile.ims.register.initial_authorization.to_string(),
                    include_mmtel_features: profile.ims.register.include_mmtel_features,
                    include_route_header: profile.ims.register.include_route_header,
                    include_visited_network: profile.ims.register.include_visited_network,
                    include_p_preferred_identity: profile.ims.register.include_p_preferred_identity,
                    visited_network_header: profile
                        .ims
                        .register
                        .visited_network_header
                        .map(str::to_string),
                    allow_methods: profile.ims.register.allow_methods.map(str::to_string),
                    strict_security_server_offer: profile.ims.register.strict_security_server_offer,
                    enable_initial_reject_fallback: profile
                        .ims
                        .register
                        .enable_initial_reject_fallback,
                    use_plain_digest_placeholder: profile.ims.register.use_plain_digest_placeholder,
                    require_sec_agree_headers: profile.ims.register.require_sec_agree_headers,
                    proxy_require_sec_agree_headers: profile
                        .ims
                        .register
                        .proxy_require_sec_agree_headers,
                    sec_agree_mode: profile.ims.register.sec_agree_mode.to_string(),
                    security_client_mechanisms: to_owned_list(
                        profile.ims.register.security_client_mechanisms,
                    ),
                    live_header_variant_set: profile
                        .ims
                        .register
                        .live_header_variant_set
                        .to_string(),
                    expires_seconds: profile.ims.register.expires_seconds,
                    access_network_info: profile.ims.register.access_network_info.to_string(),
                    pani_identity_policy: profile.ims.register.pani_identity_policy,
                    cellular_network_info: profile
                        .ims
                        .register
                        .cellular_network_info
                        .map(str::to_string),
                    cni_identity_policy: profile.ims.register.cni_identity_policy,
                    contact_mode: profile.ims.register.contact_mode.to_string(),
                    contact_param_order: to_owned_list(profile.ims.register.contact_param_order),
                    always_add_sip_instance: profile.ims.register.always_add_sip_instance,
                    enable_cellular_network_info: profile.ims.register.enable_cellular_network_info,
                    temporary_status_codes: profile.ims.register.temporary_status_codes.to_vec(),
                    forbidden_status_codes: profile.ims.register.forbidden_status_codes.to_vec(),
                    initial_reject_fallback_status_codes: profile
                        .ims
                        .register
                        .initial_reject_fallback_status_codes
                        .to_vec(),
                    temporary_retry_seconds: profile.ims.register.temporary_retry_seconds,
                },
            },
            sms: SmsPolicyRecord {
                receiver_transport: profile.sms.receiver_transport.to_string(),
                smsc_auth_required: profile.sms.smsc_auth_required,
            },
            voice: VoicePolicyRecord {
                vowifi_enabled: profile.voice.vowifi_enabled,
                carrier_fallback_enabled: profile.voice.carrier_fallback_enabled,
                preferred_codecs: to_owned_list(profile.voice.preferred_codecs),
                codec_policies: profile
                    .voice
                    .codec_policies
                    .iter()
                    .map(|policy| VoiceCodecPolicyRecord {
                        codec: policy.codec.to_string(),
                        payload_type: policy.payload_type,
                        sample_rate: policy.sample_rate,
                        fmtp: policy.fmtp.map(str::to_string),
                    })
                    .collect(),
                amr_octet_align: profile.voice.amr_octet_align,
                ptime_ms: profile.voice.ptime_ms,
                sip_endpoint_exposed: profile.voice.sip_endpoint_exposed,
                voicemail_number: profile.voice.voicemail_number.map(str::to_string),
            },
            e911: E911PolicyRecord {
                enabled: profile.e911.enabled,
                provider: profile.e911.provider.map(str::to_string),
                entitlement_url: profile.e911.entitlement_url.map(str::to_string),
                websheet_host_policy: profile.e911.websheet_host_policy.map(str::to_string),
            },
            ut: UtPolicyRecord {
                enabled: profile.ut.enabled,
                xcap_root: profile.ut.xcap_root.map(str::to_string),
                document_selector: profile.ut.document_selector.map(str::to_string),
                namespace: profile.ut.namespace.map(str::to_string),
                authentication: profile.ut.authentication.to_string(),
                partial_update: profile.ut.partial_update,
                call_waiting_selector: profile.ut.call_waiting_selector.map(str::to_string),
                diversion_rule_selector: profile.ut.diversion_rule_selector.map(str::to_string),
                oip_selector: profile.ut.oip_selector.map(str::to_string),
                oir_selector: profile.ut.oir_selector.map(str::to_string),
                tls_min_version: profile.ut.tls_min_version.to_string(),
                tls_max_version: profile.ut.tls_max_version.to_string(),
                tls_builtin_roots: profile.ut.tls_builtin_roots,
                tls_additional_ca_pem: profile.ut.tls_additional_ca_pem.map(str::to_string),
            },
        }
    }

    /// Reject records that would produce an unusable profile. Called before
    /// anything is written to the database or handed to the runtime.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_ims_only()?;
        if self.epdg.host.trim().is_empty() {
            return Err("epdg_host_required".to_string());
        }
        if self.epdg.port == 0 {
            return Err("epdg_port_invalid".to_string());
        }
        if self.ikev2.ike_proposals.is_empty() {
            return Err("ike_proposals_required".to_string());
        }
        if self.ikev2.esp_proposals.is_empty() {
            return Err("esp_proposals_required".to_string());
        }
        if self.meta.mcc == "999"
            && self
                .ikev2
                .identity_template
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err("private_plmn_ike_identity_template_required".to_string());
        }
        if !matches!(self.epdg.ip_stack.as_str(), "ipv4" | "ipv6" | "ipv4v6") {
            return Err("epdg_ip_stack_invalid".to_string());
        }
        for server in &self.epdg.dns_servers {
            if parse_dns_server(server).is_none() {
                return Err(format!("dns_server_invalid:{server}"));
            }
        }
        Ok(())
    }

    /// Validate the shared IMS/SIP portion of a catalog profile. LTE profiles
    /// do not require an ePDG or IKE row, while VoWiFi profiles call the stricter
    /// [`Self::validate`] above.
    pub fn validate_ims_only(&self) -> Result<(), String> {
        let meta = &self.meta;
        if meta.profile_id.trim().is_empty() {
            return Err("profile_id_required".to_string());
        }
        if meta.mcc.len() != 3 || !meta.mcc.chars().all(|c| c.is_ascii_digit()) {
            return Err("mcc_must_be_three_digits".to_string());
        }
        if meta.mnc.is_empty()
            || meta.mnc.len() > 3
            || !meta.mnc.chars().all(|c| c.is_ascii_digit())
        {
            return Err("mnc_must_be_two_or_three_digits".to_string());
        }
        if meta.mnc_len as usize != meta.mnc.len() {
            return Err("mnc_len_mismatch".to_string());
        }
        if meta.plmn != format!("{}{}", meta.mcc, meta.mnc) {
            return Err("plmn_mismatch".to_string());
        }
        if self.ims.domain.trim().is_empty() || self.ims.realm.trim().is_empty() {
            return Err("ims_domain_and_realm_required".to_string());
        }
        if let Some(template) = self.ikev2.identity_template.as_deref() {
            validate_ike_identity_template(template)?;
        }
        for (field, value) in [
            (
                "identity.device_model_hint",
                self.identity.device_model_hint.as_str(),
            ),
            ("ims.domain", self.ims.domain.as_str()),
            ("ims.realm", self.ims.realm.as_str()),
            ("ims.user_agent", self.ims.user_agent.as_str()),
            (
                "ims.register.supported_header",
                self.ims.register.supported_header.as_str(),
            ),
            (
                "ims.register.access_network_info",
                self.ims.register.access_network_info.as_str(),
            ),
        ] {
            validate_single_line_wire_value(field, value)?;
        }
        for (field, value) in [
            ("ims.registrar", self.ims.registrar.as_deref()),
            ("ims.pcscf", self.ims.pcscf.as_deref()),
            (
                "ims.register.visited_network_header",
                self.ims.register.visited_network_header.as_deref(),
            ),
            (
                "ims.register.allow_methods",
                self.ims.register.allow_methods.as_deref(),
            ),
            (
                "ims.register.cellular_network_info",
                self.ims.register.cellular_network_info.as_deref(),
            ),
        ] {
            if let Some(value) = value {
                validate_single_line_wire_value(field, value)?;
            }
        }
        for value in &self.ims.register.security_client_mechanisms {
            validate_single_line_wire_value("ims.register.security_client_mechanisms", value)?;
        }
        for value in &self.ims.register.contact_param_order {
            validate_single_line_wire_value("ims.register.contact_param_order", value)?;
        }
        for policy in &self.voice.codec_policies {
            if let Some(fmtp) = policy.fmtp.as_deref() {
                validate_single_line_wire_value("voice.codec_policies.fmtp", fmtp)?;
            }
        }
        if !matches!(self.ims.transport.as_str(), "tcp" | "udp") {
            return Err("ims_transport_must_be_tcp_or_udp".to_string());
        }
        if !matches!(
            self.ims.register.sec_agree_mode.as_str(),
            "auto" | "required" | "disabled"
        ) {
            return Err("sec_agree_mode_invalid".to_string());
        }
        if !matches!(
            self.ims.register.initial_authorization.as_str(),
            "none" | "aka_empty" | "digest_empty" | "implementation_variant"
        ) {
            return Err("initial_authorization_invalid".to_string());
        }
        if !matches!(
            self.ims.register.request_uri_policy.as_str(),
            "home_domain" | "registrar" | "pcscf" | "configured"
        ) {
            return Err("request_uri_policy_invalid".to_string());
        }
        if matches!(
            self.ims.register.request_uri_policy.as_str(),
            "registrar" | "configured"
        ) && self
            .ims
            .registrar
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            return Err("registrar_required_for_request_uri_policy".to_string());
        }
        if self.ims.register.include_visited_network
            && self
                .ims
                .register
                .visited_network_header
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err("visited_network_header_required".to_string());
        }
        if !matches!(
            self.ims.register.contact_mode.as_str(),
            "standard" | "android_default" | "legacy" | "custom"
        ) {
            return Err("contact_mode_invalid".to_string());
        }
        if self.ims.register.expires_seconds == 0 {
            return Err("register_expires_must_be_positive".to_string());
        }
        if self.ims.user_agent.trim().is_empty() {
            return Err("register_user_agent_required".to_string());
        }
        if (self.ims.register.include_pani_initial || self.ims.register.include_pani_authenticated)
            && self.ims.register.pani_identity_policy != AccessIdentityPolicy::Omit
            && self.ims.register.access_network_info.trim().is_empty()
        {
            return Err("access_network_info_required".to_string());
        }
        if self.ims.register.enable_cellular_network_info
            && self.ims.register.cni_identity_policy == AccessIdentityPolicy::Static
            && self
                .ims
                .register
                .cellular_network_info
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err("cellular_network_info_required_for_static_policy".to_string());
        }
        if (self.ims.register.sec_agree_mode == "required"
            || self.ims.register.require_sec_agree_headers
            || self.ims.register.proxy_require_sec_agree_headers)
            && self.ims.register.security_client_mechanisms.is_empty()
        {
            return Err("security_client_mechanism_required".to_string());
        }
        if self
            .ims
            .register
            .security_client_mechanisms
            .iter()
            .any(|mechanism| mechanism.split('/').count() != 4)
        {
            return Err("security_client_mechanism_invalid".to_string());
        }
        if let Some(imei) = self
            .identity
            .device_identity_imei
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            let digits = imei.trim();
            if digits.len() != 15 || !digits.chars().all(|c| c.is_ascii_digit()) {
                return Err("device_identity_imei_must_be_15_digits".to_string());
            }
        }
        if self.voice.preferred_codecs.is_empty() {
            return Err("voice_preferred_codecs_required".to_string());
        }
        if self
            .voice
            .preferred_codecs
            .iter()
            .any(|codec| AudioCodec::from_token(codec).is_none())
        {
            return Err("voice_codec_unsupported".to_string());
        }
        if self.voice.ptime_ms == 0 {
            return Err("voice_ptime_invalid".to_string());
        }
        let mut payload_types = std::collections::HashSet::new();
        for policy in &self.voice.codec_policies {
            let codec = AudioCodec::from_token(&policy.codec)
                .ok_or_else(|| "voice_codec_policy_unsupported".to_string())?;
            if policy
                .sample_rate
                .is_some_and(|sample_rate| sample_rate != codec.clock_rate())
            {
                return Err("voice_codec_policy_sample_rate_invalid".to_string());
            }
            if let Some(payload_type) = policy.payload_type {
                let valid = match codec.static_payload_type() {
                    Some(static_type) => payload_type == static_type,
                    None => (96..=127).contains(&payload_type),
                };
                if !valid {
                    return Err("voice_codec_policy_payload_type_invalid".to_string());
                }
                if !payload_types.insert(payload_type) {
                    return Err("voice_codec_policy_payload_type_duplicate".to_string());
                }
            }
        }
        // E911 is catalogued but deliberately not executed yet. Do not reject
        // an otherwise usable IMS profile because the product-side address
        // provisioning flow is still undecided.
        if !matches!(self.ut.authentication.as_str(), "none" | "digest_aka") {
            return Err("ut_authentication_unsupported".to_string());
        }
        if self.ut.enabled {
            let root = self
                .ut
                .xcap_root
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "ut_xcap_root_required".to_string())?;
            let parsed = url::Url::parse(root).map_err(|_| "ut_xcap_root_invalid".to_string())?;
            if parsed.scheme() != "https" || parsed.host_str().is_none() {
                return Err("ut_xcap_root_must_be_https".to_string());
            }
            if self
                .ut
                .document_selector
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
                || self
                    .ut
                    .namespace
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty())
            {
                return Err("ut_xcap_policy_incomplete".to_string());
            }
            if self.ut.authentication != "digest_aka" {
                return Err("ut_xcap_authentication_required".to_string());
            }
            let tls_rank = |value: &str| match value.trim() {
                "1.2" | "tls1.2" => Some(12_u8),
                "1.3" | "tls1.3" => Some(13_u8),
                _ => None,
            };
            let min_tls = tls_rank(&self.ut.tls_min_version)
                .ok_or_else(|| "ut_xcap_tls_version_invalid".to_string())?;
            let max_tls = tls_rank(&self.ut.tls_max_version)
                .ok_or_else(|| "ut_xcap_tls_version_invalid".to_string())?;
            if min_tls > max_tls {
                return Err("ut_xcap_tls_version_range_invalid".to_string());
            }
            if !self.ut.tls_builtin_roots
                && self
                    .ut
                    .tls_additional_ca_pem
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty())
            {
                return Err("ut_xcap_tls_trust_anchor_required".to_string());
            }
            let selectors = [
                self.ut.call_waiting_selector.as_deref(),
                self.ut.diversion_rule_selector.as_deref(),
                self.ut.oip_selector.as_deref(),
                self.ut.oir_selector.as_deref(),
            ];
            if self.ut.partial_update && selectors.iter().all(|value| value.is_none()) {
                return Err("ut_xcap_partial_selector_required".to_string());
            }
            for selector in selectors.into_iter().flatten() {
                if selector.trim().is_empty()
                    || selector.starts_with('/')
                    || selector.contains("://")
                    || selector.contains('#')
                    || selector.contains('\\')
                    || selector.contains("..")
                    || selector.chars().any(char::is_control)
                {
                    return Err("ut_xcap_partial_selector_invalid".to_string());
                }
            }
            if self
                .ut
                .diversion_rule_selector
                .as_deref()
                .is_some_and(|selector| !selector.contains("{rule-id}"))
            {
                return Err("ut_xcap_diversion_selector_template_invalid".to_string());
            }
        }
        Ok(())
    }

    /// Whether emergency-calling configuration is effectively mandatory for this
    /// carrier's country.
    ///
    /// In the US the FCC requires a VoWiFi registered address before emergency
    /// calling works, and carriers there gate registration on the entitlement
    /// exchange. Elsewhere it is normally optional, so the UI should prompt
    /// rather than block. MCC 310–316 is the North American (US) range.
    pub fn e911_expected(&self) -> bool {
        self.meta
            .mcc
            .parse::<u16>()
            .map(|mcc| (310..=316).contains(&mcc))
            .unwrap_or(false)
    }
}

const IKE_IDENTITY_TEMPLATE_PLACEHOLDERS: &[&str] = &[
    "{imsi}",
    "{mcc}",
    "{mnc}",
    "{mnc3}",
    "{plmn}",
    "{epdg_fqdn}",
    "{ims_domain}",
    "{ims_realm}",
];

fn validate_ike_identity_template(template: &str) -> Result<(), String> {
    let template = template.trim();
    if template.is_empty() || template.len() > 512 || template.chars().any(char::is_control) {
        return Err("ike_identity_template_invalid".to_string());
    }
    let mut remainder = template.to_string();
    for placeholder in IKE_IDENTITY_TEMPLATE_PLACEHOLDERS {
        remainder = remainder.replace(placeholder, "");
    }
    if remainder.contains('{') || remainder.contains('}') {
        return Err("ike_identity_template_placeholder_unsupported".to_string());
    }
    Ok(())
}

fn validate_single_line_wire_value(field: &str, value: &str) -> Result<(), String> {
    if value.chars().any(char::is_control) {
        return Err(format!("wire_value_contains_control:{field}"));
    }
    Ok(())
}

/// Accept `1.1.1.1` or `1.1.1.1:53`, defaulting the port when omitted.
pub fn parse_dns_server(value: &str) -> Option<std::net::SocketAddr> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(addr) = value.parse::<std::net::SocketAddr>() {
        return Some(addr);
    }
    value
        .parse::<std::net::IpAddr>()
        .ok()
        .map(|ip| std::net::SocketAddr::new(ip, 53))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::modems::ims::vowifi::profiles::{
        generate_standard_3gpp_profile, GB_EE_23433,
    };

    #[test]
    fn round_trips_a_builtin_profile_without_loss() {
        let record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.validate().expect("builtin profile must be valid");
        let interned = record.intern();
        assert_eq!(interned.meta.profile_id, GB_EE_23433.meta.profile_id);
        assert_eq!(interned.epdg.host, GB_EE_23433.epdg.host);
        assert_eq!(interned.ims.realm, GB_EE_23433.ims.realm);
        assert_eq!(
            interned.ikev2.ike_proposals,
            GB_EE_23433.ikev2.ike_proposals
        );
        assert_eq!(
            interned.ims.register.live_header_variant_set,
            GB_EE_23433.ims.register.live_header_variant_set
        );
    }

    #[test]
    fn json_round_trip_preserves_every_field() {
        let record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        let json = serde_json::to_string(&record).expect("serialize");
        let parsed: CarrierProfileRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, record);
    }

    #[test]
    fn interning_the_same_record_twice_returns_the_same_reference() {
        let record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        let first = record.intern();
        let second = record.intern();
        assert!(std::ptr::eq(first, second));
    }

    #[test]
    fn editing_a_record_produces_an_updated_profile() {
        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.meta.profile_id = "gb_ee_23433_edit_test".to_string();
        let before = record.intern();
        assert_eq!(before.epdg.port, 500);
        record.epdg.port = 4500;
        let after = record.intern();
        assert_eq!(after.epdg.port, 4500);
    }

    #[test]
    fn derived_profile_can_be_captured_as_a_record() {
        let derived = generate_standard_3gpp_profile("460", "01", 2);
        let record = CarrierProfileRecord::from_profile(derived);
        record.validate().expect("derived profile must be valid");
        assert_eq!(record.meta.plmn, "46001");
        assert_eq!(
            record.epdg.host,
            "epdg.epc.mnc001.mcc460.pub.3gppnetwork.org"
        );
        assert_eq!(record.ims.domain, "ims.mnc001.mcc460.3gppnetwork.org");
    }

    #[test]
    fn sec_agree_mode_and_contact_mode_are_constrained() {
        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        assert!(matches!(
            record.ims.register.sec_agree_mode.as_str(),
            "disabled" | "auto" | "required"
        ));
        record.ims.register.sec_agree_mode = "maybe".to_string();
        assert_eq!(record.validate().unwrap_err(), "sec_agree_mode_invalid");

        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.ims.register.contact_mode = "custom".to_string();
        record
            .validate()
            .expect("custom contact mode is catalog-valid");
        record.ims.register.contact_mode = "guessed".to_string();
        assert_eq!(record.validate().unwrap_err(), "contact_mode_invalid");
    }

    #[test]
    fn dns_servers_accept_bare_ip_or_ip_with_port() {
        assert_eq!(parse_dns_server("1.1.1.1").unwrap().port(), 53);
        assert_eq!(parse_dns_server("1.1.1.1:5353").unwrap().port(), 5353);
        assert!(parse_dns_server("not-an-ip").is_none());

        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.epdg.dns_servers = vec!["8.8.8.8".to_string(), "1.1.1.1:53".to_string()];
        record.validate().expect("valid dns list");
        // The single-value field mirrors the head of the list so callers that
        // only read one server agree with callers that read the list.
        let interned = record.intern();
        assert_eq!(interned.epdg.dns_server, Some("8.8.8.8"));
        assert_eq!(interned.epdg.dns_servers, &["8.8.8.8", "1.1.1.1:53"]);

        record.epdg.dns_servers = vec!["999.999.999.999".to_string()];
        assert!(record
            .validate()
            .unwrap_err()
            .starts_with("dns_server_invalid"));
    }

    #[test]
    fn device_identity_imei_must_be_fifteen_digits() {
        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.identity.device_identity_enabled = true;
        record.identity.device_identity_imei = Some("12345".to_string());
        assert_eq!(
            record.validate().unwrap_err(),
            "device_identity_imei_must_be_15_digits"
        );
        record.identity.device_identity_imei = Some("351234567890123".to_string());
        record.validate().expect("valid imei");
    }

    #[test]
    fn e911_is_read_only_metadata_and_is_only_expected_in_north_america() {
        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        // UK carrier: emergency configuration is optional.
        assert!(!record.e911_expected());
        record.e911.enabled = true;
        record.e911.websheet_host_policy = None;
        record
            .validate()
            .expect("E911 provisioning metadata must not block registration");
        record.e911.websheet_host_policy = Some("public_https".to_string());
        record.validate().expect("display metadata remains valid");

        // A US carrier is expected to carry emergency configuration.
        let mut us = CarrierProfileRecord::from_profile(&GB_EE_23433);
        us.meta.mcc = "310".to_string();
        us.meta.mnc = "260".to_string();
        us.meta.plmn = "310260".to_string();
        assert!(us.e911_expected());
    }

    #[test]
    fn register_expires_must_be_positive() {
        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        assert_eq!(record.ims.register.expires_seconds, 3600);
        record.ims.register.expires_seconds = 0;
        assert_eq!(
            record.validate().unwrap_err(),
            "register_expires_must_be_positive"
        );
    }

    #[test]
    fn older_database_json_without_new_fields_is_normalized() {
        // A row written before these fields existed must keep working, but
        // migration must happen through the database parser so field presence
        // can be distinguished from serde defaults.
        let record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        let mut value = serde_json::to_value(&record).expect("serialize");
        value.as_object_mut().unwrap().remove("schema_version");
        let register = value["ims"]["register"].as_object_mut().unwrap();
        for key in [
            "sec_agree_mode",
            "include_mmtel_features",
            "always_add_sip_instance",
            "enable_cellular_network_info",
            "expires_seconds",
            "access_network_info",
            "contact_mode",
            "temporary_status_codes",
            "forbidden_status_codes",
            "initial_reject_fallback_status_codes",
            "temporary_retry_seconds",
        ] {
            register.remove(key);
        }
        let ims = value["ims"].as_object_mut().unwrap();
        ims.remove("tcp_keepalive_seconds");
        ims.remove("options_ping_interval_seconds");

        let json = serde_json::to_string(&value).expect("serialize legacy row");
        let parsed = CarrierProfileRecord::from_database_json(&json).expect("migrate database row");
        assert_eq!(parsed.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(parsed.ims.register.sec_agree_mode, "auto");
        assert!(parsed.ims.register.include_mmtel_features);
        assert!(parsed.ims.register.always_add_sip_instance);
        assert!(!parsed.ims.register.enable_cellular_network_info);
        assert_eq!(parsed.ims.register.expires_seconds, 3600);
        assert_eq!(parsed.ims.register.access_network_info, "IEEE-802.11");
        assert_eq!(parsed.ims.tcp_keepalive_seconds, 30);
        assert_eq!(parsed.ims.register.forbidden_status_codes, vec![403]);
    }

    #[test]
    fn database_migration_preserves_explicit_optional_header_disables() {
        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.schema_version = 0;
        record.ims.register.include_mmtel_features = false;
        record.ims.register.always_add_sip_instance = false;
        record.ims.register.enable_cellular_network_info = false;
        record.ims.register.sec_agree_mode = "disabled".to_string();
        record.ims.register.require_sec_agree_headers = false;
        record.ims.register.proxy_require_sec_agree_headers = false;
        let json = serde_json::to_string(&record).expect("serialize legacy row");

        let parsed = CarrierProfileRecord::from_database_json(&json).expect("load database row");
        assert_eq!(parsed.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(!parsed.ims.register.include_mmtel_features);
        assert!(!parsed.ims.register.always_add_sip_instance);
        assert!(!parsed.ims.register.enable_cellular_network_info);
        assert_eq!(parsed.ims.register.sec_agree_mode, "disabled");
    }

    #[test]
    fn database_parser_rejects_future_schema_versions() {
        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.schema_version = CURRENT_SCHEMA_VERSION + 1;
        let json = serde_json::to_string(&record).expect("serialize future row");
        assert_eq!(
            CarrierProfileRecord::from_database_json(&json).unwrap_err(),
            format!(
                "carrier_profile_schema_unsupported:{}:{}",
                CURRENT_SCHEMA_VERSION + 1,
                CURRENT_SCHEMA_VERSION
            )
        );
    }

    #[test]
    fn database_parser_rejects_control_characters_in_wire_values() {
        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.ims.user_agent = "SimAdmin IMS\r\nX-Injected: yes".to_string();
        let json = serde_json::to_string(&record).expect("serialize malicious database row");
        assert_eq!(
            CarrierProfileRecord::from_database_json(&json).unwrap_err(),
            "wire_value_contains_control:ims.user_agent"
        );

        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.ims.register.contact_param_order = vec!["audio\nX-Injected: yes".to_string()];
        assert_eq!(
            record.validate_ims_only().unwrap_err(),
            "wire_value_contains_control:ims.register.contact_param_order"
        );

        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.ims.register.access_network_info = "IEEE-802.11\u{7f}".to_string();
        assert_eq!(
            record.validate_ims_only().unwrap_err(),
            "wire_value_contains_control:ims.register.access_network_info"
        );
    }

    #[test]
    fn validation_rejects_inconsistent_plmn_and_bad_fields() {
        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.meta.plmn = "99999".to_string();
        assert_eq!(record.validate().unwrap_err(), "plmn_mismatch");

        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.meta.mcc = "23".to_string();
        assert_eq!(record.validate().unwrap_err(), "mcc_must_be_three_digits");

        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.ims.transport = "sctp".to_string();
        assert_eq!(
            record.validate().unwrap_err(),
            "ims_transport_must_be_tcp_or_udp"
        );

        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.ikev2.ike_proposals.clear();
        assert_eq!(record.validate().unwrap_err(), "ike_proposals_required");
    }

    /// Plain `serde_json::from_value` cannot tell an absent switch from an
    /// authored `false`, and four of these default to `true`. That is why the
    /// API path must not use it -- kept as the demonstration of *why*
    /// `from_api_value` exists, with the refusal asserted separately below.
    #[test]
    fn plain_deserialization_of_a_partial_body_reenables_default_true_switches() {
        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.ims.register.include_pani_initial = false;
        record.ims.register.include_pani_authenticated = false;
        record.ims.register.include_p_preferred_identity = false;
        record.ims.register.always_add_sip_instance = false;
        record.ims.register.include_route_header = false;
        record.ims.register.enable_cellular_network_info = false;

        // Model a client that sends the record back without these fields.
        let mut value = serde_json::to_value(&record).expect("serialize");
        let register = value
            .pointer_mut("/ims/register")
            .and_then(serde_json::Value::as_object_mut)
            .expect("register object");
        for field in [
            "include_pani_initial",
            "include_pani_authenticated",
            "include_p_preferred_identity",
            "always_add_sip_instance",
            "include_route_header",
            "enable_cellular_network_info",
        ] {
            register.remove(field);
        }

        // This is the exact deserialization the axum handler performs.
        let parsed: CarrierProfileRecord =
            serde_json::from_value(value).expect("partial body still deserializes");
        let round_tripped = &parsed.ims.register;

        // The four with `default = "default_true"` flip back on. This is the
        // exposure, asserted rather than assumed.
        assert!(
            round_tripped.include_pani_initial,
            "absent include_pani_initial defaults back to true"
        );
        assert!(round_tripped.include_pani_authenticated);
        assert!(round_tripped.include_p_preferred_identity);
        assert!(round_tripped.always_add_sip_instance);

        // The two defaulting to false happen to survive, but only by accident of
        // their default matching the omit, not by presence-awareness.
        assert!(!round_tripped.include_route_header);
        assert!(!round_tripped.enable_cellular_network_info);

        assert_ne!(
            parsed, record,
            "the record must differ, which is precisely the problem"
        );
    }

    /// `from_api_value` closes the hazard above: a body missing any REGISTER
    /// switch is refused, and the error names every one that is absent so a
    /// caller needs one round trip to fix it.
    #[test]
    fn the_api_parser_refuses_a_body_missing_register_switches() {
        let record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        let complete = serde_json::to_value(&record).expect("serialize");

        // The complete body is still accepted, so the check cannot be passing
        // by rejecting everything.
        let parsed = CarrierProfileRecord::from_api_value(complete.clone())
            .expect("a complete body must stay acceptable");
        assert_eq!(parsed, record);

        // Every switch is individually required.
        for field in CarrierProfileRecord::REQUIRED_REGISTER_SWITCHES {
            let mut value = complete.clone();
            value
                .pointer_mut("/ims/register")
                .and_then(serde_json::Value::as_object_mut)
                .expect("register object")
                .remove(*field);
            let error = CarrierProfileRecord::from_api_value(value)
                .expect_err("a body missing {field} must be refused");
            assert!(
                error.starts_with("carrier_profile_register_switch_missing:"),
                "unexpected error for a missing {field}: {error}"
            );
            assert!(
                error.contains(field),
                "the error must name the missing field {field}: {error}"
            );
        }

        // Several missing at once are reported together, not one per round trip.
        let mut value = complete.clone();
        {
            let register = value
                .pointer_mut("/ims/register")
                .and_then(serde_json::Value::as_object_mut)
                .expect("register object");
            register.remove("include_pani_initial");
            register.remove("always_add_sip_instance");
        }
        let error =
            CarrierProfileRecord::from_api_value(value).expect_err("must refuse a partial body");
        assert!(error.contains("include_pani_initial"), "{error}");
        assert!(error.contains("always_add_sip_instance"), "{error}");

        // A body with no register section at all is refused distinctly, rather
        // than blamed on a missing switch.
        let mut value = complete;
        value
            .pointer_mut("/ims")
            .and_then(serde_json::Value::as_object_mut)
            .expect("ims object")
            .remove("register");
        assert_eq!(
            CarrierProfileRecord::from_api_value(value).expect_err("must refuse"),
            "carrier_profile_register_section_missing"
        );
    }

    /// The store's load path is `from_database_json`, which deserializes and
    /// then calls `normalize_legacy_database_record` with the *raw* JSON so it
    /// can tell an absent field from an authored `false`.
    ///
    /// `database_migration_preserves_explicit_optional_header_disables` already
    /// covers five switches on the legacy (`schema_version = 0`) path. This
    /// covers the current-schema path and extends to all nine, adding the four
    /// PANI/Route/P-Preferred-Identity switches, and asserts the whole record
    /// is unchanged so nothing else drifts on the way through storage.
    #[test]
    fn stored_omit_survives_the_database_load_path() {
        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.meta.profile_id = "test-omit-store-load".to_string();
        record.ims.register.include_pani_initial = false;
        record.ims.register.include_pani_authenticated = false;
        record.ims.register.include_route_header = false;
        record.ims.register.include_p_preferred_identity = false;
        record.ims.register.always_add_sip_instance = false;
        record.ims.register.enable_cellular_network_info = false;
        record.ims.register.require_sec_agree_headers = false;
        record.ims.register.proxy_require_sec_agree_headers = false;
        record.ims.register.sec_agree_mode = "disabled".to_string();

        let stored = serde_json::to_string(&record).expect("serialize for storage");
        let loaded =
            CarrierProfileRecord::from_database_json(&stored).expect("load stored omit record");
        let register = &loaded.ims.register;

        // `always_add_sip_instance` is the dangerous one: its serde default is
        // `true` and legacy normalization also forces `true` when the field is
        // absent, so only presence-awareness keeps the authored `false`.
        assert!(!register.always_add_sip_instance);
        assert!(!register.enable_cellular_network_info);
        assert!(!register.include_pani_initial);
        assert!(!register.include_pani_authenticated);
        assert!(!register.include_route_header);
        assert!(!register.include_p_preferred_identity);
        assert!(!register.require_sec_agree_headers);
        assert!(!register.proxy_require_sec_agree_headers);
        assert_eq!(register.sec_agree_mode, "disabled");
        assert_eq!(loaded, record);
    }

    /// A row written before a switch existed cannot express an omit for it, and
    /// must not be read as one. Absent `always_add_sip_instance` normalizes to
    /// `true`; absent `enable_cellular_network_info` normalizes to `false`,
    /// because CNI can disclose serving-cell data and must never be synthesized
    /// for a row that predates the switch. This is the documented asymmetry, so
    /// pin it — a future migration change has to be deliberate.
    #[test]
    fn a_legacy_row_missing_a_switch_is_not_read_as_an_omit() {
        let record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        let mut value = serde_json::to_value(&record).expect("serialize to value");
        let register = value
            .pointer_mut("/ims/register")
            .and_then(serde_json::Value::as_object_mut)
            .expect("register object");
        register.remove("always_add_sip_instance");
        register.remove("enable_cellular_network_info");
        assert!(!register.contains_key("always_add_sip_instance"));

        let loaded = CarrierProfileRecord::from_database_json(&value.to_string())
            .expect("legacy row must still load");

        assert!(
            loaded.ims.register.always_add_sip_instance,
            "an absent switch is no opinion, so the baseline true applies"
        );
        assert!(
            !loaded.ims.register.enable_cellular_network_info,
            "CNI must never be synthesized for a row that predates the switch"
        );
    }

    /// A carrier bundle's explicit `omit` reaches this record as `false`. Four
    /// of these switches carry `#[serde(default = "default_true")]`, so any
    /// layer that drops the field on the way through — an export/import round
    /// trip, a partial patch, a hand-edited row — turns the operator's "do not
    /// send" back into "send". Serialising and reparsing must keep every one of
    /// them false, and must not disturb the rest of the record.
    #[test]
    fn omitted_register_switches_survive_a_json_round_trip() {
        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.ims.register.include_pani_initial = false;
        record.ims.register.include_pani_authenticated = false;
        record.ims.register.include_route_header = false;
        record.ims.register.include_p_preferred_identity = false;
        record.ims.register.always_add_sip_instance = false;
        record.ims.register.enable_cellular_network_info = false;
        record.ims.register.require_sec_agree_headers = false;
        record.ims.register.proxy_require_sec_agree_headers = false;
        record.ims.register.sec_agree_mode = "disabled".to_string();

        let json = serde_json::to_string(&record).expect("serialize omit record");
        let parsed: CarrierProfileRecord =
            serde_json::from_str(&json).expect("deserialize omit record");
        let register = &parsed.ims.register;

        assert!(!register.include_pani_initial);
        assert!(!register.include_pani_authenticated);
        assert!(!register.include_route_header);
        assert!(!register.include_p_preferred_identity);
        assert!(!register.always_add_sip_instance);
        assert!(!register.enable_cellular_network_info);
        assert!(!register.require_sec_agree_headers);
        assert!(!register.proxy_require_sec_agree_headers);
        assert_eq!(register.sec_agree_mode, "disabled");

        // `disabled` suppresses the RFC 3329 offer at the live layer, but the
        // mechanism list is still data and must round-trip unchanged so an
        // operator can flip the mode back without re-entering it.
        assert_eq!(
            register.security_client_mechanisms,
            GB_EE_23433
                .ims
                .register
                .security_client_mechanisms
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
        assert_eq!(parsed, record);
    }
}
