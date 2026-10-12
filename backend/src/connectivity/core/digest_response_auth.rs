//! Request-bound Digest response authentication (RFC 2617/7616/7615).
//!
//! A proof belongs to one credential-bearing REGISTER, not to mutable cached
//! credentials or the nextnonce of a later response. Capture it while deriving
//! Authorization, validate the final serialized request before sending, and
//! verify its response before committing any nonce, lease or security state.

use std::fmt;

use super::{
    compute_digest_response, decode_hex, digest_hash_for_algorithm, parse_digest_params,
    strip_digest_scheme, DigestChallenge, DigestHash, ImsError,
};

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_HEADER_LINES: usize = 256;
const MAX_AUTH_BYTES: usize = 16 * 1024;
const MAX_PARAM_VALUE_BYTES: usize = 4096;
const MAX_PARAM_NAME_BYTES: usize = 128;

fn invalid() -> ImsError {
    ImsError::new("digest_response_auth_invalid")
}

fn mismatch() -> ImsError {
    ImsError::new("digest_response_auth_mismatch")
}

fn request_mismatch() -> ImsError {
    ImsError::new("digest_request_auth_mismatch")
}

/// Immutable evidence for one credential-bearing REGISTER. Contains only
/// correlation fields and the two expected digests; never password, AKA or HA1.
/// Neither this context nor the URI, scope, nonces or digests may be logged.
#[derive(Clone)]
pub struct DigestRequestProof {
    proxy: bool,
    username: String,
    realm: String,
    nonce: String,
    algorithm: String,
    digest_uri: String,
    qop: Option<String>,
    opaque: Option<String>,
    cnonce: Option<String>,
    nc: Option<String>,
    request_digest: Vec<u8>,
    response_digest: Vec<u8>,
}

impl fmt::Debug for DigestRequestProof {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DigestRequestProof { [REDACTED] }")
    }
}

impl DigestRequestProof {
    /// Capture a REGISTER proof from the exact inputs used for Authorization.
    /// The method-less API intentionally validates REGISTER only. Empty-AKA and
    /// AUTS are not credential proofs: their adapters must retain `None` instead.
    /// MD5 remains subject to the caller's existing policy gate; this does not
    /// enable any algorithm that `compute_digest_response` did not support.
    /// A policy-approved plain MD5 password may legitimately be empty (existing
    /// WiFi behavior); its emitted digest is still nonempty and validated. An
    /// empty AKA password is never evidence of an authenticated request.
    pub fn new(
        challenge: &DigestChallenge,
        username: &str,
        password: &[u8],
        digest_uri: &str,
        cnonce: &str,
        nc: &str,
    ) -> Result<Self, ImsError> {
        let hash = digest_hash_for_algorithm(&challenge.algorithm)?;
        if !matches!(challenge.qop.as_deref(), None | Some("auth")) {
            return Err(ImsError::new("digest_qop_unsupported"));
        }
        if (password.is_empty() && !challenge.algorithm.eq_ignore_ascii_case("MD5"))
            || password.len() > MAX_AUTH_BYTES
            || ![username, &challenge.realm, &challenge.nonce, digest_uri]
                .into_iter().all(valid_value)
            || digest_uri.chars().any(char::is_whitespace)
            || challenge.opaque.as_deref().is_some_and(|value| !valid_parameter_value(value))
        {
            return Err(request_mismatch());
        }
        let (qop, cnonce_context, nc_context) = if challenge.qop.is_some() {
            if !valid_value(cnonce) || !valid_nc(nc) {
                return Err(request_mismatch());
            }
            (Some("auth".to_owned()), Some(cnonce.to_owned()), Some(nc.to_owned()))
        } else {
            (None, None, None)
        };
        let digest_len = match hash {
            DigestHash::Md5 => 16,
            DigestHash::Sha256 => 32,
        };
        let compute = |method| {
            compute_digest_response(
                username, &challenge.realm, password, &challenge.algorithm, method,
                digest_uri, &challenge.nonce, challenge.qop.as_deref(), cnonce, nc,
            ).and_then(|value| decode_proof_digest(&value, digest_len))
        };
        Ok(Self {
            proxy: challenge.proxy,
            username: username.to_owned(),
            realm: challenge.realm.clone(),
            nonce: challenge.nonce.clone(),
            algorithm: challenge.algorithm.clone(),
            digest_uri: digest_uri.to_owned(),
            qop,
            opaque: challenge.opaque.clone(),
            cnonce: cnonce_context,
            nc: nc_context,
            request_digest: compute("REGISTER")?,
            // rspauth hashes A2 = ":digest-uri", NOT "REGISTER:digest-uri".
            response_digest: compute("")?,
        })
    }

