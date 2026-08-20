//! Field-level merge of the carrier catalog baseline with the per-SIM user
//! override, producing an immutable effective profile plus a source map.
//!
//! Resolution order (see `DEVELOPMENT_PLAN.md` P1.3):
//!
//! ```text
//! line_id → SimBindingKey
//!         → read-only carrier catalog baseline
//!         → per-SIM override (if present)
//!         → field-level merge + source map
//!         → access-specific validation
//!         → immutable EffectiveImsProfile
//! ```
//!
//! The result is owned (`String`), never a leaked `'static` object, so a user
//! editing an override does not accumulate permanent objects. The source map
//! lets the API explain where each effective value came from.

use crate::connectivity::modems::ims::vowifi::profiles::CarrierProfile;

use super::profile_override::{ImsAccessOverride, OverrideSource, SimOverride};

/// One effective value plus its origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveField {
    pub value: String,
    pub source: OverrideSource,
}

impl EffectiveField {
    fn catalog(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            source: OverrideSource::Catalog,
        }
    }

    fn override_(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            source: OverrideSource::SimOverride,
        }
    }
}

/// Effective VoWiFi connection facts merged from catalog + override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveVowifiProfile {
    pub profile_id: String,
    pub epdg_host: EffectiveField,
    pub epdg_port: u16,
    pub epdg_port_source: OverrideSource,
    pub apn: Option<EffectiveField>,
    pub ip_stack: EffectiveField,
    pub dns_servers: Vec<EffectiveField>,
}

/// Effective VoLTE/IMS connection facts merged from catalog + override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveImsProfile {
    pub profile_id: String,
    pub domain: EffectiveField,
    pub realm: EffectiveField,
    pub pcscf: Option<EffectiveField>,
    pub registrar: Option<EffectiveField>,
    /// APN used for the IMS bearer.
    pub ims_apn: Option<EffectiveField>,
    /// Preferred IP stack for this access. For LTE catalog rows this is the
    /// normalized `access.lte.ip_family` value.
    pub ip_stack: EffectiveField,
    /// Whether the user explicitly pinned a carrier profile.
    pub pinned_profile_id: Option<EffectiveField>,
}

/// Effective device identity resolved for one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveDeviceIdentity {
    /// The IMEI to present. `None` means "device IMEI unavailable".
    pub imei: Option<String>,
    pub source: OverrideSource,
}

/// Effective supplementary-service preferences that follow the SIM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveServices {
    pub call_waiting: Option<bool>,
    pub call_waiting_source: Option<OverrideSource>,
    pub caller_id_restriction: Option<bool>,
    pub caller_id_restriction_source: Option<OverrideSource>,
}

impl EffectiveServices {
    pub fn from_override(override_: Option<&SimOverride>) -> Self {
        match override_ {
            Some(override_) => Self {
                call_waiting: override_.services.call_waiting,
                call_waiting_source: override_
                    .services
                    .call_waiting
                    .map(|_| OverrideSource::SimOverride),
                caller_id_restriction: override_.services.caller_id_restriction,
                caller_id_restriction_source: override_
                    .services
                    .caller_id_restriction
                    .map(|_| OverrideSource::SimOverride),
            },
            None => Self {
                call_waiting: None,
                call_waiting_source: None,
                caller_id_restriction: None,
                caller_id_restriction_source: None,
            },
        }
    }
}

