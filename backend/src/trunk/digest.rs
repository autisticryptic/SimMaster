//! RFC 2617/3261 Digest authentication for the Asterisk-facing SIP endpoint.
//!
//! Operator IMS uses Digest-AKA and lives in `ims::digest_aka`; a PBX trunk
//! uses the ordinary password-based MD5 or MD5-sess variants implemented here.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestChallenge {
    pub realm: String,
    pub nonce: String,
    pub algorithm: String,
    pub qop: Option<String>,
    pub opaque: Option<String>,
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

pub fn parse_challenge(value: &str, proxy: bool) -> Result<DigestChallenge, String> {
    let trimmed = value.trim();
    let params = trimmed
        .strip_prefix("Digest ")
        .or_else(|| trimmed.strip_prefix("digest "))
        .ok_or_else(|| "trunk_digest_scheme_unsupported".to_string())?;
    let params = parse_params(params);
    let realm = required(&params, "realm", "trunk_digest_realm_missing")?;
    let nonce = required(&params, "nonce", "trunk_digest_nonce_missing")?;
    let algorithm = params
        .get("algorithm")
        .cloned()
        .unwrap_or_else(|| "MD5".to_string());
    if !algorithm.eq_ignore_ascii_case("MD5") && !algorithm.eq_ignore_ascii_case("MD5-sess") {
        return Err("trunk_digest_algorithm_unsupported".to_string());
    }
    let qop = params.get("qop").map(|raw| {
        raw.split(',')
            .map(str::trim)
            .find(|candidate| candidate.eq_ignore_ascii_case("auth"))
            .unwrap_or(raw.trim())
            .to_ascii_lowercase()
    });
    if qop.as_deref().is_some_and(|qop| qop != "auth") {
        return Err("trunk_digest_qop_unsupported".to_string());
    }
    Ok(DigestChallenge {
        realm,
        nonce,
        algorithm,
        qop,
        opaque: params.get("opaque").cloned(),
        proxy,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn build_authorization(
    challenge: &DigestChallenge,
    username: &str,
    password: &str,
    method: &str,
    digest_uri: &str,
    cnonce: &str,
    nonce_count: u32,
) -> Result<String, String> {
    let nc = format!("{nonce_count:08x}");
    let initial_ha1 = md5_hex(format!("{username}:{}:{password}", challenge.realm).as_bytes());
    let ha1 = if challenge.algorithm.eq_ignore_ascii_case("MD5-sess") {
        md5_hex(format!("{initial_ha1}:{}:{cnonce}", challenge.nonce).as_bytes())
    } else {
        initial_ha1
    };
    let ha2 = md5_hex(format!("{method}:{digest_uri}").as_bytes());
    let response = match challenge.qop.as_deref() {
        Some("auth") => {
            md5_hex(format!("{ha1}:{}:{nc}:{cnonce}:auth:{ha2}", challenge.nonce).as_bytes())
        }
        Some(_) => return Err("trunk_digest_qop_unsupported".to_string()),
        None => md5_hex(format!("{ha1}:{}:{ha2}", challenge.nonce).as_bytes()),
    };
    let mut value = format!(
        "{}: Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\", algorithm={}",
        challenge.authorization_header_name(),
        quote(username),
        quote(&challenge.realm),
        quote(&challenge.nonce),
        quote(digest_uri),
        response,
        challenge.algorithm,
    );
    if challenge.qop.is_some() {
        value.push_str(&format!(
            ", qop=auth, nc={nc}, cnonce=\"{}\"",
            quote(cnonce)
        ));
    } else if challenge.algorithm.eq_ignore_ascii_case("MD5-sess") {
        value.push_str(&format!(", cnonce=\"{}\"", quote(cnonce)));
    }
    if let Some(opaque) = &challenge.opaque {
        value.push_str(&format!(", opaque=\"{}\"", quote(opaque)));
    }
    Ok(value)
}

fn required(params: &BTreeMap<String, String>, name: &str, error: &str) -> Result<String, String> {
    params
        .get(name)
        .filter(|value| !value.is_empty())
        .cloned()
        .ok_or_else(|| error.to_string())
}

fn parse_params(input: &str) -> BTreeMap<String, String> {
    let mut params = BTreeMap::new();
    let bytes = input.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        while cursor < bytes.len() && (bytes[cursor] == b',' || bytes[cursor].is_ascii_whitespace())
        {
            cursor += 1;
        }
        let key_start = cursor;
        while cursor < bytes.len() && bytes[cursor] != b'=' {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            break;
        }
        let key = input[key_start..cursor].trim().to_ascii_lowercase();
        cursor += 1;
        let value = if cursor < bytes.len() && bytes[cursor] == b'"' {
            cursor += 1;
            let mut value = String::new();
            while cursor < bytes.len() && bytes[cursor] != b'"' {
                if bytes[cursor] == b'\\' && cursor + 1 < bytes.len() {
                    cursor += 1;
                }
                value.push(bytes[cursor] as char);
                cursor += 1;
            }
            if cursor < bytes.len() {
                cursor += 1;
            }
            value
        } else {
            let value_start = cursor;
            while cursor < bytes.len() && bytes[cursor] != b',' {
                cursor += 1;
            }
            input[value_start..cursor].trim().to_string()
        };
        if !key.is_empty() {
            params.insert(key, value);
        }
    }
    params
}

fn quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn md5_hex(bytes: &[u8]) -> String {
    format!("{:x}", md5::compute(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc2617_md5_vector_matches() {
        let challenge = DigestChallenge {
            realm: "testrealm@host.com".to_string(),
            nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093".to_string(),
            algorithm: "MD5".to_string(),
            qop: Some("auth".to_string()),
            opaque: None,
            proxy: false,
        };
        let header = build_authorization(
            &challenge,
            "Mufasa",
            "Circle Of Life",
            "GET",
            "/dir/index.html",
            "0a4f113b",
            1,
        )
        .unwrap();
        assert!(header.contains("response=\"6629fae49393a05397450978507c4ef1\""));
    }

    #[test]
    fn md5_sess_uses_nonce_and_cnonce_in_ha1() {
        let challenge = parse_challenge(
            "Digest realm=\"pbx\", nonce=\"nonce\", algorithm=MD5-sess, qop=\"auth\"",
            true,
        )
        .unwrap();
        let header = build_authorization(
            &challenge, "4101", "secret", "REGISTER", "sip:pbx", "abcdef", 1,
        )
        .unwrap();
        let base = md5_hex(b"4101:pbx:secret");
        let ha1 = md5_hex(format!("{base}:nonce:abcdef").as_bytes());
        let ha2 = md5_hex(b"REGISTER:sip:pbx");
        let expected = md5_hex(format!("{ha1}:nonce:00000001:abcdef:auth:{ha2}").as_bytes());
        assert!(header.starts_with("Proxy-Authorization: Digest"));
        assert!(header.contains(&format!("response=\"{expected}\"")));
    }

    #[test]
    fn challenge_parser_handles_quoted_qop_list() {
        let challenge = parse_challenge(
            "Digest realm=\"pbx,edge\",nonce=\"n\",qop=\"auth,auth-int\",opaque=\"o\"",
            false,
        )
        .unwrap();
        assert_eq!(challenge.realm, "pbx,edge");
        assert_eq!(challenge.qop.as_deref(), Some("auth"));
        assert_eq!(challenge.algorithm, "MD5");
    }
}
