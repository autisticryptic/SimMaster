//! VoLTE Digest-AKA adapter.
//!
//! The actual RFC 3310/4169 computation lives once in [`crate::ims::digest_aka`]
//! (shared by every IMS access leg). This module is a thin VoLTE-facing adapter
//! that:
//!   - re-exports the shared challenge/header builders, and
//!   - bridges VoLTE's concrete types (`UsimAkaApduResult`, `VolteError`) to the
//!     shared functions' neutral `AkaMaterial` / `ImsError`.
//!
//! Keeping this adapter means the rest of the VoLTE code (and its tests) still
//! call `volte::digest_aka::*` with `VolteError` semantics, while the crypto is
//! implemented and vector-tested exactly once in the shared core.

use crate::ims::digest_aka as core;
use crate::ims::ImsError;
use crate::access::vowifi::qmi_uim::UsimAkaApduResult;

use super::errors::{code, VolteError};

// Re-export the shared, transport-agnostic types/builders verbatim. These carry
// no error type, so they need no adaptation.
pub use core::{
    build_authorization_header, build_initial_authorization_header,
    build_resync_authorization_header, DigestChallenge,
};

/// RAND/AUTN pair extracted from a decoded AKA nonce.
pub use core::AkaChallenge;

/// Map a shared-core `ImsError` reason code onto the matching VoLTE error code.
///
/// The shared core emits stable neutral codes (`aka_res_empty`, `hex_invalid`,
/// 鈥?; VoLTE surfaces the `volte_`-prefixed variants its frontend contract and
/// error taxonomy expect. This is the single mapping seam.
fn map_err(err: ImsError) -> VolteError {
    let mapped = match err.code() {
        "register_nonce_not_aka" => code::REGISTER_NONCE_NOT_AKA,
        "digest_nonce_decode_failed" => code::DIGEST_NONCE_DECODE_FAILED,
        "hex_invalid" => code::HEX_INVALID,
        "aka_res_empty" => code::AKA_RES_EMPTY,
        "aka_material_invalid" => code::AKA_MATERIAL_INVALID,
        "digest_algorithm_unsupported" => code::DIGEST_ALGORITHM_UNSUPPORTED,
        "digest_qop_unsupported" => code::DIGEST_QOP_UNSUPPORTED,
        "digest_challenge_missing" => code::DIGEST_CHALLENGE_MISSING,
        "digest_realm_missing" => code::DIGEST_REALM_MISSING,
        "digest_nonce_missing" => code::DIGEST_NONCE_MISSING,
        // Any unmapped neutral code is surfaced verbatim (still greppable).
        other => return VolteError::with_detail(code::DIGEST_CHALLENGE_MISSING, other),
    };
    VolteError::new(mapped)
}

/// Borrow a `UsimAkaApduResult` as the shared `AkaMaterial` view.
fn material(aka: &UsimAkaApduResult) -> core::AkaMaterial<'_> {
    core::AkaMaterial {
        res: &aka.res,
        ck: &aka.ck,
        ik: &aka.ik,
    }
}

/// Decode an AKA nonce into RAND(16) || AUTN(16).
pub fn decode_aka_nonce(nonce: &str) -> Result<AkaChallenge, VolteError> {
    core::decode_aka_nonce(nonce).map_err(map_err)
}

/// Derive the digest "password" from AKA material (AKAv1/AKAv2-MD5).
pub fn aka_digest_password(
    algorithm: &str,
    aka: &UsimAkaApduResult,
) -> Result<Vec<u8>, VolteError> {
    core::aka_digest_password(algorithm, &material(aka)).map_err(map_err)
}