/// Resolve the effective VoWiFi profile for a line.
pub fn resolve_effective_vowifi_profile(
    catalog: &CarrierProfile,
    override_: Option<&SimOverride>,
) -> EffectiveVowifiProfile {
    let access = override_.map(|o| &o.ims_vowifi);
    let dns_override = access
        .and_then(|a| a.dns.as_ref())
        .filter(|dns| !dns.is_empty());
    let dns_servers = match dns_override {
        Some(servers) => servers
            .iter()
            .map(|s| EffectiveField::override_(s.clone()))
            .collect(),
        None => catalog
            .epdg
            .dns_servers
            .iter()
            .map(|s| EffectiveField::catalog(*s))
            .chain(
                catalog
                    .epdg
                    .dns_server
                    .iter()
                    .map(|s| EffectiveField::catalog(*s)),
            )
            .collect(),
    };
    let ip_stack = access
        .and_then(|a| a.ip_stack.as_deref())
        .filter(|s| !s.is_empty())
        .map(EffectiveField::override_)
        .unwrap_or_else(|| EffectiveField::catalog(catalog.epdg.ip_stack));

    let epdg_port = access
        .and_then(|a| a.epdg_port)
        .unwrap_or(catalog.epdg.port);
    let epdg_port_source = if access.and_then(|a| a.epdg_port).is_some() {
        OverrideSource::SimOverride
    } else {
        OverrideSource::Catalog
    };

    EffectiveVowifiProfile {
        profile_id: catalog.meta.profile_id.to_string(),
        epdg_host: access
            .and_then(|a| a.epdg_host.as_deref())
            .filter(|s| !s.is_empty())
            .map(EffectiveField::override_)
            .unwrap_or_else(|| EffectiveField::catalog(catalog.epdg.host)),
        epdg_port,
        epdg_port_source,
        apn: access
            .and_then(|a| a.apn.as_deref())
            .filter(|s| !s.is_empty())
            .map(EffectiveField::override_)
            .or_else(|| catalog.epdg.apn.map(EffectiveField::catalog)),
        ip_stack,
        dns_servers,
    }
}

/// IMS access whose independent override fields are being resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectiveImsAccess {
    Volte,
    Vowifi,
}

impl EffectiveImsAccess {
    fn override_from(self, override_: &SimOverride) -> &ImsAccessOverride {
        match self {
            Self::Volte => &override_.ims_volte,
            Self::Vowifi => &override_.ims_vowifi,
        }
    }
}

/// Resolve effective IMS identity/connection facts for one access. VoLTE and
/// VoWiFi deliberately use separate override branches even when the catalog
/// happens to contain identical values.
pub fn resolve_effective_ims_profile_for_access(
    catalog: &CarrierProfile,
    override_: Option<&SimOverride>,
    access_kind: EffectiveImsAccess,
) -> EffectiveImsProfile {
    let access = override_.map(|override_| access_kind.override_from(override_));
    EffectiveImsProfile {
        profile_id: catalog.meta.profile_id.to_string(),
        domain: access
            .and_then(|a| a.domain.as_deref())
            .filter(|s| !s.is_empty())
            .map(EffectiveField::override_)
            .unwrap_or_else(|| EffectiveField::catalog(catalog.ims.domain)),
        realm: access
            .and_then(|a| a.realm.as_deref())
            .filter(|s| !s.is_empty())
            .map(EffectiveField::override_)
            .unwrap_or_else(|| EffectiveField::catalog(catalog.ims.realm)),
        pcscf: access
            .and_then(|a| a.pcscf.as_ref())
            .filter(|v| !v.is_empty())
            .map(|v| EffectiveField::override_(v.join(",")))
            .or_else(|| catalog.ims.pcscf.map(EffectiveField::catalog)),
        registrar: access
            .and_then(|a| a.registrar.as_deref())
            .filter(|s| !s.is_empty())
            .map(EffectiveField::override_)
            .or_else(|| catalog.ims.registrar.map(EffectiveField::catalog)),
        ims_apn: access
            .and_then(|a| a.apn.as_deref())
            .filter(|s| !s.is_empty())
            .map(EffectiveField::override_)
            .or_else(|| catalog.epdg.apn.map(EffectiveField::catalog)),
        ip_stack: access
            .and_then(|a| a.ip_stack.as_deref())
            .filter(|s| !s.is_empty())
            .map(EffectiveField::override_)
            .unwrap_or_else(|| EffectiveField::catalog(catalog.epdg.ip_stack)),
        pinned_profile_id: access
            .and_then(|a| a.profile_id.as_deref())
            .filter(|s| !s.is_empty())
            .map(EffectiveField::override_),
    }
}

pub fn resolve_effective_ims_profile(
    catalog: &CarrierProfile,
    override_: Option<&SimOverride>,
) -> EffectiveImsProfile {
    resolve_effective_ims_profile_for_access(catalog, override_, EffectiveImsAccess::Volte)
}

pub fn resolve_effective_vowifi_ims_profile(
    catalog: &CarrierProfile,
    override_: Option<&SimOverride>,
) -> EffectiveImsProfile {
    resolve_effective_ims_profile_for_access(catalog, override_, EffectiveImsAccess::Vowifi)
}

