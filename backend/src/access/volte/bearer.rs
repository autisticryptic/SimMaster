//! VoLTE IMS APN bearer management via ModemManager.
//!
//! Clean-room from 3GPP TS 24.301 (EPS bearers) + the ModemManager D-Bus API.
//! The reference "borrows" bearer setup from ModemManager: it starts an IMS APN
//! bearer (`apn=ims`), reads the assigned IP/PCO settings, and cleans up stale
//! disconnected bearers. The heavy IO lives behind `#[cfg(unix)]`; the pure
//! bits (APN selection, bearer object-path validation, roaming policy) are
//! testable everywhere.
//!
//! Observed anchors: `--wds-start-network=apn=ims,3gpp-profile=`,
//! `SIMADMIN_MM_IMS_BEARER`, `/org/freedesktop/ModemManager1/Bearer/`,
//! `recreating IMS bearer to match roaming policy`,
//! `Deleted stale disconnected IMS bearer`.

use super::errors::{code, VolteError};

/// The IMS APN used for the dedicated IMS bearer.
pub const IMS_APN: &str = "ims";

/// Environment override for the bearer object path (matches the reference's
/// `SIMADMIN_MM_IMS_BEARER`), letting an operator/tester pin a specific bearer.
pub const BEARER_ENV: &str = "SIMADMIN_MM_IMS_BEARER";

/// ModemManager bearer object-path prefix.
pub const BEARER_PATH_PREFIX: &str = "/org/freedesktop/ModemManager1/Bearer/";

/// Requested IMS bearer parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BearerRequest {
    pub apn: String,
    pub allow_roaming: bool,
    /// Optional 3GPP profile id (as in `--wds-start-network=...,3gpp-profile=<n>`).
    pub profile_id: Option<u32>,
}

impl Default for BearerRequest {
    fn default() -> Self {
        Self {
            apn: IMS_APN.to_string(),
            allow_roaming: false,
            profile_id: None,
        }
    }
}

impl BearerRequest {
    /// Build the `--wds-start-network` argument observed in the reference.
    pub fn wds_start_network_arg(&self) -> String {
        match self.profile_id {
            Some(id) => format!("--wds-start-network=apn={},3gpp-profile={}", self.apn, id),
            None => format!("--wds-start-network=apn={}", self.apn),
        }
    }
}

/// Validate a ModemManager bearer object path.
pub fn is_valid_bearer_path(path: &str) -> bool {
    path.starts_with(BEARER_PATH_PREFIX)
        && path[BEARER_PATH_PREFIX.len()..]
            .chars()
            .all(|c| c.is_ascii_digit())
        && path.len() > BEARER_PATH_PREFIX.len()
}

/// Read the bearer-path override from the environment, if set and valid.
pub fn bearer_path_override() -> Option<String> {
    let raw = std::env::var(BEARER_ENV).ok()?;
    let trimmed = raw.trim();
    if is_valid_bearer_path(trimmed) {
        Some(trimmed.to_string())
    } else {
        None
    }
}

/// Roaming gate: if the network is roaming and roaming is not allowed, the IMS
/// bearer must not be brought up (mirrors `bearer_roaming_forbidden`).
pub fn check_roaming(is_roaming: bool, allow_roaming: bool) -> Result<(), VolteError> {
    if is_roaming && !allow_roaming {
        return Err(VolteError::new(code::RUNTIME_MM_BEARER_ROAMING_FORBIDDEN));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wds_arg_with_and_without_profile() {
        let r = BearerRequest::default();
        assert_eq!(r.wds_start_network_arg(), "--wds-start-network=apn=ims");
        let r2 = BearerRequest {
            apn: "ims".to_string(),
            allow_roaming: true,
            profile_id: Some(3),
        };
        assert_eq!(
            r2.wds_start_network_arg(),
            "--wds-start-network=apn=ims,3gpp-profile=3"
        );
    }

    #[test]
    fn bearer_path_validation() {
        assert!(is_valid_bearer_path(
            "/org/freedesktop/ModemManager1/Bearer/1"
        ));
        assert!(is_valid_bearer_path(
            "/org/freedesktop/ModemManager1/Bearer/42"
        ));
        assert!(!is_valid_bearer_path(
            "/org/freedesktop/ModemManager1/Bearer/"
        ));
        assert!(!is_valid_bearer_path(
            "/org/freedesktop/ModemManager1/Bearer/abc"
        ));
        assert!(!is_valid_bearer_path("/wrong/prefix/1"));
    }

    #[test]
    fn roaming_gate() {
        assert!(check_roaming(false, false).is_ok());
        assert!(check_roaming(true, true).is_ok());
        assert!(check_roaming(false, true).is_ok());
        assert_eq!(
            check_roaming(true, false).unwrap_err().code(),
            code::RUNTIME_MM_BEARER_ROAMING_FORBIDDEN
        );
    }

    #[test]
    fn default_request_uses_ims_apn() {
        assert_eq!(BearerRequest::default().apn, "ims");
        assert!(!BearerRequest::default().allow_roaming);
    }
}
