//! IMS Digest-AKA computation (RFC 3310 AKAv1-MD5, RFC 4169 AKAv2-MD5) plus SIP
//! digest-challenge parsing and Authorization header assembly.
//!
//! Clean-room from public specifications: RFC 2617 (HTTP Digest), RFC 2104
//! (HMAC), RFC 3310 (Digest AKAv1), RFC 4169 (Digest AKAv2), 3GPP TS 33.203.
//!
//! This is the single shared implementation used by every IMS access leg
//! (VoWiFi / VoLTE / future ViLTE). The USIM AKA run itself (RAND/AUTN →
//! RES/CK/IK/AUTS) is performed by SIM hardware; this module only turns that
//! material into the SIP `Authorization` proof.

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};

use super::ImsError;

/// AKA key material produced by a USIM AUTHENTICATE run. Mirrors
/// `vowifi::qmi_uim::UsimAkaApduResult` structurally so either leg can pass its
/// own result in by value/ref via `AkaMaterial`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AkaMaterial<'a> {
    pub res: &'a [u8],
    pub ck: &'a [u8],
    pub ik: &'a [u8],
}

/// A parsed WWW-Authenticate / Proxy-Authenticate digest challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestChallenge {
    pub realm: String,
    /// base64(RAND || AUTN [|| server-data]); still encoded here.
    pub nonce: String,
    pub algorithm: String,
    pub qop: Option<String>,
    pub opaque: Option<String>,
    /// True when the challenge arrived in Proxy-Authenticate.
    pub proxy: bool,
}

impl DigestChallenge {
    pub fn authorization_header_name(&self) -> &'static str {
        if self.proxy {
            "Proxy-Authorization"
        } else {
            "Authorization"
        }
    }
}

/// RAND/AUTN pair extracted from a decoded AKA nonce, ready to feed the USIM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AkaChallenge {
    pub rand: Vec<u8>,
    pub autn: Vec<u8>,
}

/// Decode an AKA nonce into RAND(16) || AUTN(16). base64 by RFC 3310 (some
/// networks send hex); first 32 bytes are RAND||AUTN, trailing data ignored.
pub fn decode_aka_nonce(nonce: &str) -> Result<AkaChallenge, ImsError> {
    let raw = decode_nonce_bytes(nonce)?;
    if raw.len() < 32 {
        return Err(ImsError::new("register_nonce_not_aka"));
    }
    Ok(AkaChallenge {
        rand: raw[..16].to_vec(),
        autn: raw[16..32].to_vec(),
    })
}

fn decode_nonce_bytes(nonce: &str) -> Result<Vec<u8>, ImsError> {
    let trimmed = nonce.trim();
    if !trimmed.is_empty()
        && trimmed.len().is_multiple_of(2)
        && trimmed.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return decode_hex(trimmed);
    }
    if let Ok(bytes) = BASE64_STANDARD.decode(trimmed.as_bytes()) {
        return Ok(bytes);
    }
    let mut padded = trimmed.to_string();
    while !padded.len().is_multiple_of(4) {
        padded.push('=');
    }
    BASE64_STANDARD
        .decode(padded.as_bytes())
        .map_err(|_| ImsError::new("digest_nonce_decode_failed"))
}

