//! Recovered legacy ENC configuration wrapping, NOT credential authentication.
//!
//! Native Pbe is salt8 || DES-CBC/PKCS5, encoded as newline-free Base64. Key/IV
//! come from1000 MD5 rounds over password||salt then16-byte digest. This weak,
//! unauthenticated legacy format is only for matching imported configuration.
//! No SDK/default key is embedded, no TLS/SIP cipher policy is weakened, and a
//! decoded value never establishes trust, hosting, billing or channel readiness.
use super::rpc_stream::MAX_FIELD_BYTES;
use base64::{engine::general_purpose::STANDARD, Engine};
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use zeroize::Zeroizing;
use std::fmt;

const MAX_KEY_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError { TooLarge, InvalidKey, KeyRequired, MalformedEnvelope, MultipleEnvelopes, DecodeFailed }
impl ConfigError {
    pub fn code(self) -> &'static str {
        match self {
            Self::TooLarge => "jegotrip_config_too_large", Self::InvalidKey => "jegotrip_config_decode_key_invalid",
            Self::KeyRequired => "jegotrip_config_decode_key_required", Self::MalformedEnvelope => "jegotrip_config_envelope_invalid",
            Self::MultipleEnvelopes => "jegotrip_config_envelope_multiple", Self::DecodeFailed => "jegotrip_config_decode_failed",
        }
    }
}

/// Explicitly supplied for the original deployment context. Client-property
/// and Common-reference keys are separate; never substitute pwd/token/SipPwd.
pub struct ConfigDecodeKey(Zeroizing<Vec<u8>>);
impl ConfigDecodeKey {
    pub fn supplied(bytes: &[u8]) -> Result<Self, ConfigError> {
        if bytes.is_empty() || bytes.len() > MAX_KEY_BYTES { return Err(ConfigError::InvalidKey); }
        Ok(Self(Zeroizing::new(bytes.to_vec())))
    }
}
impl fmt::Debug for ConfigDecodeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ConfigDecodeKey(<redacted>)") }
}

pub struct ConfigurationBytes(Zeroizing<Vec<u8>>);
impl ConfigurationBytes {
    pub fn expose_to_backend(&self) -> &[u8] { &self.0 }
}
impl fmt::Debug for ConfigurationBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigurationBytes").field("byte_length", &self.0.len()).field("data", &"<redacted>").finish()
    }
}

fn derived(password: &[u8], salt: &[u8; 8]) -> Zeroizing<[u8; 16]> {
    let mut material = Zeroizing::new(Vec::with_capacity(password.len() + salt.len()));
    material.extend_from_slice(password); material.extend_from_slice(salt);
    let mut digest = Zeroizing::new(md5::compute(&material[..]).0);
    for _ in 0..999 { *digest = md5::compute(&digest[..]).0; }
    digest
}
fn odd_parity(key: &mut [u8]) {
    for byte in key { let high = *byte & 0xfe; *byte = high | u8::from(high.count_ones() % 2 == 0); }
}

// Public DES weak/semiweak algorithm keys, not deployment secrets. Native calls
// DES_set_key_checked but only checks -1, not -2. Do not copy that undefined-key
// schedule branch; the Linux safety subset rejects all weak schedules.
const WEAK_KEYS: [[u8; 8]; 16] = [
    [0x01; 8], [0xfe; 8], [0x1f,0x1f,0x1f,0x1f,0x0e,0x0e,0x0e,0x0e], [0xe0,0xe0,0xe0,0xe0,0xf1,0xf1,0xf1,0xf1],
    [0x01,0xfe,0x01,0xfe,0x01,0xfe,0x01,0xfe], [0xfe,0x01,0xfe,0x01,0xfe,0x01,0xfe,0x01],
    [0x1f,0xe0,0x1f,0xe0,0x0e,0xf1,0x0e,0xf1], [0xe0,0x1f,0xe0,0x1f,0xf1,0x0e,0xf1,0x0e],
    [0x01,0xe0,0x01,0xe0,0x01,0xf1,0x01,0xf1], [0xe0,0x01,0xe0,0x01,0xf1,0x01,0xf1,0x01],
    [0x1f,0xfe,0x1f,0xfe,0x0e,0xfe,0x0e,0xfe], [0xfe,0x1f,0xfe,0x1f,0xfe,0x0e,0xfe,0x0e],
    [0x01,0x1f,0x01,0x1f,0x01,0x0e,0x01,0x0e], [0x1f,0x01,0x1f,0x01,0x0e,0x01,0x0e,0x01],
    [0xe0,0xfe,0xe0,0xfe,0xf1,0xfe,0xf1,0xfe], [0xfe,0xe0,0xfe,0xe0,0xfe,0xf1,0xfe,0xf1],
];

