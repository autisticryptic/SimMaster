//! Policy-gated presentation of the effective device identity.
//!
//! The effective device identity (`custom_imei` → modem IMEI → unavailable) is
//! a device-level fact shared by every access leg. It is **never** a
//! subscription identity: it must not replace the IKE EAP-AKA permanent NAI,
//! IMPI, IMPU, ICCID or IMSI, and the modem hardware IMEI is never changed via
//! AT/QMI.
//!
//! This module only renders the identity at protocol positions that a carrier
//! profile explicitly requests (`DeviceIdentityPolicy`). When the policy is not
//! enabled the output is empty so existing wire behavior is unchanged.
//!
//! Neutrality: this module is transport-agnostic and only depends on the shared
//! IMS core types. Access legs map their resolved values into
//! [`DeviceIdentityInput`] before asking for a presentation.

use std::fmt;

/// Resolved device identity plus the origin, as produced by the P1.3
/// `resolve_effective_device_identity` merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdentityInput {
    /// The IMEI to present. `None` means "device IMEI unavailable".
    pub imei: Option<String>,
    /// Where the value came from (`custom`, `modem`, or `unavailable`).
    pub source: DeviceIdentitySource,
}

/// Origin of the effective device identity. Never contains the raw IMEI so it
/// is safe for logs and API responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceIdentitySource {
    Custom,
    Modem,
    Unavailable,
}

impl DeviceIdentitySource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::Modem => "modem",
            Self::Unavailable => "unavailable",
        }
    }
}

impl fmt::Display for DeviceIdentitySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Carrier policy that decides *where* a device identity may be presented.
/// Derived from the per-carrier `ProfileIdentityPolicy` record; all flags
/// default to disabled so unknown carriers keep the previous wire behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceIdentityPolicy {
    /// Present a TS.43 / IKE device identity during IKE_AUTH.
    pub ike_device_identity_enabled: bool,
    /// Emit a GSMA-format `+sip.instance="<urn:imei:...>"` when true. When
    /// false the RFC 5626 `urn:uuid` instance id is used.
    pub imei_sip_instance_enabled: bool,
    /// Include the IMEI in TS.43 device information.
    pub ts43_device_info_enabled: bool,
}

impl DeviceIdentityPolicy {
    pub const fn none() -> Self {
        Self {
            ike_device_identity_enabled: false,
            imei_sip_instance_enabled: false,
            ts43_device_info_enabled: false,
        }
    }
}

/// Renderings of the device identity at specific protocol positions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceIdentityPresentation {
    /// IKE_AUTH IDi candidate or vendor/device identity string, when the policy
    /// enables it. Empty means "do not present".
    pub ike_device_identity: Option<String>,
    /// `+sip.instance` value. `Some("<urn:imei:...>")` when the policy requests
    /// the GSMA IMEI form; otherwise `None` (caller keeps its `urn:uuid`).
    pub sip_instance_imei: Option<String>,
    /// TS.43 device-information IMEI field, when enabled.
    pub ts43_imei: Option<String>,
}

/// Render the device identity at every policy-gated position. When the policy
/// is `none()` or the identity is unavailable, the result is all-`None`.
pub fn present_device_identity(
    identity: &DeviceIdentityInput,
    policy: DeviceIdentityPolicy,
) -> DeviceIdentityPresentation {
    let Some(imei) = identity.imei.as_deref().filter(|imei| is_valid_imei(imei)) else {
        return DeviceIdentityPresentation::default();
    };
    let mut presentation = DeviceIdentityPresentation::default();
    if policy.ike_device_identity_enabled {
        presentation.ike_device_identity = Some(imei.to_string());
    }
    if policy.imei_sip_instance_enabled {
        presentation.sip_instance_imei = Some(format!("<urn:imei:{imei}>"));
    }
    if policy.ts43_device_info_enabled {
        presentation.ts43_imei = Some(imei.to_string());
    }
    presentation
}