    /// Fail closed if the *final serialized request* differs from this proof.
    /// Adapters must call this after all driver rewriting and before any send;
    /// retransmissions may reuse the same already-validated bytes/context.
    pub fn validate_request(&self, request: &[u8]) -> Result<(), ImsError> {
        self.validate_request_inner(request).map_err(|_| request_mismatch())
    }

    fn validate_request_inner(&self, request: &[u8]) -> Result<(), ImsError> {
        let headers = auth_headers(request, "Authorization", "Proxy-Authorization")?;
        let mut words = headers.start_line.split(' ');
        if words.next() != Some("REGISTER") || words.next() != Some(self.digest_uri.as_str())
            || words.next() != Some("SIP/2.0") || words.next().is_some()
            || headers.fields.len() != 1 || headers.fields[0].0 != self.proxy
        {
            return Err(request_mismatch());
        }
        let value = &headers.fields[0].1;
        if !value.as_bytes().get(6).is_some_and(|byte| matches!(*byte, b' ' | b'\t')) {
            return Err(request_mismatch());
        }
        let input = strip_digest_scheme(value).ok_or_else(request_mismatch)?;
        let params = bounded_params(input)?;
        // An AUTS/empty proof, userhash/username* or a second scheme must not
        // masquerade as the ordinary username Digest header captured above.
        if params.iter().any(|(key, _)| !matches!(key.as_str(),
            "username" | "realm" | "nonce" | "uri" | "algorithm" | "response"
                | "qop" | "nc" | "cnonce" | "opaque"))
            || param(&params, "username") != Some(self.username.as_str())
            || param(&params, "realm") != Some(self.realm.as_str())
            || param(&params, "nonce") != Some(self.nonce.as_str())
            || param(&params, "uri") != Some(self.digest_uri.as_str())
            || !param(&params, "algorithm")
                .is_some_and(|value| value.eq_ignore_ascii_case(&self.algorithm))
            || param(&params, "qop") != self.qop.as_deref()
            || param(&params, "opaque") != self.opaque.as_deref()
            || param(&params, "nc") != self.nc.as_deref()
            || param(&params, "cnonce") != self.cnonce.as_deref()
        {
            return Err(request_mismatch());
        }
        let response = param(&params, "response").ok_or_else(request_mismatch)?;
        verify_digest(response, &self.request_digest)
    }

    pub fn verify_response(&self, response: &[u8]) -> Result<VerifiedAuthenticationInfo, ImsError> {
        validate_response_authentication(Some(self), response)
    }
}

/// A successfully validated Authentication-Info result. `rspauth_verified` is
/// false when optional rspauth was absent, even if a valid nextnonce was present.
/// A nextnonce alone does not authenticate the peer; compatibility in that case
/// continues to rely on the existing transport/security policy.
#[derive(Clone)]
pub struct VerifiedAuthenticationInfo {
    nextnonce: Option<String>,
    rspauth_verified: bool,
}

impl VerifiedAuthenticationInfo {
    pub fn nextnonce(&self) -> Option<&str> {
        self.nextnonce.as_deref()
    }

    pub fn rspauth_verified(&self) -> bool {
        self.rspauth_verified
    }
}

impl fmt::Debug for VerifiedAuthenticationInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VerifiedAuthenticationInfo { [REDACTED] }")
    }
}

