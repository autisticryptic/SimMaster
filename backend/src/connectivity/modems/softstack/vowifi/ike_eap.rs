#![allow(dead_code)]

use std::fmt;

use serde::Serialize;

pub const EAP_TYPE_AKA: u8 = 23;
pub const EAP_AKA_HEADER_LEN: usize = 8;
pub const EAP_ATTR_HEADER_LEN: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EapCode {
    Request,
    Response,
    Success,
    Failure,
    Unknown(u8),
}

impl EapCode {
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Request,
            2 => Self::Response,
            3 => Self::Success,
            4 => Self::Failure,
            other => Self::Unknown(other),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Response => "response",
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Unknown(_) => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EapAkaSubtype {
    Challenge,
    AuthenticationReject,
    SynchronizationFailure,
    Identity,
    Notification,
    Reauthentication,
    ClientError,
    Unknown(u8),
}

impl EapAkaSubtype {
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Challenge,
            2 => Self::AuthenticationReject,
            4 => Self::SynchronizationFailure,
            5 => Self::Identity,
            12 => Self::Notification,
            13 => Self::Reauthentication,
            14 => Self::ClientError,
            other => Self::Unknown(other),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Challenge => "challenge",
            Self::AuthenticationReject => "authentication_reject",
            Self::SynchronizationFailure => "synchronization_failure",
            Self::Identity => "identity",
            Self::Notification => "notification",
            Self::Reauthentication => "reauthentication",
            Self::ClientError => "client_error",
            Self::Unknown(_) => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EapAkaAttributeSummary {
    pub attribute_type: u8,
    pub role: &'static str,
    pub units: u8,
    pub value_bytes: usize,
    pub value_redacted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EapAkaPacketSummary {
    pub code: &'static str,
    pub identifier: u8,
    pub method: &'static str,
    pub subtype: &'static str,
    pub attribute_count: usize,
    pub attributes: Vec<EapAkaAttributeSummary>,
    pub raw_len: usize,
    pub secret_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EapAkaError {
    Truncated,
    LengthMismatch { declared: usize, actual: usize },
    NotAkaMethod(u8),
    MissingSubtype,
    TruncatedAttribute,
    InvalidAttributeLength { attribute_type: u8, units: u8 },
}

impl fmt::Display for EapAkaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(f, "truncated EAP packet"),
            Self::LengthMismatch { declared, actual } => {
                write!(f, "EAP length mismatch declared={declared} actual={actual}")
            }
            Self::NotAkaMethod(method) => write!(f, "unsupported EAP method {method}"),
            Self::MissingSubtype => write!(f, "missing EAP-AKA subtype"),
            Self::TruncatedAttribute => write!(f, "truncated EAP-AKA attribute"),
            Self::InvalidAttributeLength {
                attribute_type,
                units,
            } => write!(
                f,
                "invalid EAP-AKA attribute length type={attribute_type} units={units}"
            ),
        }
    }
}

impl std::error::Error for EapAkaError {}

pub fn parse_eap_aka_summary(input: &[u8]) -> Result<EapAkaPacketSummary, EapAkaError> {
    if input.len() < 4 {
        return Err(EapAkaError::Truncated);
    }

    let code = EapCode::from_u8(input[0]);
    let identifier = input[1];
    let declared = u16::from_be_bytes(input[2..4].try_into().expect("slice length")) as usize;
    if declared != input.len() {
        return Err(EapAkaError::LengthMismatch {
            declared,
            actual: input.len(),
        });
    }

    if matches!(code, EapCode::Success | EapCode::Failure) {
        return Ok(EapAkaPacketSummary {
            code: code.as_str(),
            identifier,
            method: "none",
            subtype: "none",
            attribute_count: 0,
            attributes: Vec::new(),
            raw_len: input.len(),
            secret_values_policy: "raw_eap_not_serialized",
        });
    }

    if input.len() < EAP_AKA_HEADER_LEN {
        return Err(EapAkaError::MissingSubtype);
    }
    let method = input[4];
    if method != EAP_TYPE_AKA {
        return Err(EapAkaError::NotAkaMethod(method));
    }

    let subtype = EapAkaSubtype::from_u8(input[5]);
    let attributes = summarize_attributes(&input[EAP_AKA_HEADER_LEN..])?;

    Ok(EapAkaPacketSummary {
        code: code.as_str(),
        identifier,
        method: "aka",
        subtype: subtype.as_str(),
        attribute_count: attributes.len(),
        attributes,
        raw_len: input.len(),
        secret_values_policy: "rand_autn_res_ck_ik_never_serialized",
    })
}

fn summarize_attributes(mut input: &[u8]) -> Result<Vec<EapAkaAttributeSummary>, EapAkaError> {
    let mut attributes = Vec::new();
    while !input.is_empty() {
        if input.len() < EAP_ATTR_HEADER_LEN {
            return Err(EapAkaError::TruncatedAttribute);
        }
        let attribute_type = input[0];
        let units = input[1];
        if units == 0 {
            return Err(EapAkaError::InvalidAttributeLength {
                attribute_type,
                units,
            });
        }
        let length = usize::from(units) * 4;
        if length > input.len() {
            return Err(EapAkaError::TruncatedAttribute);
        }
        attributes.push(EapAkaAttributeSummary {
            attribute_type,
            role: attribute_role(attribute_type),
            units,
            value_bytes: length.saturating_sub(EAP_ATTR_HEADER_LEN),
            value_redacted: true,
        });
        input = &input[length..];
    }
    Ok(attributes)
}

fn attribute_role(attribute_type: u8) -> &'static str {
    match attribute_type {
        1 => "challenge_input",
        2 => "authenticator_input",
        3 => "subscriber_response",
        4 => "sync_failure_proof",
        6 => "padding",
        7 => "client_nonce",
        10 => "permanent_identity_request",
        11 => "message_authenticator",
        12 => "notification_code",
        13 => "any_identity_request",
        14 => "identity_blob",
        22 => "counter",
        23 => "counter_too_small",
        24 => "nonce_s",
        101 => "checkcode",
        102 => "encrypted_data",
        135 => "result_indication",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_eap_aka_challenge_without_attribute_values() {
        let packet = [
            1,
            EAP_TYPE_AKA,
            0,
            12,
            EAP_TYPE_AKA,
            1,
            0,
            0,
            1,
            1,
            0xaa,
            0xbb,
        ];

        let summary = parse_eap_aka_summary(&packet).expect("parse eap aka");

        assert_eq!(summary.code, "request");
        assert_eq!(summary.identifier, EAP_TYPE_AKA);
        assert_eq!(summary.method, "aka");
        assert_eq!(summary.subtype, "challenge");
        assert_eq!(summary.attribute_count, 1);
        assert_eq!(summary.attributes[0].attribute_type, 1);
        assert_eq!(summary.attributes[0].role, "challenge_input");
        assert_eq!(summary.attributes[0].value_bytes, 0);
        assert!(summary.attributes[0].value_redacted);

        let json = serde_json::to_string(&summary).expect("serialize summary");
        for forbidden_key in [
            "imsi",
            "iccid",
            "msisdn",
            "rand",
            "autn",
            "res",
            "ck",
            "ik",
            "auts",
            "key_material",
        ] {
            assert!(
                !json
                    .to_ascii_lowercase()
                    .contains(&format!("\"{forbidden_key}\"")),
                "summary must not contain a {forbidden_key} field"
            );
        }
    }

    #[test]
    fn summarizes_eap_aka_attributes_without_values() {
        let packet = [
            1,
            8,
            0,
            20,
            EAP_TYPE_AKA,
            1,
            0,
            0,
            1,
            1,
            0xaa,
            0xbb,
            2,
            2,
            0xcc,
            0xdd,
            0xee,
            0xff,
            0x12,
            0x34,
        ];

        let summary = parse_eap_aka_summary(&packet).expect("parse eap aka");

        assert_eq!(summary.attribute_count, 2);
        assert_eq!(summary.attributes[0].role, "challenge_input");
        assert_eq!(summary.attributes[0].value_bytes, 0);
        assert_eq!(summary.attributes[1].role, "authenticator_input");
        assert_eq!(summary.attributes[1].value_bytes, 4);
        assert!(summary
            .attributes
            .iter()
            .all(|attribute| attribute.value_redacted));

        let json = serde_json::to_string(&summary).expect("serialize summary");
        assert!(!json.to_ascii_lowercase().contains("aabb"));
        assert!(!json.to_ascii_lowercase().contains("ccddeeff"));
    }

    #[test]
    fn accepts_success_and_failure_without_method_payload() {
        let success = [3, 9, 0, 4];
        let summary = parse_eap_aka_summary(&success).expect("parse success");

        assert_eq!(summary.code, "success");
        assert_eq!(summary.method, "none");
        assert_eq!(summary.subtype, "none");
        assert!(summary.attributes.is_empty());
    }

    #[test]
    fn rejects_malformed_eap_aka_packet() {
        assert_eq!(
            parse_eap_aka_summary(&[1, 1, 0, 6, EAP_TYPE_AKA, 1]).unwrap_err(),
            EapAkaError::MissingSubtype
        );
        assert_eq!(
            parse_eap_aka_summary(&[1, 1, 0, 8, 99, 1, 0, 0]).unwrap_err(),
            EapAkaError::NotAkaMethod(99)
        );
        assert_eq!(
            parse_eap_aka_summary(&[1, 1, 0, 12, EAP_TYPE_AKA, 1, 0, 0, 1, 0, 0, 0]).unwrap_err(),
            EapAkaError::InvalidAttributeLength {
                attribute_type: 1,
                units: 0,
            }
        );
    }
}
