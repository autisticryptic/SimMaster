//! Telegram Bot API endpoint resolution for direct and reverse-proxy access.
//!
//! Telegram's official API host is unreachable from some networks (mainland
//! China among them), so deployments commonly front it with a reverse proxy in
//! the style of `CF-Workers-TGbot`: the proxy keeps the `/bot<token>/<method>`
//! path shape and only the origin changes.
//!
//! Rules enforced here:
//!   - empty configuration keeps the official `https://api.telegram.org` base,
//!     so existing installations are not migrated implicitly;
//!   - HTTPS only, because the bot token travels inside the request path;
//!   - no credentials, query string or fragment in the configured base, so the
//!     final URL cannot be reshaped by the stored value;
//!   - IP-literal bases must be public addresses, reusing the entitlement SSRF
//!     range checks;
//!   - bot tokens are restricted to the characters Telegram actually issues,
//!     which keeps a stored token from injecting extra path segments;
//!   - the token is never interpolated into error text; callers redact it from
//!     transport errors with [`redact_token`].
//!
//! No third-party proxy domain is hardcoded: the operator supplies their own.

use std::net::IpAddr;

use url::{Host, Url};

use crate::services::e911::ssrf::is_public_address;

/// Official Telegram Bot API origin, used when no proxy base is configured.
pub const OFFICIAL_API_BASE: &str = "https://api.telegram.org";

/// Placeholder substituted for the bot token in operator-visible strings.
pub const REDACTED_TOKEN: &str = "***";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelegramEndpointError {
    BaseNotHttps,
    BaseMissingHost,
    BaseHasCredentials,
    BaseHasQuery,
    BaseHasFragment,
    BaseTraversal,
    BaseForbiddenIp(IpAddr),
    BaseInvalid(String),
    TokenEmpty,
    TokenInvalidCharacters,
}

impl std::fmt::Display for TelegramEndpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BaseNotHttps => f.write_str("telegram_api_base_must_be_https"),
            Self::BaseMissingHost => f.write_str("telegram_api_base_missing_host"),
            Self::BaseHasCredentials => f.write_str("telegram_api_base_credentials_rejected"),
            Self::BaseHasQuery => f.write_str("telegram_api_base_query_rejected"),
            Self::BaseHasFragment => f.write_str("telegram_api_base_fragment_rejected"),
            Self::BaseTraversal => f.write_str("telegram_api_base_path_traversal_rejected"),
            Self::BaseForbiddenIp(ip) => write!(f, "telegram_api_base_ip_forbidden:{ip}"),
            Self::BaseInvalid(reason) => write!(f, "telegram_api_base_invalid:{reason}"),
            Self::TokenEmpty => f.write_str("telegram_bot_token_missing"),
            Self::TokenInvalidCharacters => f.write_str("telegram_bot_token_invalid_characters"),
        }
    }
}

impl std::error::Error for TelegramEndpointError {}

/// Whether the raw operator input contains a `..` path segment.
///
/// [`Url::parse`] resolves `..` away (`/a/../b` becomes `/b`), so checking the
/// parsed path would silently rewrite the configured base instead of rejecting
/// it. The check therefore runs on the raw value, and percent-encoded dots are
/// decoded first so `%2e%2e` cannot slip past.
fn has_traversal_segment(raw: &str) -> bool {
    let after_scheme = raw.split_once("://").map_or(raw, |(_, rest)| rest);
    let path = after_scheme.split_once('/').map_or("", |(_, rest)| rest);
    let path = path.split(['?', '#']).next().unwrap_or("");
    path.split('/')
        .any(|segment| segment.to_ascii_lowercase().replace("%2e", ".") == "..")
}

