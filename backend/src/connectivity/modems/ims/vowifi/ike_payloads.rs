#![allow(dead_code)]

use std::fmt;

use super::ike_codec::{IkePayload, IkePayloadType};

pub const ENCR_AES_CBC: u16 = 12;
pub const PRF_HMAC_MD5: u16 = 1;
pub const PRF_HMAC_SHA1: u16 = 2;
pub const PRF_HMAC_SHA2_256: u16 = 5;
pub const PRF_HMAC_SHA2_384: u16 = 6;
pub const PRF_HMAC_SHA2_512: u16 = 7;
pub const AUTH_HMAC_MD5_96: u16 = 1;
pub const AUTH_HMAC_SHA1_96: u16 = 2;
pub const AUTH_HMAC_SHA2_256_128: u16 = 12;
pub const AUTH_HMAC_SHA2_384_192: u16 = 13;
pub const AUTH_HMAC_SHA2_512_256: u16 = 14;
pub const DH_MODP_768: u16 = 1;
pub const DH_MODP_1024: u16 = 2;
pub const DH_MODP_1536: u16 = 5;
pub const DH_MODP_2048: u16 = 14;
pub const DH_MODP_3072: u16 = 15;
pub const DH_MODP_4096: u16 = 16;
pub const DH_MODP_8192: u16 = 18;
pub const ESN_NO_EXTENDED_SEQUENCE_NUMBERS: u16 = 0;
pub const NOTIFY_NAT_DETECTION_SOURCE_IP: u16 = 16_388;
pub const NOTIFY_NAT_DETECTION_DESTINATION_IP: u16 = 16_389;
pub const NOTIFY_EAP_ONLY_AUTHENTICATION: u16 = 16_417;
pub const NOTIFY_IKEV2_FRAGMENTATION_SUPPORTED: u16 = 16_430;
pub const IKE_ID_RFC822_ADDR: u8 = 3;
pub const IKE_ID_FQDN: u8 = 2;
pub const CFG_REQUEST: u8 = 1;
pub const CFG_ATTR_INTERNAL_IP4_ADDRESS: u16 = 1;
pub const CFG_ATTR_INTERNAL_IP4_DNS: u16 = 3;
pub const CFG_ATTR_INTERNAL_IP6_ADDRESS: u16 = 8;
pub const CFG_ATTR_INTERNAL_IP6_DNS: u16 = 10;
pub const CFG_ATTR_INTERNAL_IP4_PCSCF: u16 = 20;
pub const CFG_ATTR_INTERNAL_IP6_PCSCF: u16 = 21;
pub const TS_IPV4_ADDR_RANGE: u8 = 7;
pub const TS_IPV6_ADDR_RANGE: u8 = 8;
pub const AUTH_METHOD_SHARED_KEY_MIC: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IkeProtocolId {
    Ike = 1,
    Ah = 2,
    Esp = 3,
}

