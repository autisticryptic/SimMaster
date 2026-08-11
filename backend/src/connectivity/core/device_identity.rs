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
