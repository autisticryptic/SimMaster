#![allow(dead_code)]

use std::fmt;

use num_bigint::BigUint;
use num_traits::{One, Zero};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Serialize;

pub const MODP_768_PUBLIC_VALUE_BYTES: usize = 96;
pub const MODP_1024_PUBLIC_VALUE_BYTES: usize = 128;
pub const MODP_1536_PUBLIC_VALUE_BYTES: usize = 192;
pub const MODP_2048_PUBLIC_VALUE_BYTES: usize = 256;
pub const MODP_3072_PUBLIC_VALUE_BYTES: usize = 384;
pub const MODP_4096_PUBLIC_VALUE_BYTES: usize = 512;
pub const MODP_8192_PUBLIC_VALUE_BYTES: usize = 1024;
const MODP_768_PRIVATE_BYTES: usize = 96;
const MODP_1024_PRIVATE_BYTES: usize = 128;
const MODP_1536_PRIVATE_BYTES: usize = 192;
const MODP_2048_PRIVATE_BYTES: usize = 256;
const MODP_3072_PRIVATE_BYTES: usize = 384;
const MODP_4096_PRIVATE_BYTES: usize = 512;
const MODP_8192_PRIVATE_BYTES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DhPublicSummary {
    pub group: &'static str,
    pub public_value_bytes: usize,
    pub ephemeral_material_present: bool,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DhGroup {
    Modp768,
    Modp1024,
    Modp1536,
    Modp2048,
    Modp3072,
    Modp4096,
    Modp8192,
}

impl DhGroup {
    pub fn from_transform_id(transform_id: u16) -> Option<Self> {
        match transform_id {
            super::ike_payloads::DH_MODP_768 => Some(Self::Modp768),
            super::ike_payloads::DH_MODP_1024 => Some(Self::Modp1024),
            super::ike_payloads::DH_MODP_1536 => Some(Self::Modp1536),
            super::ike_payloads::DH_MODP_2048 => Some(Self::Modp2048),
            super::ike_payloads::DH_MODP_3072 => Some(Self::Modp3072),
            super::ike_payloads::DH_MODP_4096 => Some(Self::Modp4096),
            super::ike_payloads::DH_MODP_8192 => Some(Self::Modp8192),
            _ => None,
        }
    }

    pub fn transform_id(self) -> u16 {
        match self {
            Self::Modp768 => super::ike_payloads::DH_MODP_768,
            Self::Modp1024 => super::ike_payloads::DH_MODP_1024,
            Self::Modp1536 => super::ike_payloads::DH_MODP_1536,
            Self::Modp2048 => super::ike_payloads::DH_MODP_2048,
            Self::Modp3072 => super::ike_payloads::DH_MODP_3072,
            Self::Modp4096 => super::ike_payloads::DH_MODP_4096,
            Self::Modp8192 => super::ike_payloads::DH_MODP_8192,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Modp768 => "modp768",
            Self::Modp1024 => "modp1024",
            Self::Modp1536 => "modp1536",
            Self::Modp2048 => "modp2048",
            Self::Modp3072 => "modp3072",
            Self::Modp4096 => "modp4096",
            Self::Modp8192 => "modp8192",
        }
    }

    fn public_value_bytes(self) -> usize {
        match self {
            Self::Modp768 => MODP_768_PUBLIC_VALUE_BYTES,
            Self::Modp1024 => MODP_1024_PUBLIC_VALUE_BYTES,
            Self::Modp1536 => MODP_1536_PUBLIC_VALUE_BYTES,
            Self::Modp2048 => MODP_2048_PUBLIC_VALUE_BYTES,
            Self::Modp3072 => MODP_3072_PUBLIC_VALUE_BYTES,
            Self::Modp4096 => MODP_4096_PUBLIC_VALUE_BYTES,
            Self::Modp8192 => MODP_8192_PUBLIC_VALUE_BYTES,
        }
    }

    fn private_bytes(self) -> usize {
        match self {
            Self::Modp768 => MODP_768_PRIVATE_BYTES,
            Self::Modp1024 => MODP_1024_PRIVATE_BYTES,
            Self::Modp1536 => MODP_1536_PRIVATE_BYTES,
            Self::Modp2048 => MODP_2048_PRIVATE_BYTES,
            Self::Modp3072 => MODP_3072_PRIVATE_BYTES,
            Self::Modp4096 => MODP_4096_PRIVATE_BYTES,
            Self::Modp8192 => MODP_8192_PRIVATE_BYTES,
        }
    }

    fn prime(self) -> BigUint {
        match self {
            Self::Modp768 => modp_768_prime(),
            Self::Modp1024 => modp_1024_prime(),
            Self::Modp1536 => modp_1536_prime(),
            Self::Modp2048 => modp_2048_prime(),
            Self::Modp3072 => modp_3072_prime(),
            Self::Modp4096 => modp_4096_prime(),
            Self::Modp8192 => modp_8192_prime(),
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

fn modp_768_prime() -> BigUint {
    BigUint::parse_bytes(MODP_768_PRIME_HEX.as_bytes(), 16).expect("static MODP 768 prime")
}

fn modp_1536_prime() -> BigUint {
    BigUint::parse_bytes(MODP_1536_PRIME_HEX.as_bytes(), 16).expect("static MODP 1536 prime")
}

fn modp_3072_prime() -> BigUint {
    BigUint::parse_bytes(MODP_3072_PRIME_HEX.as_bytes(), 16).expect("static MODP 3072 prime")
}

fn modp_4096_prime() -> BigUint {
    BigUint::parse_bytes(MODP_4096_PRIME_HEX.as_bytes(), 16).expect("static MODP 4096 prime")
}

fn modp_8192_prime() -> BigUint {
    BigUint::parse_bytes(MODP_8192_PRIME_HEX.as_bytes(), 16).expect("static MODP 8192 prime")
}

const MODP_768_PRIME_HEX: &str = concat!(
    "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74",
    "020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F1437",
    "4FE1356D6D51C245E485B576625E7EC6F44C42E9A63A3620FFFFFFFFFFFFFFFF",
);

const MODP_1536_PRIME_HEX: &str = concat!(
    "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74",
    "020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F1437",
    "4FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7ED",
    "EE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF05",
    "98DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB",
    "9ED529077096966D670C354E4ABC9804F1746C08CA237327FFFFFFFFFFFFFFFF",
);

const MODP_3072_PRIME_HEX: &str = concat!(
    "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74",
    "020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F1437",
    "4FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7ED",
    "EE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF05",
    "98DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB",
    "9ED529077096966D670C354E4ABC9804F1746C08CA18217C32905E462E36CE3B",
    "E39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCBF695581718",
    "3995497CEA956AE515D2261898FA051015728E5A8AAAC42DAD33170D04507A33",
    "A85521ABDF1CBA64ECFB850458DBEF0A8AEA71575D060C7DB3970F85A6E1E4C7",
    "ABF5AE8CDB0933D71E8C94E04A25619DCEE3D2261AD2EE6BF12FFA06D98A0864",
    "D87602733EC86A64521F2B18177B200CBBE117577A615D6C770988C0BAD946E2",
    "08E24FA074E5AB3143DB5BFCE0FD108E4B82D120A93AD2CAFFFFFFFFFFFFFFFF",
);

const MODP_4096_PRIME_HEX: &str = concat!(
    "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74",
    "020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F1437",
    "4FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7ED",
    "EE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF05",
    "98DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB",
    "9ED529077096966D670C354E4ABC9804F1746C08CA18217C32905E462E36CE3B",
    "E39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCBF695581718",
    "3995497CEA956AE515D2261898FA051015728E5A8AAAC42DAD33170D04507A33",
    "A85521ABDF1CBA64ECFB850458DBEF0A8AEA71575D060C7DB3970F85A6E1E4C7",
    "ABF5AE8CDB0933D71E8C94E04A25619DCEE3D2261AD2EE6BF12FFA06D98A0864",
    "D87602733EC86A64521F2B18177B200CBBE117577A615D6C770988C0BAD946E2",
    "08E24FA074E5AB3143DB5BFCE0FD108E4B82D120A92108011A723C12A787E6D7",
    "88719A10BDBA5B2699C327186AF4E23C1A946834B6150BDA2583E9CA2AD44CE8",
    "DBBBC2DB04DE8EF92E8EFC141FBECAA6287C59474E6BC05D99B2964FA090C3A2",
    "233BA186515BE7ED1F612970CEE2D7AFB81BDD762170481CD0069127D5B05AA9",
    "93B4EA988D8FDDC186FFB7DC90A6C08F4DF435C934063199FFFFFFFFFFFFFFFF",
);

const MODP_8192_PRIME_HEX: &str = concat!(
    "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74",
    "020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F1437",
    "4FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7ED",
    "EE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF05",
    "98DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB",
    "9ED529077096966D670C354E4ABC9804F1746C08CA18217C32905E462E36CE3B",
    "E39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCBF695581718",
    "3995497CEA956AE515D2261898FA051015728E5A8AAAC42DAD33170D04507A33",
    "A85521ABDF1CBA64ECFB850458DBEF0A8AEA71575D060C7DB3970F85A6E1E4C7",
    "ABF5AE8CDB0933D71E8C94E04A25619DCEE3D2261AD2EE6BF12FFA06D98A0864",
    "D87602733EC86A64521F2B18177B200CBBE117577A615D6C770988C0BAD946E2",
    "08E24FA074E5AB3143DB5BFCE0FD108E4B82D120A92108011A723C12A787E6D7",
    "88719A10BDBA5B2699C327186AF4E23C1A946834B6150BDA2583E9CA2AD44CE8",
    "DBBBC2DB04DE8EF92E8EFC141FBECAA6287C59474E6BC05D99B2964FA090C3A2",
    "233BA186515BE7ED1F612970CEE2D7AFB81BDD762170481CD0069127D5B05AA9",
    "93B4EA988D8FDDC186FFB7DC90A6C08F4DF435C93402849236C3FAB4D27C7026",
    "C1D4DCB2602646DEC9751E763DBA37BDF8FF9406AD9E530EE5DB382F413001AE",
    "B06A53ED9027D831179727B0865A8918DA3EDBEBCF9B14ED44CE6CBACED4BB1B",
    "DB7F1447E6CC254B332051512BD7AF426FB8F401378CD2BF5983CA01C64B92EC",
    "F032EA15D1721D03F482D7CE6E74FEF6D55E702F46980C82B5A84031900B1C9E",
    "59E7C97FBEC7E8F323A97A7E36CC88BE0F1D45B7FF585AC54BD407B22B4154AA",
    "CC8F6D7EBF48E1D814CC5ED20F8037E0A79715EEF29BE32806A1D58BB7C5DA76",
    "F550AA3D8A1FBFF0EB19CCB1A313D55CDA56C9EC2EF29632387FE8D76E3C0468",
    "043E8F663F4860EE12BF2D5B0B7474D6E694F91E6DBE115974A3926F12FEE5E4",
    "38777CB6A932DF8CD8BEC4D073B931BA3BC832B68D9DD300741FA7BF8AFC47ED",
    "2576F6936BA424663AAB639C5AE4F5683423B4742BF1C978238F16CBE39D652D",
    "E3FDB8BEFC848AD922222E04A4037C0713EB57A81A23F0C73473FC646CEA306B",
    "4BCBC8862F8385DDFA9D4B7FA2C087E879683303ED5BDD3A062B3CF5B3A278A6",
    "6D2A13F83F44F82DDF310EE074AB6A364597E899A0255DC164F31CC50846851D",
    "F9AB48195DED7EA1B1D510BD7EE74D73FAF36BC31ECFA268359046F4EB879F92",
    "4009438B481C6CD7889A002ED5EE382BC9190DA6FC026E479558E4475677E9AA",
    "9E3050E2765694DFC81F56E880B96E7160C980DD98EDD3DFFFFFFFFFFFFFFFFF",
);

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
    fn extended_modp_groups_derive_matching_shared_secrets() {
        for (group, public_bytes, private_bytes) in [
            (
                DhGroup::Modp768,
                MODP_768_PUBLIC_VALUE_BYTES,
                MODP_768_PRIVATE_BYTES,
            ),
            (
                DhGroup::Modp1536,
                MODP_1536_PUBLIC_VALUE_BYTES,
                MODP_1536_PRIVATE_BYTES,
            ),
            (
                DhGroup::Modp3072,
                MODP_3072_PUBLIC_VALUE_BYTES,
                MODP_3072_PRIVATE_BYTES,
            ),
            (
                DhGroup::Modp4096,
                MODP_4096_PUBLIC_VALUE_BYTES,
                MODP_4096_PRIVATE_BYTES,
            ),
            (
                DhGroup::Modp8192,
                MODP_8192_PUBLIC_VALUE_BYTES,
                MODP_8192_PRIVATE_BYTES,
            ),
        ] {
            let left_bytes = vec![0x22u8; private_bytes];
            let right_bytes = vec![0x33u8; private_bytes];
            let left = Modp2048Ephemeral::from_private_bytes_for_group(group, &left_bytes)
                .expect("left dh");
            let right = Modp2048Ephemeral::from_private_bytes_for_group(group, &right_bytes)
                .expect("right dh");

            let left_secret = left
                .shared_secret(right.public_value())
                .expect("left shared secret");
            let right_secret = right
                .shared_secret(left.public_value())
                .expect("right shared secret");

            assert_eq!(left.summary().group, group.as_str());
            assert_eq!(left.public_value().len(), public_bytes);
            assert_eq!(left_secret, right_secret);
            assert_eq!(left_secret.len(), public_bytes);
        }
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
