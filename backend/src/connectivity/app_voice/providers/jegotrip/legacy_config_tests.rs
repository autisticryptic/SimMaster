use super::*;
use cbc::cipher::{BlockEncrypt, KeyInit};
use std::{io::Write, process::{Command, Stdio}};

/// Independent standard OpenSSL block-CBC oracle, Actions test process ONLY.
/// Password/salt/plaintext are synthetic fixtures; target ELF is never invoked.
pub(crate) fn fixture_envelope(plaintext: &[u8], password: &[u8], salt: [u8; 8]) -> Vec<u8> {
    let mut digest = derived(password, &salt); odd_parity(&mut digest[..8]);
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let mut child = Command::new("openssl").args(["enc", "-des-cbc", "-nopad", "-provider", "default", "-provider", "legacy",
        "-K", &hex(&digest[..8]), "-iv", &hex(&digest[8..])]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("Actions openssl DES provider");
    let pad = 8 - plaintext.len() % 8; let mut padded = plaintext.to_vec(); padded.resize(plaintext.len() + pad, pad as u8);
    child.stdin.take().unwrap().write_all(&padded).unwrap();
    let result = child.wait_with_output().unwrap(); assert!(result.status.success());
    let mut raw = salt.to_vec(); raw.extend_from_slice(&result.stdout);
    let mut envelope = b"ENC(".to_vec(); envelope.extend_from_slice(STANDARD.encode(raw).as_bytes()); envelope.push(b')'); envelope
}

#[test]
fn native_pbe_kdf_has_one_password_salt_md5_then_999_digest_rounds_and_odd_des_parity() {
    let password = b"fixture-configuration-key"; let salt = [1,2,3,4,5,6,7,8];
    let digest = derived(password, &salt);
    assert_eq!(digest[..], [0x75,0xbb,0x3a,0xff,0x6e,0x80,0x3b,0xd4,0x21,0x9e,0x02,0x3d,0xf2,0xe3,0x32,0x09]);
    let mut digest = digest; odd_parity(&mut digest[..8]);
    assert_eq!(digest[..8], [0x75,0xba,0x3b,0xfe,0x6e,0x80,0x3b,0xd5]);
    assert_eq!(digest[8..], [0x21,0x9e,0x02,0x3d,0xf2,0xe3,0x32,0x09]);
}

#[test]
fn rustcrypto_des_matches_public_nist_block_fixture_without_weak_network_fallback() {
    let des = des::Des::new_from_slice(&[0x13,0x34,0x57,0x79,0x9b,0xbc,0xdf,0xf1]).unwrap();
    let mut block = [0x01,0x23,0x45,0x67,0x89,0xab,0xcd,0xef].into(); des.encrypt_block(&mut block);
    assert_eq!(block[..], [0x85,0xe8,0x13,0x54,0x0f,0x0a,0xb4,0x05]);
}

#[test]
fn openssl_fixture_decodes_salt_prefix_cbc_all_padding_sizes_and_binary_plaintext() {
    let password = b"fixture-configuration-key"; let key = ConfigDecodeKey::supplied(password).unwrap();
    for length in 0..=24 {
        let plaintext = (0..length).map(|i| (i * 17) as u8).collect::<Vec<_>>();
        let envelope = fixture_envelope(&plaintext, password, [1,2,3,4,5,6,7,8]);
        assert_eq!(decode_configuration(&envelope, Some(&key)).unwrap().expose_to_backend(), plaintext);
    }
}

#[test]
fn mismatched_interior_zero_or_oversized_pkcs5_padding_is_rejected_deterministically() {
    let password = b"fixture-padding-key"; let key = ConfigDecodeKey::supplied(password).unwrap();
    // Ten plaintext bytes give six padding bytes in block2. CBC corruption of
    // block1 changes a chosen block2 pad byte WITHOUT changing the last pad6.
    let envelope = fixture_envelope(b"0123456789", password, [4; 8]);
    let raw = STANDARD.decode(&envelope[4..envelope.len() - 1]).unwrap(); assert_eq!(raw.len(), 24);
    for (index, difference) in [(8 + 2, 1u8), (8 + 7, 6u8), (8 + 7, 6u8 ^ 9u8)] {
        let mut corrupted = raw.clone(); corrupted[index] ^= difference;
        let encoded = STANDARD.encode(corrupted); let envelope = [b"ENC(".as_slice(), encoded.as_bytes(), b")"].concat();
        assert_eq!(decode_configuration(&envelope, Some(&key)).err(), Some(ConfigError::DecodeFailed));
    }
}

#[test]
fn one_embedded_enc_replaces_only_original_substring_and_preserves_exact_prefix_suffix() {
    let password = b"fixture-reference-key"; let key = ConfigDecodeKey::supplied(password).unwrap();
    let envelope = fixture_envelope(b"tcp -h fixture.invalid -p 443;", password, [8,7,6,5,4,3,2,1]);
    let input = [b"#fixture:".as_slice(), &envelope, b"tail"].concat();
    assert_eq!(decode_configuration(&input, Some(&key)).unwrap().expose_to_backend(), b"#fixture:tcp -h fixture.invalid -p 443;tail");
    assert_eq!(decode_configuration(b"unchanged\0binary", None).unwrap().expose_to_backend(), b"unchanged\0binary");
}

#[test]
fn missing_context_key_wrong_key_bad_base64_and_malformed_wrappers_never_return_ciphertext_as_success() {
    let password = b"fixture-reference-key"; let key = ConfigDecodeKey::supplied(password).unwrap(); let wrong = ConfigDecodeKey::supplied(b"fixture-wrong-key").unwrap();
    let envelope = fixture_envelope(b"fixture plaintext", password, [8,7,6,5,4,3,2,1]);
    assert_eq!(decode_configuration(&envelope, None).err(), Some(ConfigError::KeyRequired));
    match decode_configuration(&envelope, Some(&wrong)) {
        Err(ConfigError::DecodeFailed) => {},
        Ok(value) => assert_ne!(value.expose_to_backend(), b"fixture plaintext", "CBC has no authentication tag"),
        other => panic!("unexpected legacy decode outcome {other:?}"),
    }
    for input in [b"ENC(".as_slice(), b"ENC(no closing", b"ENC()", b"ENC(bad!base64)", b"ENC(AA==)"] {
        assert!(decode_configuration(input, Some(&key)).is_err());
    }
}

#[test]
fn repeated_nested_and_recursive_enc_formats_are_not_guessed_or_key_substituted() {
    let key = ConfigDecodeKey::supplied(b"fixture-context").unwrap();
    assert_eq!(decode_configuration(b"ENC(abc)ENC(def)", Some(&key)).err(), Some(ConfigError::MultipleEnvelopes));
    assert_eq!(decode_configuration(b"ENC(ENC(abc))", Some(&key)).err(), Some(ConfigError::MultipleEnvelopes));
    let wrapped = fixture_envelope(b"ENC(remaining)", b"fixture-context", [0; 8]);
    assert_eq!(decode_configuration(&wrapped, Some(&key)).err(), Some(ConfigError::MultipleEnvelopes));
}

#[test]
fn hostile_lengths_and_keys_are_rejected_under_local_config_limits() {
    assert!(ConfigDecodeKey::supplied(&[]).is_err()); assert!(ConfigDecodeKey::supplied(&vec![0; MAX_KEY_BYTES + 1]).is_err());
    assert_eq!(decode_configuration(&vec![0; MAX_FIELD_BYTES + 1], None).err(), Some(ConfigError::TooLarge));
    let key = ConfigDecodeKey::supplied(b"fixture").unwrap();
    for raw in [vec![0; 8], vec![0; 15], vec![0; 17]] {
        let envelope = [b"ENC(".as_slice(), STANDARD.encode(raw).as_bytes(), b")"].concat();
        assert_eq!(decode_configuration(&envelope, Some(&key)).err(), Some(ConfigError::DecodeFailed));
    }
}

#[test]
fn weak_keys_and_key_plaintext_debug_values_are_not_treated_as_authorization() {
    for key in WEAK_KEYS { assert!(key.iter().all(|b| b.count_ones() % 2 == 1)); }
    let key = ConfigDecodeKey::supplied(b"fixture-secret-key").unwrap();
    let plaintext = decode_configuration(b"fixture-private-data", None).unwrap();
    assert!(!format!("{key:?}").contains("fixture")); assert!(!format!("{plaintext:?}").contains("fixture"));
}
