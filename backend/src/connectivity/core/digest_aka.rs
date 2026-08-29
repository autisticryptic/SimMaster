//! IMS Digest-AKA computation (RFC 3310 AKAv1-MD5 and RFC 4169 AKAv2 with
//! MD5/SHA-256) plus SIP digest-challenge parsing and Authorization assembly.
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
    let raw = decode_digest_nonce(nonce)?;
    if raw.len() < 32 {
        return Err(ImsError::new("register_nonce_not_aka"));
    }
    Ok(AkaChallenge {
        rand: raw[..16].to_vec(),
        autn: raw[16..32].to_vec(),
    })
}

/// Decode a Digest nonce that may be hex or base64. Access adapters use this
/// before deciding whether a short value is a carrier-approved plain MD5
/// challenge or a full RAND || AUTN AKA challenge.
pub fn decode_digest_nonce(nonce: &str) -> Result<Vec<u8>, ImsError> {
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
///   - AKAv1-MD5 (and carrier-gated plain MD5): password = RES bytes.
///   - AKAv2-MD5: base64(HMAC-MD5(RES||IK||CK, fixed-label)).
///   - AKAv2-SHA-256: base64(HMAC-SHA-256(RES||IK||CK, fixed-label)).
pub fn aka_digest_password(algorithm: &str, aka: &AkaMaterial<'_>) -> Result<Vec<u8>, ImsError> {
    if algorithm.eq_ignore_ascii_case("AKAv1-MD5") || algorithm.eq_ignore_ascii_case("MD5") {
        if aka.res.is_empty() {
            return Err(ImsError::new("aka_res_empty"));
        }
        return Ok(aka.res.to_vec());
    }
    if algorithm.eq_ignore_ascii_case("AKAv2-MD5")
        || algorithm.eq_ignore_ascii_case("AKAv2-SHA-256")
    {
        if aka.res.is_empty() || aka.ik.len() != 16 || aka.ck.len() != 16 {
            return Err(ImsError::new("aka_material_invalid"));
        }
        let mut key = Vec::with_capacity(aka.res.len() + aka.ik.len() + aka.ck.len());
        key.extend_from_slice(aka.res);
        key.extend_from_slice(aka.ik);
        key.extend_from_slice(aka.ck);
        let digest = if algorithm.eq_ignore_ascii_case("AKAv2-SHA-256") {
            hmac_sha256(&key, b"http-digest-akav2-password")
        } else {
            hmac_md5(&key, b"http-digest-akav2-password").to_vec()
        };
        return Ok(BASE64_STANDARD.encode(digest).into_bytes());
    }
    Err(ImsError::new("digest_algorithm_unsupported"))
}

/// Compute the RFC 2617/RFC 7616 digest response using the AKA-derived password.
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
    let hash = digest_hash_for_algorithm(algorithm)?;
    let ha1 = hash.hex(&a1);
    let ha2 = hash.hex(format!("{method}:{digest_uri}").as_bytes());
    let proof = match qop {
        Some("auth") => format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}"),
        Some(_) => return Err(ImsError::new("digest_qop_unsupported")),
        None => format!("{ha1}:{nonce}:{ha2}"),
    };
    Ok(hash.hex(proof.as_bytes()))
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