/// Resolve the device identity for a line. Custom IMEI from the SIM override
/// wins; otherwise the device's own IMEI (from the modem binding) is used when
/// available. The source is never logged raw.
pub fn resolve_effective_device_identity(
    override_: Option<&SimOverride>,
    device_imei: Option<&str>,
) -> EffectiveDeviceIdentity {
    if let Some(custom) = override_
        .and_then(|o| o.ims_common.custom_imei.as_deref())
        .filter(|imei| is_valid_imei(imei))
    {
        return EffectiveDeviceIdentity {
            imei: Some(custom.to_string()),
            source: OverrideSource::SimOverride,
        };
    }
    match device_imei.map(str::trim).filter(|imei| !imei.is_empty()) {
        Some(imei) => EffectiveDeviceIdentity {
            imei: Some(imei.to_string()),
            source: OverrideSource::Modem,
        },
        None => EffectiveDeviceIdentity {
            imei: None,
            source: OverrideSource::Network,
        },
    }
}

/// Effective common facts that follow the SIM regardless of access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveCommon {
    pub voicemail_number: Option<String>,
    pub voicemail_number_source: Option<OverrideSource>,
}

#[cfg(test)]
fn resolve_effective_common(override_: Option<&SimOverride>) -> EffectiveCommon {
    resolve_effective_common_with_sources(override_, None, None)
}

/// Resolve voicemail dialing identity. User intent wins, then the active SIM's
/// own MBDN/AT+CSVM value, then the read-only carrier catalog fallback.
pub fn resolve_effective_common_with_sources(
    override_: Option<&SimOverride>,
    sim_voicemail_number: Option<&str>,
    catalog_voicemail_number: Option<&str>,
) -> EffectiveCommon {
    let candidate = override_
        .and_then(|value| value.ims_common.voicemail_number.as_deref())
        .map(|value| (value, OverrideSource::SimOverride))
        .or_else(|| sim_voicemail_number.map(|value| (value, OverrideSource::Modem)))
        .or_else(|| catalog_voicemail_number.map(|value| (value, OverrideSource::Catalog)))
        .and_then(|(value, source)| {
            let value = value.trim();
            (!value.is_empty()).then(|| (value.to_string(), source))
        });
    EffectiveCommon {
        voicemail_number: candidate.as_ref().map(|(value, _)| value.clone()),
        voicemail_number_source: candidate.map(|(_, source)| source),
    }
}

/// Effective emergency facts merged from catalog + override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveEmergency {
    /// User-entered civic address intent (never treated as carrier-confirmed).
    pub e911_address: Option<String>,
    /// True when the override carries a locally-saved address.
    pub address_saved_locally: bool,
    /// Origin of the address: `SimOverride` when set, `None` otherwise.
    pub address_source: Option<OverrideSource>,
}

pub fn resolve_effective_emergency(override_: Option<&SimOverride>) -> EffectiveEmergency {
    match override_ {
        Some(override_) => {
            let address = override_.emergency.e911_address.clone();
            let address_source = address
                .as_ref()
                .filter(|address| !address.trim().is_empty())
                .map(|_| OverrideSource::SimOverride);
            EffectiveEmergency {
                e911_address: address,
                address_saved_locally: address_source.is_some(),
                address_source,
            }
        }
        None => EffectiveEmergency {
            e911_address: None,
            address_saved_locally: false,
            address_source: None,
        },
    }
}

/// 15-digit IMEI with valid check digit (GSMA IMEI TAC/SNR/CDD).
pub fn is_valid_imei(imei: &str) -> bool {
    let cleaned = imei.trim();
    if cleaned.len() != 15 || !cleaned.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    luhn_check(cleaned)
}

fn luhn_check(digits: &str) -> bool {
    let mut sum = 0u32;
    let mut double = false;
    for digit in digits.chars().rev() {
        let mut value = digit.to_digit(10).unwrap_or(0);
        if double {
            value *= 2;
            if value > 9 {
                value -= 9;
            }
        }
        sum += value;
        double = !double;
    }
    sum % 10 == 0
}