/// Normalise a configured API base into an origin plus optional path prefix.
///
/// An empty or whitespace-only value resolves to [`OFFICIAL_API_BASE`], which
/// preserves the current direct-connection default.
pub fn normalize_api_base(raw: &str) -> Result<String, TelegramEndpointError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(OFFICIAL_API_BASE.to_string());
    }

    // A bare host is a common operator input; treat it as HTTPS rather than
    // silently rejecting it, but never downgrade an explicit http:// value.
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };

    let url = Url::parse(&candidate).map_err(|err| {
        // url::ParseError text does not echo the input, so it is safe to keep.
        TelegramEndpointError::BaseInvalid(err.to_string())
    })?;

    if url.scheme() != "https" {
        return Err(TelegramEndpointError::BaseNotHttps);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(TelegramEndpointError::BaseHasCredentials);
    }
    if url.query().is_some() {
        return Err(TelegramEndpointError::BaseHasQuery);
    }
    if url.fragment().is_some() {
        return Err(TelegramEndpointError::BaseHasFragment);
    }
    if has_traversal_segment(trimmed) {
        return Err(TelegramEndpointError::BaseTraversal);
    }

    match url.host() {
        None => return Err(TelegramEndpointError::BaseMissingHost),
        Some(Host::Domain(domain)) if domain.is_empty() => {
            return Err(TelegramEndpointError::BaseMissingHost)
        }
        Some(Host::Domain(_)) => {}
        Some(Host::Ipv4(ip)) => {
            let ip = IpAddr::V4(ip);
            if !is_public_address(ip) {
                return Err(TelegramEndpointError::BaseForbiddenIp(ip));
            }
        }
        Some(Host::Ipv6(ip)) => {
            let ip = IpAddr::V6(ip);
            if !is_public_address(ip) {
                return Err(TelegramEndpointError::BaseForbiddenIp(ip));
            }
        }
    }

    let path = url.path().trim_end_matches('/');

    let mut origin = format!("https://{}", url.host_str().unwrap_or_default());
    if let Some(port) = url.port() {
        origin.push(':');
        origin.push_str(&port.to_string());
    }
    origin.push_str(path);
    Ok(origin)
}

/// Whether a bot token only uses characters Telegram issues.
///
/// Telegram tokens look like `<bot_id>:<secret>`. Restricting the charset stops
/// a stored token from adding path segments or a query string to the request.
pub fn validate_bot_token(token: &str) -> Result<&str, TelegramEndpointError> {
    let token = token.trim();
    if token.is_empty() {
        return Err(TelegramEndpointError::TokenEmpty);
    }
    if !token
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '_' | '-'))
    {
        return Err(TelegramEndpointError::TokenInvalidCharacters);
    }
    Ok(token)
}

/// Build the full Bot API method URL for a validated base and token.
pub fn method_url(
    api_base: &str,
    bot_token: &str,
    method: &str,
) -> Result<String, TelegramEndpointError> {
    let base = normalize_api_base(api_base)?;
    let token = validate_bot_token(bot_token)?;
    Ok(format!("{base}/bot{token}/{method}"))
}