/// Empty-AKA Authorization using the parameter order accepted by stricter IMS
/// parsers found in some carrier P-CSCFs.
pub fn build_initial_authorization_header_uri_first(
    username: &str,
    realm: &str,
    digest_uri: &str,
) -> String {
    format!(
        "Authorization: Digest uri=\"{}\",username=\"{}\",algorithm=AKAv1-MD5,response=\"\",realm=\"{}\",nonce=\"\"",
        quote(digest_uri),
        quote(username),
        quote(realm),
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

/// Build an AUTS resynchronization header while retaining qop/opaque parameters
/// required by stricter P-CSCFs. `cnonce` is required when the challenge chose
/// qop; callers that do not negotiate qop can use
/// [`build_resync_authorization_header`].
pub fn build_resync_authorization_header_with_digest(
    challenge: &DigestChallenge,
    username: &str,
    digest_uri: &str,
    auts: &[u8],
    cnonce: Option<&str>,
    nc: Option<&str>,
) -> String {
    let auts = BASE64_STANDARD.encode(auts);
    let mut header = format!(
        "{}: Digest username=\"{}\",realm=\"{}\",nonce=\"{}\",uri=\"{}\",response=\"\",algorithm={},auts=\"{}\"",
        challenge.authorization_header_name(),
        quote(username),
        quote(&challenge.realm),
        quote(&challenge.nonce),
        quote(digest_uri),
        challenge.algorithm,
        quote(&auts),
    );
    if let Some(qop) = challenge.qop.as_deref() {
        let cnonce = cnonce.unwrap_or_default();
        let nc = nc.unwrap_or("00000001");
        header.push_str(&format!(",qop={qop},nc={nc},cnonce=\"{}\"", quote(cnonce)));
    }
    if let Some(opaque) = challenge.opaque.as_deref() {
        header.push_str(&format!(",opaque=\"{}\"", quote(opaque)));
    }
    header
}

/// Split one `WWW-Authenticate` / `Proxy-Authenticate` field value into its
/// individual Digest challenges while preserving wire order. Commas inside
/// quoted parameters (notably `qop="auth,auth-int"`) are not separators.
pub fn split_digest_challenge_values(value: &str) -> Vec<String> {
    let value = value.trim();
    if value.is_empty() {
        return Vec::new();
    }

    let mut values = Vec::new();
    let mut start = 0usize;
    let mut escaped = false;
    let mut in_quote = false;
    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quote => escaped = true,
            '"' => in_quote = !in_quote,
            ',' if !in_quote => {
                if let Some(next_start) = digest_scheme_start_after_comma(value, index) {
                    let item = value[start..index].trim();
                    if !item.is_empty() {
                        values.push(item.to_string());
                    }
                    start = next_start;
                }
            }
            _ => {}
        }
    }

    let item = value[start..].trim();
    if !item.is_empty() {
        values.push(item.to_string());
    }
    values
}

/// Select the first syntactically valid Digest challenge this stack can
/// actually answer. All WWW fields are considered in their original order,
/// followed by Proxy fields, preserving the previous WWW-before-Proxy policy.
/// Unsupported or malformed earlier challenges do not hide a later usable AKA
/// challenge. Plain MD5 is considered only when explicitly enabled by carrier
/// policy and when the caller has a real implementation for it.
pub fn select_digest_challenge(
    www_values: &[String],
    proxy_values: &[String],
    allow_plain_md5: bool,
) -> Result<DigestChallenge, ImsError> {
    let mut last_error = None;
    let mut saw_candidate = false;

    for (proxy, values) in [(false, www_values), (true, proxy_values)] {
        for header_value in values {
            for value in split_digest_challenge_values(header_value) {
                saw_candidate = true;
                match parse_digest_challenge(&value, proxy).and_then(|challenge| {
                    validate_digest_challenge_support(challenge, allow_plain_md5)
                }) {
                    Ok(challenge) => return Ok(challenge),
                    Err(error) => last_error = Some(error),
                }
            }
        }
    }

    if !saw_candidate {
        return Err(ImsError::new("digest_challenge_missing"));
    }
    Err(last_error.unwrap_or_else(|| ImsError::new("digest_challenge_missing")))
}