/// Validate both header families before exposing any nextnonce. Missing optional
/// Authentication-Info/rspauth is compatible; an unsolicited field is not. This
/// stack currently emits one Authorization scope per request, so a field from
/// the other scope always fails closed, even alongside a valid expected field.
/// Parsing/verification errors are stable codes containing no wire material.
pub fn validate_response_authentication(
    proof: Option<&DigestRequestProof>,
    response: &[u8],
) -> Result<VerifiedAuthenticationInfo, ImsError> {
    let headers = auth_headers(response, "Authentication-Info", "Proxy-Authentication-Info")?;
    let mut status = headers.start_line.splitn(3, ' ');
    if status.next() != Some("SIP/2.0")
        || !status.next().is_some_and(|code| code.len() == 3
            && code.bytes().all(|b| b.is_ascii_digit())
            && matches!(code.as_bytes()[0], b'1'..=b'6'))
    {
        return Err(invalid());
    }
    if headers.fields.is_empty() {
        return Ok(VerifiedAuthenticationInfo { nextnonce: None, rspauth_verified: false });
    }
    let proof = proof.ok_or_else(|| ImsError::new("digest_response_auth_context_missing"))?;
    if headers.fields.iter().any(|(proxy, _)| *proxy != proof.proxy) {
        return Err(ImsError::new("digest_response_auth_scope_mismatch"));
    }
    // Combining *all* same-scope fields before the existing bounded parser
    // enforces its 64-parameter limit and case-insensitive duplicate detection
    // across both repeated and folded fields, not merely within each field.
    let mut combined = String::new();
    for (_, value) in &headers.fields {
        if !combined.is_empty() {
            combined.push(',');
        }
        // Each repeated field is an auth-param list in its own right. Quotes
        // must not open in one field and close in another, hiding rspauth as
        // part of a preceding nextnonce/extension value after comma-joining.
        bounded_params(value)?;
        if combined.len() + value.len() > MAX_AUTH_BYTES {
            return Err(invalid());
        }
        combined.push_str(value);
    }
    let params = bounded_params(&combined)?;
    // Empty quoted extension values are legal. Values that can update nonce
    // state or assert response authentication must, however, be nonempty.
    if params.iter().any(|(key, value)| value.is_empty() && matches!(key.as_str(),
        "nextnonce" | "rspauth" | "qop" | "nc" | "cnonce"))
    {
        return Err(invalid());
    }
    let qop = param(&params, "qop");
    let nc = param(&params, "nc");
    let cnonce = param(&params, "cnonce");
    let rspauth = param(&params, "rspauth");
    if nc.is_some_and(|value| !valid_nc(value)) {
        return Err(invalid());
    }
    if let Some(expected_qop) = proof.qop.as_deref() {
        if qop.is_some_and(|value| value != expected_qop)
            || nc.is_some_and(|value| Some(value) != proof.nc.as_deref())
            || cnonce.is_some_and(|value| Some(value) != proof.cnonce.as_deref())
        {
            return Err(mismatch());
        }
        if rspauth.is_some() && (qop.is_none() || nc.is_none() || cnonce.is_none()) {
            return Err(invalid());
        }
    } else if qop.is_some() || nc.is_some() || cnonce.is_some() {
        return Err(mismatch());
    }
    if let Some(value) = rspauth {
        verify_digest(value, &proof.response_digest)?;
    }
    Ok(VerifiedAuthenticationInfo {
        nextnonce: param(&params, "nextnonce").map(str::to_owned),
        rspauth_verified: rspauth.is_some(),
    })
}

fn valid_value(value: &str) -> bool {
    !value.is_empty() && valid_parameter_value(value)
}

fn valid_parameter_value(value: &str) -> bool {
    value.len() <= MAX_PARAM_VALUE_BYTES && !value.chars().any(char::is_control)
}

fn valid_nc(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|b| b.is_ascii_hexdigit())
        && value.bytes().any(|b| b != b'0')
}

fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
}

fn decode_proof_digest(value: &str, byte_len: usize) -> Result<Vec<u8>, ImsError> {
    // ASCII precheck also prevents the legacy decode_hex helper from slicing
    // a multi-byte codepoint on attacker-controlled input.
    if value.len() != byte_len * 2 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    decode_hex(value).map_err(|_| invalid())
}

#[allow(deprecated)]
fn verify_digest(value: &str, expected: &[u8]) -> Result<(), ImsError> {
    let decoded = decode_proof_digest(value, expected.len())?;
    // ring 0.17 already supplies this constant-time, equal-length comparison.
    // Its deprecation recommends another dependency, not ordinary byte/string
    // equality; retain the available audited primitive without changing deps.
    ring::constant_time::verify_slices_are_equal(&decoded, expected).map_err(|_| mismatch())
}

