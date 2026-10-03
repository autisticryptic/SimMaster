//! Bounded RFC 3329 server-offer selection. Select one installable agreement,
//! but echo the complete, ordered Security-Server list in Security-Verify.
//! Build a bounded complete client offer and accept only compatible server
//! selections when the profile requires strict matching.
use std::collections::{HashMap, HashSet};

use super::{
    errors::{code, CellularImsError},
    ipsec::{self, SecAgree, XfrmAlgs},
};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_OFFERS: usize = 16;
const MAX_PARAMETERS: usize = 32;

pub(super) struct Agreement {
    pub binding: SecAgree,
    pub algorithms: XfrmAlgs,
    // Kept private to the registration flow; never log the raw header list.
    pub verify: String,
}

fn invalid() -> CellularImsError {
    CellularImsError::new(code::SECURITY_SERVER_INVALID)
}

/// Split only outside quoted strings, respecting quoted-pair escapes. A bad
/// quote or empty list element is ambiguous and must not merge two offers.
fn split_quoted(value: &str, delimiter: u8, limit: usize) -> Result<Vec<&str>, CellularImsError> {
    let mut result = Vec::new();
    let (mut quoted, mut escaped, mut start) = (false, false, 0);
    for (index, byte) in value.bytes().enumerate() {
        if escaped {
            escaped = false;
        } else if quoted && byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            quoted = !quoted;
        } else if !quoted && byte == delimiter {
            let part = value[start..index].trim();
            if part.is_empty() || result.len() + 1 >= limit {
                return Err(invalid());
            }
            result.push(part);
            start = index + 1;
        }
    }
    let part = value[start..].trim();
    if quoted || escaped || part.is_empty() || result.len() >= limit {
        return Err(invalid());
    }
    result.push(part);
    Ok(result)
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.!%*+`'~".contains(&byte))
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}

fn quality(value: &str) -> Option<u16> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match whole {
        "1" if fraction.bytes().all(|b| b == b'0') => Some(1000),
        "0" => {
            let mut value = 0;
            for digit in fraction.bytes() {
                value = value * 10 + u16::from(digit - b'0');
            }
            Some(value * 10u16.pow((3 - fraction.len()) as u32))
        }
        _ => None,
    }
}

/// Serialize every explicitly allowed, installable alternative with the SAME
/// reserved tuple. No client q-values, inferred algorithms or silent truncation.
/// Authenticated requests repeat this frozen list, not just the selected entry.
pub(super) fn client_offer(
    binding: SecAgree,
    allowed: &[&str],
    compact: bool,
    spaced: bool,
) -> Result<String, CellularImsError> {
    let invalid = || CellularImsError::new(code::SECURITY_CLIENT_INVALID);
    if allowed.len() > MAX_OFFERS {
        return Err(invalid());
    }
    if allowed.is_empty() {
        return Ok(String::new());
    }
    if binding.spi_c == 0 || binding.spi_s == 0 || binding.port_c == 0 || binding.port_s == 0 {
        return Err(invalid());
    }
    let mut seen = HashSet::new();
    let mut offers = Vec::new();
    let separator = if spaced { "; " } else { ";" };
    for mechanism in allowed {
        let parts = mechanism.split('/').collect::<Vec<_>>();
        if parts.len() != 4
            || parts.iter().any(|part| !token(part))
            || !parts[2].eq_ignore_ascii_case("esp")
            || !parts[3].eq_ignore_ascii_case("trans")
        {
            return Err(invalid());
        }
        ipsec::xfrm_algs_from_security_server(&format!(
            "ipsec-3gpp;alg={};ealg={}",
            parts[0], parts[1]
        ))
        .map_err(|_| invalid())?;
        if !seen.insert(mechanism.to_ascii_lowercase()) {
            continue;
        }
        let mut fields = vec![
            "ipsec-3gpp".to_string(),
            format!("alg={}", parts[0]),
            format!("ealg={}", parts[1]),
        ];
        if !compact {
            fields.extend([format!("prot={}", parts[2]), format!("mod={}", parts[3])]);
        }
        fields.extend([
            format!("spi-c={}", binding.spi_c),
            format!("spi-s={}", binding.spi_s),
            format!("port-c={}", binding.port_c),
            format!("port-s={}", binding.port_s),
        ]);
        offers.push(fields.join(separator));
    }
    let header = offers.join(", ");
    if header.len() > MAX_HEADER_BYTES {
        return Err(invalid());
    }
    Ok(header)
}

/// No header means no negotiation. A present but unusable/ambiguous list is
/// an error, not permission to continue unprotected. Known-but-incomplete
/// agreements also fail closed rather than silently selecting a weaker one.
pub(super) fn select(
    values: &[String],
    allowed: &[&str],
    strict: bool,
) -> Result<Option<Agreement>, CellularImsError> {
    if values.is_empty() {
        return Ok(None);
    }
    let bytes = values.iter().try_fold(0usize, |sum, value| {
        sum.checked_add(value.len()).ok_or_else(invalid)
    })?;
    if values.len() > MAX_OFFERS
        || bytes > MAX_HEADER_BYTES
        || values.iter().any(|value| {
            value
                .chars()
                .any(|character| character.is_control() && character != '\t')
        })
    {
        return Err(invalid());
    }
    let mut offers = Vec::new();
    for value in values {
        offers.extend(split_quoted(value, b',', MAX_OFFERS)?);
        if offers.len() > MAX_OFFERS {
            return Err(invalid());
        }
    }
    let mut explicit_quality = HashSet::new();
    let mut selected: Option<(u16, HashMap<String, &str>, XfrmAlgs)> = None;
    for offer in offers {
        let parts = split_quoted(offer, b';', MAX_PARAMETERS + 1)?;
        if !token(parts[0]) {
            return Err(invalid());
        }
        let mut parameters = HashMap::new();
        for part in &parts[1..] {
            let (name, value) = part.split_once('=').unwrap_or((part, ""));
            let name = name.trim().to_ascii_lowercase();
            if !token(&name) {
                return Err(invalid());
            }
            if parameters.insert(name, value.trim()).is_some() {
                return Err(invalid());
            }
        }
        // Legacy single-offer deployments often omit q. Preserve their
        // behavior (and wire order for equally implicit preferences). Explicit
        // RFC 3329 q-values must be valid and distinct, even for unknown offers.
        let preference = match parameters.get("q") {
            Some(value) => {
                let q = quality(value).ok_or_else(invalid)?;
                if !explicit_quality.insert(q) {
                    return Err(invalid());
                }
                q
            }
            None => 1000,
        };
        if !parts[0].eq_ignore_ascii_case("ipsec-3gpp") {
            continue;
        }
        let get = |name: &str| parameters.get(name).map(|value| unquote(value));
        let (Some(alg), Some(ealg)) = (get("alg"), get("ealg")) else {
            return Err(invalid());
        };
        let prot = get("prot").unwrap_or("esp");
        let mode = get("mod").unwrap_or("trans");
        if !token(alg) || !token(ealg) || !token(prot) || !token(mode) {
            return Err(invalid());
        }
        if !prot.eq_ignore_ascii_case("esp") || !mode.eq_ignore_ascii_case("trans") {
            continue;
        }
        if strict
            && !allowed.iter().any(|mechanism| {
                let expected = mechanism.split('/').collect::<Vec<_>>();
                expected.len() == 4
                    && [alg, ealg, prot, mode]
                        .into_iter()
                        .zip(expected)
                        .all(|(actual, expected)| actual.eq_ignore_ascii_case(expected))
            })
        {
            continue;
        }
        // Pass only validated algorithm tokens to the existing XFRM mapping.
        // An unknown quoted extension must not inject an alg/ealg parameter.
        let algorithm_value = format!("ipsec-3gpp;alg={alg};ealg={ealg}");
        let Ok(algorithms) = ipsec::xfrm_algs_from_security_server(&algorithm_value) else {
            continue;
        };
        if selected
            .as_ref()
            .is_none_or(|(best, _, _)| preference > *best)
        {
            selected = Some((preference, parameters, algorithms));
        }
    }
    let (_, parameters, algorithms) = selected.ok_or_else(invalid)?;
    // RFC 3329: incomplete information for the chosen highest-preference
    // mechanism aborts negotiation; do not downgrade to a lower alternative.
    let mut binding_value = String::from("ipsec-3gpp");
    for name in ["spi-c", "spi-s", "port-c", "port-s"] {
        let value = unquote(parameters.get(name).ok_or_else(invalid)?);
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'x' || byte == b'X')
        {
            return Err(invalid());
        }
        binding_value.push_str(&format!(";{name}={value}"));
    }
    let binding = ipsec::parse_security_server(&binding_value).map_err(|_| invalid())?;
    Ok(Some(Agreement {
        binding,
        algorithms,
        verify: values.join(", "),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(algorithm: &str, encryption: &str, spi: u32, q: &str) -> String {
        format!("ipsec-3gpp;alg={algorithm};ealg={encryption};prot=esp;mod=trans;spi-c={spi};spi-s={};port-c=5064;port-s=5062{q}", spi + 1)
    }

    fn choose(values: &[String]) -> Agreement {
        select(values, &[], false).unwrap().unwrap()
    }

    #[test]
    fn all_client_alternatives_share_one_tuple_and_preserve_order() {
        let binding = SecAgree {
            spi_c: 100,
            spi_s: 101,
            port_c: 5064,
            port_s: 5062,
        };
        let allowed = [
            "hmac-sha-1-96/aes-cbc/esp/trans",
            "hmac-sha-1-96/null/esp/trans",
        ];
        for (compact, spaced) in [(false, false), (false, true), (true, false)] {
            let text = client_offer(binding, &allowed, compact, spaced).unwrap();
            let alternatives = text.split(", ").collect::<Vec<_>>();
            assert_eq!(alternatives.len(), 2);
            assert!(alternatives[0].contains("ealg=aes-cbc"));
            assert!(alternatives[1].contains("ealg=null"));
            assert!(!text.contains(";q="));
            for part in alternatives {
                assert_eq!(ipsec::parse_security_server(part).unwrap(), binding);
            }
            let chosen = select(&[offer("hmac-sha-1-96", "null", 200, "")], &allowed, true)
                .unwrap()
                .unwrap();
            assert_eq!(chosen.algorithms.auth, "hmac(sha1)");
            assert_eq!(chosen.algorithms.enc, "cipher_null");
        }
    }

    #[test]
    fn client_offers_reject_invalid_or_excessive_lists_instead_of_silently_truncating() {
        let binding = SecAgree {
            spi_c: 100,
            spi_s: 101,
            port_c: 5064,
            port_s: 5062,
        };
        for mechanism in [
            "",
            "sha1/aes",
            "hmac-sha-1-96;evil=1/aes-cbc/esp/trans",
            "hmac-sha-1-96/null/ah/trans",
            "hmac-sha-1-96/null/esp/tun",
            "unknown/null/esp/trans",
            "hmac-sha-1-96/des-cbc/esp/trans",
        ] {
            assert_eq!(
                client_offer(binding, &[mechanism], false, false)
                    .unwrap_err()
                    .code(),
                code::SECURITY_CLIENT_INVALID
            );
        }
        let duplicate = vec!["hmac-sha-1-96/aes-cbc/esp/trans"; 17];
        assert!(client_offer(binding, &duplicate, false, false).is_err());
        assert!(!client_offer(binding, &duplicate[..2], false, false)
            .unwrap()
            .contains(','));
        assert!(client_offer(
            SecAgree {
                spi_c: 0,
                ..binding
            },
            &duplicate[..1],
            false,
            false
        )
        .is_err());
    }

    #[test]
    fn strict_selection_accepts_default_transport_but_not_an_unoffered_algorithm() {
        let allowed = [
            "hmac-sha-1-96/aes-cbc/esp/trans",
            "hmac-sha-1-96/null/esp/trans",
        ];
        let compact = offer("hmac-sha-1-96", "null", 100, "").replace(";prot=esp;mod=trans", "");
        assert!(select(&[compact], &allowed, true).unwrap().is_some());
        assert!(select(&[offer("hmac-md5-96", "null", 100, "")], &allowed, true).is_err());
        assert!(select(
            &[offer("hmac-sha-1-96", "null", 100, "")],
            &allowed[..1],
            true
        )
        .is_err());
    }

    #[test]
    fn single_legacy_offer_is_unchanged() {
        let value = offer("hmac-md5-96", "null", 100, "");
        let result = choose(&[value.clone()]);
        assert_eq!(result.binding.spi_c, 100);
        assert_eq!(result.algorithms, XfrmAlgs::default());
        assert_eq!(result.verify, value);
    }

    #[test]
    fn one_line_and_multiple_lines_select_the_same_complete_offer() {
        let a = offer("hmac-sha-1-96", "aes-cbc", 100, ";q=0.2");
        let b = offer("hmac-md5-96", "null", 200, ";q=0.8");
        let wire = format!("{a}, {b}");
        let single = choose(&[wire.clone()]);
        let multiple = choose(&[a, b]);
        assert_eq!(single.binding, multiple.binding);
        assert_eq!(single.binding.spi_c, 200);
        assert_eq!(single.algorithms.auth, "hmac(md5)");
        assert_eq!(single.verify, wire);
        assert_eq!(multiple.verify, wire);
    }

    #[test]
    fn highest_quality_wins_but_verify_keeps_original_order() {
        let a = offer("hmac-sha-1-96", "aes-cbc", 100, ";q=1.000");
        let b = offer("hmac-md5-96", "null", 200, ";q=0.1");
        let result = choose(&[a.clone(), b.clone()]);
        assert_eq!(result.binding.spi_c, 100);
        assert_eq!(result.algorithms.enc, "cbc(aes)");
        assert_eq!(result.verify, format!("{a}, {b}"));
    }

    #[test]
    fn unsupported_first_offer_does_not_hide_supported_later_offer() {
        let a = offer("unsupported-algorithm", "null", 100, ";q=0.9");
        let b = offer("hmac-sha-1-96", "aes-cbc", 200, ";q=0.8");
        let result = choose(&[a.clone(), b.clone()]);
        assert_eq!(result.binding.spi_c, 200);
        assert_eq!(result.verify, format!("{a}, {b}"));
    }

    #[test]
    fn strict_profile_cannot_select_a_disallowed_algorithm() {
        let a = offer("hmac-md5-96", "null", 100, ";q=0.9");
        let b = offer("hmac-sha-1-96", "aes-cbc", 200, ";q=0.8");
        let allowed = ["hmac-sha-1-96/aes-cbc/esp/trans"];
        let result = select(&[a.clone(), b.clone()], &allowed, true)
            .unwrap()
            .unwrap();
        assert_eq!(result.binding.spi_c, 200);
        assert_eq!(result.verify, format!("{a}, {b}"));
        assert!(select(&[a], &allowed, true).is_err());
        assert!(select(&[b], &[""], true).is_err());
    }

    #[test]
    fn unknown_mechanisms_are_still_echoed_in_verify() {
        let a = "tls;q=0.9".to_string();
        let b = offer("hmac-md5-96", "null", 200, ";q=0.8");
        let result = choose(&[a.clone(), b.clone()]);
        assert_eq!(result.binding.spi_c, 200);
        assert_eq!(result.verify, format!("{a}, {b}"));
    }

    #[test]
    fn quoted_commas_semicolons_and_escapes_never_merge_offers() {
        let a = format!(
            "{};note=\"a,b;alg=not-an-algorithm;\\\"still-quoted\"",
            offer("hmac-sha-1-96", "aes-cbc", 100, ";q=0.9")
        );
        let b = offer("hmac-md5-96", "null", 200, ";q=0.8");
        let wire = format!("{a}, {b}");
        let result = choose(&[wire.clone()]);
        assert_eq!(result.binding.spi_c, 100);
        assert_eq!(result.algorithms.auth, "hmac(sha1)");
        assert_eq!(result.verify, wire);
    }

    #[test]
    fn tuple_fields_cannot_be_borrowed_from_another_offer() {
        let a = "ipsec-3gpp;alg=hmac-sha-1-96;ealg=aes-cbc;spi-c=100;spi-s=101;q=0.9";
        let b = "ipsec-3gpp;alg=hmac-md5-96;ealg=null;port-c=5064;port-s=5062;q=0.8";
        assert!(select(&[format!("{a}, {b}")], &[], false).is_err());
    }

    #[test]
    fn duplicate_parameters_invalid_quality_and_bad_quotes_fail_closed() {
        let base = offer("hmac-md5-96", "null", 100, "");
        for tail in [
            ";SPI-C=999",
            ";alg=hmac-sha-1-96",
            ";q=NaN",
            ";q=1.001",
            ";q=0.0001",
            ";q=\"0.5\"",
            ";note=\"unterminated",
            ",",
        ] {
            assert!(select(&[format!("{base}{tail}")], &[], false).is_err());
        }
        let same_q = [
            offer("hmac-md5-96", "null", 100, ";q=0.5"),
            offer("hmac-sha-1-96", "aes-cbc", 200, ";q=0.500"),
        ];
        assert!(select(&same_q, &[], false).is_err());
    }

    #[test]
    fn invalid_tuple_and_unsupported_transport_are_not_installed() {
        for value in [
            offer("hmac-md5-96", "null", 0, ""),
            offer("hmac-md5-96", "null", 100, "").replace("port-c=5064", "port-c=0"),
            offer("hmac-md5-96", "null", 100, "").replace("mod=trans", "mod=tun"),
            offer("hmac-md5-96", "null", 100, "").replace("prot=esp", "prot=ah"),
        ] {
            assert!(select(&[value], &[], false).is_err());
        }
    }

    #[test]
    fn absence_is_distinct_from_unusable_headers() {
        assert!(select(&[], &[], false).unwrap().is_none());
        assert!(select(&["".into()], &[], false).is_err());
        assert!(select(&["tls;q=1".into()], &[], false).is_err());
    }

    #[test]
    fn header_bounds_and_control_characters_are_rejected() {
        assert!(select(&vec!["tls".into(); MAX_OFFERS + 1], &[], false).is_err());
        assert!(select(&["x".repeat(MAX_HEADER_BYTES + 1)], &[], false).is_err());
        let base = offer("hmac-md5-96", "null", 100, "");
        for control in ["\r\nInjected: secret", "\0", "\x7f"] {
            assert!(select(&[format!("{base}{control}")], &[], false).is_err());
        }
    }

    #[test]
    fn incomplete_highest_preference_aborts_instead_of_downgrading() {
        let incomplete = "ipsec-3gpp;alg=hmac-sha-1-96;ealg=aes-cbc;spi-c=100;q=0.9".to_string();
        let complete = offer("hmac-md5-96", "null", 200, ";q=0.1");
        assert!(select(&[incomplete.clone(), complete.clone()], &[], false).is_err());
        assert!(select(&[complete, incomplete], &[], false).is_err());
    }

    #[test]
    fn legacy_compact_quoted_values_and_hex_spis_remain_supported() {
        let value = "IPSEC-3GPP;alg=\"hmac-md5-96\";ealg=null;spi-c=0x100;spi-s=0X101;port-c=\"5064\";port-s=5062".to_string();
        let selected = choose(&[value.clone()]);
        assert_eq!(selected.binding.spi_c, 256);
        assert_eq!(selected.verify, value);
    }

    #[test]
    fn stored_verify_list_reselects_the_exact_agreement_for_rollover() {
        let first = offer("hmac-sha-1-96", "aes-cbc", 100, ";q=0.9");
        let second = offer("hmac-md5-96", "null", 200, ";q=0.1");
        let selected = choose(&[first, second]);
        let previous = choose(&[selected.verify.clone()]);
        assert_eq!(previous.binding, selected.binding);
        assert_eq!(previous.algorithms, selected.algorithms);
        assert!(ipsec::validate_server_rollover(
            &previous.binding,
            &SecAgree {
                spi_c: 300,
                spi_s: 301,
                port_c: 5066,
                port_s: 5062,
            }
        )
        .is_ok());
    }
}
