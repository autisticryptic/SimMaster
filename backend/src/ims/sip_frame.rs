//! Transport-agnostic SIP message framing/parsing primitives (RFC 3261).
//!
//! These are byte-for-byte the routines that were duplicated in
//! `vowifi/live.rs` and `volte/sip.rs`: start-line status parsing, header/body
//! split, TCP de-coalescing via Content-Length, header value extraction, and
//! bracketed-URI extraction. They are pure and infallible or `Option`/`ImsError`
//! returning, so both legs share one implementation.

use std::net::IpAddr;

use super::ImsError;

/// Parse the SIP status code from a response frame's start line.
/// Errors: `sip_status_line_missing` / `sip_status_line_invalid` /
/// `sip_status_code_invalid` (neutral reason codes; callers remap as needed).
pub fn parse_status(frame: &[u8]) -> Result<u16, ImsError> {
    let line_end = frame
        .windows(2)
        .position(|w| w == b"\r\n")
        .or_else(|| frame.iter().position(|b| *b == b'\n'))
        .ok_or(ImsError::new("sip_status_line_missing"))?;
    let line = std::str::from_utf8(&frame[..line_end])
        .map_err(|_| ImsError::new("sip_status_line_invalid"))?;
    let mut parts = line.split_whitespace();
    if parts.next() != Some("SIP/2.0") {
        return Err(ImsError::new("sip_status_line_invalid"));
    }
    parts
        .next()
        .and_then(|v| v.parse::<u16>().ok())
        .ok_or(ImsError::new("sip_status_code_invalid"))
}

/// Offset just past the end-of-headers terminator (CRLFCRLF, or LFLF), or None.
pub fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
        .or_else(|| buf.windows(2).position(|w| w == b"\n\n").map(|i| i + 2))
}

/// Everything after the header terminator (may be empty).
pub fn body(frame: &[u8]) -> &[u8] {
    find_header_end(frame)
        .filter(|off| *off <= frame.len())
        .map(|off| &frame[off..])
        .unwrap_or(&[])
}

/// TCP de-coalescing: exact byte length of one complete SIP message at the front
/// of `buf`, honoring Content-Length. `None` ⇒ need more bytes.
pub fn complete_frame_len(buf: &[u8]) -> Option<usize> {
    let header_end = find_header_end(buf)?;
    let headers = String::from_utf8_lossy(&buf[..header_end]);
    let content_length = headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())
            .flatten()
    });
    match content_length {
        Some(len) if buf.len() >= header_end + len => Some(header_end + len),
        Some(_) => None,
        None => Some(header_end),
    }
}

/// Whether `buf` contains at least one complete SIP message.
pub fn is_complete(buf: &[u8]) -> bool {
    complete_frame_len(buf).is_some()
}

/// Whether a frame is a SIP request for the given method (start-line check).
pub fn is_request(frame: &[u8], method: &str) -> bool {
    frame.starts_with(method.as_bytes()) && frame.get(method.len()) == Some(&b' ')
}

/// Collect all values of a header (case-insensitive name, first-colon split).
pub fn header_values(frame: &[u8], header_name: &str) -> Vec<String> {
    let headers = match find_header_end(frame) {
        Some(end) => String::from_utf8_lossy(&frame[..end]),
        None => String::from_utf8_lossy(frame),
    };
    headers
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case(header_name)
                .then(|| value.trim().to_string())
        })
        .collect()
}

/// First value of a header, if present.
pub fn header_value(frame: &[u8], header_name: &str) -> Option<String> {
    header_values(frame, header_name).into_iter().next()
}

/// Extract the bracketed `<sip:...>` URI from a named header value.
pub fn header_uri(frame: &[u8], header_name: &str) -> Option<String> {
    let value = header_value(frame, header_name)?;
    uri_from_header_value(&value)
}

