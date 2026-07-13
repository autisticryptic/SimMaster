//! VoLTE P-CSCF discovery.
//!
//! Clean-room from 3GPP TS 24.229 (P-CSCF discovery) + TS 27.007. The P-CSCF
//! address is obtained from the IMS APN bearer's PCO / connection settings.
//! Different modems surface it differently; the reference uses a `+CGCONTRDP`
//! query and parses the IP settings block. Here we keep the parsing (fully
//! testable) separate from the ModemManager/AT IO (which runs on device).
//!
//! Observed data-path settings block anchors (from the reference):
//!   `IPv6 address:` / `IPv6 gateway address:` / `IPv6 primary DNS:` /
//!   `IPv4 address:` / `IPv4 gateway address:` / `IPv4 primary DNS:` ...
//! The P-CSCF is typically delivered via the PCO and equals a primary DNS /
//! dedicated P-CSCF PCO field depending on operator.

use std::net::IpAddr;

use super::errors::{code, VolteError};

/// Parsed IP settings for the IMS bearer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImsIpSettings {
    pub ipv6_address: Option<IpAddr>,
    pub ipv6_gateway: Option<IpAddr>,
    pub ipv6_dns: Vec<IpAddr>,
    pub ipv4_address: Option<IpAddr>,
    pub ipv4_gateway: Option<IpAddr>,
    pub ipv4_dns: Vec<IpAddr>,
    /// Explicit P-CSCF addresses if delivered via PCO.
    pub pcscf: Vec<IpAddr>,
}

impl ImsIpSettings {
    /// Choose the local UE address for SIP: prefer IPv6 (IMS is usually v6).
    pub fn local_addr(&self) -> Option<IpAddr> {
        self.ipv6_address.or(self.ipv4_address)
    }

    /// Resolve the P-CSCF address to register against. Preference order:
    /// explicit PCO P-CSCF > IPv6 primary DNS > IPv4 primary DNS. This mirrors
    /// the common operator behavior where the P-CSCF is delivered in the PCO,
    /// falling back to the DNS-advertised proxy.
    pub fn resolve_pcscf(&self) -> Result<IpAddr, VolteError> {
        if let Some(p) = self.pcscf.first() {
            return Ok(*p);
        }
        if let Some(dns) = self.ipv6_dns.first() {
            return Ok(*dns);
        }
        if let Some(dns) = self.ipv4_dns.first() {
            return Ok(*dns);
        }
        Err(VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))
    }

    /// Validate the family invariant: local addr and P-CSCF must share family.
    pub fn ensure_family_match(&self, pcscf: IpAddr) -> Result<IpAddr, VolteError> {
        let local = self
            .local_addr()
            .ok_or_else(|| VolteError::new(code::IP_SETTINGS_MISSING))?;
        if std::mem::discriminant(&local) != std::mem::discriminant(&pcscf) {
            return Err(VolteError::new(code::PCSCF_FAMILY_MISMATCH));
        }
        Ok(pcscf)
    }
}

/// Parse a settings block that lists `Label: value` lines, tolerant of the
/// modem/`mmcli` style output. Recognizes the IPv4/IPv6 address/gateway/DNS
/// labels and optional `P-CSCF:` lines.
pub fn parse_ip_settings(block: &str) -> ImsIpSettings {
    let mut s = ImsIpSettings::default();
    for line in block.lines() {
        let Some((label, value)) = line.split_once(':') else {
            continue;
        };
        let label = label.trim().to_ascii_lowercase();
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match label.as_str() {
            "ipv6 address" => s.ipv6_address = parse_addr(value),
            "ipv6 gateway address" | "ipv6 gateway" => s.ipv6_gateway = parse_addr(value),
            "ipv6 primary dns" | "ipv6 secondary dns" => push_addr(&mut s.ipv6_dns, value),
            "ipv4 address" => s.ipv4_address = parse_addr(value),
            "ipv4 gateway address" | "ipv4 gateway" => s.ipv4_gateway = parse_addr(value),
            "ipv4 primary dns" | "ipv4 secondary dns" => push_addr(&mut s.ipv4_dns, value),
            "p-cscf" | "pcscf" => push_addr(&mut s.pcscf, value),
            _ => {}
        }
    }
    s
}