/// Compute the RFC 2617 digest response using the AKA-derived password.
#[allow(clippy::too_many_arguments)]
pub fn compute_aka_response(
    username: &str,
    realm: &str,
    aka: &UsimAkaApduResult,
    algorithm: &str,
    method: &str,
    digest_uri: &str,
    nonce: &str,
    qop: Option<&str>,
    cnonce: &str,
    nc: &str,
) -> Result<String, VolteError> {
    core::compute_aka_response(
        username,
        realm,
        &material(aka),
        algorithm,
        method,
        digest_uri,
        nonce,
        qop,
        cnonce,
        nc,
    )
    .map_err(map_err)
}

/// Parse a digest challenge from a header value.
pub fn parse_digest_challenge(value: &str, proxy: bool) -> Result<DigestChallenge, VolteError> {
    core::parse_digest_challenge(value, proxy).map_err(map_err)
}

/// HMAC-MD5 (RFC 2104) 鈥?re-exported from the shared core.
pub use core::hmac_md5;

#[cfg(test)]
mod tests {
    use super::*;

    fn aka(res: Vec<u8>, ck: Vec<u8>, ik: Vec<u8>) -> UsimAkaApduResult {
        UsimAkaApduResult {
            res,
            ck,
            ik,
            auts: None,
        }
    }

    #[test]
    fn akav1_password_is_res_bytes() {
        let a = aka(vec![0xde, 0xad, 0xbe, 0xef], vec![0; 16], vec![0; 16]);
        assert_eq!(
            aka_digest_password("AKAv1-MD5", &a).unwrap(),
            vec![0xde, 0xad, 0xbe, 0xef]
        );
    }

    #[test]
    fn akav1_empty_res_maps_to_volte_code() {
        let a = aka(vec![], vec![0; 16], vec![0; 16]);
        assert_eq!(
            aka_digest_password("AKAv1-MD5", &a).unwrap_err().code(),
            code::AKA_RES_EMPTY
        );
    }

    #[test]
    fn akav2_bad_material_maps_to_volte_code() {
        let a = aka(vec![0x11; 8], vec![0x22; 8], vec![0x33; 16]);
        assert_eq!(
            aka_digest_password("AKAv2-MD5", &a).unwrap_err().code(),
            code::AKA_MATERIAL_INVALID
        );
    }

    #[test]
    fn unsupported_algorithm_maps_to_volte_code() {
        let a = aka(vec![1], vec![], vec![]);
        assert_eq!(
            aka_digest_password("SHA-256", &a).unwrap_err().code(),
            code::DIGEST_ALGORITHM_UNSUPPORTED
        );
    }

    #[test]
    fn digest_response_matches_rfc2617_vector() {
        // RFC 2617 搂3.5 (no qop) -> 670fd8c2df070c60b045671b8b24ff02.
        let a = aka(b"Circle Of Life".to_vec(), vec![], vec![]);
        let resp = compute_aka_response(
            "Mufasa",
            "testrealm@host.com",
            &a,
            "AKAv1-MD5",
            "GET",
            "/dir/index.html",
            "dcd98b7102dd2f0e8b11d0f600bfb0c093",
            None,
            "",
            "00000001",
        )
        .unwrap();
        assert_eq!(resp, "670fd8c2df070c60b045671b8b24ff02");
    }

    #[test]
    fn parse_challenge_and_missing_realm_maps_code() {
        let c = parse_digest_challenge(
            "Digest realm=\"r\", nonce=\"YWJjZGVm\", algorithm=AKAv1-MD5, qop=\"auth\"",
            false,
        )
        .unwrap();
        assert_eq!(c.realm, "r");
        assert_eq!(c.authorization_header_name(), "Authorization");
        assert_eq!(
            parse_digest_challenge("Digest nonce=\"n\"", false)
                .unwrap_err()
                .code(),
            code::DIGEST_REALM_MISSING
        );
    }

    #[test]
    fn decode_nonce_rejects_short() {
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        let nonce = B64.encode([0u8; 16]);
        assert_eq!(
            decode_aka_nonce(&nonce).unwrap_err().code(),
            code::REGISTER_NONCE_NOT_AKA
        );
    }
}
