#![allow(dead_code)]

use std::fmt;

use serde::Serialize;

use super::ike_codec::{IkeMessage, IkePayloadType};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IkeControlEventKind {
    Notify,
    Delete,
}

impl IkeControlEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Notify => "notify",
            Self::Delete => "delete",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IkeControlEvent {
    pub kind: &'static str,
    pub message_id: u32,
    pub protocol_id: u8,
    pub spi_size: u8,
    pub spi_present: bool,
    pub notify_type: Option<u16>,
    pub notify_name: Option<&'static str>,
    pub delete_spi_count: Option<u16>,
    pub action: &'static str,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IkeControlEventError {
    MissingControlPayload,
    TruncatedNotify,
    TruncatedDelete,
    InvalidDeleteLength,
}

impl fmt::Display for IkeControlEventError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingControlPayload => write!(f, "IKE message has no notify or delete payload"),
            Self::TruncatedNotify => write!(f, "truncated IKE notify payload"),
            Self::TruncatedDelete => write!(f, "truncated IKE delete payload"),
            Self::InvalidDeleteLength => write!(f, "invalid IKE delete payload length"),
        }
    }
}

impl std::error::Error for IkeControlEventError {}

pub fn parse_control_event(message: &IkeMessage) -> Result<IkeControlEvent, IkeControlEventError> {
    if let Some(payload) = message
        .payloads
        .iter()
        .find(|payload| payload.payload_type == IkePayloadType::Notify)
    {
        return parse_notify(message.header.message_id, &payload.body);
    }

    if let Some(payload) = message
        .payloads
        .iter()
        .find(|payload| payload.payload_type == IkePayloadType::Delete)
    {
        return parse_delete(message.header.message_id, &payload.body);
    }

    Err(IkeControlEventError::MissingControlPayload)
}

fn parse_notify(message_id: u32, body: &[u8]) -> Result<IkeControlEvent, IkeControlEventError> {
    if body.len() < 4 {
        return Err(IkeControlEventError::TruncatedNotify);
    }
    let protocol_id = body[0];
    let spi_size = body[1];
    let notify_type = u16::from_be_bytes(body[2..4].try_into().expect("slice length"));
    let expected = 4 + usize::from(spi_size);
    if body.len() < expected {
        return Err(IkeControlEventError::TruncatedNotify);
    }

    Ok(IkeControlEvent {
        kind: IkeControlEventKind::Notify.as_str(),
        message_id,
        protocol_id,
        spi_size,
        spi_present: spi_size > 0,
        notify_type: Some(notify_type),
        notify_name: Some(notify_name(notify_type)),
        delete_spi_count: None,
        action: action_for_notify(notify_type),
        sensitive_values_policy: "spi_and_notify_data_values_not_serialized",
    })
}

fn parse_delete(message_id: u32, body: &[u8]) -> Result<IkeControlEvent, IkeControlEventError> {
    if body.len() < 4 {
        return Err(IkeControlEventError::TruncatedDelete);
    }
    let protocol_id = body[0];
    let spi_size = body[1];
    let spi_count = u16::from_be_bytes(body[2..4].try_into().expect("slice length"));
    let spi_bytes = usize::from(spi_size) * usize::from(spi_count);
    if body.len() != 4 + spi_bytes {
        return Err(IkeControlEventError::InvalidDeleteLength);
    }

    Ok(IkeControlEvent {
        kind: IkeControlEventKind::Delete.as_str(),
        message_id,
        protocol_id,
        spi_size,
        spi_present: spi_size > 0 && spi_count > 0,
        notify_type: None,
        notify_name: None,
        delete_spi_count: Some(spi_count),
        action: "teardown_requested",
        sensitive_values_policy: "spi_values_not_serialized",
    })
}