fn unwrap_payload(encoded: &[u8], key: &ConfigDecodeKey) -> Result<Zeroizing<Vec<u8>>, ConfigError> {
    let mut raw = Zeroizing::new(STANDARD.decode(encoded).map_err(|_| ConfigError::DecodeFailed)?);
    if raw.len() < 16 || raw.len() % 8 != 0 || raw.len() > MAX_FIELD_BYTES { return Err(ConfigError::DecodeFailed); }
    let salt: [u8; 8] = raw[..8].try_into().map_err(|_| ConfigError::DecodeFailed)?;
    let mut digest = derived(&key.0, &salt); odd_parity(&mut digest[..8]);
    if WEAK_KEYS.iter().any(|weak| digest[..8] == weak[..]) { return Err(ConfigError::DecodeFailed); }
    let decryptor = cbc::Decryptor::<des::Des>::new_from_slices(&digest[..8], &digest[8..]).map_err(|_| ConfigError::DecodeFailed)?;
    // The in-place CBC/PKCS5 path validates ALL pad bytes before releasing any
    // plaintext. The temporary ciphertext/plaintext and schedule are zeroized.
    let plaintext = decryptor.decrypt_padded_mut::<Pkcs7>(&mut raw[8..]).map_err(|_| ConfigError::DecodeFailed)?;
    Ok(Zeroizing::new(plaintext.to_vec()))
}

/// Native replaces one ENC(...) embedded substring and preserves its exact
/// prefix/suffix. This adapter rejects malformed/repeated/nested wrappers rather
/// than native's silent encrypted fallback, and never recursively decodes.
pub fn decode_configuration(input: &[u8], key: Option<&ConfigDecodeKey>) -> Result<ConfigurationBytes, ConfigError> {
    if input.len() > MAX_FIELD_BYTES { return Err(ConfigError::TooLarge); }
    let Some(start) = input.windows(4).position(|bytes| bytes == b"ENC(") else {
        return Ok(ConfigurationBytes(Zeroizing::new(input.to_vec())));
    };
    let rest = &input[start + 4..];
    let end = rest.iter().position(|&byte| byte == b')').ok_or(ConfigError::MalformedEnvelope)?;
    if input[start + 4..].windows(4).any(|bytes| bytes == b"ENC(") { return Err(ConfigError::MultipleEnvelopes); }
    let key = key.ok_or(ConfigError::KeyRequired)?;
    let plaintext = unwrap_payload(&rest[..end], key)?;
    if plaintext.windows(4).any(|bytes| bytes == b"ENC(") { return Err(ConfigError::MultipleEnvelopes); }
    let suffix = &rest[end + 1..];
    let total = start.checked_add(plaintext.len()).and_then(|n| n.checked_add(suffix.len())).ok_or(ConfigError::TooLarge)?;
    if total > MAX_FIELD_BYTES { return Err(ConfigError::TooLarge); }
    let mut decoded = Zeroizing::new(Vec::with_capacity(total));
    decoded.extend_from_slice(&input[..start]); decoded.extend_from_slice(&plaintext); decoded.extend_from_slice(suffix);
    Ok(ConfigurationBytes(decoded))
}

#[cfg(test)]
#[path = "legacy_config_tests.rs"]
pub(crate) mod tests;
