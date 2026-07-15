use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::access::vowifi::profiles::CarrierProfile;

static SMS_MESSAGE_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SmsDirection {
    MobileOriginated,
    MobileTerminated,
}

impl SmsDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MobileOriginated => "mobile_originated",
            Self::MobileTerminated => "mobile_terminated",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SipMessageState {
    Queued,
    Submitted,
    Provisional,
    Accepted,
    Rejected,
    Timeout,
}

impl SipMessageState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Submitted => "submitted",
            Self::Provisional => "provisional",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Timeout => "timeout",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RpduAckState {
    None,
    Acked,
    Error,
}

impl RpduAckState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Acked => "acked",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SmsDeliveryState {
    Queued,
    Submitted,
    Accepted,
    Delivered,
    Received,
    Failed,
}

impl SmsDeliveryState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Submitted => "submitted",
            Self::Accepted => "accepted",
            Self::Delivered => "delivered",
            Self::Received => "received",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SmsTransportKind {
    Tcp,
    Udp,
}

impl SmsTransportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SmsPartState {
    pub reference: u16,
    pub sequence: u8,
    pub total: u8,
    pub received: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SmsDeliveryRecord {
    pub trace_id: String,
    pub message_id: String,
    pub direction: SmsDirection,
    pub sip_state: SipMessageState,
    pub rpdu_ack: RpduAckState,
    pub delivery_reported: bool,
    pub failure_cause: Option<String>,
    pub retry_count: u8,
    pub parts: Vec<SmsPartState>,
}

impl SmsDeliveryRecord {
    pub fn aggregate_state(&self) -> SmsDeliveryState {
        if self.failure_cause.is_some()
            || matches!(
                self.sip_state,
                SipMessageState::Rejected | SipMessageState::Timeout
            )
            || self.rpdu_ack == RpduAckState::Error
        {
            return SmsDeliveryState::Failed;
        }

        if self.direction == SmsDirection::MobileTerminated {
            if self.parts_complete() || self.parts.is_empty() {
                return SmsDeliveryState::Received;
            }
            return SmsDeliveryState::Submitted;
        }

        if self.delivery_reported {
            return SmsDeliveryState::Delivered;
        }

        if self.rpdu_ack == RpduAckState::Acked || self.sip_state == SipMessageState::Accepted {
            return SmsDeliveryState::Accepted;
        }

        match self.sip_state {
            SipMessageState::Queued => SmsDeliveryState::Queued,
            SipMessageState::Submitted | SipMessageState::Provisional => {
                SmsDeliveryState::Submitted
            }
            SipMessageState::Accepted => SmsDeliveryState::Accepted,
            SipMessageState::Rejected | SipMessageState::Timeout => SmsDeliveryState::Failed,
        }
    }

    pub fn api_status(&self) -> &'static str {
        match self.aggregate_state() {
            SmsDeliveryState::Queued | SmsDeliveryState::Submitted => "pending",
            SmsDeliveryState::Accepted | SmsDeliveryState::Delivered => "sent",
            SmsDeliveryState::Received => "received",
            SmsDeliveryState::Failed => "failed",
        }
    }