/// Extract a URI from a header value: `<sip:...>` bracketed form, else the
/// leading token up to the first `;` or space.
pub fn uri_from_header_value(value: &str) -> Option<String> {
    if let Some(start) = value.find('<') {
        let rest = &value[start + 1..];
        let end = rest.find('>')?;
        return Some(rest[..end].to_string());
    }
    let trimmed = value.trim();
    let end = trimmed.find([';', ' ']).unwrap_or(trimmed.len());
    Some(trimmed[..end].to_string())
}

/// Format a host for a SIP URI: bare IPv4, bracketed IPv6 (RFC 3261 §19.1.2).
pub fn sip_host(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(a) => a.to_string(),
        IpAddr::V6(a) => format!("[{a}]"),
    }
}

/// Escape a quoted SIP header parameter value.
pub fn quote_param(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn parse_status_reads_code() {
        assert_eq!(parse_status(b"SIP/2.0 200 OK\r\n\r\n").unwrap(), 200);
        assert_eq!(
            parse_status(b"SIP/2.0 401 Unauthorized\r\nWWW-Authenticate: Digest\r\n\r\n").unwrap(),
            401
        );
        assert!(parse_status(b"garbage").is_err());
    }

    #[test]
    fn frame_len_honors_content_length_and_coalescing() {
        let msg1 = b"SIP/2.0 200 OK\r\nContent-Length: 3\r\n\r\nabc";
        let msg2 = b"MESSAGE sip:x SIP/2.0\r\nContent-Length: 0\r\n\r\n";
        let mut buf = Vec::new();
        buf.extend_from_slice(msg1);
        buf.extend_from_slice(msg2);
        let len1 = complete_frame_len(&buf).unwrap();
        assert_eq!(len1, msg1.len());
        assert_eq!(complete_frame_len(&buf[len1..]).unwrap(), msg2.len());
    }

    #[test]
    fn frame_len_needs_more_when_truncated() {
        assert!(complete_frame_len(b"SIP/2.0 200 OK\r\nContent-Length: 10\r\n\r\nabc").is_none());
        assert!(complete_frame_len(b"SIP/2.0 200 OK\r\nContent-Length: 10\r\n").is_none());
    }

    #[test]
    fn is_request_matches_method() {
        assert!(is_request(b"MESSAGE sip:x SIP/2.0\r\n\r\n", "MESSAGE"));
        assert!(!is_request(b"SIP/2.0 200 OK\r\n\r\n", "MESSAGE"));
        assert!(!is_request(b"INVITE sip:x SIP/2.0\r\n\r\n", "MESSAGE"));
    }

    #[test]
    fn header_values_case_insensitive_multi() {
        let frame = b"SIP/2.0 200 OK\r\nVia: a\r\nvia: b\r\nContact: <sip:x@h>\r\n\r\n";
        assert_eq!(header_values(frame, "via"), vec!["a", "b"]);
        assert_eq!(header_value(frame, "CONTACT").as_deref(), Some("<sip:x@h>"));
    }

    #[test]
    fn header_uri_bracketed_and_bare() {
        let f = b"MESSAGE sip:x SIP/2.0\r\nFrom: <sip:+861380@h>;tag=abc\r\n\r\n";
        assert_eq!(header_uri(f, "From").as_deref(), Some("sip:+861380@h"));
        let bare = b"MESSAGE sip:x SIP/2.0\r\nFrom: sip:user@h;tag=abc\r\n\r\n";
        assert_eq!(header_uri(bare, "From").as_deref(), Some("sip:user@h"));
    }

    #[test]
    fn sip_host_brackets_ipv6_only() {
        assert_eq!(sip_host(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))), "1.2.3.4");
        assert_eq!(sip_host(IpAddr::V6(Ipv6Addr::LOCALHOST)), "[::1]");
    }

    #[test]
    fn body_after_terminator() {
        assert_eq!(
            body(b"SIP/2.0 200 OK\r\nContent-Length: 3\r\n\r\nabc"),
            b"abc"
        );
        assert_eq!(body(b"no terminator"), b"");
    }
}