fn token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

fn bounded_params(input: &str) -> Result<Vec<(String, String)>, ImsError> {
    // The challenge parser intentionally accepts some deployed unquoted nonce
    // forms. Responses/actual Authorization use auth-param token/quoted-string
    // grammar; enforce it here without changing challenge compatibility.
    let mut quoted = false;
    let mut escaped = false;
    let mut seen_equals = false;
    for byte in input.bytes() {
        if escaped {
            escaped = false;
        } else if quoted && byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            quoted = !quoted;
        } else if !quoted {
            match byte {
                b'=' if !seen_equals => seen_equals = true,
                b',' => seen_equals = false,
                b' ' | b'\t' => {}
                byte if token(byte) => {}
                _ => return Err(invalid()),
            }
        }
    }
    let params = parse_digest_params(input).map_err(|_| invalid())?;
    if params.is_empty() || params.iter().any(|(key, value)|
        key.len() > MAX_PARAM_NAME_BYTES || !valid_parameter_value(value))
    {
        return Err(invalid());
    }
    Ok(params)
}

struct AuthHeaders<'a> {
    start_line: &'a str,
    fields: Vec<(bool, String)>,
}

/// Bounded header-only scanner. Never lossy-decodes or scans/allocates a body.
/// Folded/repeated auth fields share a single wire/unfolding budget. Unknown
/// fields are not copied, but malformed framing/controls still fail closed.
fn auth_headers<'a>(
    frame: &'a [u8],
    origin_name: &str,
    proxy_name: &str,
) -> Result<AuthHeaders<'a>, ImsError> {
    let bounded = &frame[..frame.len().min(MAX_HEADER_BYTES)];
    let crlf_end = bounded.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
    let lf_end = bounded.windows(2).position(|w| w == b"\n\n").map(|i| i + 2);
    let end = match (crlf_end, lf_end) {
        (Some(a), Some(b)) => a.min(b),
        (Some(end), None) | (None, Some(end)) => end,
        (None, None) => return Err(invalid()),
    };
    let text = std::str::from_utf8(&bounded[..end]).map_err(|_| invalid())?;
    let mut lines = text.split('\n');
    let raw_start = lines.next().ok_or_else(invalid)?;
    let start_line = raw_start.strip_suffix('\r').unwrap_or(raw_start);
    if start_line.is_empty() || start_line.chars().any(char::is_control) {
        return Err(invalid());
    }
    let mut fields: Vec<(bool, String)> = Vec::new();
    let mut current_auth: Option<usize> = None;
    let mut has_header = false;
    let mut auth_bytes = 0usize;
    for (index, raw_line) in lines.enumerate() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.chars().any(|ch| ch.is_control() && ch != '\t') {
            return Err(invalid());
        }
        if line.is_empty() {
            break;
        }
        if index >= MAX_HEADER_LINES {
            return Err(invalid());
        }
        if line.starts_with([' ', '\t']) {
            if !has_header {
                return Err(invalid());
            }
            if let Some(field_index) = current_auth {
                auth_bytes += line.len() + 2;
                if auth_bytes > MAX_AUTH_BYTES {
                    return Err(invalid());
                }
                let value: &mut String = &mut fields[field_index].1;
                value.push(' ');
                value.push_str(line.trim_start_matches([' ', '\t']));
            }
            continue;
        }
        let (name, value) = line.split_once(':').ok_or_else(invalid)?;
        let name = name.trim_end_matches([' ', '\t']);
        if name.is_empty() || !name.bytes().all(token) {
            return Err(invalid());
        }
        has_header = true;
        current_auth = None;
        if name.eq_ignore_ascii_case(origin_name) || name.eq_ignore_ascii_case(proxy_name) {
            auth_bytes += line.len() + 2;
            if auth_bytes > MAX_AUTH_BYTES {
                return Err(invalid());
            }
            current_auth = Some(fields.len());
            fields.push((name.eq_ignore_ascii_case(proxy_name),
                value.trim_start_matches([' ', '\t']).to_owned()));
        }
    }
    Ok(AuthHeaders { start_line, fields })
}

#[cfg(test)]
#[path = "digest_response_auth_tests.rs"]
mod tests;
