//! Preserve the SIP helper/test path while sharing its exact SAN policy.
//! Chain/time/signature/EKU validation still belongs to rustls before this check.
pub(super) use crate::connectivity::core::tls_identity::has_exact_identity;

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
