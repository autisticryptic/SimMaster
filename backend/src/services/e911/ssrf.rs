//! SSRF-safe HTTPS client for entitlement exchanges.
//!
//! Rules (see research doc §12.1):
//!   - HTTPS only; HTTP and IP-literal URLs are rejected up front;
//!   - the target host must be in the provider allow-list;
//!   - DNS resolution and every redirect hop are re-checked against private,
//!     link-local, loopback, multicast and cloud-metadata ranges;
//!   - redirects are followed manually so each hop is re-validated;
//!   - response size is capped;
//!   - certificate/hostname verification is never disabled.

use std::net::IpAddr;

use url::Url;

pub const MAX_RESPONSE_BYTES: usize = 512 * 1024;
pub const MAX_REDIRECTS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SsrfError {
    NotHttps,
    NoHost,
    HostNotAllowed(String),
    IpLiteral(String),
    ForbiddenIp(IpAddr),
    TooManyRedirects,
    RedirectNotAllowed(String),
    InvalidUrl(String),
    TooLarge,
    Transport(String),
}

impl std::fmt::Display for SsrfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotHttps => f.write_str("entitlement_url_must_be_https"),
            Self::NoHost => f.write_str("entitlement_url_missing_host"),
            Self::HostNotAllowed(host) => write!(f, "entitlement_host_not_allowed:{host}"),
            Self::IpLiteral(host) => write!(f, "entitlement_ip_literal_rejected:{host}"),
            Self::ForbiddenIp(ip) => write!(f, "entitlement_ip_forbidden:{ip}"),
            Self::TooManyRedirects => f.write_str("entitlement_too_many_redirects"),
            Self::RedirectNotAllowed(host) => {
                write!(f, "entitlement_redirect_not_allowed:{host}")
            }
            Self::InvalidUrl(reason) => write!(f, "entitlement_invalid_url:{reason}"),
            Self::TooLarge => f.write_str("entitlement_response_too_large"),
            Self::Transport(reason) => write!(f, "entitlement_transport:{reason}"),
        }
    }
}

impl std::error::Error for SsrfError {}

/// Whether an IP may be contacted for entitlement traffic. Private, loopback,
/// link-local, multicast, reserved, documentation and cloud-metadata ranges
/// are all forbidden.
pub fn is_public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_multicast()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_unspecified()
                // 169.254.169.254 cloud metadata is link-local, covered above.
                // Reserved ranges (0/8, 240/4, 192.0.0.0/24, 198.18.0.0/15) are
                // also excluded explicitly for defence in depth.
                || ip.octets()[0] == 0
                || ip.octets()[0] >= 240
                || (ip.octets()[0] == 192 && ip.octets()[1] == 0)
                || (ip.octets()[0] == 198 && (ip.octets()[1] & 0xfe) == 0x12)
                || ip.octets()[0] == 100 && (ip.octets()[1] & 0xc0) == 0x40)
        }
        IpAddr::V6(ip) => {
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.segments()[0] & 0xffc0 == 0xfe80
                // IPv4-mapped (::ffff:0:0/96) and IPv4-compatible (::/96) reuse
                // IPv4 checks.
                || {
                    let segments = ip.segments();
                    segments[0] == 0
                        && segments[1] == 0
                        && segments[2] == 0
                        && segments[3] == 0
                })
        }
    }
}

/// Check `host` looks like a plain DNS name (not an IP literal).
pub fn is_hostname(host: &str) -> bool {
    host.parse::<IpAddr>().is_err() && !host.is_empty()
}

/// Validate the initial entitlement URL against the allow-list and its scheme.
pub fn validate_entitlement_target(url: &str, allow_list: &[String]) -> Result<Url, SsrfError> {
    let parsed = Url::parse(url).map_err(|error| SsrfError::InvalidUrl(error.to_string()))?;
    if parsed.scheme() != "https" {
        return Err(SsrfError::NotHttps);
    }
    let host = parsed
        .host_str()
        .ok_or(SsrfError::NoHost)?
        .to_ascii_lowercase();
    check_host(&host, allow_list)?;
    Ok(parsed)
}

/// Verify a hostname against the allow-list and reject IP literals.
pub fn check_host(host: &str, allow_list: &[String]) -> Result<(), SsrfError> {
    if host.parse::<IpAddr>().is_ok() {
        return Err(SsrfError::IpLiteral(host.to_string()));
    }
    let allowed = allow_list
        .iter()
        .any(|entry| entry.eq_ignore_ascii_case(host));
    if !allowed {
        return Err(SsrfError::HostNotAllowed(host.to_string()));
    }
    Ok(())
}

