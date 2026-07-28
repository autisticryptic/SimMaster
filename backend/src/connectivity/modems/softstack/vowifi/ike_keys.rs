#![allow(dead_code)]

use std::fmt;

use ring::hmac;
use serde::Serialize;

use super::ike_payloads::{
    ProposalSpec, TransformAttribute, TransformSpec, TransformType, AUTH_HMAC_SHA1_96,
    AUTH_HMAC_SHA2_256_128, AUTH_HMAC_SHA2_512_256, ENCR_AES_CBC, PRF_HMAC_SHA1, PRF_HMAC_SHA2_256,
    PRF_HMAC_SHA2_512,
};

#[derive(Clone, PartialEq, Eq)]
pub struct SecretBytes {
    bytes: Vec<u8>,
}

impl SecretBytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn expose_for_test(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn expose_for_protocol(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        for byte in &mut self.bytes {
            *byte = 0;
        }
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretBytes")
            .field("len", &self.bytes.len())
            .field("value", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IkePrfAlgorithm {
    HmacSha1,
    HmacSha256,
    HmacSha512,
}

impl IkePrfAlgorithm {
    pub fn from_transform_id(transform_id: u16) -> Option<Self> {
        match transform_id {
            PRF_HMAC_SHA1 => Some(Self::HmacSha1),
            PRF_HMAC_SHA2_256 => Some(Self::HmacSha256),
            PRF_HMAC_SHA2_512 => Some(Self::HmacSha512),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::HmacSha1 => "hmac_sha1",
            Self::HmacSha256 => "hmac_sha256",
            Self::HmacSha512 => "hmac_sha512",
        }
    }

    pub fn output_len(self) -> usize {
        match self {
            Self::HmacSha1 => 20,
            Self::HmacSha256 => 32,
            Self::HmacSha512 => 64,
        }
    }

    fn ring_algorithm(self) -> hmac::Algorithm {
        match self {
            Self::HmacSha1 => hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY,
            Self::HmacSha256 => hmac::HMAC_SHA256,
            Self::HmacSha512 => hmac::HMAC_SHA512,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IkeEncryptionAlgorithm {
    AesCbc,
}

impl IkeEncryptionAlgorithm {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AesCbc => "aes_cbc",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IkeIntegrityAlgorithm {
    HmacSha1_96,
    HmacSha256_128,
    HmacSha512_256,
}

impl IkeIntegrityAlgorithm {
    pub fn from_transform_id(transform_id: u16) -> Option<Self> {
        match transform_id {
            AUTH_HMAC_SHA1_96 => Some(Self::HmacSha1_96),
            AUTH_HMAC_SHA2_256_128 => Some(Self::HmacSha256_128),
            AUTH_HMAC_SHA2_512_256 => Some(Self::HmacSha512_256),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::HmacSha1_96 => "hmac_sha1_96",
            Self::HmacSha256_128 => "hmac_sha256_128",
            Self::HmacSha512_256 => "hmac_sha512_256",
        }
    }

    pub fn key_len(self) -> usize {
        match self {
            Self::HmacSha1_96 => 20,
            Self::HmacSha256_128 => 32,
            Self::HmacSha512_256 => 64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IkeKeySchedulePlan {
    pub prf: &'static str,
    pub encryption: &'static str,
    pub integrity: &'static str,
    pub prf_output_bytes: usize,
    pub encryption_key_bytes: usize,
    pub integrity_key_bytes: usize,
    pub total_secret_bytes: usize,
    pub exported_secret_values: bool,
    pub sensitive_values_policy: &'static str,
}

#[derive(Clone, PartialEq, Eq)]
pub struct IkeSecretBundle {
    pub skeyseed: SecretBytes,
    pub sk_d: SecretBytes,
    pub sk_ai: SecretBytes,
    pub sk_ar: SecretBytes,
    pub sk_ei: SecretBytes,
    pub sk_er: SecretBytes,
    pub sk_pi: SecretBytes,
    pub sk_pr: SecretBytes,
    plan: IkeKeySchedulePlan,
}

impl IkeSecretBundle {
    pub fn summary(&self) -> IkeKeySchedulePlan {
        self.plan.clone()
    }

    pub(crate) fn prf_bytes(&self, key: &[u8], data: &[u8]) -> Result<Vec<u8>, IkeKeyError> {
        let prf = prf_from_plan(&self.plan)?;
        Ok(prf_once(prf, key, data).expose_for_protocol().to_vec())
    }
}

impl fmt::Debug for IkeSecretBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IkeSecretBundle")
            .field("summary", &self.plan)
            .field("value", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IkeKeyError {
    MissingPrfTransform,
    MissingEncryptionTransform,
    MissingIntegrityTransform,
    MissingEncryptionKeyLength,
    UnsupportedPrf(u16),
    UnsupportedPrfName(String),
    UnsupportedEncryption(u16),
    UnsupportedIntegrity(u16),
    EmptySharedSecret,
    EmptyNonce,
    OutputTooLarge(usize),
}

impl fmt::Display for IkeKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPrfTransform => write!(f, "missing IKE PRF transform"),
            Self::MissingEncryptionTransform => write!(f, "missing IKE encryption transform"),
            Self::MissingIntegrityTransform => write!(f, "missing IKE integrity transform"),
            Self::MissingEncryptionKeyLength => write!(f, "missing IKE encryption key length"),
            Self::UnsupportedPrf(id) => write!(f, "unsupported IKE PRF transform {id}"),
            Self::UnsupportedPrfName(name) => write!(f, "unsupported IKE PRF algorithm {name}"),
            Self::UnsupportedEncryption(id) => {
                write!(f, "unsupported IKE encryption transform {id}")
            }
            Self::UnsupportedIntegrity(id) => write!(f, "unsupported IKE integrity transform {id}"),
            Self::EmptySharedSecret => write!(f, "empty IKE shared secret"),
            Self::EmptyNonce => write!(f, "empty IKE nonce"),
            Self::OutputTooLarge(size) => write!(f, "IKE key schedule output too large: {size}"),
        }
    }
}

impl std::error::Error for IkeKeyError {}

pub fn build_key_schedule_plan(proposal: &ProposalSpec) -> Result<IkeKeySchedulePlan, IkeKeyError> {
    let prf_transform =
        find_transform(proposal, TransformType::Prf).ok_or(IkeKeyError::MissingPrfTransform)?;
    let encryption_transform = find_transform(proposal, TransformType::Encryption)
        .ok_or(IkeKeyError::MissingEncryptionTransform)?;
    let integrity_transform = find_transform(proposal, TransformType::Integrity)
        .ok_or(IkeKeyError::MissingIntegrityTransform)?;

    let prf = IkePrfAlgorithm::from_transform_id(prf_transform.transform_id)
        .ok_or(IkeKeyError::UnsupportedPrf(prf_transform.transform_id))?;
    let encryption = match encryption_transform.transform_id {
        ENCR_AES_CBC => IkeEncryptionAlgorithm::AesCbc,
        other => return Err(IkeKeyError::UnsupportedEncryption(other)),
    };
    let encryption_key_bytes = encryption_key_bits(encryption_transform)
        .ok_or(IkeKeyError::MissingEncryptionKeyLength)?
        / 8;
    let integrity = IkeIntegrityAlgorithm::from_transform_id(integrity_transform.transform_id)
        .ok_or(IkeKeyError::UnsupportedIntegrity(
            integrity_transform.transform_id,
        ))?;
    let prf_output_bytes = prf.output_len();
    let integrity_key_bytes = integrity.key_len();
    let total_secret_bytes =
        (prf_output_bytes * 3) + (integrity_key_bytes * 2) + (encryption_key_bytes * 2);

    Ok(IkeKeySchedulePlan {
        prf: prf.as_str(),
        encryption: encryption.as_str(),
        integrity: integrity.as_str(),
        prf_output_bytes,
        encryption_key_bytes,
        integrity_key_bytes,
        total_secret_bytes,
        exported_secret_values: false,
        sensitive_values_policy: "secret_bytes_redacted_and_zeroed_on_drop",
    })
}

pub fn derive_ike_secret_bundle(
    proposal: &ProposalSpec,
    initiator_nonce: &[u8],
    responder_nonce: &[u8],
    initiator_spi: u64,
    responder_spi: u64,
    shared_secret: &[u8],
) -> Result<IkeSecretBundle, IkeKeyError> {
    if shared_secret.is_empty() {
        return Err(IkeKeyError::EmptySharedSecret);
    }
    if initiator_nonce.is_empty() || responder_nonce.is_empty() {
        return Err(IkeKeyError::EmptyNonce);
    }

    let plan = build_key_schedule_plan(proposal)?;
    let prf_transform =
        find_transform(proposal, TransformType::Prf).ok_or(IkeKeyError::MissingPrfTransform)?;
    let prf = IkePrfAlgorithm::from_transform_id(prf_transform.transform_id)
        .ok_or(IkeKeyError::UnsupportedPrf(prf_transform.transform_id))?;

    let mut nonce_pair = Vec::with_capacity(initiator_nonce.len() + responder_nonce.len());
    nonce_pair.extend_from_slice(initiator_nonce);
    nonce_pair.extend_from_slice(responder_nonce);
    let skeyseed = prf_once(prf, &nonce_pair, shared_secret);

    let mut seed = Vec::with_capacity(nonce_pair.len() + 16);
    seed.extend_from_slice(initiator_nonce);
    seed.extend_from_slice(responder_nonce);
    seed.extend_from_slice(&initiator_spi.to_be_bytes());
    seed.extend_from_slice(&responder_spi.to_be_bytes());
    let expanded = prf_plus(
        prf,
        skeyseed.expose_for_test(),
        &seed,
        plan.total_secret_bytes,
    )?;

    let mut offset = 0usize;
    let sk_d = take_secret(&expanded, &mut offset, plan.prf_output_bytes);
    let sk_ai = take_secret(&expanded, &mut offset, plan.integrity_key_bytes);
    let sk_ar = take_secret(&expanded, &mut offset, plan.integrity_key_bytes);
    let sk_ei = take_secret(&expanded, &mut offset, plan.encryption_key_bytes);
    let sk_er = take_secret(&expanded, &mut offset, plan.encryption_key_bytes);
    let sk_pi = take_secret(&expanded, &mut offset, plan.prf_output_bytes);
    let sk_pr = take_secret(&expanded, &mut offset, plan.prf_output_bytes);

    Ok(IkeSecretBundle {
        skeyseed,
        sk_d,
        sk_ai,
        sk_ar,
        sk_ei,
        sk_er,
        sk_pi,
        sk_pr,
        plan,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChildSaKeySchedulePlan {
    pub encryption: &'static str,
    pub integrity: &'static str,
    pub encryption_key_bytes: usize,
    pub integrity_key_bytes: usize,
    pub direction_secret_bytes: usize,
    pub total_secret_bytes: usize,
    pub exported_secret_values: bool,
    pub sensitive_values_policy: &'static str,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ChildSaSecretPair {
    pub outbound_encryption: SecretBytes,
    pub outbound_integrity: SecretBytes,
    pub inbound_encryption: SecretBytes,
    pub inbound_integrity: SecretBytes,
    plan: ChildSaKeySchedulePlan,
}

impl ChildSaSecretPair {
    pub fn summary(&self) -> ChildSaKeySchedulePlan {
        self.plan.clone()
    }

    pub(crate) fn from_protocol_parts(
        plan: ChildSaKeySchedulePlan,
        outbound_encryption: Vec<u8>,
        outbound_integrity: Vec<u8>,
        inbound_encryption: Vec<u8>,
        inbound_integrity: Vec<u8>,
    ) -> Self {
        Self {
            outbound_encryption: SecretBytes::new(outbound_encryption),
            outbound_integrity: SecretBytes::new(outbound_integrity),
            inbound_encryption: SecretBytes::new(inbound_encryption),
            inbound_integrity: SecretBytes::new(inbound_integrity),
            plan,
        }
    }

    #[cfg(test)]
    pub fn from_test_parts(
        plan: ChildSaKeySchedulePlan,
        outbound_encryption: Vec<u8>,
        outbound_integrity: Vec<u8>,
        inbound_encryption: Vec<u8>,
        inbound_integrity: Vec<u8>,
    ) -> Self {
        Self {
            outbound_encryption: SecretBytes::new(outbound_encryption),
            outbound_integrity: SecretBytes::new(outbound_integrity),
            inbound_encryption: SecretBytes::new(inbound_encryption),
            inbound_integrity: SecretBytes::new(inbound_integrity),
            plan,
        }
    }
}

impl fmt::Debug for ChildSaSecretPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChildSaSecretPair")
            .field("summary", &self.plan)
            .field("value", &"<redacted>")
            .finish()
    }
}

pub fn build_child_sa_key_schedule_plan(
    esp_proposal: &ProposalSpec,
) -> Result<ChildSaKeySchedulePlan, IkeKeyError> {
    let encryption_transform = find_transform(esp_proposal, TransformType::Encryption)
        .ok_or(IkeKeyError::MissingEncryptionTransform)?;
    let integrity_transform = find_transform(esp_proposal, TransformType::Integrity)
        .ok_or(IkeKeyError::MissingIntegrityTransform)?;
    let encryption = match encryption_transform.transform_id {
        ENCR_AES_CBC => IkeEncryptionAlgorithm::AesCbc,
        other => return Err(IkeKeyError::UnsupportedEncryption(other)),
    };
    let encryption_key_bytes = encryption_key_bits(encryption_transform)
        .ok_or(IkeKeyError::MissingEncryptionKeyLength)?
        / 8;
    let integrity = IkeIntegrityAlgorithm::from_transform_id(integrity_transform.transform_id)
        .ok_or(IkeKeyError::UnsupportedIntegrity(
            integrity_transform.transform_id,
        ))?;
    let integrity_key_bytes = integrity.key_len();
    let direction_secret_bytes = encryption_key_bytes + integrity_key_bytes;
    let total_secret_bytes = direction_secret_bytes * 2;

    Ok(ChildSaKeySchedulePlan {
        encryption: encryption.as_str(),
        integrity: integrity.as_str(),
        encryption_key_bytes,
        integrity_key_bytes,
        direction_secret_bytes,
        total_secret_bytes,
        exported_secret_values: false,
        sensitive_values_policy: "child_sa_secret_bytes_redacted_and_zeroed_on_drop",
    })
}

pub fn derive_child_sa_secret_pair(
    ike_bundle: &IkeSecretBundle,
    esp_proposal: &ProposalSpec,
    initiator_nonce: &[u8],
    responder_nonce: &[u8],
) -> Result<ChildSaSecretPair, IkeKeyError> {
    if initiator_nonce.is_empty() || responder_nonce.is_empty() {
        return Err(IkeKeyError::EmptyNonce);
    }

    let plan = build_child_sa_key_schedule_plan(esp_proposal)?;
    let prf = prf_from_plan(&ike_bundle.plan)?;
    let mut seed = Vec::with_capacity(initiator_nonce.len() + responder_nonce.len());
    seed.extend_from_slice(initiator_nonce);
    seed.extend_from_slice(responder_nonce);
    let expanded = prf_plus(
        prf,
        ike_bundle.sk_d.expose_for_protocol(),
        &seed,
        plan.total_secret_bytes,
    )?;
    let mut offset = 0usize;
    let outbound_encryption = take_secret(&expanded, &mut offset, plan.encryption_key_bytes);
    let outbound_integrity = take_secret(&expanded, &mut offset, plan.integrity_key_bytes);
    let inbound_encryption = take_secret(&expanded, &mut offset, plan.encryption_key_bytes);
    let inbound_integrity = take_secret(&expanded, &mut offset, plan.integrity_key_bytes);

    Ok(ChildSaSecretPair {
        outbound_encryption,
        outbound_integrity,
        inbound_encryption,
        inbound_integrity,
        plan,
    })
}

fn find_transform(
    proposal: &ProposalSpec,
    transform_type: TransformType,
) -> Option<&TransformSpec> {
    proposal
        .transforms
        .iter()
        .find(|transform| transform.transform_type == transform_type)
}

fn encryption_key_bits(transform: &TransformSpec) -> Option<usize> {
    transform
        .attributes
        .first()
        .map(|attribute| match attribute {
            TransformAttribute::KeyLength(bits) => usize::from(*bits),
        })
}

fn prf_once(prf: IkePrfAlgorithm, key: &[u8], data: &[u8]) -> SecretBytes {
    let key = hmac::Key::new(prf.ring_algorithm(), key);
    SecretBytes::new(hmac::sign(&key, data).as_ref().to_vec())
}

fn prf_from_plan(plan: &IkeKeySchedulePlan) -> Result<IkePrfAlgorithm, IkeKeyError> {
    match plan.prf {
        "hmac_sha1" => Ok(IkePrfAlgorithm::HmacSha1),
        "hmac_sha256" => Ok(IkePrfAlgorithm::HmacSha256),
        "hmac_sha512" => Ok(IkePrfAlgorithm::HmacSha512),
        other => Err(IkeKeyError::UnsupportedPrfName(other.to_string())),
    }
}

fn prf_plus(
    prf: IkePrfAlgorithm,
    key: &[u8],
    seed: &[u8],
    output_len: usize,
) -> Result<Vec<u8>, IkeKeyError> {
    if output_len > usize::from(u8::MAX) * prf.output_len() {
        return Err(IkeKeyError::OutputTooLarge(output_len));
    }

    let key = hmac::Key::new(prf.ring_algorithm(), key);
    let mut out = Vec::with_capacity(output_len);
    let mut previous = Vec::new();
    let mut counter = 1u8;
    while out.len() < output_len {
        let mut input = Vec::with_capacity(previous.len() + seed.len() + 1);
        input.extend_from_slice(&previous);
        input.extend_from_slice(seed);
        input.push(counter);
        previous = hmac::sign(&key, &input).as_ref().to_vec();
        out.extend_from_slice(&previous);
        counter = counter.saturating_add(1);
    }
    out.truncate(output_len);
    Ok(out)
}

fn take_secret(input: &[u8], offset: &mut usize, len: usize) -> SecretBytes {
    let end = *offset + len;
    let secret = SecretBytes::new(input[*offset..end].to_vec());
    *offset = end;
    secret
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::modems::softstack::vowifi::ike_payloads::{
        child_sa_proposal_from_profile_string, ike_proposal_from_profile_string,
    };

    #[test]
    fn builds_key_schedule_plan_from_clean_room_profile_proposal() {
        let proposal =
            ike_proposal_from_profile_string("aes128-sha256-modp2048", 1).expect("proposal");
        let plan = build_key_schedule_plan(&proposal).expect("plan");

        assert_eq!(plan.prf, "hmac_sha256");
        assert_eq!(plan.encryption, "aes_cbc");
        assert_eq!(plan.integrity, "hmac_sha256_128");
        assert_eq!(plan.prf_output_bytes, 32);
        assert_eq!(plan.encryption_key_bytes, 16);
        assert_eq!(plan.integrity_key_bytes, 32);
        assert_eq!(plan.total_secret_bytes, 192);
        assert!(!plan.exported_secret_values);
    }

    #[test]
    fn derives_deterministic_ike_secret_bundle_without_serializing_values() {
        let proposal = ike_proposal_from_profile_string("aes256-sha256-prfsha512-modp2048", 1)
            .expect("proposal");
        let bundle = derive_ike_secret_bundle(
            &proposal,
            &[0x11; 32],
            &[0x22; 32],
            0x0102_0304_0506_0708,
            0x1112_1314_1516_1718,
            &[0x33; 256],
        )
        .expect("derive keys");
        let again = derive_ike_secret_bundle(
            &proposal,
            &[0x11; 32],
            &[0x22; 32],
            0x0102_0304_0506_0708,
            0x1112_1314_1516_1718,
            &[0x33; 256],
        )
        .expect("derive keys again");

        assert_eq!(bundle.summary().prf, "hmac_sha512");
        assert_eq!(bundle.sk_d.len(), 64);
        assert_eq!(bundle.sk_ai.len(), 32);
        assert_eq!(bundle.sk_ei.len(), 32);
        assert_eq!(bundle.sk_pr.len(), 64);
        assert_eq!(bundle.sk_d.expose_for_test(), again.sk_d.expose_for_test());

        let debug = format!("{bundle:?}").to_ascii_lowercase();
        assert!(debug.contains("<redacted>"));
        for forbidden in ["33333333", "11111111", "22222222"] {
            assert!(!debug.contains(forbidden));
        }
        let json = serde_json::to_string(&bundle.summary()).expect("serialize summary");
        for forbidden_key in [
            "skeyseed", "sk_d", "sk_ai", "sk_ar", "sk_ei", "sk_er", "sk_pi", "sk_pr",
        ] {
            assert!(!json
                .to_ascii_lowercase()
                .contains(&format!("\"{forbidden_key}\"")));
        }
    }

    #[test]
    fn changes_derived_values_when_nonce_changes() {
        let proposal =
            ike_proposal_from_profile_string("aes128-sha1-modp2048", 1).expect("proposal");
        let first =
            derive_ike_secret_bundle(&proposal, &[0x11; 32], &[0x22; 32], 1, 2, &[0x44; 256])
                .expect("first");
        let second =
            derive_ike_secret_bundle(&proposal, &[0x12; 32], &[0x22; 32], 1, 2, &[0x44; 256])
                .expect("second");

        assert_ne!(first.sk_d.expose_for_test(), second.sk_d.expose_for_test());
        assert_eq!(first.summary().prf, "hmac_sha1");
        assert_eq!(first.summary().integrity_key_bytes, 20);
    }

    #[test]
    fn derives_child_sa_secret_pair_without_serializing_values() {
        let ike_proposal =
            ike_proposal_from_profile_string("aes128-sha256-modp2048", 1).expect("ike proposal");
        let ike_bundle = derive_ike_secret_bundle(
            &ike_proposal,
            &[0x11; 32],
            &[0x22; 32],
            0x0102_0304_0506_0708,
            0x1112_1314_1516_1718,
            &[0x33; 256],
        )
        .expect("derive ike bundle");
        let esp_proposal =
            child_sa_proposal_from_profile_string("aes128-sha256", 1, &[0xaa, 0xbb, 0xcc, 0xdd])
                .expect("esp proposal");

        let secrets =
            derive_child_sa_secret_pair(&ike_bundle, &esp_proposal, &[0x44; 32], &[0x55; 32])
                .expect("derive child sa keys");
        let summary = secrets.summary();

        assert_eq!(summary.encryption, "aes_cbc");
        assert_eq!(summary.integrity, "hmac_sha256_128");
        assert_eq!(summary.encryption_key_bytes, 16);
        assert_eq!(summary.integrity_key_bytes, 32);
        assert_eq!(summary.direction_secret_bytes, 48);
        assert_eq!(summary.total_secret_bytes, 96);
        assert_eq!(secrets.outbound_encryption.len(), 16);
        assert_eq!(secrets.outbound_integrity.len(), 32);
        assert_eq!(secrets.inbound_encryption.len(), 16);
        assert_eq!(secrets.inbound_integrity.len(), 32);
        assert_ne!(
            secrets.outbound_encryption.expose_for_test(),
            secrets.inbound_encryption.expose_for_test()
        );

        let debug = format!("{secrets:?}").to_ascii_lowercase();
        assert!(debug.contains("<redacted>"));
        for forbidden in ["33333333", "44444444", "55555555"] {
            assert!(!debug.contains(forbidden));
        }
        let json = serde_json::to_string(&summary).expect("serialize summary");
        for forbidden_key in [
            "outbound_encryption",
            "outbound_integrity",
            "inbound_encryption",
            "inbound_integrity",
            "key_material",
            "sk_d",
        ] {
            assert!(!json
                .to_ascii_lowercase()
                .contains(&format!("\"{forbidden_key}\"")));
        }
    }

    #[test]
    fn rejects_missing_inputs_for_key_schedule() {
        let proposal =
            ike_proposal_from_profile_string("aes128-sha256-modp2048", 1).expect("proposal");

        assert_eq!(
            derive_ike_secret_bundle(&proposal, &[0x11], &[0x22], 1, 2, &[]).unwrap_err(),
            IkeKeyError::EmptySharedSecret
        );
        assert_eq!(
            derive_ike_secret_bundle(&proposal, &[], &[0x22], 1, 2, &[0x33]).unwrap_err(),
            IkeKeyError::EmptyNonce
        );
    }
}