impl IkeProtocolId {
    pub fn as_u8(self) -> u8 {
        match self {
            Self::Ike => 1,
            Self::Ah => 2,
            Self::Esp => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TransformType {
    Encryption = 1,
    Prf = 2,
    Integrity = 3,
    DiffieHellmanGroup = 4,
    ExtendedSequenceNumbers = 5,
}

impl TransformType {
    pub fn as_u8(self) -> u8 {
        match self {
            Self::Encryption => 1,
            Self::Prf => 2,
            Self::Integrity => 3,
            Self::DiffieHellmanGroup => 4,
            Self::ExtendedSequenceNumbers => 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformAttribute {
    KeyLength(u16),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransformSpec {
    pub transform_type: TransformType,
    pub transform_id: u16,
    pub attributes: Vec<TransformAttribute>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalSpec {
    pub number: u8,
    pub protocol_id: IkeProtocolId,
    pub spi: Vec<u8>,
    pub transforms: Vec<TransformSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyProtocolId {
    None,
    Ike,
    Ah,
    Esp,
}

impl NotifyProtocolId {
    pub fn as_u8(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Ike => 1,
            Self::Ah => 2,
            Self::Esp => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ParsedTransformAttribute {
    pub attribute_type: u16,
    pub value: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ParsedTransform {
    pub transform_type: u8,
    pub transform_id: u16,
    pub attributes: Vec<ParsedTransformAttribute>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ParsedProposal {
    pub number: u8,
    pub protocol_id: u8,
    pub spi_size: u8,
    pub spi_present: bool,
    #[serde(skip_serializing)]
    pub spi: Vec<u8>,
    pub transforms: Vec<ParsedTransform>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayloadBuildError {
    EmptyTransforms,
    TooManyTransforms(usize),
    SpiTooLarge(usize),
    ProposalTooLarge(usize),
    TransformTooLarge(usize),
}

impl fmt::Display for PayloadBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTransforms => write!(f, "proposal has no transforms"),
            Self::TooManyTransforms(count) => write!(f, "too many transforms: {count}"),
            Self::SpiTooLarge(length) => write!(f, "SPI too large: {length} bytes"),
            Self::ProposalTooLarge(length) => write!(f, "proposal too large: {length} bytes"),
            Self::TransformTooLarge(length) => write!(f, "transform too large: {length} bytes"),
        }
    }
}

impl std::error::Error for PayloadBuildError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaParseError {
    TruncatedProposal,
    TruncatedTransform,
    InvalidProposalLength {
        declared: usize,
    },
    InvalidTransformLength {
        declared: usize,
    },
    InvalidAttributeLength {
        declared: usize,
    },
    SpiExceedsProposal {
        spi_size: usize,
        proposal_len: usize,
    },
    TransformCountMismatch {
        declared: usize,
        parsed: usize,
    },
}

impl fmt::Display for SaParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedProposal => write!(f, "truncated SA proposal"),
            Self::TruncatedTransform => write!(f, "truncated SA transform"),
            Self::InvalidProposalLength { declared } => {
                write!(f, "invalid SA proposal length: {declared}")
            }
            Self::InvalidTransformLength { declared } => {
                write!(f, "invalid SA transform length: {declared}")
            }
            Self::InvalidAttributeLength { declared } => {
                write!(f, "invalid SA transform attribute length: {declared}")
            }
            Self::SpiExceedsProposal {
                spi_size,
                proposal_len,
            } => write!(
                f,
                "SA proposal SPI size exceeds proposal length: spi_size={spi_size} proposal_len={proposal_len}"
            ),
            Self::TransformCountMismatch { declared, parsed } => write!(
                f,
                "SA proposal transform count mismatch: declared={declared} parsed={parsed}"
            ),
        }
    }
}

impl std::error::Error for SaParseError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposalParseError {
    Empty,
    MissingEncryption,
    MissingIntegrity,
    MissingPrf,
    MissingDhGroup,
    UnsupportedToken(String),
}

impl fmt::Display for ProposalParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "empty IKE proposal"),
            Self::MissingEncryption => write!(f, "IKE proposal has no encryption transform"),
            Self::MissingIntegrity => write!(f, "IKE proposal has no integrity transform"),
            Self::MissingPrf => write!(f, "IKE proposal has no PRF transform"),
            Self::MissingDhGroup => write!(f, "IKE proposal has no DH group"),
            Self::UnsupportedToken(token) => write!(f, "unsupported IKE proposal token: {token}"),
        }
    }
}

impl std::error::Error for ProposalParseError {}

pub fn build_sa_payload(proposals: &[ProposalSpec]) -> Result<IkePayload, PayloadBuildError> {
    let mut body = Vec::new();
    for (index, proposal) in proposals.iter().enumerate() {
        let encoded = encode_proposal(proposal, index + 1 < proposals.len())?;
        body.extend_from_slice(&encoded);
    }

    Ok(IkePayload {
        payload_type: IkePayloadType::SecurityAssociation,
        critical: false,
        body,
    })
}

pub fn build_ke_payload(dh_group: u16, public_value: &[u8]) -> IkePayload {
    let mut body = Vec::with_capacity(4 + public_value.len());
    body.extend_from_slice(&dh_group.to_be_bytes());
    body.extend_from_slice(&0u16.to_be_bytes());
    body.extend_from_slice(public_value);
    IkePayload {
        payload_type: IkePayloadType::KeyExchange,
        critical: false,
        body,
    }
}

pub fn build_nonce_payload(nonce: &[u8]) -> IkePayload {
    IkePayload {
        payload_type: IkePayloadType::Nonce,
        critical: false,
        body: nonce.to_vec(),
    }
}

pub fn build_notify_payload(
    protocol_id: NotifyProtocolId,
    spi: &[u8],
    notify_type: u16,
    data: &[u8],
) -> Result<IkePayload, PayloadBuildError> {
    if spi.len() > u8::MAX as usize {
        return Err(PayloadBuildError::SpiTooLarge(spi.len()));
    }
    let mut body = Vec::with_capacity(4 + spi.len() + data.len());
    body.push(protocol_id.as_u8());
    body.push(spi.len() as u8);
    body.extend_from_slice(&notify_type.to_be_bytes());
    body.extend_from_slice(spi);
    body.extend_from_slice(data);
    Ok(IkePayload {
        payload_type: IkePayloadType::Notify,
        critical: false,
        body,
    })
}

pub fn parse_sa_payload(body: &[u8]) -> Result<Vec<ParsedProposal>, SaParseError> {
    let mut proposals = Vec::new();
    let mut offset = 0usize;
    while offset < body.len() {
        if body.len().saturating_sub(offset) < 8 {
            return Err(SaParseError::TruncatedProposal);
        }

        let proposal_len = u16::from_be_bytes(
            body[offset + 2..offset + 4]
                .try_into()
                .expect("slice length"),
        ) as usize;
        if proposal_len < 8 {
            return Err(SaParseError::InvalidProposalLength {
                declared: proposal_len,
            });
        }
        let end = offset + proposal_len;
        if end > body.len() {
            return Err(SaParseError::TruncatedProposal);
        }

        let number = body[offset + 4];
        let protocol_id = body[offset + 5];
        let spi_size = body[offset + 6];
        let transform_count = usize::from(body[offset + 7]);
        let spi_start = offset + 8;
        let transform_start = spi_start + usize::from(spi_size);
        if transform_start > end {
            return Err(SaParseError::SpiExceedsProposal {
                spi_size: usize::from(spi_size),
                proposal_len,
            });
        }

        let spi = body[spi_start..transform_start].to_vec();
        let transforms = parse_transforms(&body[transform_start..end])?;
        if transforms.len() != transform_count {
            return Err(SaParseError::TransformCountMismatch {
                declared: transform_count,
                parsed: transforms.len(),
            });
        }

        proposals.push(ParsedProposal {
            number,
            protocol_id,
            spi_size,
            spi_present: spi_size > 0,
            spi,
            transforms,
        });
        offset = end;
    }
    Ok(proposals)
}

pub fn ike_proposal_aes128_sha256_modp2048() -> ProposalSpec {
    ike_proposal_from_profile_string("aes128-sha256-modp2048", 1).expect("static proposal")
}

pub fn build_identification_initiator_payload(identity: &str) -> IkePayload {
    let mut body = Vec::with_capacity(4 + identity.len());
    body.push(IKE_ID_RFC822_ADDR);
    body.extend_from_slice(&[0, 0, 0]);
    body.extend_from_slice(identity.as_bytes());
    IkePayload {
        payload_type: IkePayloadType::IdentificationInitiator,
        critical: false,
        body,
    }
}

pub fn build_identification_responder_payload(fqdn: &str) -> IkePayload {
    let mut body = Vec::with_capacity(4 + fqdn.len());
    body.push(IKE_ID_FQDN);
    body.extend_from_slice(&[0, 0, 0]);
    body.extend_from_slice(fqdn.as_bytes());
    IkePayload {
        payload_type: IkePayloadType::IdentificationResponder,
        critical: false,
        body,
    }
}

/// Carrier-policy-gated device identity. This is a separate Vendor ID payload:
/// it must never replace the permanent NAI carried in IDi/EAP-AKA.
pub fn build_device_identity_vendor_payload(imei: &str) -> IkePayload {
    IkePayload {
        payload_type: IkePayloadType::VendorId,
        critical: false,
        body: imei.as_bytes().to_vec(),
    }
}

pub fn build_authentication_shared_key_payload(authentication_data: &[u8]) -> IkePayload {
    let mut body = Vec::with_capacity(4 + authentication_data.len());
    body.push(AUTH_METHOD_SHARED_KEY_MIC);
    body.extend_from_slice(&[0, 0, 0]);
    body.extend_from_slice(authentication_data);
    IkePayload {
        payload_type: IkePayloadType::Authentication,
        critical: false,
        body,
    }
}

pub fn build_configuration_request_payload(ip_stack: &str) -> IkePayload {
    let mut body = Vec::new();
    body.push(CFG_REQUEST);
    body.extend_from_slice(&[0, 0, 0]);

    if requests_ipv4(ip_stack) {
        push_config_attribute(&mut body, CFG_ATTR_INTERNAL_IP4_ADDRESS, &[]);
        push_config_attribute(&mut body, CFG_ATTR_INTERNAL_IP4_DNS, &[]);
        push_config_attribute(&mut body, CFG_ATTR_INTERNAL_IP4_PCSCF, &[]);
    }
    if requests_ipv6(ip_stack) {
        push_config_attribute(&mut body, CFG_ATTR_INTERNAL_IP6_ADDRESS, &[]);
        push_config_attribute(&mut body, CFG_ATTR_INTERNAL_IP6_DNS, &[]);
        push_config_attribute(&mut body, CFG_ATTR_INTERNAL_IP6_PCSCF, &[]);
    }

    IkePayload {
        payload_type: IkePayloadType::Configuration,
        critical: false,
        body,
    }
}

pub fn build_traffic_selector_initiator_payload(ip_stack: &str) -> IkePayload {
    build_traffic_selector_payload(IkePayloadType::TrafficSelectorInitiator, ip_stack)
}

pub fn build_traffic_selector_responder_payload(ip_stack: &str) -> IkePayload {
    build_traffic_selector_payload(IkePayloadType::TrafficSelectorResponder, ip_stack)
}

pub fn child_sa_proposal_from_profile_string(
    proposal: &str,
    number: u8,
    spi: &[u8],
) -> Result<ProposalSpec, ProposalParseError> {
    let tokens = proposal
        .split('-')
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return Err(ProposalParseError::Empty);
    }

    let mut encryption: Option<TransformSpec> = None;
    let mut integrity: Option<TransformSpec> = None;

    for token in tokens {
        match token {
            "aes128" => {
                encryption = Some(TransformSpec {
                    transform_type: TransformType::Encryption,
                    transform_id: ENCR_AES_CBC,
                    attributes: vec![TransformAttribute::KeyLength(128)],
                });
            }
            "aes256" => {
                encryption = Some(TransformSpec {
                    transform_type: TransformType::Encryption,
                    transform_id: ENCR_AES_CBC,
                    attributes: vec![TransformAttribute::KeyLength(256)],
                });
            }
            "md5" => {
                integrity = Some(TransformSpec {
                    transform_type: TransformType::Integrity,
                    transform_id: AUTH_HMAC_MD5_96,
                    attributes: Vec::new(),
                });
            }
            "sha1" => {
                integrity = Some(TransformSpec {
                    transform_type: TransformType::Integrity,
                    transform_id: AUTH_HMAC_SHA1_96,
                    attributes: Vec::new(),
                });
            }
            "sha256" => {
                integrity = Some(TransformSpec {
                    transform_type: TransformType::Integrity,
                    transform_id: AUTH_HMAC_SHA2_256_128,
                    attributes: Vec::new(),
                });
            }
            "sha384" => {
                integrity = Some(TransformSpec {
                    transform_type: TransformType::Integrity,
                    transform_id: AUTH_HMAC_SHA2_384_192,
                    attributes: Vec::new(),
                });
            }
            "sha512" => {
                integrity = Some(TransformSpec {
                    transform_type: TransformType::Integrity,
                    transform_id: AUTH_HMAC_SHA2_512_256,
                    attributes: Vec::new(),
                });
            }
            other if other.starts_with("prf") || other.starts_with("modp") => {}
            other => return Err(ProposalParseError::UnsupportedToken(other.to_string())),
        }
    }

    Ok(ProposalSpec {
        number,
        protocol_id: IkeProtocolId::Esp,
        spi: spi.to_vec(),
        transforms: vec![
            encryption.ok_or(ProposalParseError::MissingEncryption)?,
            integrity.ok_or(ProposalParseError::MissingIntegrity)?,
            TransformSpec {
                transform_type: TransformType::ExtendedSequenceNumbers,
                transform_id: ESN_NO_EXTENDED_SEQUENCE_NUMBERS,
                attributes: Vec::new(),
            },
        ],
    })
}

pub fn ike_proposal_from_profile_string(
    proposal: &str,
    number: u8,
) -> Result<ProposalSpec, ProposalParseError> {
    let tokens = proposal
        .split('-')
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return Err(ProposalParseError::Empty);
    }

    let mut encryption: Option<TransformSpec> = None;
    let mut integrity: Option<TransformSpec> = None;
    let mut prf: Option<TransformSpec> = None;
    let mut dh_group: Option<TransformSpec> = None;

    for token in tokens {
        match token {
            "aes128" => {
                encryption = Some(TransformSpec {
                    transform_type: TransformType::Encryption,
                    transform_id: ENCR_AES_CBC,
                    attributes: vec![TransformAttribute::KeyLength(128)],
                });
            }
            "aes256" => {
                encryption = Some(TransformSpec {
                    transform_type: TransformType::Encryption,
                    transform_id: ENCR_AES_CBC,
                    attributes: vec![TransformAttribute::KeyLength(256)],
                });
            }
            "md5" => {
                integrity = Some(TransformSpec {
                    transform_type: TransformType::Integrity,
                    transform_id: AUTH_HMAC_MD5_96,
                    attributes: Vec::new(),
                });
            }
            "sha1" => {
                integrity = Some(TransformSpec {
                    transform_type: TransformType::Integrity,
                    transform_id: AUTH_HMAC_SHA1_96,
                    attributes: Vec::new(),
                });
            }
            "sha256" => {
                integrity = Some(TransformSpec {
                    transform_type: TransformType::Integrity,
                    transform_id: AUTH_HMAC_SHA2_256_128,
                    attributes: Vec::new(),
                });
            }
            "sha384" => {
                integrity = Some(TransformSpec {
                    transform_type: TransformType::Integrity,
                    transform_id: AUTH_HMAC_SHA2_384_192,
                    attributes: Vec::new(),
                });
            }
            "sha512" => {
                integrity = Some(TransformSpec {
                    transform_type: TransformType::Integrity,
                    transform_id: AUTH_HMAC_SHA2_512_256,
                    attributes: Vec::new(),
                });
            }
            "prfmd5" => {
                prf = Some(TransformSpec {
                    transform_type: TransformType::Prf,
                    transform_id: PRF_HMAC_MD5,
                    attributes: Vec::new(),
                });
            }
            "prfsha1" => {
                prf = Some(TransformSpec {
                    transform_type: TransformType::Prf,
                    transform_id: PRF_HMAC_SHA1,
                    attributes: Vec::new(),
                });
            }
            "prfsha256" => {
                prf = Some(TransformSpec {
                    transform_type: TransformType::Prf,
                    transform_id: PRF_HMAC_SHA2_256,
                    attributes: Vec::new(),
                });
            }
            "prfsha384" => {
                prf = Some(TransformSpec {
                    transform_type: TransformType::Prf,
                    transform_id: PRF_HMAC_SHA2_384,
                    attributes: Vec::new(),
                });
            }
            "prfsha512" => {
                prf = Some(TransformSpec {
                    transform_type: TransformType::Prf,
                    transform_id: PRF_HMAC_SHA2_512,
                    attributes: Vec::new(),
                });
            }
            "modp768" => {
                dh_group = Some(TransformSpec {
                    transform_type: TransformType::DiffieHellmanGroup,
                    transform_id: DH_MODP_768,
                    attributes: Vec::new(),
                });
            }
            "modp1024" => {
                dh_group = Some(TransformSpec {
                    transform_type: TransformType::DiffieHellmanGroup,
                    transform_id: DH_MODP_1024,
                    attributes: Vec::new(),
                });
            }
            "modp1536" => {
                dh_group = Some(TransformSpec {
                    transform_type: TransformType::DiffieHellmanGroup,
                    transform_id: DH_MODP_1536,
                    attributes: Vec::new(),
                });
            }
            "modp2048" => {
                dh_group = Some(TransformSpec {
                    transform_type: TransformType::DiffieHellmanGroup,
                    transform_id: DH_MODP_2048,
                    attributes: Vec::new(),
                });
            }
            "modp3072" => {
                dh_group = Some(TransformSpec {
                    transform_type: TransformType::DiffieHellmanGroup,
                    transform_id: DH_MODP_3072,
                    attributes: Vec::new(),
                });
            }
            "modp4096" => {
                dh_group = Some(TransformSpec {
                    transform_type: TransformType::DiffieHellmanGroup,
                    transform_id: DH_MODP_4096,
                    attributes: Vec::new(),
                });
            }
            "modp8192" => {
                dh_group = Some(TransformSpec {
                    transform_type: TransformType::DiffieHellmanGroup,
                    transform_id: DH_MODP_8192,
                    attributes: Vec::new(),
                });
            }
            other => return Err(ProposalParseError::UnsupportedToken(other.to_string())),
        }
    }

    if prf.is_none() {
        prf = Some(default_prf_for_integrity(
            integrity
                .as_ref()
                .ok_or(ProposalParseError::MissingIntegrity)?,
        )?);
    }

    Ok(ProposalSpec {
        number,
        protocol_id: IkeProtocolId::Ike,
        spi: Vec::new(),
        transforms: vec![
            encryption.ok_or(ProposalParseError::MissingEncryption)?,
            prf.ok_or(ProposalParseError::MissingPrf)?,
            integrity.ok_or(ProposalParseError::MissingIntegrity)?,
            dh_group.ok_or(ProposalParseError::MissingDhGroup)?,
        ],
    })
}

pub fn ike_proposal_dh_group_from_profile_string(
    proposal: &str,
) -> Result<u16, ProposalParseError> {
    let proposal = ike_proposal_from_profile_string(proposal, 1)?;
    proposal
        .transforms
        .iter()
        .find(|transform| transform.transform_type == TransformType::DiffieHellmanGroup)
        .map(|transform| transform.transform_id)
        .ok_or(ProposalParseError::MissingDhGroup)
}

fn default_prf_for_integrity(
    integrity: &TransformSpec,
) -> Result<TransformSpec, ProposalParseError> {
    let transform_id = match integrity.transform_id {
        AUTH_HMAC_MD5_96 => PRF_HMAC_MD5,
        AUTH_HMAC_SHA1_96 => PRF_HMAC_SHA1,
        AUTH_HMAC_SHA2_256_128 => PRF_HMAC_SHA2_256,
        AUTH_HMAC_SHA2_384_192 => PRF_HMAC_SHA2_384,
        AUTH_HMAC_SHA2_512_256 => PRF_HMAC_SHA2_512,
        _ => return Err(ProposalParseError::MissingPrf),
    };

    Ok(TransformSpec {
        transform_type: TransformType::Prf,
        transform_id,
        attributes: Vec::new(),
    })
}

fn push_config_attribute(body: &mut Vec<u8>, attribute_type: u16, value: &[u8]) {
    body.extend_from_slice(&attribute_type.to_be_bytes());
    body.extend_from_slice(&(value.len() as u16).to_be_bytes());
    body.extend_from_slice(value);
}

fn build_traffic_selector_payload(payload_type: IkePayloadType, ip_stack: &str) -> IkePayload {
    let mut selectors = Vec::new();
    let mut selector_count = 0u8;
    if requests_ipv4(ip_stack) {
        push_ipv4_selector(&mut selectors);
        selector_count = selector_count.saturating_add(1);
    }
    if requests_ipv6(ip_stack) {
        push_ipv6_selector(&mut selectors);
        selector_count = selector_count.saturating_add(1);
    }
    if selectors.is_empty() {
        push_ipv4_selector(&mut selectors);
        selector_count = 1;
    }

    let mut body = Vec::with_capacity(4 + selectors.len());
    body.push(selector_count);
    body.extend_from_slice(&[0, 0, 0]);
    body.extend_from_slice(&selectors);
    IkePayload {
        payload_type,
        critical: false,
        body,
    }
}

fn push_ipv4_selector(body: &mut Vec<u8>) {
    body.push(TS_IPV4_ADDR_RANGE);
    body.push(0);
    body.extend_from_slice(&16u16.to_be_bytes());
    body.extend_from_slice(&0u16.to_be_bytes());
    body.extend_from_slice(&u16::MAX.to_be_bytes());
    body.extend_from_slice(&[0, 0, 0, 0]);
    body.extend_from_slice(&[255, 255, 255, 255]);
}

fn push_ipv6_selector(body: &mut Vec<u8>) {
    body.push(TS_IPV6_ADDR_RANGE);
    body.push(0);
    body.extend_from_slice(&40u16.to_be_bytes());
    body.extend_from_slice(&0u16.to_be_bytes());
    body.extend_from_slice(&u16::MAX.to_be_bytes());
    body.extend_from_slice(&[0; 16]);
    body.extend_from_slice(&[0xff; 16]);
}

fn requests_ipv4(ip_stack: &str) -> bool {
    matches!(ip_stack, "ipv4" | "ipv4v6" | "dual" | "dual_stack")
}

fn requests_ipv6(ip_stack: &str) -> bool {
    matches!(ip_stack, "ipv6" | "ipv4v6" | "dual" | "dual_stack")
}

fn parse_transforms(mut input: &[u8]) -> Result<Vec<ParsedTransform>, SaParseError> {
    let mut transforms = Vec::new();
    while !input.is_empty() {
        if input.len() < 8 {
            return Err(SaParseError::TruncatedTransform);
        }
        let transform_len =
            u16::from_be_bytes(input[2..4].try_into().expect("slice length")) as usize;
        if transform_len < 8 {
            return Err(SaParseError::InvalidTransformLength {
                declared: transform_len,
            });
        }
        if transform_len > input.len() {
            return Err(SaParseError::TruncatedTransform);
        }

        let transform_type = input[4];
        let transform_id = u16::from_be_bytes(input[6..8].try_into().expect("slice length"));
        let attributes = parse_transform_attributes(&input[8..transform_len])?;
        transforms.push(ParsedTransform {
            transform_type,
            transform_id,
            attributes,
        });
        input = &input[transform_len..];
    }
    Ok(transforms)
}

fn parse_transform_attributes(
    mut input: &[u8],
) -> Result<Vec<ParsedTransformAttribute>, SaParseError> {
    let mut attributes = Vec::new();
    while !input.is_empty() {
        if input.len() < 4 {
            return Err(SaParseError::InvalidAttributeLength {
                declared: input.len(),
            });
        }

        let attribute_type = u16::from_be_bytes(input[0..2].try_into().expect("slice length"));
        let af_bit = attribute_type & 0x8000 != 0;
        if af_bit {
            let value = u16::from_be_bytes(input[2..4].try_into().expect("slice length"));
            attributes.push(ParsedTransformAttribute {
                attribute_type: attribute_type & 0x7fff,
                value,
            });
            input = &input[4..];
        } else {
            let length = u16::from_be_bytes(input[2..4].try_into().expect("slice length")) as usize;
            if length < 4 || length > input.len() {
                return Err(SaParseError::InvalidAttributeLength { declared: length });
            }
            attributes.push(ParsedTransformAttribute {
                attribute_type,
                value: length as u16,
            });
            input = &input[length..];
        }
    }
    Ok(attributes)
}

fn encode_proposal(proposal: &ProposalSpec, has_more: bool) -> Result<Vec<u8>, PayloadBuildError> {
    if proposal.transforms.is_empty() {
        return Err(PayloadBuildError::EmptyTransforms);
    }
    if proposal.transforms.len() > u8::MAX as usize {
        return Err(PayloadBuildError::TooManyTransforms(
            proposal.transforms.len(),
        ));
    }
    if proposal.spi.len() > u8::MAX as usize {
        return Err(PayloadBuildError::SpiTooLarge(proposal.spi.len()));
    }

    let mut transform_bytes = Vec::new();
    for (index, transform) in proposal.transforms.iter().enumerate() {
        transform_bytes.extend_from_slice(&encode_transform(
            transform,
            index + 1 < proposal.transforms.len(),
        )?);
    }

    let proposal_len = 8 + proposal.spi.len() + transform_bytes.len();
    if proposal_len > u16::MAX as usize {
        return Err(PayloadBuildError::ProposalTooLarge(proposal_len));
    }

    let mut out = Vec::with_capacity(proposal_len);
    out.push(if has_more { 2 } else { 0 });
    out.push(0);
    out.extend_from_slice(&(proposal_len as u16).to_be_bytes());
    out.push(proposal.number);
    out.push(proposal.protocol_id.as_u8());
    out.push(proposal.spi.len() as u8);
    out.push(proposal.transforms.len() as u8);
    out.extend_from_slice(&proposal.spi);
    out.extend_from_slice(&transform_bytes);
    Ok(out)
}

fn encode_transform(
    transform: &TransformSpec,
    has_more: bool,
) -> Result<Vec<u8>, PayloadBuildError> {
    let attributes_len = transform
        .attributes
        .iter()
        .map(|attribute| match attribute {
            TransformAttribute::KeyLength(_) => 4,
        })
        .sum::<usize>();
    let transform_len = 8 + attributes_len;
    if transform_len > u16::MAX as usize {
        return Err(PayloadBuildError::TransformTooLarge(transform_len));
    }

    let mut out = Vec::with_capacity(transform_len);
    out.push(if has_more { 3 } else { 0 });
    out.push(0);
    out.extend_from_slice(&(transform_len as u16).to_be_bytes());
    out.push(transform.transform_type.as_u8());
    out.push(0);
    out.extend_from_slice(&transform.transform_id.to_be_bytes());
    for attribute in &transform.attributes {
        match attribute {
            TransformAttribute::KeyLength(bits) => {
                out.extend_from_slice(&0x800e_u16.to_be_bytes());
                out.extend_from_slice(&bits.to_be_bytes());
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::modems::ims::vowifi::ike_codec::{
        IkeExchangeType, IkeMessage, IkePayloadType,
    };

    #[test]
    fn builds_sa_payload_with_aes128_sha256_modp2048_transforms() {
        let payload =
            build_sa_payload(&[ike_proposal_aes128_sha256_modp2048()]).expect("build sa payload");

        assert_eq!(payload.payload_type, IkePayloadType::SecurityAssociation);
        assert_eq!(payload.body[0], 0);
        assert_eq!(payload.body[4], 1);
        assert_eq!(payload.body[5], IkeProtocolId::Ike.as_u8());
        assert_eq!(payload.body[6], 0);
        assert_eq!(payload.body[7], 4);
        assert_eq!(payload.body[12], TransformType::Encryption.as_u8());
        assert_eq!(payload.body[15], 12);
    }

    #[test]
    fn parses_profile_proposals_into_ordered_ike_transforms() {
        let proposal = ike_proposal_from_profile_string("aes256-sha512-prfsha512-modp2048", 7)
            .expect("parse proposal");

        assert_eq!(proposal.number, 7);
        assert_eq!(proposal.transforms.len(), 4);
        assert_eq!(
            proposal.transforms[0].transform_type,
            TransformType::Encryption
        );
        assert_eq!(proposal.transforms[0].transform_id, ENCR_AES_CBC);
        assert_eq!(
            proposal.transforms[0].attributes,
            vec![TransformAttribute::KeyLength(256)]
        );
        assert_eq!(proposal.transforms[1].transform_type, TransformType::Prf);
        assert_eq!(proposal.transforms[1].transform_id, PRF_HMAC_SHA2_512);
        assert_eq!(
            proposal.transforms[2].transform_type,
            TransformType::Integrity
        );
        assert_eq!(proposal.transforms[2].transform_id, AUTH_HMAC_SHA2_512_256);
        assert_eq!(
            proposal.transforms[3].transform_type,
            TransformType::DiffieHellmanGroup
        );
        assert_eq!(proposal.transforms[3].transform_id, DH_MODP_2048);
    }

    #[test]
    fn parses_modp1024_ike_proposal_for_legacy_interop() {
        let proposal = ike_proposal_from_profile_string("aes128-sha1-modp1024", 3)
            .expect("parse legacy-compatible proposal");

        assert_eq!(proposal.transforms.len(), 4);
        assert_eq!(proposal.transforms[1].transform_type, TransformType::Prf);
        assert_eq!(proposal.transforms[1].transform_id, PRF_HMAC_SHA1);
        assert_eq!(
            proposal.transforms[3].transform_type,
            TransformType::DiffieHellmanGroup
        );
        assert_eq!(proposal.transforms[3].transform_id, DH_MODP_1024);
        assert_eq!(
            ike_proposal_dh_group_from_profile_string("aes128-sha256-modp1024").expect("dh group"),
            DH_MODP_1024
        );
    }

    #[test]
    fn parses_md5_sha384_and_extended_modp_proposals() {
        let md5 =
            ike_proposal_from_profile_string("aes128-md5-modp768", 1).expect("parse md5 proposal");
        assert_eq!(md5.transforms[1].transform_id, PRF_HMAC_MD5);
        assert_eq!(md5.transforms[2].transform_id, AUTH_HMAC_MD5_96);
        assert_eq!(md5.transforms[3].transform_id, DH_MODP_768);

        let sha384 = ike_proposal_from_profile_string("aes256-sha384-prfsha384-modp4096", 2)
            .expect("parse sha384 proposal");
        assert_eq!(sha384.transforms[1].transform_id, PRF_HMAC_SHA2_384);
        assert_eq!(sha384.transforms[2].transform_id, AUTH_HMAC_SHA2_384_192);
        assert_eq!(sha384.transforms[3].transform_id, DH_MODP_4096);

        for (proposal, expected_group) in [
            ("aes128-sha256-modp1536", DH_MODP_1536),
            ("aes128-sha256-modp3072", DH_MODP_3072),
            ("aes128-sha256-modp8192", DH_MODP_8192),
        ] {
            assert_eq!(
                ike_proposal_dh_group_from_profile_string(proposal).expect("dh group"),
                expected_group
            );
        }

        let esp = child_sa_proposal_from_profile_string("aes128-md5", 1, &[1, 2, 3, 4])
            .expect("parse md5 esp proposal");
        assert_eq!(esp.transforms[1].transform_id, AUTH_HMAC_MD5_96);
    }

    #[test]
    fn parses_sa_payload_without_exposing_spi_value() {
        let mut proposal =
            ike_proposal_from_profile_string("aes128-sha256-modp2048", 1).expect("parse proposal");
        proposal.protocol_id = IkeProtocolId::Esp;
        proposal.spi = vec![0xaa, 0xbb, 0xcc, 0xdd];
        let payload = build_sa_payload(&[proposal]).expect("build sa payload");

        let parsed = parse_sa_payload(&payload.body).expect("parse sa payload");

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].number, 1);
        assert_eq!(parsed[0].protocol_id, IkeProtocolId::Esp.as_u8());
        assert_eq!(parsed[0].spi_size, 4);
        assert!(parsed[0].spi_present);
        assert_eq!(parsed[0].transforms.len(), 4);
        assert_eq!(parsed[0].transforms[0].transform_type, 1);
        assert_eq!(parsed[0].transforms[0].transform_id, ENCR_AES_CBC);
        assert_eq!(
            parsed[0].transforms[0].attributes[0],
            ParsedTransformAttribute {
                attribute_type: 14,
                value: 128,
            }
        );

        let json = serde_json::to_string(&parsed).expect("serialize parsed sa");
        assert!(!json.to_ascii_lowercase().contains("aabbccdd"));
        assert!(!json.to_ascii_lowercase().contains("\"spi\""));
    }

    #[test]
    fn derives_prf_from_integrity_when_profile_omits_explicit_prf() {
        let proposal =
            ike_proposal_from_profile_string("aes128-sha1-modp2048", 1).expect("parse proposal");

        assert_eq!(proposal.transforms[1].transform_type, TransformType::Prf);
        assert_eq!(proposal.transforms[1].transform_id, PRF_HMAC_SHA1);
        assert_eq!(
            proposal.transforms[2].transform_type,
            TransformType::Integrity
        );
        assert_eq!(proposal.transforms[2].transform_id, AUTH_HMAC_SHA1_96);
    }

    #[test]
    fn builds_ke_nonce_and_notify_payloads() {
        let ke = build_ke_payload(DH_MODP_2048, &[0x11; 256]);
        assert_eq!(ke.payload_type, IkePayloadType::KeyExchange);
        assert_eq!(&ke.body[..4], &[0, 14, 0, 0]);
        assert_eq!(ke.body.len(), 260);

        let nonce = build_nonce_payload(&[0x22; 32]);
        assert_eq!(nonce.payload_type, IkePayloadType::Nonce);
        assert_eq!(nonce.body.len(), 32);

        let notify = build_notify_payload(
            NotifyProtocolId::None,
            &[],
            NOTIFY_IKEV2_FRAGMENTATION_SUPPORTED,
            &[],
        )
        .expect("notify payload");
        assert_eq!(notify.payload_type, IkePayloadType::Notify);
        assert_eq!(notify.body, vec![0, 0, 0x40, 0x2e]);
    }

    #[test]
    fn builds_auth_init_payloads_from_public_ikev2_shapes() {
        let idi = build_identification_initiator_payload(
            "0234331234567890@nai.epc.mnc033.mcc234.3gppnetwork.org",
        );
        assert_eq!(idi.payload_type, IkePayloadType::IdentificationInitiator);
        assert_eq!(idi.body[0], IKE_ID_RFC822_ADDR);

        let cp = build_configuration_request_payload("ipv4v6");
        assert_eq!(cp.payload_type, IkePayloadType::Configuration);
        assert_eq!(cp.body[0], CFG_REQUEST);
        assert!(cp
            .body
            .windows(2)
            .any(|pair| { pair == CFG_ATTR_INTERNAL_IP4_ADDRESS.to_be_bytes() }));
        assert!(cp
            .body
            .windows(2)
            .any(|pair| { pair == CFG_ATTR_INTERNAL_IP6_ADDRESS.to_be_bytes() }));

        let proposal = child_sa_proposal_from_profile_string("aes128-sha256", 1, &[1, 2, 3, 4])
            .expect("child sa proposal");
        assert_eq!(proposal.protocol_id, IkeProtocolId::Esp);
        assert_eq!(proposal.transforms.len(), 3);
        assert_eq!(
            proposal.transforms[2].transform_type,
            TransformType::ExtendedSequenceNumbers
        );

        let tsi = build_traffic_selector_initiator_payload("ipv4v6");
        assert_eq!(tsi.payload_type, IkePayloadType::TrafficSelectorInitiator);
        assert_eq!(tsi.body[0], 2);
        assert_eq!(tsi.body[4], TS_IPV4_ADDR_RANGE);
        assert!(tsi.body.contains(&TS_IPV6_ADDR_RANGE));
    }

    #[test]
    fn builds_complete_ike_sa_init_message_from_clean_room_payloads() {
        let message = IkeMessage::new_request(
            0x1010_2020_3030_4040,
            IkeExchangeType::IkeSaInit,
            0,
            vec![
                build_sa_payload(&[ike_proposal_aes128_sha256_modp2048()]).expect("sa payload"),
                build_ke_payload(DH_MODP_2048, &[0x33; 256]),
                build_nonce_payload(&[0x44; 32]),
                build_notify_payload(
                    NotifyProtocolId::None,
                    &[],
                    NOTIFY_IKEV2_FRAGMENTATION_SUPPORTED,
                    &[],
                )
                .expect("notify payload"),
            ],
        );

        let decoded = IkeMessage::decode(&message.encode().expect("encode")).expect("decode");
        assert_eq!(decoded.payloads.len(), 4);
        assert_eq!(
            decoded.payloads[0].payload_type,
            IkePayloadType::SecurityAssociation
        );
        assert_eq!(
            decoded.payloads[1].payload_type,
            IkePayloadType::KeyExchange
        );
        assert_eq!(decoded.payloads[2].payload_type, IkePayloadType::Nonce);
        assert_eq!(decoded.payloads[3].payload_type, IkePayloadType::Notify);
    }

    #[test]
    fn rejects_invalid_proposal_inputs() {
        let empty = ProposalSpec {
            number: 1,
            protocol_id: IkeProtocolId::Ike,
            spi: Vec::new(),
            transforms: Vec::new(),
        };
        assert_eq!(
            build_sa_payload(&[empty]).unwrap_err(),
            PayloadBuildError::EmptyTransforms
        );

        assert_eq!(
            build_notify_payload(NotifyProtocolId::Esp, &[0u8; 300], 1, &[]).unwrap_err(),
            PayloadBuildError::SpiTooLarge(300)
        );

        assert_eq!(
            ike_proposal_from_profile_string("aes128-modp2048", 1).unwrap_err(),
            ProposalParseError::MissingIntegrity
        );
        assert_eq!(
            ike_proposal_from_profile_string("chacha20-sha256-modp2048", 1).unwrap_err(),
            ProposalParseError::UnsupportedToken("chacha20".to_string())
        );
        assert_eq!(
            parse_sa_payload(&[0, 0, 0, 7, 1, 1, 0]).unwrap_err(),
            SaParseError::TruncatedProposal
        );
    }

    #[test]
    fn serialized_specs_do_not_include_private_identity_or_key_material() {
        let payload =
            build_sa_payload(&[ike_proposal_aes128_sha256_modp2048()]).expect("build sa payload");
        let json = format!("{payload:?}");

        for forbidden in [
            "imsi", "iccid", "msisdn", "spi_in", "spi_out", "ck:", "ik:", "aka_key", "secret",
        ] {
            assert!(!json.to_ascii_lowercase().contains(forbidden));
        }
    }
}
