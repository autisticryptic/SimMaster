#![allow(dead_code)]

use std::fmt;

use super::profiles::CarrierProfile;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IkeIdentityError {
    EmptyImsi,
    InvalidImsi,
    ImsiPlmnMismatch,
}

impl fmt::Display for IkeIdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyImsi => write!(f, "IMSI is empty"),
            Self::InvalidImsi => write!(f, "IMSI has invalid shape"),
            Self::ImsiPlmnMismatch => write!(f, "IMSI does not match carrier profile PLMN"),
        }
    }
}

impl std::error::Error for IkeIdentityError {}

pub fn build_permanent_nai(
    profile: &'static CarrierProfile,
    imsi: &str,
) -> Result<String, IkeIdentityError> {
    let digits = imsi.trim();
    if digits.is_empty() {
        return Err(IkeIdentityError::EmptyImsi);
    }
    if digits.len() < 5 || digits.len() > 16 || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(IkeIdentityError::InvalidImsi);
    }
    if !digits.starts_with(profile.meta.plmn) {
        return Err(IkeIdentityError::ImsiPlmnMismatch);
    }

    Ok(format!(
        "0{}@nai.epc.mnc{}.mcc{}.3gppnetwork.org",
        digits,
        three_digit_mnc(profile),
        profile.meta.mcc
    ))
}

fn three_digit_mnc(profile: &'static CarrierProfile) -> String {
    format!("{:0>3}", profile.meta.mnc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::modems::softstack::vowifi::profiles::{GB_EE_23433, NL_VODAFONE_20404};

    #[test]
    fn builds_3gpp_permanent_nai_without_losing_mnc_leading_zero() {
        let nai = build_permanent_nai(&NL_VODAFONE_20404, "204041234567890").expect("nai");

        assert!(nai.starts_with("020404"));
        assert!(nai.ends_with("@nai.epc.mnc004.mcc204.3gppnetwork.org"));
    }

    #[test]
    fn rejects_identity_that_does_not_match_profile_plmn() {
        assert_eq!(
            build_permanent_nai(&GB_EE_23433, "204041234567890").unwrap_err(),
            IkeIdentityError::ImsiPlmnMismatch
        );
    }

    #[test]
    fn rejects_invalid_imsi_shape() {
        assert_eq!(
            build_permanent_nai(&GB_EE_23433, "23433abc").unwrap_err(),
            IkeIdentityError::InvalidImsi
        );
    }
}