fn decode_hex(s: &str) -> Result<Vec<u8>, ImsError> {
    if !s.len().is_multiple_of(2) {
        return Err(ImsError::new("hex_invalid"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ImsError::new("hex_invalid")))
        .collect()
}

/// Derive the digest "password" from AKA material per RFC 3310 / RFC 4169.
///   - AKAv1-MD5 (and plain MD5): password = RES bytes.
///   - AKAv2-MD5: password = base64( HMAC-MD5( RES||IK||CK, "http-digest-akav2-password" ) ).
pub fn aka_digest_password(algorithm: &str, aka: &AkaMaterial<'_>) -> Result<Vec<u8>, ImsError> {
    if algorithm.eq_ignore_ascii_case("AKAv1-MD5") || algorithm.eq_ignore_ascii_case("MD5") {
        if aka.res.is_empty() {
            return Err(ImsError::new("aka_res_empty"));
        }
        return Ok(aka.res.to_vec());
    }
    if algorithm.eq_ignore_ascii_case("AKAv2-MD5") {
        if aka.res.is_empty() || aka.ik.len() != 16 || aka.ck.len() != 16 {
            return Err(ImsError::new("aka_material_invalid"));
        }
        let mut key = Vec::with_capacity(aka.res.len() + aka.ik.len() + aka.ck.len());
        key.extend_from_slice(aka.res);
        key.extend_from_slice(aka.ik);
        key.extend_from_slice(aka.ck);
        let digest = hmac_md5(&key, b"http-digest-akav2-password");
        return Ok(BASE64_STANDARD.encode(digest).into_bytes());
    }
    Err(ImsError::new("digest_algorithm_unsupported"))
}

/// Compute the RFC 2617 digest response using the AKA-derived password.
#[allow(clippy::too_many_arguments)]
pub fn compute_aka_response(
    username: &str,
    realm: &str,
    aka: &AkaMaterial<'_>,
    algorithm: &str,
    method: &str,
    digest_uri: &str,
    nonce: &str,
    qop: Option<&str>,
    cnonce: &str,
    nc: &str,
) -> Result<String, ImsError> {
    let password = aka_digest_password(algorithm, aka)?;
    let mut a1 = Vec::with_capacity(username.len() + realm.len() + password.len() + 2);
    a1.extend_from_slice(username.as_bytes());
    a1.push(b':');
    a1.extend_from_slice(realm.as_bytes());
    a1.push(b':');
    a1.extend_from_slice(&password);
    let ha1 = md5_hex(&a1);
    let ha2 = md5_hex(format!("{method}:{digest_uri}").as_bytes());
    let proof = match qop {
        Some("auth") => format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}"),
        Some(_) => return Err(ImsError::new("digest_qop_unsupported")),
        None => format!("{ha1}:{nonce}:{ha2}"),
    };
    Ok(md5_hex(proof.as_bytes()))
}

/// Assemble a full `Authorization`/`Proxy-Authorization` header line (no CRLF).
pub fn build_authorization_header(
    challenge: &DigestChallenge,
    username: &str,
    digest_uri: &str,
    response: &str,
    cnonce: &str,
    nc: &str,
) -> String {
    let mut header = format!(
        "{}: Digest username=\"{}\",realm=\"{}\",nonce=\"{}\",uri=\"{}\",response=\"{}\",algorithm={}",
        challenge.authorization_header_name(),
        quote(username),
        quote(&challenge.realm),
        quote(&challenge.nonce),
        quote(digest_uri),
        response,
        challenge.algorithm,
    );
    if let Some(qop) = &challenge.qop {
        header.push_str(&format!(",qop={qop},nc={nc},cnonce=\"{cnonce}\""));
    }
    if let Some(opaque) = &challenge.opaque {
        header.push_str(&format!(",opaque=\"{}\"", quote(opaque)));
    }
    header
}

/// Build the initial empty-AKA Authorization header (before the 401).
pub fn build_initial_authorization_header(username: &str, realm: &str, digest_uri: &str) -> String {
    format!(
        "Authorization: Digest username=\"{}\",realm=\"{}\",nonce=\"\",uri=\"{}\",response=\"\",algorithm=AKAv1-MD5",
        quote(username),
        quote(realm),
        quote(digest_uri),
    )
}

/// Build a resync Authorization header carrying AUTS (base64) after an AKA
/// synchronization failure.
pub fn build_resync_authorization_header(
    challenge: &DigestChallenge,
    username: &str,
    digest_uri: &str,
    auts: &[u8],
) -> String {
    format!(
        "{}: Digest username=\"{}\",realm=\"{}\",nonce=\"{}\",uri=\"{}\",response=\"\",algorithm={},auts=\"{}\"",
        challenge.authorization_header_name(),
        quote(username),
        quote(&challenge.realm),
        quote(&challenge.nonce),
        quote(digest_uri),
        challenge.algorithm,
        BASE64_STANDARD.encode(auts),
    )
}

