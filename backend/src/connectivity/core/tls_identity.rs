//! Additional exact certificate SAN policy after rustls has verified chain,
//! time, signature, EKU and server name. Shared by SIP and the explicitly
//! configured Jegotrip TLS adapter; neither accepts wildcard/CN/URI guesses.
//! This bounded DER walk does not replace cryptographic validation.
use std::net::IpAddr;

fn element(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = input.split_first()?;
    if tag & 0x1f == 0x1f { return None; }
    let (&first, rest) = rest.split_first()?;
    let (length, rest) = if first & 0x80 == 0 {
        (usize::from(first), rest)
    } else {
        let bytes = usize::from(first & 0x7f);
        if bytes == 0 || bytes > 4 || rest.len() < bytes || rest[0] == 0 { return None; }
        let length = rest[..bytes].iter().try_fold(0usize, |length, byte| length.checked_mul(256)?.checked_add(usize::from(*byte)))?;
        if length < 128 { return None; }
        (length, &rest[bytes..])
    };
    if length > rest.len() { return None; }
    Some((tag, &rest[..length], &rest[length..]))
}

fn sequence(input: &[u8], expected: u8) -> Option<&[u8]> {
    let (tag, body, tail) = element(input)?;
    (tag == expected && tail.is_empty()).then_some(body)
}

pub(crate) fn has_exact_identity(certificate: &[u8], authority: &str) -> bool {
    if certificate.len() > 262_144 { return false; }
    let Some(cert) = sequence(certificate, 0x30) else { return false; };
    let Some((0x30, mut tbs, _)) = element(cert) else { return false; };
    let mut extensions = None;
    for _ in 0..32 {
        if tbs.is_empty() { break; }
        let Some((tag, body, tail)) = element(tbs) else { return false; };
        tbs = tail;
        if tag == 0xa3 {
            if extensions.replace(body).is_some() { return false; }
        }
    }
    if !tbs.is_empty() { return false; }
    let Some(mut extensions) = extensions.and_then(|body| sequence(body, 0x30)) else { return false; };
    let mut san = None;
    for _ in 0..64 {
        if extensions.is_empty() { break; }
        let Some((0x30, body, tail)) = element(extensions) else { return false; };
        extensions = tail;
        let Some((0x06, oid, tail)) = element(body) else { return false; };
        let Some((tag, value, tail)) = element(tail) else { return false; };
        let (tag, value, tail) = if tag == 0x01 {
            if value != [0xff] && value != [0] { return false; }
            let Some(next) = element(tail) else { return false; }; next
        } else { (tag, value, tail) };
        if tag != 0x04 || !tail.is_empty() { return false; }
        if oid == [0x55, 0x1d, 0x11] && san.replace(value).is_some() { return false; }
    }
    if !extensions.is_empty() { return false; }
    let Some(mut names) = san.and_then(|value| sequence(value, 0x30)) else { return false; };
    let ip = authority.parse::<IpAddr>().ok();
    let dns = authority.strip_suffix('.').unwrap_or(authority);
    let mut matched = false;
    for _ in 0..128 {
        if names.is_empty() { return matched; }
        let Some((tag, value, tail)) = element(names) else { return false; };
        names = tail;
        matched |= match (tag, ip) {
            (0x82, None) => value.is_ascii() && value.eq_ignore_ascii_case(dns.as_bytes()),
            (0x87, Some(IpAddr::V4(ip))) => value == ip.octets(),
            (0x87, Some(IpAddr::V6(ip))) => value == ip.octets(),
            _ => false,
        };
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tlv(tag: u8, body: &[u8]) -> Vec<u8> {
        let mut result = vec![tag];
        if body.len() < 128 { result.push(body.len() as u8); }
        else { result.extend([0x82, (body.len() >> 8) as u8, body.len() as u8]); }
        result.extend_from_slice(body); result
    }
    fn certificate(names: Vec<u8>) -> Vec<u8> {
        let mut extension = tlv(0x06, &[0x55, 0x1d, 0x11]);
        extension.extend(tlv(0x04, &tlv(0x30, &names)));
        tlv(0x30, &tlv(0x30, &tlv(0xa3, &tlv(0x30, &tlv(0x30, &extension)))))
    }
    #[test]
    fn sip_identity_rejects_wildcard_cn_guesses_and_wrong_san_types() {
        let exact = certificate(tlv(0x82, b"IMS.EXAMPLE"));
        assert!(has_exact_identity(&exact, "ims.example"));
        assert!(has_exact_identity(&exact, "ims.example."));
        assert!(!has_exact_identity(&certificate(tlv(0x82, b"*.example")), "ims.example"));
        assert!(!has_exact_identity(&certificate(tlv(0x86, b"sip:ims.example")), "ims.example"));
        assert!(!has_exact_identity(&certificate(tlv(0x82, b"127.0.0.1")), "127.0.0.1"));
        assert!(has_exact_identity(&certificate(tlv(0x87, &[127,0,0,1])), "127.0.0.1"));
    }
    #[test]
    fn sip_identity_der_is_bounded_and_never_accepts_truncated_certificates() {
        let certificate = certificate(tlv(0x82, b"ims.example"));
        for length in 0..certificate.len() { assert!(!has_exact_identity(&certificate[..length], "ims.example")); }
        assert!(!has_exact_identity(&[0x30, 0x80, 0, 0], "ims.example"));
        let mut invalid = certificate; invalid.push(0);
        assert!(!has_exact_identity(&invalid, "ims.example"));
    }
}