/// Replace the bot token anywhere it appears in operator-visible text.
///
/// Transport errors and HTTP bodies can echo the request URL, which carries the
/// token in its path.
pub fn redact_token(text: &str, bot_token: &str) -> String {
    let token = bot_token.trim();
    if token.is_empty() {
        return text.to_string();
    }
    text.replace(token, REDACTED_TOKEN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_base_keeps_official_direct_endpoint() {
        assert_eq!(normalize_api_base("").unwrap(), OFFICIAL_API_BASE);
        assert_eq!(normalize_api_base("   ").unwrap(), OFFICIAL_API_BASE);
    }

    #[test]
    fn reverse_proxy_origin_and_path_prefix_are_preserved() {
        assert_eq!(
            normalize_api_base("https://tg.example.com").unwrap(),
            "https://tg.example.com"
        );
        assert_eq!(
            normalize_api_base("https://tg.example.com/").unwrap(),
            "https://tg.example.com"
        );
        assert_eq!(
            normalize_api_base("https://tg.example.com/proxy/").unwrap(),
            "https://tg.example.com/proxy"
        );
        assert_eq!(
            normalize_api_base("https://tg.example.com:8443/proxy").unwrap(),
            "https://tg.example.com:8443/proxy"
        );
    }

    #[test]
    fn bare_host_is_upgraded_to_https() {
        assert_eq!(
            normalize_api_base("tg.example.com/proxy").unwrap(),
            "https://tg.example.com/proxy"
        );
    }

    #[test]
    fn plaintext_and_other_schemes_are_rejected() {
        assert_eq!(
            normalize_api_base("http://tg.example.com"),
            Err(TelegramEndpointError::BaseNotHttps)
        );
        assert_eq!(
            normalize_api_base("ftp://tg.example.com"),
            Err(TelegramEndpointError::BaseNotHttps)
        );
    }

    #[test]
    fn credentials_query_fragment_and_traversal_are_rejected() {
        assert_eq!(
            normalize_api_base("https://user:pass@tg.example.com"),
            Err(TelegramEndpointError::BaseHasCredentials)
        );
        assert_eq!(
            normalize_api_base("https://tg.example.com/?a=1"),
            Err(TelegramEndpointError::BaseHasQuery)
        );
        assert_eq!(
            normalize_api_base("https://tg.example.com/#frag"),
            Err(TelegramEndpointError::BaseHasFragment)
        );
        for base in [
            "https://tg.example.com/a/../b",
            "https://tg.example.com/../proxy",
            "tg.example.com/a/../b",
            "https://tg.example.com/a/%2e%2e/b",
            "https://tg.example.com/a/%2E%2E/b",
        ] {
            assert_eq!(
                normalize_api_base(base),
                Err(TelegramEndpointError::BaseTraversal),
                "traversal base {base} must be rejected"
            );
        }
        // A host or segment merely containing dots is not traversal.
        assert_eq!(
            normalize_api_base("https://tg.example.com/a..b").unwrap(),
            "https://tg.example.com/a..b"
        );
    }

    #[test]
    fn private_and_loopback_ip_bases_are_rejected() {
        for base in [
            "https://127.0.0.1",
            "https://10.0.0.5:8443",
            "https://192.168.1.10",
            "https://169.254.169.254",
            "https://[::1]",
            "https://[fd00::1]",
        ] {
            match normalize_api_base(base) {
                Err(TelegramEndpointError::BaseForbiddenIp(_)) => {}
                other => panic!("expected forbidden ip for {base}, got {other:?}"),
            }
        }
    }

    #[test]
    fn public_ip_literal_base_is_allowed_documentation_is_not() {
        assert_eq!(
            normalize_api_base("https://8.8.8.8").unwrap(),
            "https://8.8.8.8"
        );
        assert_eq!(
            normalize_api_base("https://[2001:4860:4860::8888]:8443").unwrap(),
            "https://[2001:4860:4860::8888]:8443"
        );
        for base in ["https://203.0.113.10:8443", "https://198.51.100.7"] {
            match normalize_api_base(base) {
                Err(TelegramEndpointError::BaseForbiddenIp(_)) => {}
                other => panic!("documentation range {base} must be rejected, got {other:?}"),
            }
        }
    }

    #[test]
    fn token_charset_is_enforced() {
        assert_eq!(
            validate_bot_token("123456:AAbb_cc-dd").unwrap(),
            "123456:AAbb_cc-dd"
        );
        assert_eq!(
            validate_bot_token("  123456:AAbb  ").unwrap(),
            "123456:AAbb"
        );
        assert_eq!(
            validate_bot_token(""),
            Err(TelegramEndpointError::TokenEmpty)
        );
        for bad in ["123/456", "123456:AA bb", "123456:AA?bb", "123456:AA#bb"] {
            assert_eq!(
                validate_bot_token(bad),
                Err(TelegramEndpointError::TokenInvalidCharacters),
                "token {bad} must be rejected"
            );
        }
    }

    #[test]
    fn method_url_keeps_bot_path_shape_for_direct_and_proxy() {
        assert_eq!(
            method_url("", "123:abc", "sendMessage").unwrap(),
            "https://api.telegram.org/bot123:abc/sendMessage"
        );
        assert_eq!(
            method_url("https://tg.example.com/proxy", "123:abc", "sendMessage").unwrap(),
            "https://tg.example.com/proxy/bot123:abc/sendMessage"
        );
    }

    #[test]
    fn redaction_removes_token_from_urls_and_bodies() {
        let token = "123456:AAbbCC";
        let text = format!(
            "error sending request for url (https://tg.example.com/bot{token}/sendMessage): timeout"
        );
        let redacted = redact_token(&text, token);
        assert!(!redacted.contains(token));
        assert!(redacted.contains(REDACTED_TOKEN));
        assert_eq!(redact_token("no token here", ""), "no token here");
    }
}
