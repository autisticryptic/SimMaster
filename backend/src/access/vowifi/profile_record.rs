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

use super::profiles::{
    self, CarrierProfile, CarrierProfileMeta, E911Policy, EpdgPolicy, Ikev2Policy, ImsPolicy,
    ProfileIdentityPolicy, RegisterPolicy, SmsPolicy, VoicePolicy,
};

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterPolicyRecord {
    pub supported_header: String,
    #[serde(default)]
    pub include_pani_authenticated: bool,
    #[serde(default)]
    pub strict_security_server_offer: bool,
    #[serde(default)]
    pub enable_initial_reject_fallback: bool,
    #[serde(default)]
    pub use_plain_digest_placeholder: bool,
    #[serde(default)]
    pub require_sec_agree_headers: bool,
    /// `auto` | `required` | `disabled`.
    #[serde(default = "default_sec_agree_mode")]
    pub sec_agree_mode: String,
    pub security_client_mechanisms: Vec<String>,
    pub live_header_variant_set: String,
    #[serde(default = "default_expires_seconds")]
    pub expires_seconds: u32,
    #[serde(default = "default_access_network_info")]
    pub access_network_info: String,
    /// `android_default` | `legacy`.
    #[serde(default = "default_contact_mode")]
    pub contact_mode: String,
    #[serde(default)]
    pub contact_param_order: Vec<String>,
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

fn default_expires_seconds() -> u32 {
    profiles::DEFAULT_REGISTER_EXPIRES_SECONDS
}

fn default_access_network_info() -> String {
    profiles::DEFAULT_ACCESS_NETWORK_INFO.to_string()
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
pub struct VoicePolicyRecord {
    #[serde(default)]
    pub vowifi_enabled: bool,
    #[serde(default)]
    pub carrier_fallback_enabled: bool,
    pub preferred_codecs: Vec<String>,
    #[serde(default)]
    pub amr_octet_align: bool,
    pub ptime_ms: u16,
    #[serde(default)]
    pub sip_endpoint_exposed: bool,
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
pub struct CarrierProfileRecord {
    pub meta: CarrierProfileMetaRecord,
    pub identity: ProfileIdentityPolicyRecord,
    pub epdg: EpdgPolicyRecord,
    pub ikev2: Ikev2PolicyRecord,
    pub ims: ImsPolicyRecord,
    pub sms: SmsPolicyRecord,
    pub voice: VoicePolicyRecord,
    pub e911: E911PolicyRecord,
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

impl CarrierProfileRecord {
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
                    include_pani_authenticated: self.ims.register.include_pani_authenticated,
                    strict_security_server_offer: self.ims.register.strict_security_server_offer,
                    enable_initial_reject_fallback: self
                        .ims
                        .register
                        .enable_initial_reject_fallback,
                    use_plain_digest_placeholder: self.ims.register.use_plain_digest_placeholder,
                    require_sec_agree_headers: self.ims.register.require_sec_agree_headers,
                    sec_agree_mode: intern_str(&self.ims.register.sec_agree_mode),
                    security_client_mechanisms: intern_list(
                        &self.ims.register.security_client_mechanisms,
                    ),
                    live_header_variant_set: intern_str(&self.ims.register.live_header_variant_set),
                    expires_seconds: self.ims.register.expires_seconds,
                    access_network_info: intern_str(&self.ims.register.access_network_info),
                    contact_mode: intern_str(&self.ims.register.contact_mode),
                    contact_param_order: intern_list(&self.ims.register.contact_param_order),
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
                amr_octet_align: self.voice.amr_octet_align,
                ptime_ms: self.voice.ptime_ms,
                sip_endpoint_exposed: self.voice.sip_endpoint_exposed,
            },
            e911: E911Policy {
                enabled: self.e911.enabled,
                provider: intern_opt(self.e911.provider.as_ref()),
                entitlement_url: intern_opt(self.e911.entitlement_url.as_ref()),
                websheet_host_policy: intern_opt(self.e911.websheet_host_policy.as_ref()),
            },
        }
    }

    /// Snapshot an existing (built-in or derived) profile as an editable record.
    /// This is how the built-ins are seeded into the database on first run.
    pub fn from_profile(profile: &CarrierProfile) -> Self {
        let to_owned_list =
            |values: &'static [&'static str]| values.iter().map(|v| v.to_string()).collect();
        Self {
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
                    include_pani_authenticated: profile.ims.register.include_pani_authenticated,
                    strict_security_server_offer: profile.ims.register.strict_security_server_offer,
                    enable_initial_reject_fallback: profile
                        .ims
                        .register
                        .enable_initial_reject_fallback,
                    use_plain_digest_placeholder: profile.ims.register.use_plain_digest_placeholder,
                    require_sec_agree_headers: profile.ims.register.require_sec_agree_headers,
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
                    contact_mode: profile.ims.register.contact_mode.to_string(),
                    contact_param_order: to_owned_list(profile.ims.register.contact_param_order),
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
                amr_octet_align: profile.voice.amr_octet_align,
                ptime_ms: profile.voice.ptime_ms,
                sip_endpoint_exposed: profile.voice.sip_endpoint_exposed,
            },
            e911: E911PolicyRecord {
                enabled: profile.e911.enabled,
                provider: profile.e911.provider.map(str::to_string),
                entitlement_url: profile.e911.entitlement_url.map(str::to_string),
                websheet_host_policy: profile.e911.websheet_host_policy.map(str::to_string),
            },
        }
    }

    /// Reject records that would produce an unusable profile. Called before
    /// anything is written to the database or handed to the runtime.
    pub fn validate(&self) -> Result<(), String> {
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
        if self.epdg.host.trim().is_empty() {
            return Err("epdg_host_required".to_string());
        }
        if self.epdg.port == 0 {
            return Err("epdg_port_invalid".to_string());
        }
        if self.ims.domain.trim().is_empty() || self.ims.realm.trim().is_empty() {
            return Err("ims_domain_and_realm_required".to_string());
        }
        if self.ikev2.ike_proposals.is_empty() {
            return Err("ike_proposals_required".to_string());
        }
        if self.ikev2.esp_proposals.is_empty() {
            return Err("esp_proposals_required".to_string());
        }
        if !matches!(self.ims.transport.as_str(), "tcp" | "udp") {
            return Err("ims_transport_must_be_tcp_or_udp".to_string());
        }
        if !matches!(self.epdg.ip_stack.as_str(), "ipv4" | "ipv6" | "ipv4v6") {
            return Err("epdg_ip_stack_invalid".to_string());
        }
        if !matches!(
            self.ims.register.sec_agree_mode.as_str(),
            "auto" | "required" | "disabled"
        ) {
            return Err("sec_agree_mode_invalid".to_string());
        }
        if !matches!(
            self.ims.register.contact_mode.as_str(),
            "android_default" | "legacy"
        ) {
            return Err("contact_mode_invalid".to_string());
        }
        if self.ims.register.expires_seconds == 0 {
            return Err("register_expires_must_be_positive".to_string());
        }
        for server in &self.epdg.dns_servers {
            if parse_dns_server(server).is_none() {
                return Err(format!("dns_server_invalid:{server}"));
            }
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
        if self.e911.enabled && self.e911.websheet_host_policy.is_none() {
            return Err("e911_websheet_host_policy_required_when_enabled".to_string());
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
    use crate::access::vowifi::profiles::{generate_standard_3gpp_profile, GB_EE_23433};

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
        assert_eq!(record.ims.register.sec_agree_mode, "auto");
        record.ims.register.sec_agree_mode = "maybe".to_string();
        assert_eq!(record.validate().unwrap_err(), "sec_agree_mode_invalid");

        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.ims.register.contact_mode = "custom".to_string();
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
    fn e911_requires_a_host_policy_and_is_only_expected_in_north_america() {
        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        // UK carrier: emergency configuration is optional.
        assert!(!record.e911_expected());
        record.e911.enabled = true;
        record.e911.websheet_host_policy = None;
        assert_eq!(
            record.validate().unwrap_err(),
            "e911_websheet_host_policy_required_when_enabled"
        );
        record.e911.websheet_host_policy = Some("public_https".to_string());
        record.validate().expect("valid once the policy is present");

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
    fn older_json_without_the_new_fields_still_loads_with_defaults() {
        // A record written before the auth fields existed must keep working.
        let record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        let mut value = serde_json::to_value(&record).expect("serialize");
        let register = value["ims"]["register"].as_object_mut().unwrap();
        for key in [
            "sec_agree_mode",
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

        let parsed: CarrierProfileRecord = serde_json::from_value(value).expect("deserialize");
        parsed.validate().expect("defaults must be valid");
        assert_eq!(parsed.ims.register.sec_agree_mode, "auto");
        assert_eq!(parsed.ims.register.expires_seconds, 3600);
        assert_eq!(parsed.ims.register.access_network_info, "IEEE-802.11");
        assert_eq!(parsed.ims.tcp_keepalive_seconds, 30);
        assert_eq!(parsed.ims.register.forbidden_status_codes, vec![403]);
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
}