/// A `+sip.instance` value that is stable for one subscription across access
/// legs and across restarts.
///
/// RFC 5626 §4.1: the instance id identifies the *UE*, not the registration. A
/// device that registers the same IMPU from two access legs (VoLTE over the LTE
/// bearer and VoWiFi over the ePDG) must present the same instance id, or the
/// S-CSCF keeps two independent bindings for one identity and terminating calls
/// are forked to — or delivered at — whichever binding the TAS happens to pick.
/// A freshly randomised uuid per registration also loses the binding's identity
/// across a restart, so the network keeps a stale contact for the old one.
///
/// When carrier policy asks for the GSMA IMEI form, that is already stable and
/// is used as-is. Otherwise the uuid is derived from the IMPI, which is the
/// subscription's own permanent identity: an RFC 4122 version 3 (MD5
/// name-based) uuid over a fixed namespace string. The IMPI never reaches the
/// wire through this value — MD5 is one-way here, and the input is an identity
/// the UE already sends in `Authorization` anyway.
pub fn stable_sip_instance(impi: &str, imei: Option<&str>, prefer_imei: bool) -> String {
    if prefer_imei {
        if let Some(imei) = imei.map(str::trim).filter(|imei| is_valid_imei(imei)) {
            return format!("urn:imei:{imei}");
        }
    }
    format!("urn:uuid:{}", name_based_uuid_v3(impi.trim()))
}