pub fn notify_name(notify_type: u16) -> &'static str {
    match notify_type {
        1 => "unsupported_critical_payload",
        4 => "invalid_ike_spi",
        5 => "invalid_major_version",
        7 => "invalid_syntax",
        9 => "invalid_message_id",
        11 => "invalid_spi",
        14 => "no_proposal_chosen",
        17 => "invalid_ke_payload",
        24 => "authentication_failed",
        34 => "single_pair_required",
        35 => "no_additional_sas",
        36 => "internal_address_failure",
        37 => "failed_cp_required",
        38 => "ts_unacceptable",
        39 => "invalid_selectors",
        40 => "unacceptable_addresses",
        41 => "unexpected_nat_detected",
        43 => "temporary_failure",
        44 => "child_sa_not_found",
        45 => "invalid_group_id",
        46 => "authorization_failed",
        47 => "state_not_found",
        16_384 => "initial_contact",
        16_385 => "set_window_size",
        16_386 => "additional_ts_possible",
        16_387 => "ipcomp_supported",
        16_388 => "nat_detection_source_ip",
        16_389 => "nat_detection_destination_ip",
        16_390 => "cookie",
        16_391 => "use_transport_mode",
        16_393 => "rekey_sa",
        16_403 => "authentication_lifetime",
        16_417 => "eap_only_authentication",
        16_430 => "ikev2_fragmentation_supported",
        8_192..=16_383 | 40_960..=65_535 => "private_or_extension",
        _ => "unknown",
    }
}

fn action_for_notify(notify_type: u16) -> &'static str {
    match notify_type {
        1 | 4 | 5 | 7 | 9 | 11 | 14 | 17 | 24 | 45 | 46 | 47 => "fail_exchange",
        34 | 35 | 36 | 37 | 38 | 39 | 40 | 41 | 43 => "child_sa_negotiation_failed",
        44 => "mark_child_sa_missing",
        16_393 => "schedule_rekey",
        16_403 => "reauth_required",
        16_384 | 16_385 | 16_386 | 16_387 | 16_388 | 16_389 | 16_390 | 16_391 | 16_417 | 16_430 => {
            "record_capability"
        }
        _ => "record_only",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::modems::ims::vowifi::{
        ike_codec::{IkeExchangeType, IkeMessage, IkePayload},
        ike_payloads::{build_notify_payload, NotifyProtocolId},
    };

    #[test]
    fn parses_notify_event_without_serializing_spi_or_data() {
        let notify = build_notify_payload(
            NotifyProtocolId::Esp,
            &[0xaa, 0xbb, 0xcc, 0xdd],
            16_393,
            &[1, 2],
        )
        .expect("notify");
        let message = IkeMessage::new_request(1, IkeExchangeType::Informational, 7, vec![notify]);

        let event = parse_control_event(&message).expect("parse control event");

        assert_eq!(event.kind, "notify");
        assert_eq!(event.message_id, 7);
        assert_eq!(event.protocol_id, 3);
        assert_eq!(event.spi_size, 4);
        assert!(event.spi_present);
        assert_eq!(event.notify_type, Some(16_393));
        assert_eq!(event.notify_name, Some("rekey_sa"));
        assert_eq!(event.action, "schedule_rekey");

        let json = serde_json::to_string(&event).expect("serialize event");
        assert!(!json.to_ascii_lowercase().contains("aabbccdd"));
        assert!(!json.to_ascii_lowercase().contains("\"spi\""));
    }

    #[test]
    fn parses_delete_event_as_teardown_request() {
        let delete = IkePayload {
            payload_type: IkePayloadType::Delete,
            critical: false,
            body: vec![3, 4, 0, 2, 0xaa, 0xbb, 0xcc, 0xdd, 0x11, 0x22, 0x33, 0x44],
        };
        let message = IkeMessage::new_request(1, IkeExchangeType::Informational, 8, vec![delete]);

        let event = parse_control_event(&message).expect("parse delete");

        assert_eq!(event.kind, "delete");
        assert_eq!(event.delete_spi_count, Some(2));
        assert_eq!(event.action, "teardown_requested");
        assert!(event.spi_present);
    }

    #[test]
    fn rejects_malformed_control_payloads() {
        let message = IkeMessage::new_request(
            1,
            IkeExchangeType::Informational,
            1,
            vec![IkePayload {
                payload_type: IkePayloadType::Notify,
                critical: false,
                body: vec![0, 4, 0],
            }],
        );
        assert_eq!(
            parse_control_event(&message).unwrap_err(),
            IkeControlEventError::TruncatedNotify
        );

        let message = IkeMessage::new_request(
            1,
            IkeExchangeType::Informational,
            1,
            vec![IkePayload {
                payload_type: IkePayloadType::Delete,
                critical: false,
                body: vec![3, 4, 0, 2, 0xaa],
            }],
        );
        assert_eq!(
            parse_control_event(&message).unwrap_err(),
            IkeControlEventError::InvalidDeleteLength
        );
    }
}