/// Validate a user-edited override without touching the network or the store.
/// Returns a list of diagnosable problems; an empty list means the override is
/// acceptable to persist.
pub fn validate_override(override_: &SimOverride) -> Vec<String> {
    let mut problems = Vec::new();
    if let Some(imei) = override_.ims_common.custom_imei.as_deref() {
        if !imei.trim().is_empty() && !is_valid_imei(imei) {
            problems.push("custom_imei_must_be_15_digits".to_string());
        }
    }
    let vowifi = &override_.ims_vowifi;
    if vowifi.spoof_imsi
        && vowifi
            .custom_imsi
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
    {
        problems.push("ims_vowifi.custom_imsi_required_when_spoof_enabled".to_string());
    }
    if let Some(imsi) = vowifi.custom_imsi.as_deref() {
        let digits = imsi.trim();
        if !digits.is_empty()
            && (digits.len() < 5
                || digits.len() > 16
                || !digits.bytes().all(|byte| byte.is_ascii_digit()))
        {
            problems.push("ims_vowifi.custom_imsi_must_be_5_to_16_digits".to_string());
        }
    }
    if let Some(number) = override_.ims_common.voicemail_number.as_deref() {
        let number = number.trim();
        if !number.is_empty()
            && (number.len() > 32
                || !number
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'+' | b'*' | b'#')))
        {
            problems.push("voicemail_number_invalid".to_string());
        }
    }
    for (access, name) in [
        (&override_.ims_vowifi, "ims_vowifi"),
        (&override_.ims_volte, "ims_volte"),
    ] {
        validate_access(access, name, &mut problems);
    }
    if let Some(address) = override_.emergency.e911_address.as_deref() {
        let cleaned = address.trim();
        if !cleaned.is_empty() && cleaned.len() > 512 {
            problems.push("e911_address_too_long".to_string());
        }
    }
    problems
}

fn validate_access(access: &ImsAccessOverride, name: &str, problems: &mut Vec<String>) {
    if let Some(ip_stack) = access.ip_stack.as_deref() {
        if !matches!(ip_stack.trim(), "ipv4" | "ipv6" | "ipv4v6") {
            problems.push(format!("{name}.ip_stack_invalid"));
        }
    }
    if let Some(epdg_host) = access.epdg_host.as_deref() {
        if epdg_host.trim().is_empty() {
            problems.push(format!("{name}.epdg_host_must_not_be_empty"));
        }
    }
    if let Some(port) = access.epdg_port {
        if port == 0 {
            problems.push(format!("{name}.epdg_port_must_not_be_zero"));
        }
    }
    if let Some(pcscf) = access.pcscf.as_ref() {
        if pcscf.iter().any(|addr| addr.trim().is_empty()) {
            problems.push(format!("{name}.pcscf_must_not_contain_empty_entry"));
        }
    }
    for (field, value) in [
        ("domain", access.domain.as_deref()),
        ("realm", access.realm.as_deref()),
        ("registrar", access.registrar.as_deref()),
    ] {
        if value.is_some_and(|value| value.trim().is_empty()) {
            problems.push(format!("{name}.{field}_must_not_be_empty"));
        }
    }
}

/// Field map produced for the effective profile. Each entry pairs a logical
/// field name with the source that contributed its value.
#[allow(clippy::too_many_arguments)]
pub fn source_map_of(
    vowifi: &EffectiveVowifiProfile,
    volte_ims: &EffectiveImsProfile,
    vowifi_ims: &EffectiveImsProfile,
    identity: &EffectiveDeviceIdentity,
    common: &EffectiveCommon,
    services: &EffectiveServices,
    emergency: &EffectiveEmergency,
) -> Vec<(String, OverrideSource)> {
    let mut map = Vec::new();
    map.push(("profile_id".to_string(), OverrideSource::Catalog));
    push_field(&mut map, "vowifi.epdg_host", &vowifi.epdg_host);
    map.push(("vowifi.epdg_port".to_string(), vowifi.epdg_port_source));
    if let Some(apn) = &vowifi.apn {
        push_field(&mut map, "vowifi.apn", apn);
    }
    push_field(&mut map, "vowifi.ip_stack", &vowifi.ip_stack);
    for (index, server) in vowifi.dns_servers.iter().enumerate() {
        push_field(&mut map, &format!("vowifi.dns[{index}]"), server);
    }
    for (prefix, ims) in [("volte_ims", volte_ims), ("vowifi_ims", vowifi_ims)] {
        push_field(&mut map, &format!("{prefix}.domain"), &ims.domain);
        push_field(&mut map, &format!("{prefix}.realm"), &ims.realm);
        if let Some(pcscf) = &ims.pcscf {
            push_field(&mut map, &format!("{prefix}.pcscf"), pcscf);
        }
        if let Some(registrar) = &ims.registrar {
            push_field(&mut map, &format!("{prefix}.registrar"), registrar);
        }
        if let Some(apn) = &ims.ims_apn {
            push_field(&mut map, &format!("{prefix}.apn"), apn);
        }
        push_field(&mut map, &format!("{prefix}.ip_stack"), &ims.ip_stack);
        if let Some(pinned) = &ims.pinned_profile_id {
            push_field(&mut map, &format!("{prefix}.profile_id"), pinned);
        }
    }
    map.push(("identity.imei".to_string(), identity.source));
    if let Some(source) = common.voicemail_number_source {
        map.push(("common.voicemail_number".to_string(), source));
    }
    if let Some(source) = services.call_waiting_source {
        map.push(("services.call_waiting".to_string(), source));
    }
    if let Some(source) = services.caller_id_restriction_source {
        map.push(("services.caller_id_restriction".to_string(), source));
    }
    if let Some(source) = emergency.address_source {
        map.push(("emergency.e911_address".to_string(), source));
    }
    map
}

