//! Privacy-safe, bounded REGISTER-failure diagnostics, not SIP negotiation.
//!
//! Nothing returned here may feed offer acceptance or authorize a retry. Labels
//! describe only observed syntax, not supported/selected algorithms or the cause
//! of a failure. No input strings or identifiers escape through `Summary`.

use super::sip;

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_ENTRIES: usize = 16;
const MAX_PARAMETERS: usize = 32;

/// Unique, first-seen labels/codes; vectors are not positionally associated.
/// SHA1 spellings `hmac-sha-1-96` and `hmac-sha1-96` share one canonical label.
/// Missing fields produce no labels; unrecognized tokens produce `unknown`.
/// Any malformed/ambiguous or over-limit header discards *all* observations.
#[derive(Debug, Default)]
pub(super) struct Summary {
    /// `ipsec-3gpp`, `tls`, or `unknown`.
    pub(super) security_mechanisms: Vec<&'static str>,
    /// `hmac-sha-1-96`, `hmac-md5-96`, or `unknown`.
    pub(super) integrity: Vec<&'static str>,
    /// `aes-cbc`, `null`, or `unknown`.
    pub(super) encryption: Vec<&'static str>,
    pub(super) malformed: bool,
    pub(super) warning_codes: Vec<u16>,
    /// `authentication_failure`, `security_client`, `security_agreement`, or `other`.
    pub(super) warning_classes: Vec<&'static str>,
    /// Only a numeric REGISTER CSeq; arbitrary trailing fields never escape.
    pub(super) response_cseq: Option<u32>,
}

/// Examine at most 16 KiB through a complete CRLFCRLF header terminator, and at
/// most 16 Security-Server/Warning list entries combined (before deduplication).
/// Bodies are neither scanned nor interpreted. Header truncation is malformed.
/// Conservatively reject folding, control characters (including HTAB), invalid
/// UTF-8, non-ASCII whitespace and noncanonical line endings instead of guessing.
pub(super) fn summarize(response: &[u8]) -> Summary {
    summarize_checked(response).unwrap_or_else(|| Summary {
        malformed: true,
        ..Summary::default()
    })
}

fn summarize_checked(response: &[u8]) -> Option<Summary> {
    let headers = bounded_headers(response)?;
    let mut summary = Summary::default();
    let cseq = sip::header_values(headers, "CSeq");
    if let [value] = cseq.as_slice() {
        let mut fields = value.split_whitespace();
        let number = fields.next().and_then(|n| n.parse::<u32>().ok());
        if fields.next().is_some_and(|m| m.eq_ignore_ascii_case("REGISTER")) && fields.next().is_none() {
            summary.response_cseq = number;
        }
    }
    let mut entries = 0;

    // Only pass the bounded, validated header section to the shared extractor:
    // it is otherwise deliberately permissive about incomplete/folded input.
    for value in sip::header_values(headers, "Security-Server") {
        let offers = split_quoted(&value, b',', MAX_ENTRIES - entries)?;
        entries += offers.len();
        for offer in offers {
            summarize_security(offer, &mut summary)?;
        }
    }
    for value in sip::header_values(headers, "Warning") {
        let warnings = split_quoted(&value, b',', MAX_ENTRIES - entries)?;
        entries += warnings.len();
        for warning in warnings {
            let (code, class) = summarize_warning(warning)?;
            push_unique(&mut summary.warning_codes, code);
            push_unique(&mut summary.warning_classes, class);
        }
    }
    Some(summary)
}

