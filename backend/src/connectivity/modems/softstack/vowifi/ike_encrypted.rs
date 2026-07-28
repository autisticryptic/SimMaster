#![allow(dead_code)]

use std::fmt;

use aes::{Aes128, Aes256};
use cbc::cipher::{block_padding::NoPadding, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use ring::{hmac, rand::SecureRandom};
use serde::Serialize;

use super::{
    ike_codec::{
        IkeCodecError, IkeExchangeType, IkeFlags, IkeHeader, IkeMessage, IkePayload, IkePayloadType,
    },
    ike_keys::{IkeIntegrityAlgorithm, IkeKeySchedulePlan, IkeSecretBundle},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EncryptedPayloadMode {
    MetadataOnly,
    CiphertextReady,
}

impl EncryptedPayloadMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MetadataOnly => "metadata_only",
            Self::CiphertextReady => "ciphertext_ready",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EncryptedPayloadPlan {
    pub mode: &'static str,
    pub outer_payload: &'static str,
    pub first_inner_payload: &'static str,
    pub cipher: &'static str,
    pub integrity: &'static str,
    pub iv_bytes: usize,
    pub block_bytes: usize,
    pub icv_bytes: usize,
    pub encrypted_payload_bytes: usize,
    pub inner_payload_count: usize,
    pub exported_plaintext: bool,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IkeSkDirection {
    InitiatorToResponder,
    ResponderToInitiator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncryptedPayloadError {
    UnsupportedCipher(String),
    UnsupportedIntegrity(String),
    EmptyInnerPayloads,
    RandomFailed,
    PacketTooLarge(usize),
    IntegrityMismatch,
    InvalidEncryptedPayload,
    InvalidPadding,
    Codec(IkeCodecError),
}

impl fmt::Display for EncryptedPayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedCipher(cipher) => write!(f, "unsupported IKE cipher: {cipher}"),
            Self::UnsupportedIntegrity(integrity) => {
                write!(f, "unsupported IKE integrity algorithm: {integrity}")
            }
            Self::EmptyInnerPayloads => write!(f, "encrypted payload has no inner payloads"),
            Self::RandomFailed => write!(f, "encrypted payload random generation failed"),
            Self::PacketTooLarge(size) => write!(f, "encrypted payload too large: {size}"),
            Self::IntegrityMismatch => write!(f, "encrypted payload integrity check failed"),
            Self::InvalidEncryptedPayload => write!(f, "invalid encrypted payload"),
            Self::InvalidPadding => write!(f, "invalid encrypted payload padding"),
            Self::Codec(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for EncryptedPayloadError {}

impl From<IkeCodecError> for EncryptedPayloadError {
    fn from(value: IkeCodecError) -> Self {
        Self::Codec(value)
    }
}

pub fn build_encrypted_payload_plan(
    key_schedule: &IkeKeySchedulePlan,
    inner_payloads: &[IkePayload],
) -> Result<EncryptedPayloadPlan, EncryptedPayloadError> {
    if inner_payloads.is_empty() {
        return Err(EncryptedPayloadError::EmptyInnerPayloads);
    }
    let (iv_bytes, block_bytes) = match key_schedule.encryption {
        "aes_cbc" => (16, 16),
        other => return Err(EncryptedPayloadError::UnsupportedCipher(other.to_string())),
    };
    let icv_bytes = match key_schedule.integrity {
        "hmac_sha1_96" => 12,
        "hmac_sha256_128" => 16,
        "hmac_sha512_256" => 32,
        other => {
            return Err(EncryptedPayloadError::UnsupportedIntegrity(
                other.to_string(),
            ))
        }
    };
    let plaintext_len = inner_payloads
        .iter()
        .map(|payload| 4 + payload.body.len())
        .sum::<usize>();
    let padding_len = padding_len_for_block(plaintext_len + 1, block_bytes);
    let encrypted_payload_bytes = iv_bytes + plaintext_len + padding_len + 1 + icv_bytes;

    Ok(EncryptedPayloadPlan {
        mode: EncryptedPayloadMode::MetadataOnly.as_str(),
        outer_payload: "encrypted",
        first_inner_payload: inner_payloads[0].payload_type.name(),
        cipher: key_schedule.encryption,
        integrity: key_schedule.integrity,
        iv_bytes,
        block_bytes,
        icv_bytes,
        encrypted_payload_bytes,
        inner_payload_count: inner_payloads.len(),
        exported_plaintext: false,
        sensitive_values_policy: "plaintext_ciphertext_and_integrity_tags_not_serialized",
    })
}

pub fn build_encrypted_metadata_payload(plan: &EncryptedPayloadPlan) -> IkePayload {
    IkePayload {
        payload_type: IkePayloadType::Encrypted,
        critical: false,
        body: format!(
            "simadmin-ike-encrypted-metadata:{}:{}:{}",
            plan.mode, plan.cipher, plan.integrity
        )
        .into_bytes(),
    }
}

pub fn build_encrypted_payload(
    bundle: &IkeSecretBundle,
    direction: IkeSkDirection,
    inner_payloads: &[IkePayload],
) -> Result<IkePayload, EncryptedPayloadError> {
    if inner_payloads.is_empty() {
        return Err(EncryptedPayloadError::EmptyInnerPayloads);
    }
    let plan = bundle.summary();
    let (iv_bytes, block_bytes) = cipher_shape(&plan)?;
    let icv_bytes = integrity_len(&plan)?;
    let mut plaintext = encode_inner_payload_chain(inner_payloads)?;
    let padding_len = padding_len_for_block(plaintext.len() + 1, block_bytes);
    for value in 0..padding_len {
        plaintext.push(value as u8);
    }
    plaintext.push(padding_len as u8);

    let mut iv = vec![0u8; iv_bytes];
    ring::rand::SystemRandom::new()
        .fill(&mut iv)
        .map_err(|_| EncryptedPayloadError::RandomFailed)?;
    let encryption_key = match direction {
        IkeSkDirection::InitiatorToResponder => bundle.sk_ei.expose_for_protocol(),
        IkeSkDirection::ResponderToInitiator => bundle.sk_er.expose_for_protocol(),
    };
    let ciphertext = encrypt_cbc(&plan, encryption_key, &iv, &plaintext)?;
    let mut body = Vec::with_capacity(iv.len() + ciphertext.len() + icv_bytes);
    body.extend_from_slice(&iv);
    body.extend_from_slice(&ciphertext);

    Ok(IkePayload {
        payload_type: IkePayloadType::Encrypted,
        critical: false,
        body,
    })
}

pub fn encode_encrypted_message(
    message: &IkeMessage,
    first_inner_payload: IkePayloadType,
    bundle: &IkeSecretBundle,
    direction: IkeSkDirection,
) -> Result<Vec<u8>, EncryptedPayloadError> {
    let icv_bytes = integrity_len(&bundle.summary())?;
    let mut encoded =
        encode_message_with_first_encrypted_inner_payload(message, first_inner_payload, icv_bytes)?;
    let integrity_key = match direction {
        IkeSkDirection::InitiatorToResponder => bundle.sk_ai.expose_for_protocol(),
        IkeSkDirection::ResponderToInitiator => bundle.sk_ar.expose_for_protocol(),
    };
    let tag = integrity_tag(&bundle.summary(), integrity_key, &encoded, icv_bytes)?;
    encoded.extend_from_slice(&tag);
    Ok(encoded)
}

pub fn decrypt_encrypted_payload_from_message(
    encoded_message: &[u8],
    bundle: &IkeSecretBundle,
    direction: IkeSkDirection,
) -> Result<Vec<IkePayload>, EncryptedPayloadError> {
    let plan = bundle.summary();
    let icv_bytes = integrity_len(&plan)?;
    if encoded_message.len() < icv_bytes + 32 {
        return Err(EncryptedPayloadError::InvalidEncryptedPayload);
    }
    let (first_inner_payload, encrypted_body, received_icv) =
        encrypted_body_from_encoded_message(encoded_message, icv_bytes)?;
    let message_without_icv = &encoded_message[..encoded_message.len() - icv_bytes];
    let integrity_key = match direction {
        IkeSkDirection::InitiatorToResponder => bundle.sk_ai.expose_for_protocol(),
        IkeSkDirection::ResponderToInitiator => bundle.sk_ar.expose_for_protocol(),
    };
    let expected = integrity_tag(&plan, integrity_key, message_without_icv, icv_bytes)?;
    if !constant_time_eq(&expected, received_icv) {
        return Err(EncryptedPayloadError::IntegrityMismatch);
    }

    let encryption_key = match direction {
        IkeSkDirection::InitiatorToResponder => bundle.sk_ei.expose_for_protocol(),
        IkeSkDirection::ResponderToInitiator => bundle.sk_er.expose_for_protocol(),
    };
    decrypt_encrypted_payload_body(&plan, encryption_key, first_inner_payload, &encrypted_body)
}

pub fn encrypted_response_header_matches(
    encoded_message: &[u8],
    initiator_spi: u64,
    exchange_type: IkeExchangeType,
    message_id: u32,
) -> Result<bool, EncryptedPayloadError> {
    if encoded_message.len() < 32 {
        return Err(EncryptedPayloadError::InvalidEncryptedPayload);
    }
    let declared =
        u32::from_be_bytes(encoded_message[24..28].try_into().expect("slice length")) as usize;
    if declared != encoded_message.len() {
        return Err(EncryptedPayloadError::InvalidEncryptedPayload);
    }
    let header_initiator_spi =
        u64::from_be_bytes(encoded_message[0..8].try_into().expect("slice length"));
    let next_payload = IkePayloadType::from_u8(encoded_message[16]);
    let version = encoded_message[17] >> 4;
    let header_exchange = IkeExchangeType::from_u8(encoded_message[18]);
    let flags = IkeFlags::from_u8(encoded_message[19]);
    let header_message_id =
        u32::from_be_bytes(encoded_message[20..24].try_into().expect("slice length"));
    Ok(header_initiator_spi == initiator_spi
        && next_payload == IkePayloadType::Encrypted
        && version == 2
        && header_exchange == exchange_type
        && flags.response
        && header_message_id == message_id)
}

pub fn decrypt_encrypted_payload_body(
    plan: &IkeKeySchedulePlan,
    encryption_key: &[u8],
    first_inner_payload: IkePayloadType,
    body: &[u8],
) -> Result<Vec<IkePayload>, EncryptedPayloadError> {
    let (iv_bytes, block_bytes) = cipher_shape(plan)?;
    if body.len() <= iv_bytes || !(body.len() - iv_bytes).is_multiple_of(block_bytes) {
        return Err(EncryptedPayloadError::InvalidEncryptedPayload);
    }
    let (iv, ciphertext) = body.split_at(iv_bytes);
    let plaintext = decrypt_cbc(plan, encryption_key, iv, ciphertext)?;
    let plaintext = strip_padding(&plaintext)?;
    decode_inner_payload_chain(first_inner_payload, plaintext)
}

fn encode_inner_payload_chain(payloads: &[IkePayload]) -> Result<Vec<u8>, EncryptedPayloadError> {
    let mut out = Vec::new();
    for (index, payload) in payloads.iter().enumerate() {
        let next_payload = payloads
            .get(index + 1)
            .map(|next| next.payload_type)
            .unwrap_or(IkePayloadType::NoNext);
        let len = 4 + payload.body.len();
        if len > u16::MAX as usize {
            return Err(EncryptedPayloadError::PacketTooLarge(len));
        }
        out.push(next_payload.as_u8());
        out.push(if payload.critical { 0x80 } else { 0 });
        out.extend_from_slice(&(len as u16).to_be_bytes());
        out.extend_from_slice(&payload.body);
    }
    Ok(out)
}

fn decode_inner_payload_chain(
    first_payload_type: IkePayloadType,
    mut input: &[u8],
) -> Result<Vec<IkePayload>, EncryptedPayloadError> {
    let mut payloads = Vec::new();
    let mut current_type = first_payload_type;
    while !input.is_empty() {
        if input.len() < 4 {
            return Err(EncryptedPayloadError::InvalidEncryptedPayload);
        }
        let next_type = IkePayloadType::from_u8(input[0]);
        let critical = input[1] & 0x80 != 0;
        let len = u16::from_be_bytes([input[2], input[3]]) as usize;
        if len < 4 || len > input.len() {
            return Err(EncryptedPayloadError::InvalidEncryptedPayload);
        }
        payloads.push(IkePayload {
            payload_type: current_type,
            critical,
            body: input[4..len].to_vec(),
        });
        current_type = next_type;
        input = &input[len..];
    }
    Ok(payloads)
}

pub fn encrypted_message_from_payload(
    initiator_spi: u64,
    responder_spi: u64,
    exchange_type: IkeExchangeType,
    initiator_request: bool,
    message_id: u32,
    payload: IkePayload,
) -> IkeMessage {
    IkeMessage {
        header: IkeHeader {
            initiator_spi,
            responder_spi,
            next_payload: IkePayloadType::Encrypted,
            major_version: 2,
            minor_version: 0,
            exchange_type,
            flags: IkeFlags {
                initiator: initiator_request,
                response: false,
                version: false,
            },
            message_id,
            length: 0,
        },
        payloads: vec![payload],
    }
}

fn cipher_shape(plan: &IkeKeySchedulePlan) -> Result<(usize, usize), EncryptedPayloadError> {
    match plan.encryption {
        "aes_cbc" => Ok((16, 16)),
        other => Err(EncryptedPayloadError::UnsupportedCipher(other.to_string())),
    }
}

fn integrity_len(plan: &IkeKeySchedulePlan) -> Result<usize, EncryptedPayloadError> {
    match plan.integrity {
        "hmac_sha1_96" => Ok(12),
        "hmac_sha256_128" => Ok(16),
        "hmac_sha512_256" => Ok(32),
        other => Err(EncryptedPayloadError::UnsupportedIntegrity(
            other.to_string(),
        )),
    }
}

fn encrypt_cbc(
    plan: &IkeKeySchedulePlan,
    key: &[u8],
    iv: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, EncryptedPayloadError> {
    match (plan.encryption, key.len()) {
        ("aes_cbc", 16) => Ok(cbc::Encryptor::<Aes128>::new_from_slices(key, iv)
            .map_err(|_| EncryptedPayloadError::InvalidEncryptedPayload)?
            .encrypt_padded_vec_mut::<NoPadding>(plaintext)),
        ("aes_cbc", 32) => Ok(cbc::Encryptor::<Aes256>::new_from_slices(key, iv)
            .map_err(|_| EncryptedPayloadError::InvalidEncryptedPayload)?
            .encrypt_padded_vec_mut::<NoPadding>(plaintext)),
        (other, _) => Err(EncryptedPayloadError::UnsupportedCipher(other.to_string())),
    }
}

fn decrypt_cbc(
    plan: &IkeKeySchedulePlan,
    key: &[u8],
    iv: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, EncryptedPayloadError> {
    match (plan.encryption, key.len()) {
        ("aes_cbc", 16) => cbc::Decryptor::<Aes128>::new_from_slices(key, iv)
            .map_err(|_| EncryptedPayloadError::InvalidEncryptedPayload)?
            .decrypt_padded_vec_mut::<NoPadding>(ciphertext)
            .map_err(|_| EncryptedPayloadError::InvalidEncryptedPayload),
        ("aes_cbc", 32) => cbc::Decryptor::<Aes256>::new_from_slices(key, iv)
            .map_err(|_| EncryptedPayloadError::InvalidEncryptedPayload)?
            .decrypt_padded_vec_mut::<NoPadding>(ciphertext)
            .map_err(|_| EncryptedPayloadError::InvalidEncryptedPayload),
        (other, _) => Err(EncryptedPayloadError::UnsupportedCipher(other.to_string())),
    }
}

fn integrity_tag(
    plan: &IkeKeySchedulePlan,
    key: &[u8],
    message_without_icv: &[u8],
    icv_bytes: usize,
) -> Result<Vec<u8>, EncryptedPayloadError> {
    let algorithm = match plan.integrity {
        "hmac_sha1_96" => IkeIntegrityAlgorithm::HmacSha1_96,
        "hmac_sha256_128" => IkeIntegrityAlgorithm::HmacSha256_128,
        "hmac_sha512_256" => IkeIntegrityAlgorithm::HmacSha512_256,
        other => {
            return Err(EncryptedPayloadError::UnsupportedIntegrity(
                other.to_string(),
            ))
        }
    };
    let ring_algorithm = match algorithm {
        IkeIntegrityAlgorithm::HmacSha1_96 => hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY,
        IkeIntegrityAlgorithm::HmacSha256_128 => hmac::HMAC_SHA256,
        IkeIntegrityAlgorithm::HmacSha512_256 => hmac::HMAC_SHA512,
    };
    let key = hmac::Key::new(ring_algorithm, key);
    let mut tag = hmac::sign(&key, message_without_icv).as_ref().to_vec();
    tag.truncate(icv_bytes);
    Ok(tag)
}

fn strip_padding(plaintext: &[u8]) -> Result<&[u8], EncryptedPayloadError> {
    let Some((&pad_len, body)) = plaintext.split_last() else {
        return Err(EncryptedPayloadError::InvalidPadding);
    };
    let padding_len = usize::from(pad_len);
    if padding_len > body.len() {
        return Err(EncryptedPayloadError::InvalidPadding);
    }
    Ok(&body[..body.len() - padding_len])
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (l, r) in left.iter().zip(right) {
        diff |= l ^ r;
    }
    diff == 0
}

fn encode_message_with_first_encrypted_inner_payload(
    message: &IkeMessage,
    first_inner_payload: IkePayloadType,
    icv_bytes: usize,
) -> Result<Vec<u8>, EncryptedPayloadError> {
    let encrypted = message
        .payloads
        .first()
        .filter(|payload| payload.payload_type == IkePayloadType::Encrypted)
        .ok_or(EncryptedPayloadError::InvalidEncryptedPayload)?;
    let payload_len = 4usize
        .checked_add(encrypted.body.len())
        .and_then(|len| len.checked_add(icv_bytes))
        .ok_or(EncryptedPayloadError::PacketTooLarge(usize::MAX))?;
    if payload_len > u16::MAX as usize {
        return Err(EncryptedPayloadError::PacketTooLarge(payload_len));
    }
    let total_len = 28usize
        .checked_add(payload_len)
        .ok_or(EncryptedPayloadError::PacketTooLarge(usize::MAX))?;
    if total_len > u32::MAX as usize {
        return Err(EncryptedPayloadError::PacketTooLarge(total_len));
    }

    let mut out = Vec::with_capacity(total_len);
    out.extend_from_slice(&message.header.initiator_spi.to_be_bytes());
    out.extend_from_slice(&message.header.responder_spi.to_be_bytes());
    out.push(IkePayloadType::Encrypted.as_u8());
    out.push((message.header.major_version << 4) | (message.header.minor_version & 0x0f));
    out.push(message.header.exchange_type.as_u8());
    out.push(message.header.flags.as_u8());
    out.extend_from_slice(&message.header.message_id.to_be_bytes());
    out.extend_from_slice(&(total_len as u32).to_be_bytes());
    out.push(first_inner_payload.as_u8());
    out.push(if encrypted.critical { 0x80 } else { 0 });
    out.extend_from_slice(&(payload_len as u16).to_be_bytes());
    out.extend_from_slice(&encrypted.body);
    Ok(out)
}

fn encrypted_body_from_encoded_message(
    encoded: &[u8],
    icv_bytes: usize,
) -> Result<(IkePayloadType, Vec<u8>, &[u8]), EncryptedPayloadError> {
    if encoded.len() < 32 || encoded[16] != IkePayloadType::Encrypted.as_u8() {
        return Err(EncryptedPayloadError::InvalidEncryptedPayload);
    }
    let declared = u32::from_be_bytes(encoded[24..28].try_into().expect("slice length")) as usize;
    if declared != encoded.len() {
        return Err(EncryptedPayloadError::InvalidEncryptedPayload);
    }
    let first_inner_payload = IkePayloadType::from_u8(encoded[28]);
    let payload_len = u16::from_be_bytes([encoded[30], encoded[31]]) as usize;
    if payload_len < 4 || 28 + payload_len != encoded.len() {
        return Err(EncryptedPayloadError::InvalidEncryptedPayload);
    }
    let body_with_icv = &encoded[32..];
    if body_with_icv.len() < icv_bytes {
        return Err(EncryptedPayloadError::InvalidEncryptedPayload);
    }
    let (body, icv) = body_with_icv.split_at(body_with_icv.len() - icv_bytes);
    Ok((first_inner_payload, body.to_vec(), icv))
}

trait PayloadTypeName {
    fn name(self) -> &'static str;
}

impl PayloadTypeName for IkePayloadType {
    fn name(self) -> &'static str {
        match self {
            Self::NoNext => "no_next",
            Self::SecurityAssociation => "security_association",
            Self::KeyExchange => "key_exchange",
            Self::IdentificationInitiator => "identification_initiator",
            Self::IdentificationResponder => "identification_responder",
            Self::Certificate => "certificate",
            Self::CertificateRequest => "certificate_request",
            Self::Authentication => "authentication",
            Self::Nonce => "nonce",
            Self::Notify => "notify",
            Self::Delete => "delete",
            Self::VendorId => "vendor_id",
            Self::TrafficSelectorInitiator => "traffic_selector_initiator",
            Self::TrafficSelectorResponder => "traffic_selector_responder",
            Self::Encrypted => "encrypted",
            Self::Configuration => "configuration",
            Self::ExtensibleAuthentication => "eap",
            Self::EncryptedFragment => "encrypted_fragment",
            Self::Unknown(_) => "unknown",
        }
    }
}

fn padding_len_for_block(length_with_pad_len_byte: usize, block_bytes: usize) -> usize {
    let rem = length_with_pad_len_byte % block_bytes;
    if rem == 0 {
        0
    } else {
        block_bytes - rem
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::modems::softstack::vowifi::{
        ike_codec::{IkeExchangeType, IkePayloadType},
        ike_keys::{build_key_schedule_plan, derive_ike_secret_bundle},
        ike_payloads::{build_nonce_payload, ike_proposal_from_profile_string},
    };

    #[test]
    fn builds_metadata_plan_for_encrypted_ike_auth_payload() {
        let proposal =
            ike_proposal_from_profile_string("aes128-sha256-modp2048", 1).expect("proposal");
        let key_schedule = build_key_schedule_plan(&proposal).expect("key plan");
        let plan = build_encrypted_payload_plan(
            &key_schedule,
            &[
                IkePayload {
                    payload_type: IkePayloadType::IdentificationInitiator,
                    critical: false,
                    body: vec![0x11; 17],
                },
                build_nonce_payload(&[0x22; 32]),
            ],
        )
        .expect("encrypted plan");

        assert_eq!(plan.mode, "metadata_only");
        assert_eq!(plan.outer_payload, "encrypted");
        assert_eq!(plan.iv_bytes, 16);
        assert_eq!(plan.block_bytes, 16);
        assert_eq!(plan.icv_bytes, 16);
        assert_eq!(plan.inner_payload_count, 2);
        assert!(!plan.exported_plaintext);

        let payload = build_encrypted_metadata_payload(&plan);
        assert_eq!(payload.payload_type, IkePayloadType::Encrypted);
        assert!(!payload.body.is_empty());
    }

    #[test]
    fn encrypted_plan_summary_does_not_serialize_plaintext_or_tags() {
        let proposal =
            ike_proposal_from_profile_string("aes128-sha1-modp2048", 1).expect("proposal");
        let key_schedule = build_key_schedule_plan(&proposal).expect("key plan");
        let plan = build_encrypted_payload_plan(&key_schedule, &[build_nonce_payload(&[0x22; 32])])
            .expect("encrypted plan");

        assert_eq!(plan.icv_bytes, 12);
        let json = serde_json::to_string(&plan).expect("serialize plan");
        for forbidden_key in [
            "plaintext",
            "ciphertext",
            "integrity_tag",
            "iv_value",
            "sk_ei",
            "sk_ai",
        ] {
            assert!(
                !json
                    .to_ascii_lowercase()
                    .contains(&format!("\"{forbidden_key}\"")),
                "encrypted plan must not contain a {forbidden_key} field"
            );
        }
    }

    #[test]
    fn rejects_empty_inner_payload_set() {
        let proposal =
            ike_proposal_from_profile_string("aes128-sha256-modp2048", 1).expect("proposal");
        let key_schedule = build_key_schedule_plan(&proposal).expect("key plan");

        assert_eq!(
            build_encrypted_payload_plan(&key_schedule, &[]).unwrap_err(),
            EncryptedPayloadError::EmptyInnerPayloads
        );
    }

    #[test]
    fn encrypts_and_decrypts_sk_payload_without_exporting_values() {
        let proposal =
            ike_proposal_from_profile_string("aes128-sha256-modp2048", 1).expect("proposal");
        let bundle = derive_ike_secret_bundle(
            &proposal,
            &[0x11; 32],
            &[0x22; 32],
            0x0102_0304_0506_0708,
            0x1112_1314_1516_1718,
            &[0x33; 256],
        )
        .expect("derive bundle");
        let inner_payloads = vec![
            IkePayload {
                payload_type: IkePayloadType::IdentificationInitiator,
                critical: false,
                body: b"identity-redacted-placeholder".to_vec(),
            },
            IkePayload {
                payload_type: IkePayloadType::TrafficSelectorInitiator,
                critical: false,
                body: vec![0x44; 8],
            },
        ];
        let encrypted = build_encrypted_payload(
            &bundle,
            IkeSkDirection::InitiatorToResponder,
            &inner_payloads,
        )
        .expect("encrypt payload");
        let message = encrypted_message_from_payload(
            0x0102_0304_0506_0708,
            0x1112_1314_1516_1718,
            IkeExchangeType::IkeAuth,
            true,
            1,
            encrypted,
        );
        let encoded = encode_encrypted_message(
            &message,
            IkePayloadType::IdentificationInitiator,
            &bundle,
            IkeSkDirection::InitiatorToResponder,
        )
        .expect("encode encrypted message");
        let decoded = decrypt_encrypted_payload_from_message(
            &encoded,
            &bundle,
            IkeSkDirection::InitiatorToResponder,
        )
        .expect("decrypt payload");

        assert_eq!(decoded.len(), inner_payloads.len());
        assert_eq!(
            decoded[0].payload_type,
            IkePayloadType::IdentificationInitiator
        );
        assert_eq!(
            decoded[1].payload_type,
            IkePayloadType::TrafficSelectorInitiator
        );
        assert_eq!(decoded[1].body, vec![0x44; 8]);

        let json = serde_json::to_string(&bundle.summary()).expect("serialize summary");
        for forbidden in ["sk_ai", "sk_ei", "333333", "identity-redacted-placeholder"] {
            assert!(!json.to_ascii_lowercase().contains(forbidden));
        }
    }

    #[test]
    fn encrypted_message_rejects_integrity_tampering() {
        let proposal =
            ike_proposal_from_profile_string("aes128-sha1-modp2048", 1).expect("proposal");
        let bundle =
            derive_ike_secret_bundle(&proposal, &[0x11; 32], &[0x22; 32], 1, 2, &[0x33; 256])
                .expect("derive bundle");
        let encrypted = build_encrypted_payload(
            &bundle,
            IkeSkDirection::ResponderToInitiator,
            &[IkePayload {
                payload_type: IkePayloadType::ExtensibleAuthentication,
                critical: false,
                body: vec![0x01, 0x02, 0x03, 0x04],
            }],
        )
        .expect("encrypt");
        let message =
            encrypted_message_from_payload(1, 2, IkeExchangeType::IkeAuth, false, 1, encrypted);
        let mut encoded = encode_encrypted_message(
            &message,
            IkePayloadType::ExtensibleAuthentication,
            &bundle,
            IkeSkDirection::ResponderToInitiator,
        )
        .expect("encode");
        let last = encoded.last_mut().expect("has icv");
        *last ^= 0x01;

        assert_eq!(
            decrypt_encrypted_payload_from_message(
                &encoded,
                &bundle,
                IkeSkDirection::ResponderToInitiator,
            )
            .unwrap_err(),
            EncryptedPayloadError::IntegrityMismatch
        );
    }
}
