#![allow(dead_code)]

use std::fmt;

use super::profiles::{self, CarrierProfile};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IkeIdentityError {
    EmptyImsi,
    InvalidImsi,
    ImsiPlmnMismatch,
    PrivateIdentityTemplateRequired,
    InvalidIdentityTemplate,
}

impl fmt::Display for IkeIdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyImsi => write!(f, "IMSI is empty"),
            Self::InvalidImsi => write!(f, "IMSI has invalid shape"),
            Self::ImsiPlmnMismatch => write!(f, "IMSI does not match carrier profile PLMN"),
            Self::PrivateIdentityTemplateRequired => {
                write!(f, "private PLMN requires an explicit IKE identity template")
            }
            Self::InvalidIdentityTemplate => write!(f, "IKE identity template is invalid"),
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

    if let Some(template) = profile
        .ikev2
        .identity_template
        .map(str::trim)
        .filter(|template| !template.is_empty())
    {
        return expand_identity_template(profile, digits, template);
    }

    let realm = profiles::standard_epc_nai_realm(profile.meta.mcc, profile.meta.mnc)
        .ok_or(IkeIdentityError::PrivateIdentityTemplateRequired)?;
    Ok(format!("0{digits}@{realm}"))
}

fn expand_identity_template(
    profile: &'static CarrierProfile,
    imsi: &str,
    template: &str,
) -> Result<String, IkeIdentityError> {
    let expanded = template
        .replace("{imsi}", imsi)
        .replace("{mcc}", profile.meta.mcc)
        .replace("{mnc}", profile.meta.mnc)
        .replace("{mnc3}", &format!("{:0>3}", profile.meta.mnc))
        .replace("{plmn}", profile.meta.plmn)
        .replace("{epdg_fqdn}", profile.epdg.host)
        .replace("{ims_domain}", profile.ims.domain)
        .replace("{ims_realm}", profile.ims.realm);
    if expanded.is_empty()
        || expanded.len() > 512
        || expanded.contains('{')
        || expanded.contains('}')
        || expanded.chars().any(char::is_control)
    {
        return Err(IkeIdentityError::InvalidIdentityTemplate);
    }
    Ok(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::modems::ims::vowifi::{
        profile_record::CarrierProfileRecord,
        profiles::{GB_EE_23433, NL_VODAFONE_20404},
    };

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

    #[test]
    fn private_plmn_uses_explicit_template_with_live_sim_and_profile_values() {
        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.meta.profile_id = "ike-private-template-test".to_string();
        record.meta.mcc = "999".to_string();
        record.meta.mnc = "99".to_string();
        record.meta.mnc_len = 2;
        record.meta.plmn = "99999".to_string();
        record.ims.domain = "ims.private.example".to_string();
        record.ims.realm = "aka.private.example".to_string();
        record.epdg.host = "epdg.private.example".to_string();
        record.ikev2.identity_template =
            Some("private-{imsi}@{ims_realm};plmn={plmn};epdg={epdg_fqdn}".to_string());
        let profile = record.intern();

        assert_eq!(
            build_permanent_nai(profile, "999991234567890").unwrap(),
            "private-999991234567890@aka.private.example;plmn=99999;epdg=epdg.private.example"
        );
    }

    #[test]
    fn private_plmn_without_explicit_template_never_gets_public_nai_realm() {
        let mut record = CarrierProfileRecord::from_profile(&GB_EE_23433);
        record.meta.profile_id = "ike-private-missing-template-test".to_string();
        record.meta.mcc = "999".to_string();
        record.meta.mnc = "99".to_string();
        record.meta.mnc_len = 2;
        record.meta.plmn = "99999".to_string();
        record.ikev2.identity_template = None;
        let profile = record.intern();

        assert_eq!(
            build_permanent_nai(profile, "999991234567890").unwrap_err(),
            IkeIdentityError::PrivateIdentityTemplateRequired
        );
    }
}