fn bounded_headers(response: &[u8]) -> Option<&[u8]> {
    let prefix = &response[..response.len().min(MAX_HEADER_BYTES)];
    let end = prefix.windows(4).position(|part| part == b"\r\n\r\n")? + 4;
    let text = std::str::from_utf8(&prefix[..end - 4]).ok()?;
    let mut lines = text.split("\r\n");
    let status_line = lines.next()?;
    if status_line.chars().any(invalid_character) {
        return None;
    }
    let (status, _) = status_line.strip_prefix("SIP/2.0 ")?.split_once(' ')?;
    if status.len() != 3 || !status.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if !(100..=699).contains(&status.parse::<u16>().ok()?) {
        return None;
    }

    let mut fields = 0;
    for line in lines {
        if line.starts_with(' ') || line.chars().any(invalid_character) {
            return None;
        }
        let (name, _) = line.split_once(':')?;
        let name = name.trim_end_matches(' ');
        if !token(name) {
            return None;
        }
        if name.eq_ignore_ascii_case("Security-Server") || name.eq_ignore_ascii_case("Warning") {
            fields += 1;
            if fields > MAX_ENTRIES {
                return None;
            }
        }
    }
    Some(&prefix[..end])
}

// The shared extractor trims Unicode whitespace. Reject it before extraction
// so it cannot silently repair a noncanonical token into a recognized label.
fn invalid_character(character: char) -> bool {
    character.is_control() || (character.is_whitespace() && character != ' ')
}

fn push_unique<T: PartialEq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.!%*+`'~".contains(&byte))
}

/// Quoted commas/semicolons and quoted-pair escapes cannot create list entries.
/// Never return a partial list on exhaustion, unclosed quotes or empty entries.
fn split_quoted(value: &str, delimiter: u8, limit: usize) -> Option<Vec<&str>> {
    if limit == 0 {
        return None;
    }
    let mut parts = Vec::new();
    let (mut quoted, mut escaped, mut start) = (false, false, 0);
    for (index, byte) in value.bytes().enumerate() {
        if escaped {
            escaped = false;
        } else if quoted && byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            quoted = !quoted;
        } else if !quoted && byte == delimiter {
            let part = value[start..index].trim_matches(' ');
            if part.is_empty() || parts.len() + 1 >= limit {
                return None;
            }
            parts.push(part);
            start = index + 1;
        }
    }
    let part = value[start..].trim_matches(' ');
    if quoted || escaped || part.is_empty() || parts.len() >= limit {
        return None;
    }
    parts.push(part);
    Some(parts)
}

/// Validate one complete quoted string, not just its first/last characters.
/// Keep escapes opaque: an escaped algorithm spelling is not an exact alias.
fn quoted_content(value: &str) -> Option<&str> {
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    let mut escaped = false;
    for byte in inner.bytes() {
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            return None;
        }
    }
    (!escaped).then_some(inner)
}

fn summarize_security(offer: &str, summary: &mut Summary) -> Option<()> {
    let parts = split_quoted(offer, b';', MAX_PARAMETERS + 1)?;
    let mechanism = parts[0];
    if !token(mechanism) {
        return None;
    }
    let label = if mechanism.eq_ignore_ascii_case("ipsec-3gpp") {
        "ipsec-3gpp"
    } else if mechanism.eq_ignore_ascii_case("tls") {
        "tls"
    } else {
        "unknown"
    };
    push_unique(&mut summary.security_mechanisms, label);

    let mut names: Vec<&str> = Vec::new();
    for part in &parts[1..] {
        let (name, value) = match part.split_once('=') {
            Some((name, value)) => (name.trim_matches(' '), Some(value.trim_matches(' '))),
            None => (*part, None),
        };
        if !token(name) || names.iter().any(|previous| previous.eq_ignore_ascii_case(name)) {
            return None;
        }
        names.push(name);
        let value = match value {
            Some(value) if token(value) => Some(value),
            Some(value) => Some(quoted_content(value)?),
            None => None,
        };
        if name.eq_ignore_ascii_case("alg") {
            let value = value?;
            if value.is_empty() {
                return None;
            }
            let label = if value.eq_ignore_ascii_case("hmac-sha-1-96")
                || value.eq_ignore_ascii_case("hmac-sha1-96")
            {
                "hmac-sha-1-96"
            } else if value.eq_ignore_ascii_case("hmac-md5-96") {
                "hmac-md5-96"
            } else {
                "unknown"
            };
            push_unique(&mut summary.integrity, label);
        } else if name.eq_ignore_ascii_case("ealg") {
            let value = value?;
            if value.is_empty() {
                return None;
            }
            let label = if value.eq_ignore_ascii_case("aes-cbc") {
                "aes-cbc"
            } else if value.eq_ignore_ascii_case("null") {
                "null"
            } else {
                "unknown"
            };
            push_unique(&mut summary.encryption, label);
        }
    }
    Some(())
}

fn summarize_warning(warning: &str) -> Option<(u16, &'static str)> {
    let (code, rest) = warning.split_once(' ')?;
    if code.len() != 3 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let code = code.parse::<u16>().ok()?;
    // RFC 3261 SIP warning codes occupy 300..399. Do not emit arbitrary numbers
    // from warning agents/text or accept status codes as warning codes.
    if !(300..=399).contains(&code) {
        return None;
    }
    let (agent, quoted) = rest.trim_start_matches(' ').split_once(' ')?;
    if agent.is_empty()
        || !agent.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || b"-_.!%*+`'~:[]".contains(&byte)
        })
    {
        return None;
    }
    let text = quoted_content(quoted.trim_matches(' '))?;
    Some((code, warning_class(text)))
}