/// Parse a digest challenge from a header value (text after `WWW-Authenticate:`).
pub fn parse_digest_challenge(value: &str, proxy: bool) -> Result<DigestChallenge, ImsError> {
    let trimmed = value.trim();
    let params = trimmed
        .strip_prefix("Digest ")
        .or_else(|| trimmed.strip_prefix("digest "))
        .ok_or(ImsError::new("digest_challenge_missing"))?;
    let map = parse_digest_params(params);
    let realm = map
        .iter()
        .find(|(k, _)| k == "realm")
        .map(|(_, v)| v.clone())
        .ok_or(ImsError::new("digest_realm_missing"))?;
    let nonce = map
        .iter()
        .find(|(k, _)| k == "nonce")
        .map(|(_, v)| v.clone())
        .ok_or(ImsError::new("digest_nonce_missing"))?;
    let algorithm = map
        .iter()
        .find(|(k, _)| k == "algorithm")
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| "AKAv1-MD5".to_string());
    let qop = select_qop(&map);
    let opaque = map
        .iter()
        .find(|(k, _)| k == "opaque")
        .map(|(_, v)| v.clone());
    Ok(DigestChallenge {
        realm,
        nonce,
        algorithm,
        qop,
        opaque,
        proxy,
    })
}

fn select_qop(params: &[(String, String)]) -> Option<String> {
    let raw = params.iter().find(|(k, _)| k == "qop").map(|(_, v)| v)?;
    raw.split([',', ' '])
        .map(|s| s.trim())
        .find(|s| s.eq_ignore_ascii_case("auth"))
        .map(|_| "auth".to_string())
        .or_else(|| Some(raw.trim().to_string()))
}

/// Parse `key=value` / `key="value"` pairs, honoring commas inside quotes.
fn parse_digest_params(input: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i] == b',' || bytes[i].is_ascii_whitespace()) {
            i += 1;
        }
        let key_start = i;
        while i < bytes.len() && bytes[i] != b'=' {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let key = input[key_start..i].trim().to_ascii_lowercase();
        i += 1;
        let value = if i < bytes.len() && bytes[i] == b'"' {
            i += 1;
            let mut buf = String::new();
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    buf.push(bytes[i + 1] as char);
                    i += 2;
                    continue;
                }
                buf.push(bytes[i] as char);
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            buf
        } else {
            let val_start = i;
            while i < bytes.len() && bytes[i] != b',' {
                i += 1;
            }
            input[val_start..i].trim().to_string()
        };
        if !key.is_empty() {
            out.push((key, value));
        }
    }
    out
}

fn quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn md5_hex(bytes: &[u8]) -> String {
    format!("{:x}", md5::compute(bytes))
}