/// Re-check a resolved IP after DNS. Called after DNS and after every redirect.
pub fn check_resolved_ip(ip: IpAddr) -> Result<(), SsrfError> {
    if is_public_address(ip) {
        Ok(())
    } else {
        Err(SsrfError::ForbiddenIp(ip))
    }
}

/// Re-validate a redirect Location header against the same allow-list.
pub fn validate_redirect(location: &str, allow_list: &[String]) -> Result<Url, SsrfError> {
    let parsed = Url::parse(location).map_err(|error| SsrfError::InvalidUrl(error.to_string()))?;
    if parsed.scheme() != "https" {
        return Err(SsrfError::NotHttps);
    }
    let host = parsed
        .host_str()
        .ok_or(SsrfError::NoHost)?
        .to_ascii_lowercase();
    check_host(&host, allow_list)?;
    Ok(parsed)
}

/// Resolve a host to public IPs. In the real client this wraps
/// `tokio::net::lookup_host`; the pure function lets tests inject a resolver.
pub fn first_public_ip<F>(host: &str, resolve: F) -> Result<IpAddr, SsrfError>
where
    F: Fn(&str) -> Vec<IpAddr>,
{
    let candidates = resolve(host);
    if candidates.is_empty() {
        return Err(SsrfError::HostNotAllowed(host.to_string()));
    }
    for ip in &candidates {
        if is_public_address(*ip) {
            return Ok(*ip);
        }
    }
    // None of the resolved addresses is public -> treat as forbidden.
    let first = candidates[0];
    Err(SsrfError::ForbiddenIp(first))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    const ALLOWED: [&str; 2] = ["entitlement.example.net", "websheet.example.net"];

    fn allowed() -> Vec<String> {
        ALLOWED.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn validates_scheme_and_allowlist() {
        assert!(
            validate_entitlement_target("https://entitlement.example.net/query", &allowed())
                .is_ok()
        );
        assert_eq!(
            validate_entitlement_target("http://entitlement.example.net/query", &allowed()),
            Err(SsrfError::NotHttps)
        );
        assert!(matches!(
            validate_entitlement_target("https://evil.example/query", &allowed()),
            Err(SsrfError::HostNotAllowed(_))
        ));
    }

    #[test]
    fn rejects_ip_literals() {
        assert!(matches!(
            validate_entitlement_target("https://127.0.0.1/query", &allowed()),
            Err(SsrfError::IpLiteral(_))
        ));
        assert!(matches!(
            validate_entitlement_target("https://169.254.169.254/latest/meta-data", &allowed()),
            Err(SsrfError::IpLiteral(_))
        ));
    }

    #[test]
    fn public_ip_classification() {
        for ip in [
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
            IpAddr::V4(Ipv4Addr::new(169, 254, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255)),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)),
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1)),
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)),
            IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)),
            IpAddr::V6(Ipv6Addr::new(0xff01, 0, 0, 0, 0, 0, 0, 1)),
        ] {
            assert!(!is_public_address(ip), "{ip} must be non-public");
        }

        for ip in [
            IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
            IpAddr::V4(Ipv4Addr::new(104, 26, 0, 1)),
            IpAddr::V6(Ipv6Addr::new(0x2606, 0x4700, 0, 0, 0, 0, 0, 0)),
        ] {
            assert!(is_public_address(ip), "{ip} must be public");
        }
    }

    #[test]
    fn dns_recheck_forbids_private_resolution() {
        let resolve = |host: &str| match host {
            "entitlement.example.net" => vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))],
            _ => vec![],
        };
        assert!(matches!(
            first_public_ip("entitlement.example.net", resolve),
            Err(SsrfError::ForbiddenIp(IpAddr::V4(ip))) if ip == Ipv4Addr::new(10, 0, 0, 5)
        ));
    }

    #[test]
    fn dns_recheck_accepts_public_resolution() {
        let resolve = |_host: &str| vec![IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))];
        assert_eq!(
            first_public_ip("entitlement.example.net", resolve),
            Ok(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)))
        );
    }

    #[test]
    fn redirect_revalidated_against_allowlist() {
        assert!(validate_redirect("https://websheet.example.net/terms", &allowed()).is_ok());
        assert!(matches!(
            validate_redirect("https://attacker.example/steal", &allowed()),
            Err(SsrfError::HostNotAllowed(_))
        ));
        assert_eq!(
            validate_redirect("http://websheet.example.net/terms", &allowed()),
            Err(SsrfError::NotHttps)
        );
    }
}
