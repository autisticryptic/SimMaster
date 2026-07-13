#![allow(dead_code)]

use std::fmt;

use num_bigint::BigUint;
use num_traits::{One, Zero};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Serialize;

pub const MODP_1024_PUBLIC_VALUE_BYTES: usize = 128;
pub const MODP_2048_PUBLIC_VALUE_BYTES: usize = 256;
const MODP_1024_PRIVATE_BYTES: usize = 128;
const MODP_2048_PRIVATE_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DhPublicSummary {
    pub group: &'static str,
    pub public_value_bytes: usize,
    pub ephemeral_material_present: bool,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DhGroup {
    Modp1024,
    Modp2048,
}

impl DhGroup {
    pub fn from_transform_id(transform_id: u16) -> Option<Self> {
        match transform_id {
            super::ike_payloads::DH_MODP_1024 => Some(Self::Modp1024),
            super::ike_payloads::DH_MODP_2048 => Some(Self::Modp2048),
            _ => None,
        }
    }

    pub fn transform_id(self) -> u16 {
        match self {
            Self::Modp1024 => super::ike_payloads::DH_MODP_1024,
            Self::Modp2048 => super::ike_payloads::DH_MODP_2048,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Modp1024 => "modp1024",
            Self::Modp2048 => "modp2048",
        }
    }

    fn public_value_bytes(self) -> usize {
        match self {
            Self::Modp1024 => MODP_1024_PUBLIC_VALUE_BYTES,
            Self::Modp2048 => MODP_2048_PUBLIC_VALUE_BYTES,
        }
    }

    fn private_bytes(self) -> usize {
        match self {
            Self::Modp1024 => MODP_1024_PRIVATE_BYTES,
            Self::Modp2048 => MODP_2048_PRIVATE_BYTES,
        }
    }

    fn prime(self) -> BigUint {
        match self {
            Self::Modp1024 => modp_1024_prime(),
            Self::Modp2048 => modp_2048_prime(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Modp2048Ephemeral {
    group: DhGroup,
    private_value: BigUint,
    public_value: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DhError {
    RandomFailed,
    InvalidPrivateValue,
    InvalidPeerPublicValue,
}

impl fmt::Display for DhError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RandomFailed => write!(f, "DH random generation failed"),
            Self::InvalidPrivateValue => write!(f, "DH private value is invalid"),
            Self::InvalidPeerPublicValue => write!(f, "DH peer public value is invalid"),
        }
    }
}

impl std::error::Error for DhError {}

impl Modp2048Ephemeral {
    pub fn generate() -> Result<Self, DhError> {
        Self::generate_for_group(DhGroup::Modp2048)
    }

    pub fn generate_for_group(group: DhGroup) -> Result<Self, DhError> {
        let rng = SystemRandom::new();
        let mut private_bytes = vec![0u8; group.private_bytes()];
        rng.fill(&mut private_bytes)
            .map_err(|_| DhError::RandomFailed)?;
        Self::from_private_bytes_for_group(group, &private_bytes)
    }

    pub fn from_private_bytes(private_bytes: &[u8]) -> Result<Self, DhError> {
        Self::from_private_bytes_for_group(DhGroup::Modp2048, private_bytes)
    }

    pub fn from_private_bytes_for_group(
        group: DhGroup,
        private_bytes: &[u8],
    ) -> Result<Self, DhError> {
        let modulus = group.prime();
        let one = BigUint::one();
        let max_private = &modulus - &one;
        let private_value = (BigUint::from_bytes_be(private_bytes) % &max_private) + &one;
        if private_value.is_zero() || private_value >= modulus {
            return Err(DhError::InvalidPrivateValue);
        }

        let generator = BigUint::from(2u8);
        let public_value = left_pad_to_len(
            generator.modpow(&private_value, &modulus).to_bytes_be(),
            group.public_value_bytes(),
        );

        Ok(Self {
            group,
            private_value,
            public_value,
        })
    }

    pub fn public_value(&self) -> &[u8] {
        &self.public_value
    }

    pub fn summary(&self) -> DhPublicSummary {
        DhPublicSummary {
            group: self.group.as_str(),
            public_value_bytes: self.public_value.len(),
            ephemeral_material_present: true,
            sensitive_values_policy: "ephemeral_dh_values_not_serialized",
        }
    }

    pub fn shared_secret(&self, peer_public_value: &[u8]) -> Result<Vec<u8>, DhError> {
        if peer_public_value.is_empty() || peer_public_value.len() > self.group.public_value_bytes()
        {
            return Err(DhError::InvalidPeerPublicValue);
        }

        let modulus = self.group.prime();
        let peer = BigUint::from_bytes_be(peer_public_value);
        let one = BigUint::one();
        if peer <= one || peer >= (&modulus - &one) {
            return Err(DhError::InvalidPeerPublicValue);
        }

        Ok(left_pad_to_len(
            peer.modpow(&self.private_value, &modulus).to_bytes_be(),
            self.group.public_value_bytes(),
        ))
    }
}

fn left_pad_to_len(mut value: Vec<u8>, len: usize) -> Vec<u8> {
    if value.len() > len {
        value.split_off(value.len() - len)
    } else if value.len() < len {
        let mut padded = vec![0u8; len - value.len()];
        padded.extend_from_slice(&value);
        padded
    } else {
        value
    }
}

fn modp_2048_prime() -> BigUint {
    BigUint::parse_bytes(MODP_2048_PRIME_HEX.as_bytes(), 16).expect("static MODP 2048 prime")
}

fn modp_1024_prime() -> BigUint {
    BigUint::parse_bytes(MODP_1024_PRIME_HEX.as_bytes(), 16).expect("static MODP 1024 prime")
}

const MODP_1024_PRIME_HEX: &str = concat!(
    "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E08",
    "8A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD",
    "3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E",
    "7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899F",
    "A5AE9F24117C4B1FE649286651ECE65381FFFFFFFFFFFFFFFF",
);

const MODP_2048_PRIME_HEX: &str = concat!(
    "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E08",
    "8A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD",
    "3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E",
    "7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899F",
    "A5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF05",
    "98DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C",
    "62F356208552BB9ED529077096966D670C354E4ABC9804F174",
    "6C08CA18217C32905E462E36CE3BE39E772C180E86039B2783",
    "A2EC07A28FB5C55DF06F4C52C9DE2BCBF6955817183995497C",
    "EA956AE515D2261898FA051015728E5A8AACAA68FFFFFFFFFFFFFFFF",
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modp_2048_generates_fixed_width_public_value_without_serializing_private_material() {
        let dh = Modp2048Ephemeral::from_private_bytes(&[0x11; MODP_2048_PRIVATE_BYTES])
            .expect("build deterministic dh");

        assert_eq!(dh.public_value().len(), MODP_2048_PUBLIC_VALUE_BYTES);
        let summary = dh.summary();
        assert!(summary.ephemeral_material_present);

        let json = serde_json::to_string(&summary).expect("serialize summary");
        for forbidden in [
            "private_value",
            "shared_secret",
            "key_material",
            "payload",
            "spi",
        ] {
            assert!(!json.to_ascii_lowercase().contains(forbidden));
        }
    }

    #[test]
    fn modp_2048_derives_matching_shared_secret() {
        let left = Modp2048Ephemeral::from_private_bytes(&[0x22; MODP_2048_PRIVATE_BYTES])
            .expect("left dh");
        let right = Modp2048Ephemeral::from_private_bytes(&[0x33; MODP_2048_PRIVATE_BYTES])
            .expect("right dh");

        let left_secret = left
            .shared_secret(right.public_value())
            .expect("left shared secret");
        let right_secret = right
            .shared_secret(left.public_value())
            .expect("right shared secret");

        assert_eq!(left_secret, right_secret);
        assert_eq!(left_secret.len(), MODP_2048_PUBLIC_VALUE_BYTES);
    }

    #[test]
    fn modp_1024_derives_matching_shared_secret() {
        let left = Modp2048Ephemeral::from_private_bytes_for_group(
            DhGroup::Modp1024,
            &[0x22; MODP_1024_PRIVATE_BYTES],
        )
        .expect("left dh");
        let right = Modp2048Ephemeral::from_private_bytes_for_group(
            DhGroup::Modp1024,
            &[0x33; MODP_1024_PRIVATE_BYTES],
        )
        .expect("right dh");

        let left_secret = left
            .shared_secret(right.public_value())
            .expect("left shared secret");
        let right_secret = right
            .shared_secret(left.public_value())
            .expect("right shared secret");

        assert_eq!(left.summary().group, "modp1024");
        assert_eq!(left.public_value().len(), MODP_1024_PUBLIC_VALUE_BYTES);
        assert_eq!(left_secret, right_secret);
        assert_eq!(left_secret.len(), MODP_1024_PUBLIC_VALUE_BYTES);
    }

    #[test]
    fn rejects_invalid_peer_public_values() {
        let dh =
            Modp2048Ephemeral::from_private_bytes(&[0x44; MODP_2048_PRIVATE_BYTES]).expect("dh");

        assert_eq!(
            dh.shared_secret(&[]).unwrap_err(),
            DhError::InvalidPeerPublicValue
        );
        assert_eq!(
            dh.shared_secret(&[0; MODP_2048_PUBLIC_VALUE_BYTES])
                .unwrap_err(),
            DhError::InvalidPeerPublicValue
        );
    }
}