fn warning_class(text: &str) -> &'static str {
    let (mut authentication, mut client, mut agreement) = (false, false, false);
    let mut previous: &str = "";
    for word in text
        .split(|character: char| !character.is_ascii_alphabetic() && character != '-')
        .filter(|word| !word.is_empty())
    {
        authentication |= previous.eq_ignore_ascii_case("authentication")
            && word.eq_ignore_ascii_case("failure");
        client |= word.eq_ignore_ascii_case("security-client")
            || (previous.eq_ignore_ascii_case("security") && word.eq_ignore_ascii_case("client"));
        agreement |= word.eq_ignore_ascii_case("sec-agree")
            || word.eq_ignore_ascii_case("security-agreement")
            || (previous.eq_ignore_ascii_case("security") && word.eq_ignore_ascii_case("agreement"));
        previous = word;
    }
    if authentication {
        "authentication_failure"
    } else if client {
        "security_client"
    } else if agreement {
        "security_agreement"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(headers: &str) -> Vec<u8> {
        format!("SIP/2.0 421 Extension Required\r\n{headers}\r\n\r\n").into_bytes()
    }

    fn assert_empty(summary: &Summary) {
        assert!(summary.security_mechanisms.is_empty());
        assert!(summary.integrity.is_empty());
        assert!(summary.encryption.is_empty());
        assert!(summary.warning_codes.is_empty());
        assert!(summary.warning_classes.is_empty());
    }

    fn assert_malformed(frame: &[u8]) {
        let summary = summarize(frame);
        assert!(summary.malformed);
        assert_empty(&summary);
    }

    #[test]
    fn cseq_diagnostics_never_expose_untrusted_method_or_trailing_fields() {
        for value in ["7 REGISTER private-sentinel", "7 method-sentinel", "private-sentinel REGISTER"] {
            let summary = summarize(&response(&format!("CSeq: {value}")));
            assert_eq!(summary.response_cseq, None);
            assert!(!format!("{summary:?}").contains("sentinel"));
        }
        assert_eq!(summarize(&response("CSeq: 7 REGISTER")).response_cseq, Some(7));
        assert_eq!(summarize(&response("CSeq: 7 REGISTER\r\nCSeq: 8 REGISTER")).response_cseq, None);
    }

    #[test]
    fn known_421_warnings_and_algorithm_aliases() {
        let summary = summarize(&response(concat!(
            "sEcUrItY-sErVeR: IPSEC-3GPP;alg=HMAC-SHA1-96;ealg=AES-CBC, ",
            "ipsec-3gpp;alg=\"hmac-sha-1-96\";ealg=null, ",
            "ipsec-3gpp;alg=hmac-md5-96;ealg=null, TLS\r\n",
            "wArNiNg: 399 pcscf.example \"Authentication Failure\", ",
            "399 pcscf.example \"Security-Client header missing\"\r\n",
            "Warning: 380 [2001:db8::1]:5060 \"Without sec-agree and security is configured on\""
        )));
        assert!(!summary.malformed);
        assert_eq!(summary.security_mechanisms, vec!["ipsec-3gpp", "tls"]);
        assert_eq!(summary.integrity, vec!["hmac-sha-1-96", "hmac-md5-96"]);
        assert_eq!(summary.encryption, vec!["aes-cbc", "null"]);
        assert_eq!(summary.warning_codes, vec![399, 380]);
        assert_eq!(summary.warning_classes, vec![
            "authentication_failure", "security_client", "security_agreement"
        ]);
    }

    #[test]
    fn debug_never_contains_privacy_sentinels() {
        let frame = response(concat!(
            "Security-Server: mechanism-sentinel;alg=integrity-sentinel;ealg=encryption-sentinel;",
            "spi-c=294817356;spi-s=294817357;port-c=49177;port-s=49178;",
            "nonce=\"nonce-sentinel\";username=\"username-sentinel\";",
            "realm=\"realm-sentinel\";uri=\"sip:uri-sentinel@example.invalid\"\r\n",
            "Warning: 399 agent-sentinel.invalid:49179 \"Authentication Failure, text-sentinel\"\r\n",
            "WWW-Authenticate: Digest nonce=\"auth-nonce-sentinel\", realm=\"auth-realm-sentinel\"\r\n",
            "To: <sip:subscriber-sentinel@example.invalid>\r\n",
            "X-private-sentinel: header-value-sentinel"
        ));
        let summary = summarize(&frame);
        assert!(!summary.malformed);
        assert_eq!(summary.security_mechanisms, vec!["unknown"]);
        assert_eq!(summary.integrity, vec!["unknown"]);
        assert_eq!(summary.encryption, vec!["unknown"]);
        let debug = format!("{summary:?}");
        for sentinel in [
            "sentinel", "294817356", "294817357", "49177", "49178", "49179",
            "sip:", "example.invalid", "Security-Server", "WWW-Authenticate",
            "Authentication Failure", "nonce", "username", "realm",
        ] {
            assert!(!debug.contains(sentinel));
        }
    }

    #[test]
    fn quoted_commas_semicolons_and_escapes_are_not_entries() {
        let summary = summarize(&response(concat!(
            "Security-Server: ipsec-3gpp;alg=\"opaque, tls;alg=hmac-md5-96\";",
            "ealg=\"opaque\\\",null\";x=\"comma,semicolon;escaped\\\\\", tls\r\n",
            "Warning: 399 proxy \"opaque \\\"quote, 380 fake\\\", Security-Client\", ",
            "381 proxy \"unclassified\""
        )));
        assert!(!summary.malformed);
        assert_eq!(summary.security_mechanisms, vec!["ipsec-3gpp", "tls"]);
        assert_eq!(summary.integrity, vec!["unknown"]);
        assert_eq!(summary.encryption, vec!["unknown"]);
        assert_eq!(summary.warning_codes, vec![399, 381]);
        assert_eq!(summary.warning_classes, vec!["security_client", "other"]);
    }

    #[test]
    fn malformed_lists_clear_even_previously_collected_values() {
        for bad in [
            "Warning: 399 proxy \"unterminated",
            "Warning: 399 proxy \"dangling\\",
            "Warning: 399 proxy \"closed\" trailing",
            "Warning: 399 proxy \"two\" \"strings\"",
            "Warning: 399 proxy unquoted",
            "Warning: 399 \"agent\" \"text\"",
            "Warning: 399 proxy \"text\",",
            "Warning: ,399 proxy \"text\"",
            "Warning: 399 proxy \"text\",,380 proxy \"text\"",
            "Security-Server: ipsec-3gpp;alg=\"unterminated",
            "Security-Server: ipsec-3gpp;alg=prefix\"quoted\"",
            "Security-Server: ipsec-3gpp;alg=\"two\"\"strings\"",
            "Security-Server: ipsec-3gpp;alg=hmac-sha1-96;ALG=hmac-md5-96",
            "Security-Server: ipsec-3gpp;x=one;X=two",
            "Security-Server: ipsec-3gpp;alg=",
            "Security-Server: ipsec-3gpp;ealg=\"\"",
            "Security-Server: ipsec-3gpp;alg",
            "Security-Server: tls;",
            "Security-Server: tls,,tls",
            "Security-Server:",
            "Warning:",
        ] {
            assert_malformed(&response(&format!(
                "Security-Server: tls\r\nWarning: 399 proxy \"Authentication Failure\"\r\n{bad}"
            )));
        }
    }

    #[test]
    fn controls_folding_invalid_utf8_and_line_endings_are_malformed() {
        for control in [
            '\0', '\t', '\n', '\r', '\u{000b}', '\u{001b}', '\u{007f}', '\u{0085}',
            '\u{00a0}', '\u{2028}',
        ] {
            assert_malformed(&response(&format!(
                "Warning: 399 proxy \"Authentication{control} Failure\""
            )));
            assert_malformed(&response(&format!("Security-Server: tls{control}")));
            assert_malformed(&response(&format!("X-Unrelated: ignored{control}")));
        }
        assert_malformed(&response("Security-Server: tls,\r\n ipsec-3gpp;alg=hmac-md5-96"));
        assert_malformed(&response("X-Unrelated: value\r\n Warning: 399 proxy \"text\""));
        assert_malformed(b"SIP/2.0 421 Result\nWarning: 399 proxy \"text\"\n\n");
        assert_malformed(b"SIP/2.0 421 Result\r\nWarning: 399 proxy \"\xff\"\r\n\r\n");
        assert_malformed(b"Warning: 399 proxy \"text\"\r\n\r\n");
        assert_malformed(&response("invalid-header-without-colon"));
    }

    #[test]
    fn only_three_digit_sip_warning_range_is_emitted() {
        for code in ["99", "0399", "+399", "-399", "3a9", "299", "400", "421", "999", "65535"] {
            assert_malformed(&response(&format!("Warning: {code} proxy \"text\"")));
        }
        let summary = summarize(&response(
            "Warning: 300 proxy \"text 49177\", 399 proxy \"text 294817356\""
        ));
        assert!(!summary.malformed);
        assert_eq!(summary.warning_codes, vec![300, 399]);
        assert_eq!(summary.warning_classes, vec!["other"]);
    }

    #[test]
    fn exact_entry_limit_unknowns_and_duplicates_are_bounded() {
        let offers: Vec<_> = (0..MAX_ENTRIES)
            .map(|index| format!("private-{index};alg=private-alg-{index};ealg=private-ealg-{index}"))
            .collect();
        let summary = summarize(&response(&format!("Security-Server: {}", offers.join(","))));
        assert!(!summary.malformed);
        assert_eq!(summary.security_mechanisms, vec!["unknown"]);
        assert_eq!(summary.integrity, vec!["unknown"]);
        assert_eq!(summary.encryption, vec!["unknown"]);
        assert!(!format!("{summary:?}").contains("private"));
        assert_malformed(&response(&format!("Security-Server: {},tls", offers.join(","))));

        let warnings = vec!["399 private-agent \"private-text\""; MAX_ENTRIES];
        let summary = summarize(&response(&format!("Warning: {}", warnings.join(","))));
        assert!(!summary.malformed);
        assert_eq!(summary.warning_codes, vec![399]);
        assert_eq!(summary.warning_classes, vec!["other"]);
        assert!(!format!("{summary:?}").contains("private"));
        assert_malformed(&response(&format!(
            "Warning: {},399 private-agent \"private-text\"", warnings.join(",")
        )));
        assert_malformed(&response(&format!(
            "Security-Server: tls\r\nWarning: {}", warnings.join(",")
        )));
        assert_malformed(&response(&vec!["Security-Server: tls"; MAX_ENTRIES + 1].join("\r\n")));
    }

    #[test]
    fn duplicate_headers_and_parameters_cannot_leak_values() {
        let summary = summarize(&response(concat!(
            "Security-Server: tls\r\nSecurity-Server: TLS\r\n",
            "Warning: 399 private-agent-a \"private-text-a\"\r\n",
            "Warning: 399 private-agent-b \"private-text-b\""
        )));
        assert!(!summary.malformed);
        assert_eq!(summary.security_mechanisms, vec!["tls"]);
        assert_eq!(summary.warning_codes, vec![399]);
        assert_eq!(summary.warning_classes, vec!["other"]);
        assert!(!format!("{summary:?}").contains("private"));
        assert_malformed(&response(
            "Security-Server: ipsec-3gpp;alg=private-a;alg=private-b"
        ));
    }

    #[test]
    fn parameter_count_is_bounded() {
        let parameters: Vec<_> = (0..MAX_PARAMETERS).map(|index| format!("p{index}=x")).collect();
        let header = format!("Security-Server: tls;{}", parameters.join(";"));
        assert!(!summarize(&response(&header)).malformed);
        assert_malformed(&response(&format!("{header};extra=x")));
    }

    #[test]
    fn header_byte_boundary_truncation_and_body_isolation() {
        let mut frame = response("Security-Server: tls\r\nX-Padding: ");
        let padding = vec![b'x'; MAX_HEADER_BYTES - frame.len()];
        frame.splice(frame.len() - 4..frame.len() - 4, padding);
        assert_eq!(frame.len(), MAX_HEADER_BYTES);
        assert!(!summarize(&frame).malformed);
        let mut too_long = frame.clone();
        too_long.insert(too_long.len() - 4, b'x');
        assert_malformed(&too_long);
        for length in [0, 1, MAX_HEADER_BYTES - 4, MAX_HEADER_BYTES - 3,
            MAX_HEADER_BYTES - 2, MAX_HEADER_BYTES - 1]
        {
            assert_malformed(&frame[..length]);
        }
        let short = response("Security-Server: tls");
        for length in 0..short.len() {
            assert_malformed(&short[..length]);
        }
        frame.extend_from_slice(b"Warning: 399 body-agent \"Authentication Failure\"\0\xff");
        frame.extend(vec![b'x'; MAX_HEADER_BYTES * 2]);
        let summary = summarize(&frame);
        assert!(!summary.malformed);
        assert_eq!(summary.security_mechanisms, vec!["tls"]);
        assert!(summary.warning_codes.is_empty());
        assert!(summary.warning_classes.is_empty());
    }

    #[test]
    fn absent_headers_are_not_unknown_and_classes_use_only_warning_text() {
        let empty = summarize(b"SIP/2.0 421 Extension Required\r\n\r\n");
        assert!(!empty.malformed);
        assert_empty(&empty);
        let summary = summarize(&response(concat!(
            "Warning: 399 Security-Client \"opaque\", ",
            "399 proxy \"SECURITY CLIENT missing\", ",
            "399 proxy \"SECURITY AGREEMENT required\", ",
            "399 proxy \"notauthentication failureish\""
        )));
        assert!(!summary.malformed);
        assert_eq!(summary.warning_classes, vec!["other", "security_client", "security_agreement"]);
    }
}