/// RFC 4122 §4.3 name-based UUID using MD5 (version 3).
fn name_based_uuid_v3(name: &str) -> String {
    // A fixed, project-local namespace so the digest cannot collide with any
    // other MD5 use in this codebase.
    let mut input = Vec::with_capacity(name.len() + 32);
    input.extend_from_slice(b"simadmin/ims/sip-instance/v1:");
    input.extend_from_slice(name.as_bytes());
    let mut bytes = *md5::compute(&input);
    // Version 3 and the RFC 4122 variant, per §4.3 steps 4 and 5.
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// 15-digit IMEI with a valid GSMA Luhn check digit. Kept local so this module
/// has no dependency on the access-leg override code.
pub fn is_valid_imei(imei: &str) -> bool {
    let cleaned = imei.trim();
    if cleaned.len() != 15 || !cleaned.bytes().all(|byte| byte.is_ascii_digit()) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(imei: &str) -> DeviceIdentityInput {
        DeviceIdentityInput {
            imei: Some(imei.to_string()),
            source: DeviceIdentitySource::Custom,
        }
    }

    fn modem() -> DeviceIdentityInput {
        DeviceIdentityInput {
            imei: Some("490154203237518".to_string()),
            source: DeviceIdentitySource::Modem,
        }
    }

    fn unavailable() -> DeviceIdentityInput {
        DeviceIdentityInput {
            imei: None,
            source: DeviceIdentitySource::Unavailable,
        }
    }

    #[test]
    fn stable_sip_instance_is_identical_for_both_access_legs() {
        // The whole point: the VoLTE leg and the VoWiFi leg call this
        // independently, and must arrive at the same instance id or the S-CSCF
        // holds two RFC 5626 bindings for one IMPU.
        let impi = "502120000000001@ims.mnc012.mcc502.3gppnetwork.org";
        let volte = stable_sip_instance(impi, None, false);
        let vowifi = stable_sip_instance(impi, None, false);
        assert_eq!(volte, vowifi);
        assert!(volte.starts_with("urn:uuid:"));
    }

    #[test]
    fn stable_sip_instance_survives_a_restart_and_differs_per_subscription() {
        let a = stable_sip_instance("502120000000001@ims.example", None, false);
        let b = stable_sip_instance("502120000000002@ims.example", None, false);
        assert_ne!(a, b, "different IMPIs must not share an instance id");
        // Same input, computed again as a fresh process would: unchanged.
        assert_eq!(a, stable_sip_instance("502120000000001@ims.example", None, false));
    }

    #[test]
    fn stable_sip_instance_uuid_is_rfc4122_version_3() {
        let value = stable_sip_instance("502120000000001@ims.example", None, false);
        let uuid = value.strip_prefix("urn:uuid:").expect("urn:uuid prefix");
        let fields: Vec<&str> = uuid.split('-').collect();
        let lengths: Vec<usize> = fields.iter().map(|field| field.len()).collect();
        assert_eq!(lengths, vec![8, 4, 4, 4, 12], "uuid shape: {uuid}");
        assert!(uuid.bytes().all(|byte| byte == b'-' || byte.is_ascii_hexdigit()));
        // Version nibble (§4.3 step 4) and variant bits (step 5).
        assert!(fields[2].starts_with('3'), "version 3 expected, got {uuid}");
        assert!(
            matches!(fields[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'),
            "RFC 4122 variant expected, got {uuid}"
        );
    }

    #[test]
    fn stable_sip_instance_never_leaks_the_impi() {
        let impi = "502120000000001@ims.example";
        let value = stable_sip_instance(impi, None, false);
        assert!(!value.contains("502120000000001"));
        assert!(!value.contains("ims.example"));
    }

    #[test]
    fn stable_sip_instance_prefers_imei_only_when_policy_asks_and_imei_is_valid() {
        let impi = "502120000000001@ims.example";
        // Policy off: a valid IMEI is ignored.
        assert!(stable_sip_instance(impi, Some("490154203237518"), false).starts_with("urn:uuid:"));
        // Policy on with a valid IMEI: GSMA form.
        assert_eq!(
            stable_sip_instance(impi, Some("490154203237518"), true),
            "urn:imei:490154203237518"
        );
        // Policy on but the IMEI fails Luhn/length: fall back to the derived
        // uuid, still stable rather than random.
        let short = stable_sip_instance(impi, Some("12345"), true);
        assert!(short.starts_with("urn:uuid:"));
        assert_eq!(short, stable_sip_instance(impi, None, false));
        // Absent IMEI behaves the same way.
        assert_eq!(
            stable_sip_instance(impi, None, true),
            stable_sip_instance(impi, None, false)
        );
    }

    #[test]
    fn source_strings_never_contain_raw_imei() {
        assert_eq!(DeviceIdentitySource::Custom.as_str(), "custom");
        assert_eq!(DeviceIdentitySource::Modem.as_str(), "modem");
        assert_eq!(DeviceIdentitySource::Unavailable.as_str(), "unavailable");
    }

    #[test]
    fn all_none_policy_renders_nothing_even_with_imei() {
        let identity = modem();
        let presentation = present_device_identity(&identity, DeviceIdentityPolicy::none());
        assert_eq!(presentation, DeviceIdentityPresentation::default());
    }

    #[test]
    fn ike_policy_renders_imei_for_ike_auth() {
        let identity = modem();
        let policy = DeviceIdentityPolicy {
            ike_device_identity_enabled: true,
            ..DeviceIdentityPolicy::none()
        };
        let presentation = present_device_identity(&identity, policy);
        assert_eq!(
            presentation.ike_device_identity.as_deref(),
            Some("490154203237518")
        );
        assert_eq!(presentation.sip_instance_imei, None);
        assert_eq!(presentation.ts43_imei, None);
    }

    #[test]
    fn imei_sip_instance_uses_gsma_urn_format() {
        let identity = custom("351234567890124");
        let policy = DeviceIdentityPolicy {
            imei_sip_instance_enabled: true,
            ..DeviceIdentityPolicy::none()
        };
        let presentation = present_device_identity(&identity, policy);
        assert_eq!(
            presentation.sip_instance_imei.as_deref(),
            Some("<urn:imei:351234567890124>")
        );
    }

    #[test]
    fn ts43_policy_renders_imei_for_device_info() {
        let identity = modem();
        let policy = DeviceIdentityPolicy {
            ts43_device_info_enabled: true,
            ..DeviceIdentityPolicy::none()
        };
        let presentation = present_device_identity(&identity, policy);
        assert_eq!(presentation.ts43_imei.as_deref(), Some("490154203237518"));
    }

    #[test]
    fn unavailable_identity_renders_nothing_even_when_policy_enabled() {
        let identity = unavailable();
        let policy = DeviceIdentityPolicy {
            ike_device_identity_enabled: true,
            imei_sip_instance_enabled: true,
            ts43_device_info_enabled: true,
        };
        assert_eq!(
            present_device_identity(&identity, policy),
            DeviceIdentityPresentation::default()
        );
    }

    #[test]
    fn invalid_custom_imei_renders_nothing() {
        let identity = custom("12345");
        let policy = DeviceIdentityPolicy {
            imei_sip_instance_enabled: true,
            ..DeviceIdentityPolicy::none()
        };
        assert_eq!(
            present_device_identity(&identity, policy),
            DeviceIdentityPresentation::default()
        );
    }

    #[test]
    fn is_valid_imei_accepts_gsma_valid_and_rejects_bad_checksum() {
        assert!(is_valid_imei("490154203237518"));
        assert!(is_valid_imei("351234567890124"));
        assert!(!is_valid_imei("351234567890123"));
        assert!(!is_valid_imei("12345"));
        assert!(!is_valid_imei("35123456789012a"));
    }
}