    pub fn parts_complete(&self) -> bool {
        if self.parts.is_empty() {
            return false;
        }

        let total = self.parts[0].total;
        total != 0
            && self.parts.iter().all(|part| part.total == total)
            && (1..=total).all(|seq| {
                self.parts
                    .iter()
                    .any(|part| part.sequence == seq && part.received)
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SmsRpduSummary {
    pub direction: &'static str,
    pub rpdu_kind: &'static str,
    pub user_data_bytes: usize,
    pub segment_reference_present: bool,
    pub segment_reference: Option<u16>,
    pub segment_sequence: Option<u8>,
    pub segment_total: Option<u8>,
    pub values_redacted: bool,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SmsSipMessageSummary {
    pub direction: &'static str,
    pub method: &'static str,
    pub transport: &'static str,
    pub sip_state: &'static str,
    pub sip_status: Option<u16>,
    pub content_type: &'static str,
    pub body_bytes: usize,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SmsAckPlan {
    pub ack_kind: &'static str,
    pub transport: &'static str,
    pub sip_response_code: u16,
    pub rp_ack_present: bool,
    pub failure_cause: Option<String>,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SmsReassemblyState {
    pub key_scope: &'static str,
    pub reference: u16,
    pub expected_parts: u8,
    pub received_parts: u8,
    pub complete: bool,
    pub duplicate_parts: u8,
    pub last_sequence: Option<u8>,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SmsDeliveryPublicRecord {
    pub trace_id: String,
    pub message_id: String,
    pub direction: &'static str,
    pub state: &'static str,
    pub api_status: &'static str,
    pub sip_state: &'static str,
    pub rpdu_ack: &'static str,
    pub delivery_reported: bool,
    pub failure_cause: Option<String>,
    pub retry_count: u8,
    pub parts: Vec<SmsPartState>,
    pub parts_complete: bool,
    pub db_fact_source: &'static str,
    pub sensitive_values_policy: &'static str,
}

impl SmsDeliveryPublicRecord {
    pub fn from_record(record: &SmsDeliveryRecord) -> Self {
        Self {
            trace_id: record.trace_id.clone(),
            message_id: record.message_id.clone(),
            direction: record.direction.as_str(),
            state: record.aggregate_state().as_str(),
            api_status: record.api_status(),
            sip_state: record.sip_state.as_str(),
            rpdu_ack: record.rpdu_ack.as_str(),
            delivery_reported: record.delivery_reported,
            failure_cause: record.failure_cause.clone(),
            retry_count: record.retry_count,
            parts: record.parts.clone(),
            parts_complete: record.parts_complete(),
            db_fact_source: "vowifi_sms_delivery",
            sensitive_values_policy: "phone_numbers_text_rpdu_and_sip_body_not_serialized",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SmsRuntimePublicState {
    pub profile_id: &'static str,
    pub plmn: &'static str,
    pub sms_ready: bool,
    pub receiver_transport: &'static str,
    pub subscribe_reg_ready: bool,
    pub pending_delivery_count: u16,
    pub mo: SmsDeliveryPublicRecord,
    pub mt: SmsDeliveryPublicRecord,
    pub last_sip_message: Option<SmsSipMessageSummary>,
    pub last_rpdu: Option<SmsRpduSummary>,
    pub last_ack: Option<SmsAckPlan>,
    pub reassembly: Option<SmsReassemblyState>,
    pub state_consistency_policy: &'static str,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmsRuntimeError {
    InvalidPart { sequence: u8, total: u8 },
    SipRejected(u16),
    InconsistentState(&'static str),
}

impl std::fmt::Display for SmsRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPart { sequence, total } => {
                write!(f, "invalid SMS part sequence={sequence} total={total}")
            }
            Self::SipRejected(code) => write!(f, "SIP MESSAGE rejected code={code}"),
            Self::InconsistentState(reason) => write!(f, "inconsistent SMS state: {reason}"),
        }
    }
}

impl std::error::Error for SmsRuntimeError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoSmsSubmission {
    pub trace_id: String,
    pub message_id: String,
    pub rp_message_reference: u8,
    pub part_index: u8,
    pub part_count: u8,
    pub body: Vec<u8>,
    pub body_bytes: usize,
    pub text_utf16_units: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoSmsSipOutcome {
    pub trace_id: String,
    pub message_id: String,
    pub sip_status: u16,
    pub rpdu_ack: RpduAckState,
    pub delivery_state: SmsDeliveryState,
    pub failure_cause: Option<String>,
    pub mt_deliveries: Vec<MtSmsDeliver>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MtSmsDeliver {
    pub rp_message_reference: u8,
    pub originator: String,
    pub text: String,
    pub user_data_bytes: usize,
    pub service_center_timestamp: String,
    pub segment_reference: Option<u16>,
    pub segment_sequence: u8,
    pub segment_total: u8,
}

impl MtSmsDeliver {
    pub fn is_duplicate_delivery(&self, other: &Self) -> bool {
        self.originator == other.originator
            && self.text == other.text
            && self.service_center_timestamp == other.service_center_timestamp
            && self.segment_reference == other.segment_reference
            && self.segment_sequence == other.segment_sequence
            && self.segment_total == other.segment_total
    }
}

impl MoSmsSipOutcome {
    pub fn api_status(&self) -> &'static str {
        match self.delivery_state {
            SmsDeliveryState::Queued | SmsDeliveryState::Submitted => "pending",
            SmsDeliveryState::Accepted | SmsDeliveryState::Delivered => "sent",
            SmsDeliveryState::Received => "received",
            SmsDeliveryState::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmsEncodingError {
    EmptyRecipient,
    EmptyText,
    InvalidAddress,
    TextTooLong,
    BodyTooLong,
}

impl std::fmt::Display for SmsEncodingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            Self::EmptyRecipient => "sms_recipient_empty",
            Self::EmptyText => "sms_text_empty",
            Self::InvalidAddress => "sms_address_invalid",
            Self::TextTooLong => "sms_text_too_long",
            Self::BodyTooLong => "sms_body_too_long",
        };
        write!(f, "{reason}")
    }
}

impl std::error::Error for SmsEncodingError {}

pub fn build_single_part_mo_submission(
    recipient: &str,
    text: &str,
    service_center: &str,
) -> Result<MoSmsSubmission, SmsEncodingError> {
    let text = text.trim_end_matches(['\r', '\n']);
    if recipient.trim().is_empty() {
        return Err(SmsEncodingError::EmptyRecipient);
    }
    if text.is_empty() {
        return Err(SmsEncodingError::EmptyText);
    }

    let counter = SMS_MESSAGE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let rp_message_reference = (counter & 0xff) as u8;
    let message_id = format!("vowifi-sms-{timestamp:x}-{counter:x}");
    let trace_id = format!("vowifi-sms-trace-{timestamp:x}-{counter:x}");
    let tpdu = build_sms_submit_tpdu(recipient, text, rp_message_reference)?;
    let sc_address = encode_address_value(service_center)?;

    let mut body = Vec::with_capacity(5 + sc_address.len() + tpdu.len());
    body.push(0x00);
    body.push(rp_message_reference);
    body.push(0x00);
    push_len_prefixed(&mut body, &sc_address)?;
    push_len_prefixed(&mut body, &tpdu)?;

    Ok(MoSmsSubmission {
        trace_id,
        message_id,
        rp_message_reference,
        part_index: 1,
        part_count: 1,
        body_bytes: body.len(),
        text_utf16_units: text.encode_utf16().count(),
        body,
    })
}

/// Build one or more MO submissions, adding an 8-bit concatenation UDH when
/// the payload exceeds the single-SMS GSM7/UCS2 limit.
pub fn build_mo_submissions(
    recipient: &str,
    text: &str,
    service_center: &str,
) -> Result<Vec<MoSmsSubmission>, SmsEncodingError> {
    let text = text.trim_end_matches(['\r', '\n']);
    if recipient.trim().is_empty() {
        return Err(SmsEncodingError::EmptyRecipient);
    }
    if text.is_empty() {
        return Err(SmsEncodingError::EmptyText);
    }
    if let Some(septets) = gsm7_septets(text) {
        if septets.len() <= 160 {
            return build_single_part_mo_submission(recipient, text, service_center)
                .map(|submission| vec![submission]);
        }
        let parts = split_gsm7_text(text, 153)?;
        return build_concatenated_submissions(recipient, service_center, text, parts, true);
    }
    if text.encode_utf16().count() <= 70 {
        return build_single_part_mo_submission(recipient, text, service_center)
            .map(|submission| vec![submission]);
    }
    let parts = split_ucs2_text(text, 67)?;
    build_concatenated_submissions(recipient, service_center, text, parts, false)
}

fn build_concatenated_submissions(
    recipient: &str,
    service_center: &str,
    full_text: &str,
    parts: Vec<String>,
    gsm7: bool,
) -> Result<Vec<MoSmsSubmission>, SmsEncodingError> {
    let part_count = u8::try_from(parts.len()).map_err(|_| SmsEncodingError::TextTooLong)?;
    if part_count < 2 {
        return Err(SmsEncodingError::TextTooLong);
    }
    let counter = SMS_MESSAGE_COUNTER.fetch_add(u64::from(part_count), Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let concat_reference = (counter & 0xff) as u8;
    let message_id = format!("ims-sms-{timestamp:x}-{counter:x}");
    let trace_id = format!("ims-sms-trace-{timestamp:x}-{counter:x}");
    let sc_address = encode_address_value(service_center)?;
    let mut submissions = Vec::with_capacity(parts.len());

    for (index, part) in parts.into_iter().enumerate() {
        let sequence = u8::try_from(index + 1).map_err(|_| SmsEncodingError::TextTooLong)?;
        let reference = concat_reference.wrapping_add(sequence.wrapping_sub(1));
        let udh = [0x05, 0x00, 0x03, concat_reference, part_count, sequence];
        let encoded = if gsm7 {
            let septets = gsm7_septets(&part).ok_or(SmsEncodingError::TextTooLong)?;
            EncodedSubmitUserData {
                dcs: 0x00,
                user_data_length: u8::try_from(7 + septets.len())
                    .map_err(|_| SmsEncodingError::TextTooLong)?,
                user_data: pack_gsm7_with_udh(&udh, &septets),
            }
        } else {
            let mut user_data = udh.to_vec();
            for unit in part.encode_utf16() {
                user_data.extend_from_slice(&unit.to_be_bytes());
            }
            EncodedSubmitUserData {
                dcs: 0x08,
                user_data_length: u8::try_from(user_data.len())
                    .map_err(|_| SmsEncodingError::TextTooLong)?,
                user_data,
            }
        };
        let tpdu = build_sms_submit_tpdu_encoded(recipient, reference, 0x41, encoded)?;
        let mut body = Vec::with_capacity(5 + sc_address.len() + tpdu.len());
        body.push(0x00);
        body.push(reference);
        body.push(0x00);
        push_len_prefixed(&mut body, &sc_address)?;
        push_len_prefixed(&mut body, &tpdu)?;
        submissions.push(MoSmsSubmission {
            trace_id: trace_id.clone(),
            message_id: message_id.clone(),
            rp_message_reference: reference,
            part_index: sequence,
            part_count,
            body_bytes: body.len(),
            text_utf16_units: full_text.encode_utf16().count(),
            body,
        });
    }
    Ok(submissions)
}

fn split_gsm7_text(text: &str, max_septets: usize) -> Result<Vec<String>, SmsEncodingError> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let width = if gsm7_basic_value(ch).is_some() {
            1
        } else if gsm7_extension_value(ch).is_some() {
            2
        } else {
            return Err(SmsEncodingError::TextTooLong);
        };
        if used + width > max_septets && !current.is_empty() {
            parts.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push(ch);
        used += width;
    }
    if !current.is_empty() {
        parts.push(current);
    }
    Ok(parts)
}

fn split_ucs2_text(text: &str, max_units: usize) -> Result<Vec<String>, SmsEncodingError> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let width = ch.len_utf16();
        if used + width > max_units && !current.is_empty() {
            parts.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push(ch);
        used += width;
    }
    if !current.is_empty() {
        parts.push(current);
    }
    Ok(parts)
}

pub fn classify_rp_ack(body: &[u8], expected_reference: u8) -> RpduAckState {
    if body.len() < 2 || body[1] != expected_reference {
        return RpduAckState::None;
    }
    match body[0] {
        0x03 => RpduAckState::Acked,
        0x05 => RpduAckState::Error,
        _ => RpduAckState::None,
    }
}

pub fn build_network_rp_ack(reference: u8) -> Vec<u8> {
    vec![0x02, reference]
}

pub fn parse_mt_rp_data(body: &[u8]) -> Result<MtSmsDeliver, SmsEncodingError> {
    if body.len() < 5 || body[0] != 0x01 {
        return Err(SmsEncodingError::BodyTooLong);
    }
    let reference = body[1];
    let mut offset = 2usize;
    let origin_len = usize::from(body[offset]);
    offset = offset.checked_add(1).ok_or(SmsEncodingError::BodyTooLong)?;
    if offset + origin_len > body.len() {
        return Err(SmsEncodingError::BodyTooLong);
    }
    offset += origin_len;
    if offset >= body.len() {
        return Err(SmsEncodingError::BodyTooLong);
    }
    parse_mt_rp_user_data(reference, body, offset).or_else(|_| {
        let destination_len = usize::from(body[offset]);
        let destination_end = offset
            .checked_add(1)
            .and_then(|value| value.checked_add(destination_len))
            .ok_or(SmsEncodingError::BodyTooLong)?;
        if destination_end >= body.len() {
            return Err(SmsEncodingError::BodyTooLong);
        }
        parse_mt_rp_user_data(reference, body, destination_end)
    })
}

fn parse_mt_rp_user_data(
    reference: u8,
    body: &[u8],
    length_offset: usize,
) -> Result<MtSmsDeliver, SmsEncodingError> {
    if length_offset >= body.len() {
        return Err(SmsEncodingError::BodyTooLong);
    }
    let user_data_len = usize::from(body[length_offset]);
    let user_data_offset = length_offset
        .checked_add(1)
        .ok_or(SmsEncodingError::BodyTooLong)?;
    if user_data_len == 0 || user_data_offset + user_data_len > body.len() {
        return Err(SmsEncodingError::BodyTooLong);
    }
    parse_sms_deliver_tpdu(
        reference,
        &body[user_data_offset..user_data_offset + user_data_len],
    )
}

fn parse_sms_deliver_tpdu(reference: u8, tpdu: &[u8]) -> Result<MtSmsDeliver, SmsEncodingError> {
    if tpdu.len() < 2 {
        return Err(SmsEncodingError::BodyTooLong);
    }
    let first_octet = tpdu[0];
    let user_data_header_present = first_octet & 0x40 != 0;
    let mut offset = 1usize;
    let originator_digits = usize::from(tpdu[offset]);
    offset += 1;
    if offset >= tpdu.len() {
        return Err(SmsEncodingError::BodyTooLong);
    }
    let ton_npi = tpdu[offset];
    offset += 1;
    let originator_octets = address_value_octets(ton_npi, originator_digits);
    if offset + originator_octets + 3 + 7 > tpdu.len() {
        return Err(SmsEncodingError::BodyTooLong);
    }
    let originator = decode_address_value(
        ton_npi,
        &tpdu[offset..offset + originator_octets],
        originator_digits,
    );
    offset += originator_octets;
    offset += 1;
    let dcs = tpdu[offset];
    offset += 1;
    let scts_end = offset.checked_add(7).ok_or(SmsEncodingError::BodyTooLong)?;
    if scts_end > tpdu.len() {
        return Err(SmsEncodingError::BodyTooLong);
    }
    let service_center_timestamp = hex_lower(&tpdu[offset..scts_end]);
    offset = scts_end;
    if offset >= tpdu.len() {
        return Err(SmsEncodingError::BodyTooLong);
    }
    let udl = usize::from(tpdu[offset]);
    offset += 1;
    if offset > tpdu.len() {
        return Err(SmsEncodingError::BodyTooLong);
    }
    let raw_user_data = &tpdu[offset..];
    let header = if user_data_header_present {
        parse_user_data_header(raw_user_data)?
    } else {
        SmsUserDataHeader::empty()
    };
    let text = match dcs {
        0x08 => {
            let user_data = raw_user_data
                .get(..std::cmp::min(raw_user_data.len(), udl))
                .ok_or(SmsEncodingError::BodyTooLong)?;
            let payload = user_data
                .get(header.header_octets..)
                .ok_or(SmsEncodingError::BodyTooLong)?;
            decode_ucs2_text(payload, payload.len())?
        }
        _ => decode_gsm7_user_data(raw_user_data, udl, header.header_octets),
    };
    Ok(MtSmsDeliver {
        rp_message_reference: reference,
        originator,
        text,
        user_data_bytes: raw_user_data.len(),
        service_center_timestamp,
        segment_reference: header.segment.reference,
        segment_sequence: header.segment.sequence,
        segment_total: header.segment.total,
    })
}

fn build_sms_submit_tpdu(
    recipient: &str,
    text: &str,
    message_reference: u8,
) -> Result<Vec<u8>, SmsEncodingError> {
    let encoded = encode_submit_user_data(text)?;
    build_sms_submit_tpdu_encoded(recipient, message_reference, 0x01, encoded)
}

fn build_sms_submit_tpdu_encoded(
    recipient: &str,
    message_reference: u8,
    first_octet: u8,
    encoded: EncodedSubmitUserData,
) -> Result<Vec<u8>, SmsEncodingError> {
    let destination = encode_address_value(recipient)?;

    let recipient_digits = normalized_address_digits(recipient)?.digits;
    let mut tpdu = Vec::with_capacity(7 + destination.len() + encoded.user_data.len());
    tpdu.push(first_octet);
    tpdu.push(message_reference);
    tpdu.push(u8::try_from(recipient_digits.len()).map_err(|_| SmsEncodingError::InvalidAddress)?);
    tpdu.extend_from_slice(&destination);
    tpdu.push(0x00);
    tpdu.push(encoded.dcs);
    tpdu.push(encoded.user_data_length);
    tpdu.extend_from_slice(&encoded.user_data);
    Ok(tpdu)
}

struct EncodedSubmitUserData {
    dcs: u8,
    user_data_length: u8,
    user_data: Vec<u8>,
}

fn encode_submit_user_data(text: &str) -> Result<EncodedSubmitUserData, SmsEncodingError> {
    if let Some((user_data, septets)) = encode_gsm7_user_data(text) {
        if septets > 160 {
            return Err(SmsEncodingError::TextTooLong);
        }
        return Ok(EncodedSubmitUserData {
            dcs: 0x00,
            user_data_length: u8::try_from(septets).map_err(|_| SmsEncodingError::TextTooLong)?,
            user_data,
        });
    }

    let user_data = encode_ucs2_user_data(text)?;
    if user_data.len() > 140 {
        return Err(SmsEncodingError::TextTooLong);
    }
    Ok(EncodedSubmitUserData {
        dcs: 0x08,
        user_data_length: u8::try_from(user_data.len())
            .map_err(|_| SmsEncodingError::TextTooLong)?,
        user_data,
    })
}

fn encode_gsm7_user_data(text: &str) -> Option<(Vec<u8>, usize)> {
    let septets = gsm7_septets(text)?;
    let mut out = vec![0u8; (septets.len() * 7).div_ceil(8)];
    for (index, septet) in septets.iter().copied().enumerate() {
        write_septet(&mut out, index * 7, septet);
    }
    Some((out, septets.len()))
}

fn gsm7_septets(text: &str) -> Option<Vec<u8>> {
    let mut septets = Vec::with_capacity(text.len());
    for ch in text.chars() {
        if let Some(value) = gsm7_basic_value(ch) {
            septets.push(value);
        } else {
            let value = gsm7_extension_value(ch)?;
            septets.push(0x1b);
            septets.push(value);
        }
    }

    Some(septets)
}

fn pack_gsm7_with_udh(udh: &[u8], septets: &[u8]) -> Vec<u8> {
    // GSM 03.38 text following a UDH starts on the next septet boundary, not
    // immediately after the last UDH bit.  A six-octet concatenation header
    // therefore occupies seven septets (49 bits), leaving one fill bit between
    // the 48-bit header and the first character.
    let header_septets = (udh.len() * 8).div_ceil(7);
    let text_bit_offset = header_septets * 7;
    let mut out = vec![0u8; (text_bit_offset + septets.len() * 7).div_ceil(8)];
    out[..udh.len()].copy_from_slice(udh);
    for (index, septet) in septets.iter().copied().enumerate() {
        write_septet(&mut out, text_bit_offset + index * 7, septet);
    }
    out
}

fn write_septet(out: &mut [u8], bit_index: usize, septet: u8) {
    for bit in 0..7 {
        if septet & (1 << bit) == 0 {
            continue;
        }
        let target = bit_index + bit;
        out[target / 8] |= 1 << (target % 8);
    }
}

fn encode_ucs2_user_data(text: &str) -> Result<Vec<u8>, SmsEncodingError> {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.is_empty() {
        return Err(SmsEncodingError::EmptyText);
    }
    if units.len() > 70 {
        return Err(SmsEncodingError::TextTooLong);
    }
    let mut out = Vec::with_capacity(units.len() * 2);
    for unit in units {
        out.extend_from_slice(&unit.to_be_bytes());
    }
    Ok(out)
}

fn push_len_prefixed(out: &mut Vec<u8>, value: &[u8]) -> Result<(), SmsEncodingError> {
    let len = u8::try_from(value.len()).map_err(|_| SmsEncodingError::BodyTooLong)?;
    out.push(len);
    out.extend_from_slice(value);
    Ok(())
}

fn encode_address_value(address: &str) -> Result<Vec<u8>, SmsEncodingError> {
    let normalized = normalized_address_digits(address)?;
    let mut out = Vec::with_capacity(1 + normalized.digits.len().div_ceil(2));
    out.push(if normalized.international { 0x91 } else { 0x81 });
    out.extend_from_slice(&encode_semi_octets(&normalized.digits));
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedAddressDigits {
    digits: String,
    international: bool,
}

fn normalized_address_digits(address: &str) -> Result<NormalizedAddressDigits, SmsEncodingError> {
    let trimmed = address.trim();
    let mut digits = String::new();
    let mut international = false;
    for (index, ch) in trimmed.chars().enumerate() {
        match ch {
            '+' if index == 0 => international = true,
            '0'..='9' => digits.push(ch),
            ' ' | '-' | '(' | ')' => {}
            _ => return Err(SmsEncodingError::InvalidAddress),
        }
    }
    if digits.is_empty() || digits.len() > 20 {
        return Err(SmsEncodingError::InvalidAddress);
    }
    Ok(NormalizedAddressDigits {
        digits,
        international,
    })
}

fn encode_semi_octets(digits: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(digits.len().div_ceil(2));
    let mut iter = digits.as_bytes().iter();
    while let Some(first) = iter.next() {
        let low = first - b'0';
        let high = iter.next().map(|digit| digit - b'0').unwrap_or(0x0f);
        out.push(low | (high << 4));
    }
    out
}

fn hex_lower(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn address_value_octets(ton_npi: u8, digits: usize) -> usize {
    if address_type_is_alphanumeric(ton_npi) {
        (digits * 7).div_ceil(8)
    } else {
        digits.div_ceil(2)
    }
}

fn address_type_is_alphanumeric(ton_npi: u8) -> bool {
    ton_npi & 0x70 == 0x50
}

fn address_type_is_international(ton_npi: u8) -> bool {
    ton_npi & 0x70 == 0x10
}

fn decode_address_value(ton_npi: u8, value: &[u8], digits: usize) -> String {
    if address_type_is_alphanumeric(ton_npi) {
        return decode_gsm7_text(value, digits);
    }

    let mut out =
        String::with_capacity(digits + usize::from(address_type_is_international(ton_npi)));
    if address_type_is_international(ton_npi) {
        out.push('+');
    }
    for byte in value {
        for nibble in [byte & 0x0f, byte >> 4] {
            if out.trim_start_matches('+').len() >= digits || nibble == 0x0f {
                continue;
            }
            let ch = match nibble {
                0x00..=0x09 => char::from(b'0' + nibble),
                0x0a => '*',
                0x0b => '#',
                0x0c => 'a',
                0x0d => 'b',
                0x0e => 'c',
                _ => ' ',
            };
            out.push(ch);
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SmsSegmentInfo {
    reference: Option<u16>,
    sequence: u8,
    total: u8,
}

impl SmsSegmentInfo {
    fn single() -> Self {
        Self {
            reference: None,
            sequence: 1,
            total: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SmsUserDataHeader {
    segment: SmsSegmentInfo,
    header_octets: usize,
}

impl SmsUserDataHeader {
    fn empty() -> Self {
        Self {
            segment: SmsSegmentInfo::single(),
            header_octets: 0,
        }
    }
}

fn parse_user_data_header(user_data: &[u8]) -> Result<SmsUserDataHeader, SmsEncodingError> {
    let Some((&header_len, rest)) = user_data.split_first() else {
        return Err(SmsEncodingError::BodyTooLong);
    };
    let header_total = 1usize
        .checked_add(usize::from(header_len))
        .ok_or(SmsEncodingError::BodyTooLong)?;
    if header_total > user_data.len() {
        return Err(SmsEncodingError::BodyTooLong);
    }

    let header = &rest[..usize::from(header_len)];
    let mut offset = 0usize;
    let mut segment = SmsSegmentInfo::single();
    while offset + 2 <= header.len() {
        let iei = header[offset];
        let len = usize::from(header[offset + 1]);
        offset += 2;
        if offset + len > header.len() {
            return Err(SmsEncodingError::BodyTooLong);
        }
        let value = &header[offset..offset + len];
        match (iei, value) {
            (0x00, [reference, total, sequence])
                if *total != 0 && *sequence != 0 && *sequence <= *total =>
            {
                segment = SmsSegmentInfo {
                    reference: Some(u16::from(*reference)),
                    sequence: *sequence,
                    total: *total,
                };
            }
            (0x08, [reference_hi, reference_lo, total, sequence])
                if *total != 0 && *sequence != 0 && *sequence <= *total =>
            {
                segment = SmsSegmentInfo {
                    reference: Some(u16::from_be_bytes([*reference_hi, *reference_lo])),
                    sequence: *sequence,
                    total: *total,
                };
            }
            _ => {}
        }
        offset += len;
    }

    Ok(SmsUserDataHeader {
        segment,
        header_octets: header_total,
    })
}

fn decode_ucs2_text(user_data: &[u8], octets: usize) -> Result<String, SmsEncodingError> {
    let len = std::cmp::min(user_data.len(), octets);
    if !len.is_multiple_of(2) {
        return Err(SmsEncodingError::BodyTooLong);
    }
    let units: Vec<u16> = user_data[..len]
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect();
    String::from_utf16(&units).map_err(|_| SmsEncodingError::BodyTooLong)
}

fn decode_gsm7_user_data(user_data: &[u8], udl_septets: usize, header_octets: usize) -> String {
    if header_octets == 0 {
        return decode_gsm7_text(user_data, udl_septets);
    }

    let header_septets = (header_octets * 8).div_ceil(7);
    if udl_septets <= header_septets {
        return String::new();
    }
    let text_septets = udl_septets - header_septets;
    decode_gsm7_text_from_bit_offset(user_data, text_septets, header_septets * 7)
}

fn decode_gsm7_text(user_data: &[u8], septet_count: usize) -> String {
    decode_gsm7_text_from_bit_offset(user_data, septet_count, 0)
}

fn decode_gsm7_text_from_bit_offset(
    user_data: &[u8],
    septet_count: usize,
    bit_offset: usize,
) -> String {
    let available_bits = user_data.len() * 8;
    if bit_offset >= available_bits {
        return String::new();
    }
    let max_septets = (available_bits - bit_offset) / 7;
    let count = std::cmp::min(septet_count, max_septets);
    let mut out = String::with_capacity(count);
    let mut escaped = false;
    for index in 0..count {
        let bit_index = bit_offset + index * 7;
        let byte_index = bit_index / 8;
        let shift = bit_index % 8;
        let mut value = ((user_data[byte_index] as u16) >> shift) & 0x7f;
        if shift > 1 && byte_index + 1 < user_data.len() {
            value |= ((user_data[byte_index + 1] as u16) << (8 - shift)) & 0x7f;
        }
        let value = value as u8;
        if escaped {
            if let Some(ch) = gsm7_extension_char(value) {
                out.push(ch);
            }
            escaped = false;
            continue;
        }
        if value == 0x1b {
            escaped = true;
            continue;
        }
        out.push(gsm7_basic_char(value));
    }
    out
}

fn gsm7_basic_char(value: u8) -> char {
    match value {
        0x00 => '@',
        0x01 => '拢',
        0x02 => '$',
        0x03 => '楼',
        0x04 => '猫',
        0x05 => '茅',
        0x06 => '霉',
        0x07 => '矛',
        0x08 => '貌',
        0x09 => '脟',
        0x0a => '\n',
        0x0b => '脴',
        0x0c => '酶',
        0x0d => '\r',
        0x0e => '脜',
        0x0f => '氓',
        0x10 => '螖',
        0x11 => '_',
        0x12 => '桅',
        0x13 => '螕',
        0x14 => '螞',
        0x15 => '惟',
        0x16 => '螤',
        0x17 => '唯',
        0x18 => '危',
        0x19 => '螛',
        0x1a => '螢',
        0x1c => '脝',
        0x1d => '忙',
        0x1e => '脽',
        0x1f => '脡',
        0x20..=0x3f | 0x41..=0x5a | 0x61..=0x7a => char::from(value),
        0x40 => '隆',
        0x5b => '脛',
        0x5c => '脰',
        0x5d => '脩',
        0x5e => '脺',
        0x5f => '§',
        0x60 => '驴',
        0x7b => '盲',
        0x7c => '枚',
        0x7d => '帽',
        0x7e => '眉',
        0x7f => '脿',
        _ => ' ',
    }
}

fn gsm7_basic_value(ch: char) -> Option<u8> {
    match ch {
        '@' => Some(0x00),
        '\u{00a3}' => Some(0x01),
        '$' => Some(0x02),
        '\u{00a5}' => Some(0x03),
        '\u{00e8}' => Some(0x04),
        '\u{00e9}' => Some(0x05),
        '\u{00f9}' => Some(0x06),
        '\u{00ec}' => Some(0x07),
        '\u{00f2}' => Some(0x08),
        '\u{00c7}' => Some(0x09),
        '\n' => Some(0x0a),
        '\u{00d8}' => Some(0x0b),
        '\u{00f8}' => Some(0x0c),
        '\r' => Some(0x0d),
        '\u{00c5}' => Some(0x0e),
        '\u{00e5}' => Some(0x0f),
        '\u{0394}' => Some(0x10),
        '_' => Some(0x11),
        '\u{03a6}' => Some(0x12),
        '\u{0393}' => Some(0x13),
        '\u{039b}' => Some(0x14),
        '\u{03a9}' => Some(0x15),
        '\u{03a0}' => Some(0x16),
        '\u{03a8}' => Some(0x17),
        '\u{03a3}' => Some(0x18),
        '\u{0398}' => Some(0x19),
        '\u{039e}' => Some(0x1a),
        '\u{00c6}' => Some(0x1c),
        '\u{00e6}' => Some(0x1d),
        '\u{00df}' => Some(0x1e),
        '\u{00c9}' => Some(0x1f),
        ' '..='?' | 'A'..='Z' | 'a'..='z' => Some(ch as u8),
        '\u{00a1}' => Some(0x40),
        '\u{00c4}' => Some(0x5b),
        '\u{00d6}' => Some(0x5c),
        '\u{00d1}' => Some(0x5d),
        '\u{00dc}' => Some(0x5e),
        '\u{00a7}' => Some(0x5f),
        '\u{00bf}' => Some(0x60),
        '\u{00e4}' => Some(0x7b),
        '\u{00f6}' => Some(0x7c),
        '\u{00f1}' => Some(0x7d),
        '\u{00fc}' => Some(0x7e),
        '\u{00e0}' => Some(0x7f),
        _ => None,
    }
}

fn gsm7_extension_char(value: u8) -> Option<char> {
    match value {
        0x0a => Some('\u{000c}'),
        0x14 => Some('^'),
        0x28 => Some('{'),
        0x29 => Some('}'),
        0x2f => Some('\\'),
        0x3c => Some('['),
        0x3d => Some('~'),
        0x3e => Some(']'),
        0x40 => Some('|'),
        0x65 => Some('€'),
        _ => None,
    }
}

fn gsm7_extension_value(ch: char) -> Option<u8> {
    match ch {
        '\u{000c}' => Some(0x0a),
        '^' => Some(0x14),
        '{' => Some(0x28),
        '}' => Some(0x29),
        '\\' => Some(0x2f),
        '[' => Some(0x3c),
        '~' => Some(0x3d),
        ']' => Some(0x3e),
        '|' => Some(0x40),
        '\u{20ac}' => Some(0x65),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmsRuntimeStateMachine {
    profile: &'static CarrierProfile,
    receiver_transport: SmsTransportKind,
    subscribe_reg_ready: bool,
    mo: SmsDeliveryRecord,
    mt: SmsDeliveryRecord,
    last_sip_message: Option<SmsSipMessageSummary>,
    last_rpdu: Option<SmsRpduSummary>,
    last_ack: Option<SmsAckPlan>,
    reassembly: Option<SmsReassemblyState>,
}

impl SmsRuntimeStateMachine {
    pub fn new(profile: &'static CarrierProfile) -> Self {
        Self {
            profile,
            receiver_transport: if profile.sms.receiver_transport == "udp" {
                SmsTransportKind::Udp
            } else {
                SmsTransportKind::Tcp
            },
            subscribe_reg_ready: false,
            mo: SmsDeliveryRecord {
                trace_id: "sms-mo-dry-run".to_string(),
                message_id: "mo-dry-run-0001".to_string(),
                direction: SmsDirection::MobileOriginated,
                sip_state: SipMessageState::Queued,
                rpdu_ack: RpduAckState::None,
                delivery_reported: false,
                failure_cause: None,
                retry_count: 0,
                parts: Vec::new(),
            },
            mt: SmsDeliveryRecord {
                trace_id: "sms-mt-dry-run".to_string(),
                message_id: "mt-dry-run-0001".to_string(),
                direction: SmsDirection::MobileTerminated,
                sip_state: SipMessageState::Queued,
                rpdu_ack: RpduAckState::None,
                delivery_reported: false,
                failure_cause: None,
                retry_count: 0,
                parts: Vec::new(),
            },
            last_sip_message: None,
            last_rpdu: None,
            last_ack: None,
            reassembly: None,
        }
    }

    pub fn mark_subscribe_reg_ready(&mut self) {
        self.subscribe_reg_ready = true;
    }

    pub fn queue_mo_text(&mut self, user_data_bytes: usize) -> SmsRpduSummary {
        self.mo.sip_state = SipMessageState::Queued;
        let rpdu = SmsRpduSummary {
            direction: "mobile_originated",
            rpdu_kind: "rp_data",
            user_data_bytes,
            segment_reference_present: false,
            segment_reference: None,
            segment_sequence: None,
            segment_total: None,
            values_redacted: true,
            sensitive_values_policy: "sms_text_tpdu_and_rpdu_bytes_not_serialized",
        };
        self.last_rpdu = Some(rpdu.clone());
        rpdu
    }

    pub fn submit_mo_sip_message(&mut self) -> SmsSipMessageSummary {
        self.mo.sip_state = SipMessageState::Submitted;
        let summary = SmsSipMessageSummary {
            direction: "outbound",
            method: "MESSAGE",
            transport: self.receiver_transport.as_str(),
            sip_state: self.mo.sip_state.as_str(),
            sip_status: None,
            content_type: "application/vnd.3gpp.sms",
            body_bytes: self
                .last_rpdu
                .as_ref()
                .map(|rpdu| rpdu.user_data_bytes)
                .unwrap_or_default(),
            sensitive_values_policy: "sip_message_body_not_serialized",
        };
        self.last_sip_message = Some(summary.clone());
        summary
    }

    pub fn accept_mo_sip_response(
        &mut self,
        sip_status: u16,
    ) -> Result<SmsSipMessageSummary, SmsRuntimeError> {
        if !(200..300).contains(&sip_status) {
            self.mo.sip_state = SipMessageState::Rejected;
            self.mo.failure_cause = Some(format!("sip_{sip_status}"));
            return Err(SmsRuntimeError::SipRejected(sip_status));
        }

        self.mo.sip_state = SipMessageState::Accepted;
        let summary = SmsSipMessageSummary {
            direction: "inbound",
            method: "MESSAGE",
            transport: self.receiver_transport.as_str(),
            sip_state: self.mo.sip_state.as_str(),
            sip_status: Some(sip_status),
            content_type: "application/vnd.3gpp.sms",
            body_bytes: 0,
            sensitive_values_policy: "sip_response_body_not_serialized",
        };
        self.last_sip_message = Some(summary.clone());
        Ok(summary)
    }

    pub fn accept_mo_rp_ack(&mut self) -> SmsAckPlan {
        self.mo.rpdu_ack = RpduAckState::Acked;
        let ack = SmsAckPlan {
            ack_kind: "rp_ack",
            transport: self.receiver_transport.as_str(),
            sip_response_code: 200,
            rp_ack_present: true,
            failure_cause: None,
            sensitive_values_policy: "ack_body_not_serialized",
        };
        self.last_ack = Some(ack.clone());
        ack
    }

    pub fn mark_mo_delivery_reported(&mut self) {
        self.mo.delivery_reported = true;
    }

    pub fn receive_mt_part(
        &mut self,
        reference: u16,
        sequence: u8,
        total: u8,
        user_data_bytes: usize,
    ) -> Result<SmsAckPlan, SmsRuntimeError> {
        if sequence == 0 || total == 0 || sequence > total {
            return Err(SmsRuntimeError::InvalidPart { sequence, total });
        }

        self.mt.sip_state = SipMessageState::Accepted;
        self.mt.rpdu_ack = RpduAckState::Acked;
        if !self
            .mt
            .parts
            .iter()
            .any(|part| part.reference == reference && part.sequence == sequence)
        {
            self.mt.parts.push(SmsPartState {
                reference,
                sequence,
                total,
                received: true,
            });
        }

        let duplicate_parts = self
            .mt
            .parts
            .iter()
            .filter(|part| part.reference == reference && part.sequence == sequence)
            .count()
            .saturating_sub(1) as u8;
        let received_parts = self
            .mt
            .parts
            .iter()
            .filter(|part| part.reference == reference && part.received)
            .count() as u8;
        let complete = self.mt.parts_complete();

        self.last_rpdu = Some(SmsRpduSummary {
            direction: "mobile_terminated",
            rpdu_kind: "rp_data",
            user_data_bytes,
            segment_reference_present: true,
            segment_reference: Some(reference),
            segment_sequence: Some(sequence),
            segment_total: Some(total),
            values_redacted: true,
            sensitive_values_policy: "sms_text_tpdu_and_rpdu_bytes_not_serialized",
        });
        self.last_sip_message = Some(SmsSipMessageSummary {
            direction: "inbound",
            method: "MESSAGE",
            transport: self.receiver_transport.as_str(),
            sip_state: self.mt.sip_state.as_str(),
            sip_status: None,
            content_type: "application/vnd.3gpp.sms",
            body_bytes: user_data_bytes,
            sensitive_values_policy: "sip_message_body_not_serialized",
        });
        self.reassembly = Some(SmsReassemblyState {
            key_scope: "sender_destination_dcs_reference",
            reference,
            expected_parts: total,
            received_parts,
            complete,
            duplicate_parts,
            last_sequence: Some(sequence),
            sensitive_values_policy: "sender_destination_and_sms_text_not_serialized",
        });

        let ack = SmsAckPlan {
            ack_kind: if complete {
                "rp_ack_complete"
            } else {
                "rp_ack_part"
            },
            transport: self.receiver_transport.as_str(),
            sip_response_code: 200,
            rp_ack_present: true,
            failure_cause: None,
            sensitive_values_policy: "ack_body_not_serialized",
        };
        self.last_ack = Some(ack.clone());
        Ok(ack)
    }

    pub fn assert_state_consistency(&self) -> Result<(), SmsRuntimeError> {
        if self.mo.rpdu_ack == RpduAckState::Acked && self.mo.api_status() == "pending" {
            return Err(SmsRuntimeError::InconsistentState(
                "mo_rp_ack_must_not_remain_pending",
            ));
        }
        if self.mt.parts_complete() && self.mt.api_status() != "received" {
            return Err(SmsRuntimeError::InconsistentState(
                "complete_mt_reassembly_must_be_received",
            ));
        }
        Ok(())
    }

    pub fn snapshot(&self) -> SmsRuntimePublicState {
        let mo_public = SmsDeliveryPublicRecord::from_record(&self.mo);
        let mt_public = SmsDeliveryPublicRecord::from_record(&self.mt);
        let pending_delivery_count = u16::from(mo_public.api_status == "pending")
            + u16::from(mt_public.api_status == "pending");
        SmsRuntimePublicState {
            profile_id: self.profile.meta.profile_id,
            plmn: self.profile.meta.plmn,
            sms_ready: self.subscribe_reg_ready
                && mo_public.api_status == "sent"
                && mt_public.api_status == "received",
            receiver_transport: self.receiver_transport.as_str(),
            subscribe_reg_ready: self.subscribe_reg_ready,
            pending_delivery_count,
            mo: mo_public,
            mt: mt_public,
            last_sip_message: self.last_sip_message.clone(),
            last_rpdu: self.last_rpdu.clone(),
            last_ack: self.last_ack.clone(),
            reassembly: self.reassembly.clone(),
            state_consistency_policy:
                "vowifi_sms_delivery_is_single_fact_source_for_logs_api_and_ui",
            sensitive_values_policy: "phone_numbers_sms_text_rpdu_body_and_sip_body_not_serialized",
        }
    }
}

pub fn build_dry_run_sms_snapshot(profile: &'static CarrierProfile) -> SmsRuntimePublicState {
    let mut machine = SmsRuntimeStateMachine::new(profile);
    machine.mark_subscribe_reg_ready();
    machine.queue_mo_text(23);
    machine.submit_mo_sip_message();
    machine
        .accept_mo_sip_response(202)
        .expect("synthetic SIP MESSAGE response is accepted");
    machine.accept_mo_rp_ack();
    machine.mark_mo_delivery_reported();
    machine
        .receive_mt_part(42, 1, 2, 134)
        .expect("first synthetic MT part is valid");
    machine
        .receive_mt_part(42, 2, 2, 118)
        .expect("second synthetic MT part completes reassembly");
    machine
        .assert_state_consistency()
        .expect("dry-run SMS states remain API/log consistent");
    machine.snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::vowifi::profiles::GB_EE_23433;

    fn mo_record() -> SmsDeliveryRecord {
        SmsDeliveryRecord {
            trace_id: "trace-test".to_string(),
            message_id: "msg-test".to_string(),
            direction: SmsDirection::MobileOriginated,
            sip_state: SipMessageState::Submitted,
            rpdu_ack: RpduAckState::None,
            delivery_reported: false,
            failure_cause: None,
            retry_count: 0,
            parts: Vec::new(),
        }
    }

    #[test]
    fn acked_mo_sms_is_not_left_pending_for_public_api() {
        let mut record = mo_record();
        record.rpdu_ack = RpduAckState::Acked;

        assert_eq!(record.aggregate_state(), SmsDeliveryState::Accepted);
        assert_eq!(record.api_status(), "sent");
    }

    #[test]
    fn delivery_report_takes_mo_sms_to_delivered() {
        let mut record = mo_record();
        record.sip_state = SipMessageState::Accepted;
        record.delivery_reported = true;

        assert_eq!(record.aggregate_state(), SmsDeliveryState::Delivered);
        assert_eq!(record.api_status(), "sent");
    }

    #[test]
    fn submitted_and_provisional_mo_sms_stay_pending_until_ack() {
        let mut record = mo_record();
        record.sip_state = SipMessageState::Queued;
        assert_eq!(record.aggregate_state(), SmsDeliveryState::Queued);
        assert_eq!(record.api_status(), "pending");

        record.sip_state = SipMessageState::Provisional;
        assert_eq!(record.aggregate_state(), SmsDeliveryState::Submitted);
        assert_eq!(record.api_status(), "pending");
    }

    #[test]
    fn rejected_or_timed_out_mo_sms_is_failed() {
        let mut record = mo_record();
        record.sip_state = SipMessageState::Rejected;
        assert_eq!(record.aggregate_state(), SmsDeliveryState::Failed);
        assert_eq!(record.api_status(), "failed");

        record.sip_state = SipMessageState::Timeout;
        assert_eq!(record.aggregate_state(), SmsDeliveryState::Failed);
        assert_eq!(record.api_status(), "failed");
    }

    #[test]
    fn mt_long_sms_requires_all_fragments() {
        let record = SmsDeliveryRecord {
            trace_id: "trace-mt".to_string(),
            message_id: "msg-mt".to_string(),
            direction: SmsDirection::MobileTerminated,
            sip_state: SipMessageState::Accepted,
            rpdu_ack: RpduAckState::Acked,
            delivery_reported: false,
            failure_cause: None,
            retry_count: 0,
            parts: vec![
                SmsPartState {
                    reference: 42,
                    sequence: 1,
                    total: 2,
                    received: true,
                },
                SmsPartState {
                    reference: 42,
                    sequence: 2,
                    total: 2,
                    received: true,
                },
            ],
        };

        assert!(record.parts_complete());
        assert_eq!(record.aggregate_state(), SmsDeliveryState::Received);
        assert_eq!(record.api_status(), "received");
    }

    #[test]
    fn mo_sip_message_2xx_and_rp_ack_reaches_sent_without_pending_drift() {
        let mut machine = SmsRuntimeStateMachine::new(&GB_EE_23433);
        machine.mark_subscribe_reg_ready();
        let rpdu = machine.queue_mo_text(23);
        assert_eq!(rpdu.rpdu_kind, "rp_data");

        let outbound = machine.submit_mo_sip_message();
        assert_eq!(outbound.method, "MESSAGE");
        assert_eq!(outbound.transport, "tcp");

        machine
            .accept_mo_sip_response(202)
            .expect("SIP 2xx accepted");
        machine.accept_mo_rp_ack();
        machine
            .assert_state_consistency()
            .expect("state consistent");

        let snapshot = machine.snapshot();
        assert_eq!(snapshot.mo.state, "accepted");
        assert_eq!(snapshot.mo.api_status, "sent");
        assert_eq!(snapshot.mo.rpdu_ack, "acked");
        assert_eq!(snapshot.pending_delivery_count, 0);
    }

    #[test]
    fn builds_single_part_gsm7_mo_rp_data_without_private_values_in_ids() {
        let submission = build_single_part_mo_submission("+441234567890", "Hi", "+447785016005")
            .expect("single part SMS");

        assert_eq!(submission.part_index, 1);
        assert_eq!(submission.part_count, 1);
        assert_eq!(submission.text_utf16_units, 2);
        assert_eq!(submission.body[0], 0x00);
        assert_eq!(submission.body[1], submission.rp_message_reference);
        assert_eq!(submission.body[2], 0x00);
        assert_eq!(submission.body[3], 7);
        assert_eq!(submission.body[4], 0x91);
        let tpdu_offset = 4 + usize::from(submission.body[3]);
        let tpdu_len = usize::from(submission.body[tpdu_offset]);
        let tpdu = &submission.body[tpdu_offset + 1..tpdu_offset + 1 + tpdu_len];
        assert_eq!(tpdu[0], 0x01);
        assert_eq!(tpdu[1], submission.rp_message_reference);
        assert_eq!(tpdu[2], 12);
        assert_eq!(tpdu[3], 0x91);
        assert_eq!(tpdu[10], 0x00);
        assert_eq!(tpdu[11], 0x00);
        assert_eq!(tpdu[12], 2);
        assert_eq!(&tpdu[13..], &[0xc8, 0x34]);
        assert!(!submission.message_id.contains("441234"));
        assert!(!submission.trace_id.contains("447785"));
    }

    #[test]
    fn falls_back_to_ucs2_mo_rp_data_for_non_gsm7_text() {
        let submission = build_single_part_mo_submission("+441234567890", "余额", "+447785016005")
            .expect("single part SMS");
        let tpdu_offset = 4 + usize::from(submission.body[3]);
        let tpdu_len = usize::from(submission.body[tpdu_offset]);
        let tpdu = &submission.body[tpdu_offset + 1..tpdu_offset + 1 + tpdu_len];

        assert_eq!(tpdu[11], 0x08);
        assert_eq!(tpdu[12], 4);
        assert_eq!(&tpdu[13..], &[0x4f, 0x59, 0x98, 0x9d]);
    }

    #[test]
    fn encodes_short_code_commands_as_gsm7_not_ucs2() {
        let submission = build_single_part_mo_submission("10086", "CHECK", "+447785016005")
            .expect("short code command SMS");
        let tpdu_offset = 4 + usize::from(submission.body[3]);
        let tpdu_len = usize::from(submission.body[tpdu_offset]);
        let tpdu = &submission.body[tpdu_offset + 1..tpdu_offset + 1 + tpdu_len];

        assert_eq!(tpdu[8], 0x00);
        assert_eq!(tpdu[9], 5);
        assert_eq!(decode_gsm7_text(&tpdu[10..], 5), "CHECK");
    }

    #[test]
    fn gsm7_section_sign_round_trips_without_encoding_corruption() {
        let (packed, septets) = encode_gsm7_user_data("§").expect("GSM7 section sign");
        assert_eq!(septets, 1);
        assert_eq!(decode_gsm7_text(&packed, septets), "§");
    }

    #[test]
    fn classifies_rp_ack_and_error_by_message_reference() {
        assert_eq!(classify_rp_ack(&[0x03, 0x22], 0x22), RpduAckState::Acked);
        assert_eq!(
            classify_rp_ack(&[0x05, 0x22, 0x2f], 0x22),
            RpduAckState::Error
        );
        assert_eq!(classify_rp_ack(&[0x03, 0x23], 0x22), RpduAckState::None);
    }

    fn build_test_mt_rp_data(
        reference: u8,
        originator: &str,
        text: &str,
        segment: Option<(u16, u8, u8)>,
    ) -> Vec<u8> {
        let service_center = encode_address_value("+8613800100500").expect("service center");
        let originator_digits = normalized_address_digits(originator)
            .expect("originator digits")
            .digits
            .len();
        let originator_address = encode_address_value(originator).expect("originator address");
        let mut user_data = Vec::new();
        if let Some((segment_reference, total, sequence)) = segment {
            user_data.push(0x05);
            user_data.push(0x00);
            user_data.push(0x03);
            user_data.push(segment_reference as u8);
            user_data.push(total);
            user_data.push(sequence);
        }
        user_data.extend_from_slice(&encode_ucs2_user_data(text).expect("ucs2 user data"));

        let mut tpdu = Vec::new();
        tpdu.push(if segment.is_some() { 0x40 } else { 0x00 });
        tpdu.push(originator_digits as u8);
        tpdu.extend_from_slice(&originator_address);
        tpdu.push(0x00);
        tpdu.push(0x08);
        tpdu.extend_from_slice(&[0x42, 0x10, 0x10, 0x12, 0x34, 0x56, 0x00]);
        tpdu.push(user_data.len() as u8);
        tpdu.extend_from_slice(&user_data);

        let mut body = Vec::new();
        body.push(0x01);
        body.push(reference);
        push_len_prefixed(&mut body, &service_center).expect("rp origin address");
        push_len_prefixed(&mut body, &tpdu).expect("rp user data");
        body
    }

    fn build_test_mt_rp_data_with_empty_destination(
        reference: u8,
        originator: &str,
        text: &str,
    ) -> Vec<u8> {
        let mut body = build_test_mt_rp_data(reference, originator, text, None);
        let origin_len_offset = 2usize;
        let destination_len_offset = origin_len_offset + 1 + usize::from(body[origin_len_offset]);
        body.insert(destination_len_offset, 0x00);
        body
    }

    fn build_test_mt_rp_data_gsm7_segment(
        reference: u8,
        originator: &str,
        text: &str,
        segment_reference: u8,
        total: u8,
        sequence: u8,
    ) -> Vec<u8> {
        let service_center = encode_address_value("+8613800100500").expect("service center");
        let originator_digits = normalized_address_digits(originator)
            .expect("originator digits")
            .digits
            .len();
        let originator_address = encode_address_value(originator).expect("originator address");

        let udh = [0x05, 0x00, 0x03, segment_reference, total, sequence];
        let header_septets = (udh.len() * 8).div_ceil(7);
        let text_septets = text.chars().count();
        let udl = header_septets + text_septets;
        let mut user_data = vec![0u8; (udl * 7).div_ceil(8)];
        user_data[..udh.len()].copy_from_slice(&udh);
        pack_gsm7_ascii_at_bit_offset(text, &mut user_data, header_septets * 7);

        let mut tpdu = Vec::new();
        tpdu.push(0x40);
        tpdu.push(originator_digits as u8);
        tpdu.extend_from_slice(&originator_address);
        tpdu.push(0x00);
        tpdu.push(0x00);
        tpdu.extend_from_slice(&[0x42, 0x10, 0x10, 0x12, 0x34, 0x56, 0x00]);
        tpdu.push(udl as u8);
        tpdu.extend_from_slice(&user_data);

        let mut body = Vec::new();
        body.push(0x01);
        body.push(reference);
        push_len_prefixed(&mut body, &service_center).expect("rp origin address");
        push_len_prefixed(&mut body, &tpdu).expect("rp user data");
        body
    }

    fn pack_gsm7_ascii_at_bit_offset(text: &str, out: &mut [u8], bit_offset: usize) {
        for (index, ch) in text.chars().enumerate() {
            let septet = ch as u8 & 0x7f;
            let bit_index = bit_offset + index * 7;
            for bit in 0..7 {
                if septet & (1 << bit) == 0 {
                    continue;
                }
                let target = bit_index + bit;
                out[target / 8] |= 1 << (target % 8);
            }
        }
    }

    #[test]
    fn parses_mt_rp_data_with_ucs2_sms_deliver_and_builds_ack() {
        let body = build_test_mt_rp_data(0x33, "10086", "OK", None);
        let deliver = parse_mt_rp_data(&body).expect("network RP-DATA");

        assert_eq!(deliver.rp_message_reference, 0x33);
        assert_eq!(deliver.originator, "10086");
        assert_eq!(deliver.text, "OK");
        assert_eq!(deliver.service_center_timestamp, "42101012345600");
        assert_eq!(deliver.segment_reference, None);
        assert_eq!(deliver.segment_sequence, 1);
        assert_eq!(deliver.segment_total, 1);
        assert_eq!(
            build_network_rp_ack(deliver.rp_message_reference),
            vec![0x02, 0x33]
        );
    }

    #[test]
    fn parses_mt_rp_data_with_empty_destination_address() {
        let body = build_test_mt_rp_data_with_empty_destination(0x36, "10086", "OK");
        let deliver = parse_mt_rp_data(&body).expect("network RP-DATA with empty RP-DA");

        assert_eq!(deliver.rp_message_reference, 0x36);
        assert_eq!(deliver.originator, "10086");
        assert_eq!(deliver.text, "OK");
    }

    #[test]
    fn parses_mt_rp_data_concatenation_header_without_exposing_pdu() {
        let body = build_test_mt_rp_data(0x34, "10086", "A", Some((0x4a, 2, 1)));
        let deliver = parse_mt_rp_data(&body).expect("segmented network RP-DATA");

        assert_eq!(deliver.originator, "10086");
        assert_eq!(deliver.text, "A");
        assert_eq!(deliver.segment_reference, Some(0x4a));
        assert_eq!(deliver.segment_sequence, 1);
        assert_eq!(deliver.segment_total, 2);
    }

    #[test]
    fn parses_gsm7_mt_segment_with_udh_without_septet_misalignment() {
        let body = build_test_mt_rp_data_gsm7_segment(
            0x35,
            "10086",
            "You don't have any credit balance",
            0x7b,
            2,
            1,
        );
        let deliver = parse_mt_rp_data(&body).expect("GSM7 segmented network RP-DATA");

        assert_eq!(deliver.originator, "10086");
        assert_eq!(deliver.text, "You don't have any credit balance");
        assert_eq!(deliver.segment_reference, Some(0x7b));
        assert_eq!(deliver.segment_sequence, 1);
        assert_eq!(deliver.segment_total, 2);
    }

    #[test]
    fn mt_duplicate_key_ignores_rp_reference_retransmission() {
        let first = parse_mt_rp_data(&build_test_mt_rp_data(0x34, "10086", "OK", None))
            .expect("first delivery");
        let retry = parse_mt_rp_data(&build_test_mt_rp_data(0x35, "10086", "OK", None))
            .expect("retry delivery");

        assert!(first.is_duplicate_delivery(&retry));
    }

    #[test]
    fn mt_tcp_long_sms_reassembles_two_parts_and_acknowledges_each_part() {
        let mut machine = SmsRuntimeStateMachine::new(&GB_EE_23433);
        machine.mark_subscribe_reg_ready();
        let first = machine.receive_mt_part(77, 1, 2, 130).expect("first part");
        assert_eq!(first.ack_kind, "rp_ack_part");
        assert!(!machine.snapshot().mt.parts_complete);

        let second = machine.receive_mt_part(77, 2, 2, 120).expect("second part");
        assert_eq!(second.ack_kind, "rp_ack_complete");

        let snapshot = machine.snapshot();
        assert_eq!(snapshot.receiver_transport, "tcp");
        assert_eq!(snapshot.mt.state, "received");
        assert_eq!(snapshot.mt.api_status, "received");
        assert_eq!(snapshot.mt.parts.len(), 2);
        assert!(snapshot.mt.parts_complete);
        assert_eq!(
            snapshot.reassembly.as_ref().map(|item| item.complete),
            Some(true)
        );
    }

    #[test]
    fn invalid_mt_part_is_rejected_before_state_update() {
        let mut machine = SmsRuntimeStateMachine::new(&GB_EE_23433);
        let err = machine
            .receive_mt_part(1, 3, 2, 50)
            .expect_err("sequence cannot exceed total");

        assert!(matches!(
            err,
            SmsRuntimeError::InvalidPart {
                sequence: 3,
                total: 2
            }
        ));
        assert!(machine.snapshot().mt.parts.is_empty());
    }

    #[test]
    fn gsm7_long_mo_sms_uses_153_septet_parts_and_concat_udh() {
        let submissions =
            build_mo_submissions("+8613800138000", &"A".repeat(161), "+8613800100500").unwrap();
        assert_eq!(submissions.len(), 2);
        let concat_reference = submissions[0].rp_message_reference;
        for (index, submission) in submissions.iter().enumerate() {
            let expected = [
                0x05,
                0x00,
                0x03,
                concat_reference,
                0x02,
                u8::try_from(index + 1).unwrap(),
            ];
            assert_eq!(submission.part_count, 2);
            assert!(submission.body.windows(6).any(|window| window == expected));
        }
        assert_eq!(submissions[0].message_id, submissions[1].message_id);
    }

    #[test]
    fn ucs2_long_mo_sms_splits_on_utf16_boundaries() {
        let submissions =
            build_mo_submissions("+8613800138000", &"测".repeat(71), "+8613800100500").unwrap();
        assert_eq!(submissions.len(), 2);
        assert_eq!(submissions[0].part_index, 1);
        assert_eq!(submissions[1].part_index, 2);
        assert_eq!(submissions[0].text_utf16_units, 71);
        assert_eq!(submissions[1].text_utf16_units, 71);
    }

    #[test]
    fn gsm7_extension_characters_count_as_two_septets_when_splitting() {
        let submissions =
            build_mo_submissions("+8613800138000", &"{".repeat(81), "+8613800100500").unwrap();
        assert_eq!(submissions.len(), 2);
    }

    #[test]
    fn dry_run_sms_snapshot_is_ready_and_serializes_no_private_content() {
        let snapshot = build_dry_run_sms_snapshot(&GB_EE_23433);

        assert!(snapshot.sms_ready);
        assert_eq!(snapshot.mo.api_status, "sent");
        assert_eq!(snapshot.mt.api_status, "received");
        assert_eq!(snapshot.receiver_transport, "tcp");
        assert_eq!(snapshot.pending_delivery_count, 0);
        assert_eq!(
            snapshot.reassembly.as_ref().map(|item| item.expected_parts),
            Some(2)
        );

        let json = serde_json::to_string(&snapshot).expect("serialize sms snapshot");
        for forbidden in [
            "\"phone_number\"",
            "\"sender\"",
            "\"recipient\"",
            "\"content\"",
            "\"text\"",
            "\"tpdu\"",
            "\"rpdu\"",
            "\"sip_body\"",
            "hello",
            "message body",
        ] {
            assert!(
                !json.to_ascii_lowercase().contains(forbidden),
                "SMS snapshot must not expose {forbidden}"
            );
        }
    }

    #[test]
    fn gsm7_long_mo_sms_round_trips_text_after_udh_fill_bit() {
        let text = "SimAdmin multipart round trip. ".repeat(7);
        let submissions = build_mo_submissions("+8613800138000", &text, "+8613800100500").unwrap();
        assert!(submissions.len() > 1);

        let mut decoded = String::new();
        for submission in submissions {
            let body = &submission.body;
            let service_center_len = usize::from(body[3]);
            let tpdu_len_offset = 4 + service_center_len;
            let tpdu_len = usize::from(body[tpdu_len_offset]);
            let tpdu = &body[tpdu_len_offset + 1..tpdu_len_offset + 1 + tpdu_len];
            let destination_digits = usize::from(tpdu[2]);
            let destination_octets = 1 + destination_digits.div_ceil(2);
            let udl_offset = 3 + destination_octets + 2;
            let udl = usize::from(tpdu[udl_offset]);
            let user_data = &tpdu[udl_offset + 1..];
            decoded.push_str(&decode_gsm7_user_data(user_data, udl, 6));
        }

        assert_eq!(decoded, text);
    }
}