/// HMAC-MD5 (RFC 2104). The `md5` crate has no HMAC and `ring` has no MD5, so
/// this is hand-rolled — the single shared copy for all IMS legs.
pub fn hmac_md5(key: &[u8], data: &[u8]) -> [u8; 16] {
    const BLOCK_LEN: usize = 64;
    let mut normalized = [0u8; BLOCK_LEN];
    if key.len() > BLOCK_LEN {
        normalized[..16].copy_from_slice(&md5::compute(key).0);
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK_LEN];
    let mut opad = [0x5cu8; BLOCK_LEN];
    for i in 0..BLOCK_LEN {
        ipad[i] ^= normalized[i];
        opad[i] ^= normalized[i];
    }
    let mut inner = Vec::with_capacity(BLOCK_LEN + data.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(data);
    let inner_digest = md5::compute(&inner);
    let mut outer = Vec::with_capacity(BLOCK_LEN + 16);
    outer.extend_from_slice(&opad);
    outer.extend_from_slice(&inner_digest.0);
    md5::compute(&outer).0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material<'a>(res: &'a [u8], ck: &'a [u8], ik: &'a [u8]) -> AkaMaterial<'a> {
        AkaMaterial { res, ck, ik }
    }

    #[test]
    fn hmac_md5_matches_rfc2104_vectors() {
        let mac = hmac_md5(&[0x0b; 16], b"Hi There");
        let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "9294727a3638bb1c13f48ef8158bfc9d");
        let mac2 = hmac_md5(b"Jefe", b"what do ya want for nothing?");
        let hex2: String = mac2.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex2, "750c783e6ab0b503eaa86e310a5db738");
    }

    #[test]
    fn akav1_password_is_res_bytes() {
        let aka = material(&[0xde, 0xad, 0xbe, 0xef], &[0; 16], &[0; 16]);
        assert_eq!(
            aka_digest_password("AKAv1-MD5", &aka).unwrap(),
            vec![0xde, 0xad, 0xbe, 0xef]
        );
    }

    #[test]
    fn akav2_password_is_base64_hmac() {
        let res = [0x11u8; 8];
        let ck = [0x22u8; 16];
        let ik = [0x33u8; 16];
        let aka = material(&res, &ck, &ik);
        let pw = aka_digest_password("AKAv2-MD5", &aka).unwrap();
        let mut key = Vec::new();
        key.extend_from_slice(&res);
        key.extend_from_slice(&ik);
        key.extend_from_slice(&ck);
        let expected = BASE64_STANDARD
            .encode(hmac_md5(&key, b"http-digest-akav2-password"))
            .into_bytes();
        assert_eq!(pw, expected);
    }

    #[test]
    fn digest_response_matches_rfc2617_plain_md5() {
        // RFC 2617 §3.5 example (no qop): expected 670fd8c2df070c60b045671b8b24ff02
        let res = b"Circle Of Life";
        let aka = material(res, &[], &[]);
        let resp = compute_aka_response(
            "Mufasa",
            "testrealm@host.com",
            &aka,
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
    fn digest_response_matches_rfc2617_qop_auth() {
        // expected 6629fae49393a05397450978507c4ef1
        let res = b"Circle Of Life";
        let aka = material(res, &[], &[]);
        let resp = compute_aka_response(
            "Mufasa",
            "testrealm@host.com",
            &aka,
            "MD5",
            "GET",
            "/dir/index.html",
            "dcd98b7102dd2f0e8b11d0f600bfb0c093",
            Some("auth"),
            "0a4f113b",
            "00000001",
        )
        .unwrap();
        assert_eq!(resp, "6629fae49393a05397450978507c4ef1");
    }

    #[test]
    fn parse_challenge_extracts_fields_and_selects_auth() {
        let value = "Digest realm=\"ims.mnc000.mcc460.3gppnetwork.org\", nonce=\"YWJjZGVm\", algorithm=AKAv1-MD5, qop=\"auth,auth-int\", opaque=\"xyz\"";
        let c = parse_digest_challenge(value, true).unwrap();
        assert_eq!(c.realm, "ims.mnc000.mcc460.3gppnetwork.org");
        assert_eq!(c.nonce, "YWJjZGVm");
        assert_eq!(c.algorithm, "AKAv1-MD5");
        assert_eq!(c.qop.as_deref(), Some("auth"));
        assert_eq!(c.opaque.as_deref(), Some("xyz"));
        assert_eq!(c.authorization_header_name(), "Proxy-Authorization");
    }

    #[test]
    fn decode_aka_nonce_splits_rand_autn() {
        let mut raw = Vec::new();
        raw.extend(0u8..16);
        raw.extend(16u8..32);
        let nonce = BASE64_STANDARD.encode(&raw);
        let c = decode_aka_nonce(&nonce).unwrap();
        assert_eq!(c.rand, (0u8..16).collect::<Vec<_>>());
        assert_eq!(c.autn, (16u8..32).collect::<Vec<_>>());
    }

    #[test]
    fn build_authorization_header_shape() {
        let challenge = DigestChallenge {
            realm: "r".to_string(),
            nonce: "n".to_string(),
            algorithm: "AKAv1-MD5".to_string(),
            qop: Some("auth".to_string()),
            opaque: Some("op".to_string()),
            proxy: false,
        };
        let h =
            build_authorization_header(&challenge, "user", "sip:r", "abc123", "cnon", "00000001");
        assert!(h.starts_with("Authorization: Digest username=\"user\""));
        assert!(h.contains("qop=auth,nc=00000001,cnonce=\"cnon\""));
        assert!(h.contains("opaque=\"op\""));
    }
}
