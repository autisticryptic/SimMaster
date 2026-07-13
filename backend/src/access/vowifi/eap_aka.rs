#![allow(dead_code)]

use std::fmt;

use ring::{digest, hmac};

use super::{ike_eap::EAP_TYPE_AKA, qmi_uim::UsimAkaApduResult};

const EAP_CODE_RESPONSE: u8 = 2;
const EAP_AKA_SUBTYPE_CHALLENGE: u8 = 1;
const EAP_AKA_SUBTYPE_SYNC_FAILURE: u8 = 4;
const EAP_AKA_SUBTYPE_IDENTITY: u8 = 5;
const EAP_AKA_SUBTYPE_NOTIFICATION: u8 = 12;
const AT_RAND: u8 = 1;
const AT_AUTN: u8 = 2;
const AT_RES: u8 = 3;
const AT_AUTS: u8 = 4;
const AT_PERMANENT_ID_REQ: u8 = 10;
const AT_MAC: u8 = 11;
const AT_NOTIFICATION: u8 = 12;
const AT_ANY_ID_REQ: u8 = 13;
const AT_IDENTITY: u8 = 14;
const AT_FULLAUTH_ID_REQ: u8 = 17;
const AT_RESULT_IND: u8 = 135;
const EAP_AKA_HEADER_LEN: usize = 8;
const MAC_LEN: usize = 16;
const K_AUT_LEN: usize = 16;
const K_ENCR_LEN: usize = 16;
const KEY_STREAM_LEN: usize = K_ENCR_LEN + K_AUT_LEN + 64 + 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EapAkaChallenge {
    pub identifier: u8,
    pub rand: Vec<u8>,
    pub autn: Vec<u8>,
    pub mac_present: bool,
    pub result_indication: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EapAkaNotification {
    pub identifier: u8,
    pub code: u16,
    pub success: bool,
    pub after_challenge: bool,
    pub mac_required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EapAkaIdentityRequest {
    pub identifier: u8,
    pub request_kind: &'static str,
}

#[derive(Clone, PartialEq, Eq)]
pub struct EapAkaResponsePacket {
    packet: Vec<u8>,
    key_material: Option<EapAkaKeyMaterial>,
}

impl EapAkaResponsePacket {
    pub fn expose_for_ike_encryption(&self) -> &[u8] {
        &self.packet
    }

    pub fn msk_for_ike_auth(&self) -> Option<&[u8]> {
        self.key_material.as_ref().map(|keys| keys.msk.as_slice())
    }

    pub fn notification_response(
        &self,
        request_packet: &[u8],
    ) -> Result<EapAkaResponsePacket, EapAkaProtocolError> {
        let notification = parse_notification(request_packet)?;
        let keys = self
            .key_material
            .as_ref()
            .ok_or(EapAkaProtocolError::MissingKeyMaterial)?;
        build_notification_response(&notification, keys)
    }

    pub fn identity_response(
        &self,
        request_packet: &[u8],
        identity: &str,
    ) -> Result<EapAkaResponsePacket, EapAkaProtocolError> {
        let request = parse_identity_request(request_packet)?;
        build_identity_response(&request, identity, self.key_material.clone())
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct EapAkaKeyMaterial {
    k_aut: [u8; K_AUT_LEN],
    msk: Vec<u8>,
}

impl std::fmt::Debug for EapAkaResponsePacket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EapAkaResponsePacket")
            .field("packet_len", &self.packet.len())
            .field(
                "key_material",
                &self.key_material.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl std::fmt::Debug for EapAkaKeyMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EapAkaKeyMaterial")
            .field("k_aut", &"<redacted>")
            .field("msk_len", &self.msk.len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EapAkaProtocolError {
    Truncated,
    LengthMismatch,
    UnsupportedMethod,
    UnsupportedSubtype,
    AttributeTruncated,
    InvalidAttributeLength,
    MissingRand,
    MissingAutn,
    MissingMac,
    MissingNotification,
    MissingIdentityRequest,
    MissingKeyMaterial,
    InvalidAkaResult,
    PacketTooLarge,
}

impl fmt::Display for EapAkaProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(f, "truncated EAP-AKA packet"),
            Self::LengthMismatch => write!(f, "EAP-AKA length mismatch"),
            Self::UnsupportedMethod => write!(f, "unsupported EAP method"),
            Self::UnsupportedSubtype => write!(f, "unsupported EAP-AKA subtype"),
            Self::AttributeTruncated => write!(f, "truncated EAP-AKA attribute"),
            Self::InvalidAttributeLength => write!(f, "invalid EAP-AKA attribute length"),
            Self::MissingRand => write!(f, "EAP-AKA challenge missing RAND"),
            Self::MissingAutn => write!(f, "EAP-AKA challenge missing AUTN"),
            Self::MissingMac => write!(f, "EAP-AKA challenge missing MAC"),
            Self::MissingNotification => write!(f, "EAP-AKA notification missing code"),
            Self::MissingIdentityRequest => write!(f, "EAP-AKA identity request missing selector"),
            Self::MissingKeyMaterial => write!(f, "EAP-AKA key material unavailable"),
            Self::InvalidAkaResult => write!(f, "invalid AKA result for EAP-AKA"),
            Self::PacketTooLarge => write!(f, "EAP-AKA packet too large"),
        }
    }
}

pub fn parse_identity_request(packet: &[u8]) -> Result<EapAkaIdentityRequest, EapAkaProtocolError> {
    if packet.len() < EAP_AKA_HEADER_LEN {
        return Err(EapAkaProtocolError::Truncated);
    }
    let declared = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    if declared != packet.len() {
        return Err(EapAkaProtocolError::LengthMismatch);
    }
    if packet[4] != EAP_TYPE_AKA {
        return Err(EapAkaProtocolError::UnsupportedMethod);
    }
    if packet[5] != EAP_AKA_SUBTYPE_IDENTITY {
        return Err(EapAkaProtocolError::UnsupportedSubtype);
    }

    for attr in parse_attributes(&packet[EAP_AKA_HEADER_LEN..])? {
        match attr.attribute_type {
            AT_PERMANENT_ID_REQ => {
                return Ok(EapAkaIdentityRequest {
                    identifier: packet[1],
                    request_kind: "permanent",
                });
            }
            AT_FULLAUTH_ID_REQ => {
                return Ok(EapAkaIdentityRequest {
                    identifier: packet[1],
                    request_kind: "full_auth",
                });
            }
            AT_ANY_ID_REQ => {
                return Ok(EapAkaIdentityRequest {
                    identifier: packet[1],
                    request_kind: "any",
                });
            }
            _ => {}
        }
    }

    Err(EapAkaProtocolError::MissingIdentityRequest)
}

pub fn parse_notification(packet: &[u8]) -> Result<EapAkaNotification, EapAkaProtocolError> {
    if packet.len() < EAP_AKA_HEADER_LEN {
        return Err(EapAkaProtocolError::Truncated);
    }
    let declared = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    if declared != packet.len() {
        return Err(EapAkaProtocolError::LengthMismatch);
    }
    if packet[4] != EAP_TYPE_AKA {
        return Err(EapAkaProtocolError::UnsupportedMethod);
    }
    if packet[5] != EAP_AKA_SUBTYPE_NOTIFICATION {
        return Err(EapAkaProtocolError::UnsupportedSubtype);
    }

    for attr in parse_attributes(&packet[EAP_AKA_HEADER_LEN..])? {
        if attr.attribute_type == AT_NOTIFICATION {
            if attr.value.len() != 2 {
                return Err(EapAkaProtocolError::InvalidAttributeLength);
            }
            let code = u16::from_be_bytes([attr.value[0], attr.value[1]]);
            let after_challenge = code & 0x4000 == 0;
            return Ok(EapAkaNotification {
                identifier: packet[1],
                code,
                success: code & 0x8000 != 0,
                after_challenge,
                mac_required: after_challenge,
            });
        }
    }

    Err(EapAkaProtocolError::MissingNotification)
}

impl std::error::Error for EapAkaProtocolError {}

pub fn parse_challenge(packet: &[u8]) -> Result<EapAkaChallenge, EapAkaProtocolError> {
    if packet.len() < EAP_AKA_HEADER_LEN {
        return Err(EapAkaProtocolError::Truncated);
    }
    let declared = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    if declared != packet.len() {
        return Err(EapAkaProtocolError::LengthMismatch);
    }
    if packet[4] != EAP_TYPE_AKA {
        return Err(EapAkaProtocolError::UnsupportedMethod);
    }
    if packet[5] != EAP_AKA_SUBTYPE_CHALLENGE {
        return Err(EapAkaProtocolError::UnsupportedSubtype);
    }

    let mut rand = None;
    let mut autn = None;
    let mut mac_present = false;
    let mut result_indication = false;
    for attr in parse_attributes(&packet[EAP_AKA_HEADER_LEN..])? {
        match attr.attribute_type {
            AT_RAND => rand = Some(extract_reserved_prefixed_value(attr.value, 16)?),
            AT_AUTN => autn = Some(extract_reserved_prefixed_value(attr.value, 16)?),
            AT_MAC => {
                if attr.value.len() != 2 + MAC_LEN {
                    return Err(EapAkaProtocolError::InvalidAttributeLength);
                }
                mac_present = true;
            }
            AT_RESULT_IND => result_indication = true,
            _ => {}
        }
    }

    Ok(EapAkaChallenge {
        identifier: packet[1],
        rand: rand.ok_or(EapAkaProtocolError::MissingRand)?,
        autn: autn.ok_or(EapAkaProtocolError::MissingAutn)?,
        mac_present,
        result_indication,
    })
}

pub fn build_challenge_response(
    challenge: &EapAkaChallenge,
    identity: &str,
    aka: &UsimAkaApduResult,
) -> Result<EapAkaResponsePacket, EapAkaProtocolError> {
    if !challenge.mac_present {
        return Err(EapAkaProtocolError::MissingMac);
    }
    if aka.res.is_empty() || aka.ck.len() != 16 || aka.ik.len() != 16 || aka.auts.is_some() {
        return Err(EapAkaProtocolError::InvalidAkaResult);
    }
    let keys = derive_key_material(identity.as_bytes(), &aka.ik, &aka.ck)?;
    let mut attributes = Vec::new();
    push_at_res(&mut attributes, &aka.res)?;
    if challenge.result_indication {
        push_attribute(&mut attributes, AT_RESULT_IND, &[0, 0])?;
    }
    let mac_offset = attributes.len() + EAP_AKA_HEADER_LEN + 4;
    push_attribute(&mut attributes, AT_MAC, &[0; 18])?;
    let mut packet = build_eap_packet(
        EAP_CODE_RESPONSE,
        challenge.identifier,
        EAP_AKA_SUBTYPE_CHALLENGE,
        &attributes,
    )?;
    let mac = calculate_mac(&keys.k_aut, &packet, &[])?;
    packet[mac_offset..mac_offset + MAC_LEN].copy_from_slice(&mac);
    Ok(EapAkaResponsePacket {
        packet,
        key_material: Some(keys),
    })
}

pub fn build_sync_failure_response(
    challenge: &EapAkaChallenge,
    auts: &[u8],
) -> Result<EapAkaResponsePacket, EapAkaProtocolError> {
    let mut attributes = Vec::new();
    push_attribute(&mut attributes, AT_AUTS, auts)?;
    Ok(EapAkaResponsePacket {
        packet: build_eap_packet(
            EAP_CODE_RESPONSE,
            challenge.identifier,
            EAP_AKA_SUBTYPE_SYNC_FAILURE,
            &attributes,
        )?,
        key_material: None,
    })
}

fn build_notification_response(
    notification: &EapAkaNotification,
    keys: &EapAkaKeyMaterial,
) -> Result<EapAkaResponsePacket, EapAkaProtocolError> {
    let mut attributes = Vec::new();
    let mut mac_offset = None;
    if notification.mac_required {
        mac_offset = Some(attributes.len() + EAP_AKA_HEADER_LEN + 4);
        push_attribute(&mut attributes, AT_MAC, &[0; 18])?;
    }
    let mut packet = build_eap_packet(
        EAP_CODE_RESPONSE,
        notification.identifier,
        EAP_AKA_SUBTYPE_NOTIFICATION,
        &attributes,
    )?;
    if let Some(offset) = mac_offset {
        let mac = calculate_mac(&keys.k_aut, &packet, &[])?;
        packet[offset..offset + MAC_LEN].copy_from_slice(&mac);
    }
    Ok(EapAkaResponsePacket {
        packet,
        key_material: Some(keys.clone()),
    })
}

fn build_identity_response(
    request: &EapAkaIdentityRequest,
    identity: &str,
    key_material: Option<EapAkaKeyMaterial>,
) -> Result<EapAkaResponsePacket, EapAkaProtocolError> {
    let mut value = Vec::with_capacity(2 + identity.len());
    if identity.len() > u16::MAX as usize {
        return Err(EapAkaProtocolError::PacketTooLarge);
    }
    value.extend_from_slice(&(identity.len() as u16).to_be_bytes());
    value.extend_from_slice(identity.as_bytes());
    let mut attributes = Vec::new();
    push_attribute(&mut attributes, AT_IDENTITY, &value)?;
    Ok(EapAkaResponsePacket {
        packet: build_eap_packet(
            EAP_CODE_RESPONSE,
            request.identifier,
            EAP_AKA_SUBTYPE_IDENTITY,
            &attributes,
        )?,
        key_material,
    })
}

fn derive_key_material(
    identity: &[u8],
    ik: &[u8],
    ck: &[u8],
) -> Result<EapAkaKeyMaterial, EapAkaProtocolError> {
    let mut mk_input = Vec::with_capacity(identity.len() + ik.len() + ck.len());
    mk_input.extend_from_slice(identity);
    mk_input.extend_from_slice(ik);
    mk_input.extend_from_slice(ck);
    let mk = digest::digest(&digest::SHA1_FOR_LEGACY_USE_ONLY, &mk_input);
    let key_stream = fips186_2_prf(mk.as_ref(), KEY_STREAM_LEN);
    let mut k_aut = [0u8; K_AUT_LEN];
    k_aut.copy_from_slice(&key_stream[K_ENCR_LEN..K_ENCR_LEN + K_AUT_LEN]);
    Ok(EapAkaKeyMaterial {
        k_aut,
        msk: key_stream[K_ENCR_LEN + K_AUT_LEN..K_ENCR_LEN + K_AUT_LEN + 64].to_vec(),
    })
}

fn fips186_2_prf(seed: &[u8], out_len: usize) -> Vec<u8> {
    let mut xkey = [0u8; 64];
    let seed_len = seed.len().min(xkey.len());
    xkey[..seed_len].copy_from_slice(&seed[..seed_len]);
    let mut output = Vec::with_capacity(out_len);
    let t = [
        0x6745_2301,
        0xefcd_ab89,
        0x98ba_dcfe,
        0x1032_5476,
        0xc3d2_e1f0,
    ];

    while output.len() < out_len {
        for _ in 0..2 {
            let words = sha1_compress(t, &xkey);
            let mut w = [0u8; 20];
            for (index, word) in words.iter().enumerate() {
                w[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
            }
            let remaining = out_len - output.len();
            output.extend_from_slice(&w[..remaining.min(w.len())]);
            add_one_and_w(&mut xkey[..20], &w);
            if output.len() >= out_len {
                break;
            }
        }
    }
    output
}

fn sha1_compress(mut state: [u32; 5], block: &[u8; 64]) -> [u32; 5] {
    let mut w = [0u32; 80];
    for (i, chunk) in block.chunks_exact(4).enumerate().take(16) {
        w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    for i in 16..80 {
        w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
    }

    let mut a = state[0];
    let mut b = state[1];
    let mut c = state[2];
    let mut d = state[3];
    let mut e = state[4];
    for (i, word) in w.iter().enumerate() {
        let (f, k) = match i {
            0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999),
            20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
            _ => (b ^ c ^ d, 0xca62_c1d6),
        };
        let temp = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(*word);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = temp;
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state
}

fn add_one_and_w(xkey: &mut [u8], w: &[u8; 20]) {
    let mut carry = 1u16;
    for index in (0..20).rev() {
        carry += u16::from(xkey[index]) + u16::from(w[index]);
        xkey[index] = (carry & 0xff) as u8;
        carry >>= 8;
    }
}

fn calculate_mac(
    k_aut: &[u8; K_AUT_LEN],
    packet: &[u8],
    extra: &[u8],
) -> Result<[u8; MAC_LEN], EapAkaProtocolError> {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, k_aut);
    let mut ctx = hmac::Context::with_key(&key);
    ctx.update(packet);
    ctx.update(extra);
    let tag = ctx.sign();
    let mut out = [0u8; MAC_LEN];
    out.copy_from_slice(&tag.as_ref()[..MAC_LEN]);
    Ok(out)
}

fn build_eap_packet(
    code: u8,
    identifier: u8,
    subtype: u8,
    attributes: &[u8],
) -> Result<Vec<u8>, EapAkaProtocolError> {
    let len = EAP_AKA_HEADER_LEN
        .checked_add(attributes.len())
        .ok_or(EapAkaProtocolError::PacketTooLarge)?;
    if len > u16::MAX as usize {
        return Err(EapAkaProtocolError::PacketTooLarge);
    }
    let mut out = Vec::with_capacity(len);
    out.push(code);
    out.push(identifier);
    out.extend_from_slice(&(len as u16).to_be_bytes());
    out.push(EAP_TYPE_AKA);
    out.push(subtype);
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(attributes);
    Ok(out)
}

fn push_at_res(out: &mut Vec<u8>, res: &[u8]) -> Result<(), EapAkaProtocolError> {
    if res.len() > 128 {
        return Err(EapAkaProtocolError::PacketTooLarge);
    }
    let mut value = Vec::with_capacity(2 + res.len());
    value.extend_from_slice(&((res.len() as u16) * 8).to_be_bytes());
    value.extend_from_slice(res);
    push_attribute(out, AT_RES, &value)
}

fn push_attribute(
    out: &mut Vec<u8>,
    attribute_type: u8,
    value: &[u8],
) -> Result<(), EapAkaProtocolError> {
    let padded_len = 2usize
        .checked_add(value.len())
        .ok_or(EapAkaProtocolError::PacketTooLarge)?;
    let pad = (4 - padded_len % 4) % 4;
    let total = padded_len + pad;
    if total / 4 > u8::MAX as usize {
        return Err(EapAkaProtocolError::PacketTooLarge);
    }
    out.push(attribute_type);
    out.push((total / 4) as u8);
    out.extend_from_slice(value);
    out.extend(std::iter::repeat(0).take(pad));
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct EapAkaAttribute<'a> {
    attribute_type: u8,
    value: &'a [u8],
}

fn parse_attributes(mut input: &[u8]) -> Result<Vec<EapAkaAttribute<'_>>, EapAkaProtocolError> {
    let mut out = Vec::new();
    while !input.is_empty() {
        if input.len() < 4 {
            return Err(EapAkaProtocolError::AttributeTruncated);
        }
        let units = input[1];
        if units == 0 {
            return Err(EapAkaProtocolError::InvalidAttributeLength);
        }
        let len = usize::from(units) * 4;
        if len > input.len() {
            return Err(EapAkaProtocolError::AttributeTruncated);
        }
        out.push(EapAkaAttribute {
            attribute_type: input[0],
            value: &input[2..len],
        });
        input = &input[len..];
    }
    Ok(out)
}

fn extract_reserved_prefixed_value(
    value: &[u8],
    expected_len: usize,
) -> Result<Vec<u8>, EapAkaProtocolError> {
    if value.len() != 2 + expected_len {
        return Err(EapAkaProtocolError::InvalidAttributeLength);
    }
    Ok(value[2..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_challenge() -> Vec<u8> {
        let mut attrs = Vec::new();
        let mut rand = vec![0, 0];
        rand.extend_from_slice(&[0x11; 16]);
        push_attribute(&mut attrs, AT_RAND, &rand).expect("rand");
        let mut autn = vec![0, 0];
        autn.extend_from_slice(&[0x22; 16]);
        push_attribute(&mut attrs, AT_AUTN, &autn).expect("autn");
        push_attribute(&mut attrs, AT_MAC, &[0; 18]).expect("mac");
        build_eap_packet(1, 7, EAP_AKA_SUBTYPE_CHALLENGE, &attrs).expect("packet")
    }

    #[test]
    fn parses_challenge_inputs_without_exposing_values() {
        let challenge = parse_challenge(&sample_challenge()).expect("challenge");

        assert_eq!(challenge.identifier, 7);
        assert_eq!(challenge.rand.len(), 16);
        assert_eq!(challenge.autn.len(), 16);
        assert!(challenge.mac_present);
    }

    #[test]
    fn builds_signed_challenge_response_with_at_res_and_at_mac() {
        let challenge = parse_challenge(&sample_challenge()).expect("challenge");
        let aka = UsimAkaApduResult {
            res: vec![0x33; 8],
            ck: vec![0x44; 16],
            ik: vec![0x55; 16],
            auts: None,
        };

        let response = build_challenge_response(
            &challenge,
            "0234331234567890@nai.epc.mnc033.mcc234.3gppnetwork.org",
            &aka,
        )
        .expect("response");
        let packet = response.expose_for_ike_encryption();

        assert_eq!(packet[0], EAP_CODE_RESPONSE);
        assert_eq!(packet[1], 7);
        assert_eq!(packet[4], EAP_TYPE_AKA);
        assert_eq!(packet[5], EAP_AKA_SUBTYPE_CHALLENGE);
        assert!(packet[EAP_AKA_HEADER_LEN..].contains(&AT_RES));
        assert!(packet[EAP_AKA_HEADER_LEN..].contains(&AT_MAC));
    }

    #[test]
    fn builds_sync_failure_without_mac() {
        let challenge = parse_challenge(&sample_challenge()).expect("challenge");
        let response = build_sync_failure_response(&challenge, &[0x66; 14]).expect("sync");

        let packet = response.expose_for_ike_encryption();
        assert_eq!(packet[5], EAP_AKA_SUBTYPE_SYNC_FAILURE);
        assert!(packet[EAP_AKA_HEADER_LEN..].contains(&AT_AUTS));
        assert!(!packet[EAP_AKA_HEADER_LEN..].contains(&AT_MAC));
    }

    #[test]
    fn builds_identity_response_without_exposing_identity_in_debug() {
        let mut attrs = Vec::new();
        push_attribute(&mut attrs, AT_PERMANENT_ID_REQ, &[0, 0]).expect("id req");
        let request =
            build_eap_packet(1, 9, EAP_AKA_SUBTYPE_IDENTITY, &attrs).expect("identity request");
        let challenge = parse_challenge(&sample_challenge()).expect("challenge");
        let aka = UsimAkaApduResult {
            res: vec![0x33; 8],
            ck: vec![0x44; 16],
            ik: vec![0x55; 16],
            auts: None,
        };
        let response = build_challenge_response(
            &challenge,
            "0234331234567890@nai.epc.mnc033.mcc234.3gppnetwork.org",
            &aka,
        )
        .expect("challenge response");
        let identity_response = response
            .identity_response(
                &request,
                "0234331234567890@nai.epc.mnc033.mcc234.3gppnetwork.org",
            )
            .expect("identity response");

        let packet = identity_response.expose_for_ike_encryption();
        assert_eq!(packet[0], EAP_CODE_RESPONSE);
        assert_eq!(packet[1], 9);
        assert_eq!(packet[5], EAP_AKA_SUBTYPE_IDENTITY);
        assert!(packet[EAP_AKA_HEADER_LEN..].contains(&AT_IDENTITY));
        assert!(identity_response.msk_for_ike_auth().is_some());

        let debug = format!("{identity_response:?}");
        assert!(!debug.contains("023433"));
        assert!(debug.contains("packet_len"));
    }
}