fn push_field(map: &mut Vec<(String, OverrideSource)>, name: &str, field: &EffectiveField) {
    map.push((name.to_string(), field.source));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::modems::ims::profile_override::ImsCommonOverride;
    use crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433;

    fn no_override_ref() -> Option<&'static SimOverride> {
        None
    }

    #[test]
    fn vowifi_merge_prefers_override_fields() {
        let mut override_ = SimOverride {
            ims_vowifi: ImsAccessOverride {
                epdg_host: Some("override.epdg.example".to_string()),
                epdg_port: Some(4500),
                ..Default::default()
            },
            ..Default::default()
        };
        let effective = resolve_effective_vowifi_profile(&GB_EE_23433, Some(&override_));
        assert_eq!(effective.epdg_host.value, "override.epdg.example");
        assert_eq!(effective.epdg_host.source, OverrideSource::SimOverride);
        assert_eq!(effective.epdg_port, 4500);
        assert_eq!(effective.epdg_port_source, OverrideSource::SimOverride);
        assert_eq!(effective.ip_stack.value, "ipv6");
        assert_eq!(effective.ip_stack.source, OverrideSource::Catalog);

        override_.ims_vowifi.epdg_host = None;
        override_.ims_vowifi.epdg_port = None;
        let effective = resolve_effective_vowifi_profile(&GB_EE_23433, Some(&override_));
        assert_eq!(
            effective.epdg_host.value,
            "epdg.epc.mnc033.mcc234.pub.3gppnetwork.org"
        );
        assert_eq!(effective.epdg_host.source, OverrideSource::Catalog);
        assert_eq!(effective.epdg_port, 500);
        assert_eq!(effective.epdg_port_source, OverrideSource::Catalog);
    }

    #[test]
    fn no_override_means_pure_catalog() {
        let vowifi = resolve_effective_vowifi_profile(&GB_EE_23433, no_override_ref());
        let ims = resolve_effective_ims_profile(&GB_EE_23433, no_override_ref());
        assert_eq!(vowifi.epdg_host.source, OverrideSource::Catalog);
        assert_eq!(vowifi.epdg_port_source, OverrideSource::Catalog);
        assert_eq!(
            vowifi
                .dns_servers
                .iter()
                .all(|d| d.source == OverrideSource::Catalog),
            true
        );
        assert_eq!(ims.domain.source, OverrideSource::Catalog);
        assert_eq!(ims.ip_stack.value, "ipv6");
        assert_eq!(ims.ip_stack.source, OverrideSource::Catalog);
        assert!(ims.pinned_profile_id.is_none());
    }

    #[test]
    fn dns_override_replaces_catalog_servers() {
        let override_ = SimOverride {
            ims_vowifi: ImsAccessOverride {
                dns: Some(vec!["9.9.9.9".to_string()]),
                ..Default::default()
            },
            ..Default::default()
        };
        let effective = resolve_effective_vowifi_profile(&GB_EE_23433, Some(&override_));
        assert_eq!(effective.dns_servers.len(), 1);
        assert_eq!(effective.dns_servers[0].value, "9.9.9.9");
        assert_eq!(effective.dns_servers[0].source, OverrideSource::SimOverride);
    }

    #[test]
    fn device_identity_prefers_custom_imei() {
        let override_ = SimOverride {
            ims_common: ImsCommonOverride {
                custom_imei: Some("490154203237518".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let identity = resolve_effective_device_identity(Some(&override_), Some("999999999999999"));
        assert_eq!(identity.imei.as_deref(), Some("490154203237518"));
        assert_eq!(identity.source, OverrideSource::SimOverride);
    }

    #[test]
    fn device_identity_uses_modem_imei_when_not_overridden() {
        let identity =
            resolve_effective_device_identity(no_override_ref(), Some("999999999999999"));
        assert_eq!(identity.imei.as_deref(), Some("999999999999999"));
        assert_eq!(identity.source, OverrideSource::Modem);
        let missing = resolve_effective_device_identity(no_override_ref(), None);
        assert!(missing.imei.is_none());
        assert_eq!(missing.source, OverrideSource::Network);
    }

    #[test]
    fn invalid_custom_imei_is_ignored() {
        let override_ = SimOverride {
            ims_common: ImsCommonOverride {
                custom_imei: Some("12345".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let identity = resolve_effective_device_identity(Some(&override_), Some("999999999999999"));
        assert_eq!(identity.imei.as_deref(), Some("999999999999999"));
        assert_eq!(identity.source, OverrideSource::Modem);
    }

    #[test]
    fn voicemail_source_order_is_override_then_sim_then_catalog() {
        let mut override_ = SimOverride::default();
        override_.ims_common.voicemail_number = Some("*86".to_string());
        let common =
            resolve_effective_common_with_sources(Some(&override_), Some("123"), Some("456"));
        assert_eq!(common.voicemail_number.as_deref(), Some("*86"));
        assert_eq!(
            common.voicemail_number_source,
            Some(OverrideSource::SimOverride)
        );

        override_.ims_common.voicemail_number = None;
        let common =
            resolve_effective_common_with_sources(Some(&override_), Some("123"), Some("456"));
        assert_eq!(common.voicemail_number.as_deref(), Some("123"));
        assert_eq!(common.voicemail_number_source, Some(OverrideSource::Modem));

        let common = resolve_effective_common_with_sources(None, None, Some("456"));
        assert_eq!(common.voicemail_number.as_deref(), Some("456"));
        assert_eq!(
            common.voicemail_number_source,
            Some(OverrideSource::Catalog)
        );
    }

    #[test]
    fn imei_validation_accepts_gsma_valid_and_rejects_bad_checksum() {
        assert!(is_valid_imei("490154203237518"));
        assert!(is_valid_imei("351234567890124"));
        assert!(!is_valid_imei("351234567890123"));
        assert!(!is_valid_imei("12345"));
        assert!(!is_valid_imei("35123456789012a"));
        assert!(!is_valid_imei("3512345678901234"));
    }

    #[test]
    fn validate_override_reports_diagnosable_problems() {
        let override_ = SimOverride {
            ims_common: ImsCommonOverride {
                custom_imei: Some("12345".to_string()),
                ..Default::default()
            },
            ims_vowifi: ImsAccessOverride {
                epdg_host: Some("  ".to_string()),
                epdg_port: Some(0),
                ..Default::default()
            },
            ..Default::default()
        };
        let problems = validate_override(&override_);
        assert!(problems.contains(&"custom_imei_must_be_15_digits".to_string()));
        assert!(problems.contains(&"ims_vowifi.epdg_host_must_not_be_empty".to_string()));
        assert!(problems.contains(&"ims_vowifi.epdg_port_must_not_be_zero".to_string()));
    }

    #[test]
    fn valid_override_has_no_problems() {
        let override_ = SimOverride {
            ims_vowifi: ImsAccessOverride {
                epdg_host: Some("epdg.example".to_string()),
                epdg_port: Some(4500),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(validate_override(&override_).is_empty());
    }

    #[test]
    fn spoofed_vowifi_imsi_requires_a_valid_decimal_identity() {
        let mut override_ = SimOverride::default();
        override_.ims_vowifi.spoof_imsi = true;
        assert!(validate_override(&override_)
            .contains(&"ims_vowifi.custom_imsi_required_when_spoof_enabled".to_string()));

        override_.ims_vowifi.custom_imsi = Some("46000invalid".to_string());
        assert!(validate_override(&override_)
            .contains(&"ims_vowifi.custom_imsi_must_be_5_to_16_digits".to_string()));

        override_.ims_vowifi.custom_imsi = Some("460001234567890".to_string());
        assert!(validate_override(&override_).is_empty());
    }

    #[test]
    fn source_map_lists_only_present_sources() {
        let override_ = SimOverride {
            ims_vowifi: ImsAccessOverride {
                epdg_host: Some("override.epdg.example".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let vowifi = resolve_effective_vowifi_profile(&GB_EE_23433, Some(&override_));
        let volte_ims = resolve_effective_ims_profile(&GB_EE_23433, Some(&override_));
        let vowifi_ims = resolve_effective_vowifi_ims_profile(&GB_EE_23433, Some(&override_));
        let identity = resolve_effective_device_identity(Some(&override_), None);
        let common = resolve_effective_common(Some(&override_));
        let services = EffectiveServices::from_override(Some(&override_));
        let emergency = resolve_effective_emergency(Some(&override_));
        let map = source_map_of(
            &vowifi,
            &volte_ims,
            &vowifi_ims,
            &identity,
            &common,
            &services,
            &emergency,
        );
        assert!(map.contains(&("vowifi.epdg_host".to_string(), OverrideSource::SimOverride)));
        assert!(map.contains(&("vowifi.epdg_port".to_string(), OverrideSource::Catalog)));
        assert!(map.contains(&("volte_ims.domain".to_string(), OverrideSource::Catalog)));
        assert!(map.contains(&("vowifi_ims.domain".to_string(), OverrideSource::Catalog)));
        assert!(map.contains(&("identity.imei".to_string(), OverrideSource::Network)));
    }

    #[test]
    fn volte_and_vowifi_ims_overrides_remain_independent() {
        let override_ = SimOverride {
            ims_volte: ImsAccessOverride {
                domain: Some("volte.example".to_string()),
                realm: Some("volte-realm.example".to_string()),
                registrar: Some("sip:volte-reg.example".to_string()),
                ..Default::default()
            },
            ims_vowifi: ImsAccessOverride {
                domain: Some("vowifi.example".to_string()),
                realm: Some("vowifi-realm.example".to_string()),
                registrar: Some("sip:vowifi-reg.example".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let volte = resolve_effective_ims_profile(&GB_EE_23433, Some(&override_));
        let vowifi = resolve_effective_vowifi_ims_profile(&GB_EE_23433, Some(&override_));
        assert_eq!(volte.domain.value, "volte.example");
        assert_eq!(volte.realm.value, "volte-realm.example");
        assert_eq!(volte.registrar.unwrap().value, "sip:volte-reg.example");
        assert_eq!(vowifi.domain.value, "vowifi.example");
        assert_eq!(vowifi.realm.value, "vowifi-realm.example");
        assert_eq!(vowifi.registrar.unwrap().value, "sip:vowifi-reg.example");
    }

    #[test]
    fn volte_connection_snapshot_uses_only_volte_access_fields() {
        let override_ = SimOverride {
            ims_volte: ImsAccessOverride {
                profile_id: Some("volte-profile".to_string()),
                apn: Some("volte-ims".to_string()),
                pcscf: Some(vec!["192.0.2.10".to_string(), "192.0.2.11".to_string()]),
                ..Default::default()
            },
            ims_vowifi: ImsAccessOverride {
                profile_id: Some("vowifi-profile".to_string()),
                apn: Some("vowifi-ims".to_string()),
                pcscf: Some(vec!["198.51.100.10".to_string()]),
                ..Default::default()
            },
            ..Default::default()
        };

        let effective = resolve_effective_ims_profile(&GB_EE_23433, Some(&override_));
        assert_eq!(effective.pinned_profile_id.unwrap().value, "volte-profile");
        assert_eq!(effective.ims_apn.unwrap().value, "volte-ims");
        assert_eq!(effective.pcscf.unwrap().value, "192.0.2.10,192.0.2.11");
    }

    #[test]
    fn volte_ip_stack_override_wins_over_lte_catalog_hint() {
        let override_ = SimOverride {
            ims_volte: ImsAccessOverride {
                ip_stack: Some("ipv4".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let effective = resolve_effective_ims_profile(&GB_EE_23433, Some(&override_));
        assert_eq!(effective.ip_stack.value, "ipv4");
        assert_eq!(effective.ip_stack.source, OverrideSource::SimOverride);
    }
}