/// Strip a possible prefix length / netmask suffix and parse an IP.
fn parse_addr(value: &str) -> Option<IpAddr> {
    let head = value
        .split_whitespace()
        .next()
        .unwrap_or(value)
        .split('/')
        .next()
        .unwrap_or(value);
    head.parse::<IpAddr>().ok()
}

fn push_addr(list: &mut Vec<IpAddr>, value: &str) {
    if let Some(addr) = parse_addr(value) {
        if !list.contains(&addr) {
            list.push(addr);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    const SAMPLE: &str = "\
IPv6 address: 2001:db8::2/64
IPv6 gateway address: 2001:db8::1
IPv6 primary DNS: 2001:db8::53
IPv6 secondary DNS: 2001:db8::54
IPv4 address: 10.0.0.2
IPv4 gateway address: 10.0.0.1
IPv4 primary DNS: 10.0.0.53";

    #[test]
    fn parses_ipv6_and_ipv4_blocks() {
        let s = parse_ip_settings(SAMPLE);
        assert_eq!(
            s.ipv6_address,
            Some(IpAddr::V6("2001:db8::2".parse::<Ipv6Addr>().unwrap()))
        );
        assert_eq!(
            s.ipv6_gateway,
            Some(IpAddr::V6("2001:db8::1".parse::<Ipv6Addr>().unwrap()))
        );
        assert_eq!(s.ipv6_dns.len(), 2);
        assert_eq!(
            s.ipv4_address,
            Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)))
        );
    }

    #[test]
    fn local_addr_prefers_ipv6() {
        let s = parse_ip_settings(SAMPLE);
        assert_eq!(
            s.local_addr(),
            Some(IpAddr::V6("2001:db8::2".parse::<Ipv6Addr>().unwrap()))
        );
    }

    #[test]
    fn resolve_pcscf_prefers_explicit_then_dns() {
        let mut s = parse_ip_settings(SAMPLE);
        // No explicit P-CSCF -> IPv6 primary DNS.
        assert_eq!(
            s.resolve_pcscf().unwrap(),
            IpAddr::V6("2001:db8::53".parse::<Ipv6Addr>().unwrap())
        );
        // Explicit PCO P-CSCF wins.
        s.pcscf.push(IpAddr::V6("2001:db8::99".parse::<Ipv6Addr>().unwrap()));
        assert_eq!(
            s.resolve_pcscf().unwrap(),
            IpAddr::V6("2001:db8::99".parse::<Ipv6Addr>().unwrap())
        );
    }

    #[test]
    fn resolve_pcscf_errors_when_nothing() {
        let s = ImsIpSettings::default();
        assert_eq!(
            s.resolve_pcscf().unwrap_err().code(),
            code::RUNTIME_ALL_PCSCF_FAILED
        );
    }

    #[test]
    fn family_mismatch_detected() {
        let s = parse_ip_settings("IPv6 address: 2001:db8::2");
        let v4 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        assert_eq!(
            s.ensure_family_match(v4).unwrap_err().code(),
            code::PCSCF_FAMILY_MISMATCH
        );
        let v6 = IpAddr::V6("2001:db8::1".parse::<Ipv6Addr>().unwrap());
        assert_eq!(s.ensure_family_match(v6).unwrap(), v6);
    }

    #[test]
    fn parse_addr_strips_prefix_len() {
        assert_eq!(
            parse_addr("2001:db8::2/64"),
            Some(IpAddr::V6("2001:db8::2".parse::<Ipv6Addr>().unwrap()))
        );
        assert_eq!(
            parse_addr("10.0.0.2 255.255.255.0"),
            Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)))
        );
    }
}