/// Parse a digest challenge from a header value (text after `WWW-Authenticate:`).
pub fn parse_digest_challenge(value: &str, proxy: bool) -> Result<DigestChallenge, ImsError> {
    let params = strip_digest_scheme(value).ok_or(ImsError::new("digest_challenge_missing"))?;
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

fn validate_digest_challenge_support(
    challenge: DigestChallenge,
    allow_plain_md5: bool,
) -> Result<DigestChallenge, ImsError> {
    let algorithm_supported = challenge.algorithm.eq_ignore_ascii_case("AKAv1-MD5")
        || challenge.algorithm.eq_ignore_ascii_case("AKAv2-MD5")
        || challenge.algorithm.eq_ignore_ascii_case("AKAv2-SHA-256")
        || (allow_plain_md5 && challenge.algorithm.eq_ignore_ascii_case("MD5"));
    if !algorithm_supported {
        return Err(ImsError::new("digest_algorithm_unsupported"));
    }
    if !matches!(challenge.qop.as_deref(), None | Some("auth")) {
        return Err(ImsError::new("digest_qop_unsupported"));
    }
    Ok(challenge)
}

fn strip_digest_scheme(value: &str) -> Option<&str> {
    let value = value.trim();
    let scheme = value.get(..6)?;
    if !scheme.eq_ignore_ascii_case("Digest") {
        return None;
    }
    let rest = value.get(6..)?;
    rest.chars()
        .next()
        .is_some_and(char::is_whitespace)
        .then(|| rest.trim_start())
}

fn digest_scheme_start_after_comma(value: &str, comma_index: usize) -> Option<usize> {
    let rest = value.get(comma_index + 1..)?;
    let trimmed = rest.trim_start();
    let skipped = rest.len() - trimmed.len();
    starts_with_digest_scheme(trimmed).then_some(comma_index + 1 + skipped)
}

fn starts_with_digest_scheme(value: &str) -> bool {
    let Some(prefix) = value.get(..6) else {
        return false;
    };
    prefix.eq_ignore_ascii_case("Digest")
        && value
            .get(6..)
            .and_then(|rest| rest.chars().next())
            .is_some_and(char::is_whitespace)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DigestHash {
    Md5,
    Sha256,
}

impl DigestHash {
    fn hex(self, bytes: &[u8]) -> String {
        match self {
            Self::Md5 => format!("{:x}", md5::compute(bytes)),
            Self::Sha256 => hex_lower(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref()),
        }
    }
}

fn digest_hash_for_algorithm(algorithm: &str) -> Result<DigestHash, ImsError> {
    if algorithm.eq_ignore_ascii_case("AKAv1-MD5")
        || algorithm.eq_ignore_ascii_case("AKAv2-MD5")
        || algorithm.eq_ignore_ascii_case("MD5")
    {
        return Ok(DigestHash::Md5);
    }
    if algorithm.eq_ignore_ascii_case("AKAv2-SHA-256") {
        return Ok(DigestHash::Sha256);
    }
    Err(ImsError::new("digest_algorithm_unsupported"))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
    ring::hmac::sign(&key, data).as_ref().to_vec()
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
    fn akav2_md5_password_is_base64_hmac() {
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
    fn akav2_sha256_password_and_digest_response_match_independent_vector() {
        let res = [0x11u8; 8];
        let ck = [0x22u8; 16];
        let ik = [0x33u8; 16];
        let aka = material(&res, &ck, &ik);
        let password = aka_digest_password("AKAv2-SHA-256", &aka).unwrap();
        assert_eq!(
            password.as_slice(),
            b"y1XGaxmHAuuo8s2MPYfXz/CZeEa1RBiLycGrpY293pQ="
        );
        assert_eq!(
            compute_aka_response(
                "user@example.com",
                "ims.example",
                &aka,
                "AKAv2-SHA-256",
                "REGISTER",
                "sip:ims.example",
                "nonce",
                Some("auth"),
                "cafebabe",
                "00000001",
            )
            .unwrap(),
            "7b1e4b6f0343c2970757c768727ff7d3ad8e0f8d4c815b2d68b38f9784cb11c5"
        );
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
    fn digest_challenge_splitter_preserves_quoted_qop_commas() {
        let values = split_digest_challenge_values(
            "Digest realm=\"one\", qop=\"auth,auth-int\", nonce=\"a\", Digest realm=\"two\", nonce=\"b\"",
        );

        assert_eq!(values.len(), 2);
        assert!(values[0].contains("qop=\"auth,auth-int\""));
        assert!(values[1].starts_with("Digest realm=\"two\""));
    }

    #[test]
    fn challenge_selector_skips_unsupported_and_malformed_candidates() {
        let www = vec![
            "Digest realm=\"unsupported\", nonce=\"one\", algorithm=SHA-512".to_string(),
            "Digest nonce=\"missing-realm\", algorithm=AKAv1-MD5".to_string(),
            "Digest realm=\"selected\", nonce=\"three\", algorithm=AKAv2-SHA-256, qop=\"auth\""
                .to_string(),
        ];

        let challenge = select_digest_challenge(&www, &[], false).unwrap();
        assert_eq!(challenge.realm, "selected");
        assert_eq!(challenge.algorithm, "AKAv2-SHA-256");
        assert!(!challenge.proxy);
    }

    #[test]
    fn challenge_selector_handles_repeated_and_compound_header_values() {
        let www = vec![format!(
            "Basic realm=\"legacy\", Digest realm=\"plain\", nonce=\"one\", algorithm=MD5, Digest realm=\"aka\", nonce=\"two\", algorithm=AKAv1-MD5, qop=\"auth,auth-int\""
        )];

        let challenge = select_digest_challenge(&www, &[], false).unwrap();
        assert_eq!(challenge.realm, "aka");
        assert_eq!(challenge.qop.as_deref(), Some("auth"));
    }

    #[test]
    fn challenge_selector_keeps_topmost_supported_wire_order() {
        let www = vec![
            "Digest realm=\"first\", nonce=\"one\", algorithm=AKAv1-MD5".to_string(),
            "Digest realm=\"stronger-but-later\", nonce=\"two\", algorithm=AKAv2-SHA-256"
                .to_string(),
        ];

        let challenge = select_digest_challenge(&www, &[], false).unwrap();
        assert_eq!(challenge.realm, "first");
        assert_eq!(challenge.algorithm, "AKAv1-MD5");
    }

    #[test]
    fn challenge_selector_uses_proxy_after_unusable_www_and_gates_plain_md5() {
        let www = vec!["Digest realm=\"www\", nonce=\"one\", algorithm=MD5".to_string()];
        let proxy = vec!["Digest realm=\"proxy\", nonce=\"two\", algorithm=AKAv2-MD5".to_string()];

        let challenge = select_digest_challenge(&www, &proxy, false).unwrap();
        assert_eq!(challenge.realm, "proxy");
        assert!(challenge.proxy);

        let plain = select_digest_challenge(&www, &[], true).unwrap();
        assert_eq!(plain.realm, "www");
        assert_eq!(plain.algorithm, "MD5");
        assert!(!plain.proxy);
    }

    #[test]
    fn challenge_selector_skips_unsupported_qop() {
        let www = vec![
            "Digest realm=\"auth-int-only\", nonce=\"one\", algorithm=AKAv1-MD5, qop=\"auth-int\""
                .to_string(),
            "dIgEsT realm=\"auth\", nonce=\"two\", algorithm=AKAv1-MD5, qop=\"auth\"".to_string(),
        ];

        let challenge = select_digest_challenge(&www, &[], false).unwrap();
        assert_eq!(challenge.realm, "auth");
        assert_eq!(challenge.qop.as_deref(), Some("auth"));
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

    #[test]
    fn resync_header_can_retain_digest_qop_and_opaque() {
        let challenge = DigestChallenge {
            realm: "r".to_string(),
            nonce: "n".to_string(),
            algorithm: "AKAv1-MD5".to_string(),
            qop: Some("auth".to_string()),
            opaque: Some("op".to_string()),
            proxy: true,
        };
        let header = build_resync_authorization_header_with_digest(
            &challenge,
            "user",
            "sip:r",
            &[0x01, 0x02],
            Some("cnon"),
            Some("00000001"),
        );

        assert!(header.starts_with("Proxy-Authorization: Digest username=\"user\""));
        assert!(header.contains("auts=\"AQI=\""));
        assert!(header.contains("qop=auth,nc=00000001,cnonce=\"cnon\""));
        assert!(header.contains("opaque=\"op\""));
    }
}
