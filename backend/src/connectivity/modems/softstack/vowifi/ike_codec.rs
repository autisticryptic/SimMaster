#![allow(dead_code)]

use std::fmt;

pub const IKEV2_MAJOR_VERSION: u8 = 2;
pub const IKE_HEADER_LEN: usize = 28;
pub const GENERIC_PAYLOAD_HEADER_LEN: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IkePayloadType {
    NoNext = 0,
    SecurityAssociation = 33,
    KeyExchange = 34,
    IdentificationInitiator = 35,
    IdentificationResponder = 36,
    Certificate = 37,
    CertificateRequest = 38,
    Authentication = 39,
    Nonce = 40,
    Notify = 41,
    Delete = 42,
    VendorId = 43,
    TrafficSelectorInitiator = 44,
    TrafficSelectorResponder = 45,
    Encrypted = 46,
    Configuration = 47,
    ExtensibleAuthentication = 48,
    EncryptedFragment = 53,
    Unknown(u8),
}

impl IkePayloadType {
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::NoNext,
            33 => Self::SecurityAssociation,
            34 => Self::KeyExchange,
            35 => Self::IdentificationInitiator,
            36 => Self::IdentificationResponder,
            37 => Self::Certificate,
            38 => Self::CertificateRequest,
            39 => Self::Authentication,
            40 => Self::Nonce,
            41 => Self::Notify,
            42 => Self::Delete,
            43 => Self::VendorId,
            44 => Self::TrafficSelectorInitiator,
            45 => Self::TrafficSelectorResponder,
            46 => Self::Encrypted,
            47 => Self::Configuration,
            48 => Self::ExtensibleAuthentication,
            53 => Self::EncryptedFragment,
            other => Self::Unknown(other),
        }
    }

    pub fn as_u8(self) -> u8 {
        match self {
            Self::NoNext => 0,
            Self::SecurityAssociation => 33,
            Self::KeyExchange => 34,
            Self::IdentificationInitiator => 35,
            Self::IdentificationResponder => 36,
            Self::Certificate => 37,
            Self::CertificateRequest => 38,
            Self::Authentication => 39,
            Self::Nonce => 40,
            Self::Notify => 41,
            Self::Delete => 42,
            Self::VendorId => 43,
            Self::TrafficSelectorInitiator => 44,
            Self::TrafficSelectorResponder => 45,
            Self::Encrypted => 46,
            Self::Configuration => 47,
            Self::ExtensibleAuthentication => 48,
            Self::EncryptedFragment => 53,
            Self::Unknown(value) => value,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IkeExchangeType {
    IkeSaInit = 34,
    IkeAuth = 35,
    CreateChildSa = 36,
    Informational = 37,
    SessionResume = 38,
    Unknown(u8),
}

impl IkeExchangeType {
    pub fn from_u8(value: u8) -> Self {
        match value {
            34 => Self::IkeSaInit,
            35 => Self::IkeAuth,
            36 => Self::CreateChildSa,
            37 => Self::Informational,
            38 => Self::SessionResume,
            other => Self::Unknown(other),
        }
    }

    pub fn as_u8(self) -> u8 {
        match self {
            Self::IkeSaInit => 34,
            Self::IkeAuth => 35,
            Self::CreateChildSa => 36,
            Self::Informational => 37,
            Self::SessionResume => 38,
            Self::Unknown(value) => value,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IkeFlags {
    pub initiator: bool,
    pub response: bool,
    pub version: bool,
}

impl IkeFlags {
    pub fn request_from_initiator() -> Self {
        Self {
            initiator: true,
            response: false,
            version: false,
        }
    }

    pub fn from_u8(value: u8) -> Self {
        Self {
            response: value & 0x20 != 0,
            version: value & 0x10 != 0,
            initiator: value & 0x08 != 0,
        }
    }

    pub fn as_u8(self) -> u8 {
        let mut value = 0u8;
        if self.response {
            value |= 0x20;
        }
        if self.version {
            value |= 0x10;
        }
        if self.initiator {
            value |= 0x08;
        }
        value
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IkeHeader {
    pub initiator_spi: u64,
    pub responder_spi: u64,
    pub next_payload: IkePayloadType,
    pub major_version: u8,
    pub minor_version: u8,
    pub exchange_type: IkeExchangeType,
    pub flags: IkeFlags,
    pub message_id: u32,
    pub length: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IkePayload {
    pub payload_type: IkePayloadType,
    pub critical: bool,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IkeMessage {
    pub header: IkeHeader,
    pub payloads: Vec<IkePayload>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IkeCodecError {
    TruncatedHeader,
    TruncatedPayload,
    InvalidMajorVersion(u8),
    InvalidMessageLength { declared: usize, actual: usize },
    InvalidPayloadLength { declared: usize },
    MessageTooLarge(usize),
}

impl fmt::Display for IkeCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedHeader => write!(f, "truncated IKE header"),
            Self::TruncatedPayload => write!(f, "truncated IKE payload"),
            Self::InvalidMajorVersion(version) => {
                write!(f, "invalid IKE major version {version}")
            }
            Self::InvalidMessageLength { declared, actual } => {
                write!(
                    f,
                    "invalid IKE message length declared={declared} actual={actual}"
                )
            }
            Self::InvalidPayloadLength { declared } => {
                write!(f, "invalid IKE payload length declared={declared}")
            }
            Self::MessageTooLarge(length) => write!(f, "IKE message too large: {length} bytes"),
        }
    }
}

impl std::error::Error for IkeCodecError {}

impl IkeMessage {
    pub fn new_request(
        initiator_spi: u64,
        exchange_type: IkeExchangeType,
        message_id: u32,
        payloads: Vec<IkePayload>,
    ) -> Self {
        let next_payload = payloads
            .first()
            .map(|payload| payload.payload_type)
            .unwrap_or(IkePayloadType::NoNext);
        Self {
            header: IkeHeader {
                initiator_spi,
                responder_spi: 0,
                next_payload,
                major_version: IKEV2_MAJOR_VERSION,
                minor_version: 0,
                exchange_type,
                flags: IkeFlags::request_from_initiator(),
                message_id,
                length: 0,
            },
            payloads,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, IkeCodecError> {
        let payload_len = self
            .payloads
            .iter()
            .map(|payload| GENERIC_PAYLOAD_HEADER_LEN + payload.body.len())
            .sum::<usize>();
        let total_len = IKE_HEADER_LEN + payload_len;
        if total_len > u32::MAX as usize {
            return Err(IkeCodecError::MessageTooLarge(total_len));
        }

        let mut out = Vec::with_capacity(total_len);
        out.extend_from_slice(&self.header.initiator_spi.to_be_bytes());
        out.extend_from_slice(&self.header.responder_spi.to_be_bytes());
        out.push(
            self.payloads
                .first()
                .map(|payload| payload.payload_type.as_u8())
                .unwrap_or(IkePayloadType::NoNext.as_u8()),
        );
        out.push((self.header.major_version << 4) | (self.header.minor_version & 0x0f));
        out.push(self.header.exchange_type.as_u8());
        out.push(self.header.flags.as_u8());
        out.extend_from_slice(&self.header.message_id.to_be_bytes());
        out.extend_from_slice(&(total_len as u32).to_be_bytes());

        for (index, payload) in self.payloads.iter().enumerate() {
            let next_payload = self
                .payloads
                .get(index + 1)
                .map(|next| next.payload_type)
                .unwrap_or(IkePayloadType::NoNext);
            let payload_len = GENERIC_PAYLOAD_HEADER_LEN + payload.body.len();
            if payload_len > u16::MAX as usize {
                return Err(IkeCodecError::MessageTooLarge(payload_len));
            }
            out.push(next_payload.as_u8());
            out.push(if payload.critical { 0x80 } else { 0 });
            out.extend_from_slice(&(payload_len as u16).to_be_bytes());
            out.extend_from_slice(&payload.body);
        }

        Ok(out)
    }

    pub fn decode(input: &[u8]) -> Result<Self, IkeCodecError> {
        if input.len() < IKE_HEADER_LEN {
            return Err(IkeCodecError::TruncatedHeader);
        }

        let initiator_spi = u64::from_be_bytes(input[0..8].try_into().expect("slice length"));
        let responder_spi = u64::from_be_bytes(input[8..16].try_into().expect("slice length"));
        let next_payload = IkePayloadType::from_u8(input[16]);
        let version = input[17];
        let major_version = version >> 4;
        let minor_version = version & 0x0f;
        if major_version != IKEV2_MAJOR_VERSION {
            return Err(IkeCodecError::InvalidMajorVersion(major_version));
        }
        let exchange_type = IkeExchangeType::from_u8(input[18]);
        let flags = IkeFlags::from_u8(input[19]);
        let message_id = u32::from_be_bytes(input[20..24].try_into().expect("slice length"));
        let length = u32::from_be_bytes(input[24..28].try_into().expect("slice length"));
        let declared = length as usize;
        if declared != input.len() {
            return Err(IkeCodecError::InvalidMessageLength {
                declared,
                actual: input.len(),
            });
        }

        let mut payloads = Vec::new();
        let mut offset = IKE_HEADER_LEN;
        let mut current_type = next_payload;
        while current_type != IkePayloadType::NoNext {
            if input.len().saturating_sub(offset) < GENERIC_PAYLOAD_HEADER_LEN {
                return Err(IkeCodecError::TruncatedPayload);
            }

            let following_type = IkePayloadType::from_u8(input[offset]);
            let flags = input[offset + 1];
            let critical = flags & 0x80 != 0;
            let payload_len = u16::from_be_bytes(
                input[offset + 2..offset + 4]
                    .try_into()
                    .expect("slice length"),
            ) as usize;
            if payload_len < GENERIC_PAYLOAD_HEADER_LEN {
                return Err(IkeCodecError::InvalidPayloadLength {
                    declared: payload_len,
                });
            }
            let end = offset + payload_len;
            if end > input.len() {
                return Err(IkeCodecError::TruncatedPayload);
            }

            payloads.push(IkePayload {
                payload_type: current_type,
                critical,
                body: input[offset + GENERIC_PAYLOAD_HEADER_LEN..end].to_vec(),
            });

            offset = end;
            current_type = following_type;
        }

        if offset != input.len() {
            return Err(IkeCodecError::InvalidMessageLength {
                declared: offset,
                actual: input.len(),
            });
        }

        Ok(Self {
            header: IkeHeader {
                initiator_spi,
                responder_spi,
                next_payload,
                major_version,
                minor_version,
                exchange_type,
                flags,
                message_id,
                length,
            },
            payloads,
        })
    }
}

impl IkePayload {
    pub fn security_association(body: Vec<u8>) -> Self {
        Self {
            payload_type: IkePayloadType::SecurityAssociation,
            critical: false,
            body,
        }
    }

    pub fn key_exchange(body: Vec<u8>) -> Self {
        Self {
            payload_type: IkePayloadType::KeyExchange,
            critical: false,
            body,
        }
    }

    pub fn nonce(body: Vec<u8>) -> Self {
        Self {
            payload_type: IkePayloadType::Nonce,
            critical: false,
            body,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_init_message() -> IkeMessage {
        IkeMessage::new_request(
            0x0102_0304_0506_0708,
            IkeExchangeType::IkeSaInit,
            0,
            vec![
                IkePayload::security_association(vec![0x01, 0x02, 0x03, 0x04]),
                IkePayload::key_exchange(vec![0x05, 0x06, 0x07]),
                IkePayload::nonce(vec![0x08; 32]),
            ],
        )
    }

    #[test]
    fn encodes_and_decodes_ike_sa_init_header_and_payload_chain() {
        let message = sample_init_message();
        let encoded = message.encode().expect("encode message");

        assert_eq!(encoded.len(), IKE_HEADER_LEN + 4 + 4 + 4 + 3 + 4 + 32);
        assert_eq!(encoded[16], IkePayloadType::SecurityAssociation.as_u8());
        assert_eq!(encoded[17], 0x20);
        assert_eq!(encoded[18], IkeExchangeType::IkeSaInit.as_u8());
        assert_eq!(encoded[19], 0x08);

        let decoded = IkeMessage::decode(&encoded).expect("decode message");
        assert_eq!(decoded.header.initiator_spi, 0x0102_0304_0506_0708);
        assert_eq!(decoded.header.responder_spi, 0);
        assert_eq!(decoded.header.major_version, 2);
        assert_eq!(decoded.header.exchange_type, IkeExchangeType::IkeSaInit);
        assert_eq!(decoded.header.flags, IkeFlags::request_from_initiator());
        assert_eq!(decoded.payloads.len(), 3);
        assert_eq!(
            decoded.payloads[0].payload_type,
            IkePayloadType::SecurityAssociation
        );
        assert_eq!(
            decoded.payloads[1].payload_type,
            IkePayloadType::KeyExchange
        );
        assert_eq!(decoded.payloads[2].payload_type, IkePayloadType::Nonce);
        assert_eq!(decoded.payloads[2].body, vec![0x08; 32]);
    }

    #[test]
    fn rejects_truncated_header_and_payload() {
        assert_eq!(
            IkeMessage::decode(&[0u8; IKE_HEADER_LEN - 1]).unwrap_err(),
            IkeCodecError::TruncatedHeader
        );

        let mut encoded = sample_init_message().encode().expect("encode message");
        encoded.truncate(encoded.len() - 2);
        let actual_len = encoded.len();
        encoded[24..28].copy_from_slice(&(actual_len as u32).to_be_bytes());
        assert_eq!(
            IkeMessage::decode(&encoded).unwrap_err(),
            IkeCodecError::TruncatedPayload
        );
    }

    #[test]
    fn rejects_invalid_major_version_and_length_mismatch() {
        let mut encoded = sample_init_message().encode().expect("encode message");
        encoded[17] = 0x10;
        assert_eq!(
            IkeMessage::decode(&encoded).unwrap_err(),
            IkeCodecError::InvalidMajorVersion(1)
        );

        let mut encoded = sample_init_message().encode().expect("encode message");
        encoded[27] = encoded[27].saturating_add(1);
        assert!(matches!(
            IkeMessage::decode(&encoded).unwrap_err(),
            IkeCodecError::InvalidMessageLength { .. }
        ));
    }

    #[test]
    fn preserves_unknown_payload_type_without_interpreting_private_data() {
        let message = IkeMessage::new_request(
            0x1111,
            IkeExchangeType::IkeAuth,
            1,
            vec![IkePayload {
                payload_type: IkePayloadType::Unknown(201),
                critical: true,
                body: vec![0xaa, 0xbb],
            }],
        );

        let decoded = IkeMessage::decode(&message.encode().expect("encode")).expect("decode");
        assert_eq!(
            decoded.payloads[0].payload_type,
            IkePayloadType::Unknown(201)
        );
        assert!(decoded.payloads[0].critical);
        assert_eq!(decoded.payloads[0].body, vec![0xaa, 0xbb]);
    }
}
