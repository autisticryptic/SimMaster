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

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use tokio::net::UdpSocket;

use super::errors::{code, VolteError};

const DNS_TIMEOUT: Duration = Duration::from_secs(4);
const DNS_PORT: u16 = 53;
const SIP_PORT: u16 = 5060;
const ENV_PCSCF: &str = "SIMADMIN_VOLTE_PCSCF";

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

/// Discover a P-CSCF without changing the system resolver. IMS APNs commonly
/// provide private DNS servers that are reachable only through the dedicated
/// bearer, so queries are sent directly from the bearer address.
pub async fn discover_pcscf(
    settings: &ImsIpSettings,
    home_domain: &str,
) -> Result<IpAddr, VolteError> {
    if let Ok(explicit) = std::env::var(ENV_PCSCF) {
        if let Ok(address) = explicit.trim().parse::<IpAddr>() {
            return settings.ensure_family_match(address);
        }
    }
    if let Some(address) = settings.pcscf.first().copied() {
        return settings.ensure_family_match(address);
    }

    let local = settings
        .local_addr()
        .ok_or_else(|| VolteError::new(code::IP_SETTINGS_MISSING))?;
    let dns_servers = if local.is_ipv6() {
        &settings.ipv6_dns
    } else {
        &settings.ipv4_dns
    };
    let pcscf_name = format!("pcscf.{home_domain}");
    let srv_names = [
        format!("_sip._udp.{home_domain}"),
        format!("_sip._tcp.{home_domain}"),
    ];

    for server in dns_servers {
        if server.is_ipv4() != local.is_ipv4() {
            continue;
        }
        let address_type = if local.is_ipv6() { 28 } else { 1 };
        if let Ok(records) = query_dns(local, *server, &pcscf_name, address_type).await {
            if let Some(address) = records.addresses.into_iter().find(|item| {
                item.is_ipv4() == local.is_ipv4()
            }) {
                return Ok(address);
            }
        }

        for srv_name in &srv_names {
            let Ok(records) = query_dns(local, *server, srv_name, 33).await else {
                continue;
            };
            for target in records.srv_targets {
                if let Ok(target_records) = query_dns(local, *server, &target, address_type).await {
                    if let Some(address) = target_records.addresses.into_iter().find(|item| {
                        item.is_ipv4() == local.is_ipv4()
                    }) {
                        return Ok(address);
                    }
                }
            }
        }
    }
    // Some Qualcomm/operator combinations expose P-CSCF candidates in the
    // DNS slots but do not run a recursive DNS service on those addresses.
    // Preserve the documented data-path fallback after bounded DNS attempts.
    settings.resolve_pcscf()
}

pub fn pcscf_socket(address: IpAddr) -> SocketAddr {
    SocketAddr::new(address, SIP_PORT)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct DnsRecords {
    addresses: Vec<IpAddr>,
    srv_targets: Vec<String>,
}

async fn query_dns(
    local: IpAddr,
    server: IpAddr,
    name: &str,
    record_type: u16,
) -> Result<DnsRecords, VolteError> {
    let query_id = dns_query_id(name, record_type);
    let query = build_dns_query(query_id, name, record_type)?;
    let socket = UdpSocket::bind(SocketAddr::new(local, 0))
        .await
        .map_err(|_| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?;
    socket
        .send_to(&query, SocketAddr::new(server, DNS_PORT))
        .await
        .map_err(|_| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?;
    let mut response = [0u8; 4096];
    let (read, _) = tokio::time::timeout(DNS_TIMEOUT, socket.recv_from(&mut response))
        .await
        .map_err(|_| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?
        .map_err(|_| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?;
    parse_dns_response(query_id, &response[..read])
}

fn dns_query_id(name: &str, record_type: u16) -> u16 {
    let mut hash = 0x5a17u16 ^ record_type;
    for byte in name.bytes() {
        hash = hash.rotate_left(5) ^ u16::from(byte);
    }
    hash
}

fn build_dns_query(id: u16, name: &str, record_type: u16) -> Result<Vec<u8>, VolteError> {
    let mut query = Vec::with_capacity(64 + name.len());
    query.extend_from_slice(&id.to_be_bytes());
    query.extend_from_slice(&0x0100u16.to_be_bytes());
    query.extend_from_slice(&1u16.to_be_bytes());
    query.extend_from_slice(&0u16.to_be_bytes());
    query.extend_from_slice(&0u16.to_be_bytes());
    query.extend_from_slice(&0u16.to_be_bytes());
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED));
        }
        query.push(label.len() as u8);
        query.extend_from_slice(label.as_bytes());
    }
    query.push(0);
    query.extend_from_slice(&record_type.to_be_bytes());
    query.extend_from_slice(&1u16.to_be_bytes());
    Ok(query)
}

fn parse_dns_response(id: u16, packet: &[u8]) -> Result<DnsRecords, VolteError> {
    if packet.len() < 12 || u16::from_be_bytes([packet[0], packet[1]]) != id {
        return Err(VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED));
    }
    let flags = u16::from_be_bytes([packet[2], packet[3]]);
    if flags & 0x000f != 0 {
        return Err(VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED));
    }
    let questions = usize::from(u16::from_be_bytes([packet[4], packet[5]]));
    let answers = usize::from(u16::from_be_bytes([packet[6], packet[7]]));
    let authorities = usize::from(u16::from_be_bytes([packet[8], packet[9]]));
    let additional = usize::from(u16::from_be_bytes([packet[10], packet[11]]));
    let mut offset = 12usize;
    for _ in 0..questions {
        offset = read_dns_name(packet, offset)?.1;
        offset = offset
            .checked_add(4)
            .filter(|end| *end <= packet.len())
            .ok_or_else(|| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?;
    }

    let mut records = DnsRecords::default();
    for _ in 0..answers + authorities + additional {
        offset = read_dns_name(packet, offset)?.1;
        if offset + 10 > packet.len() {
            return Err(VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED));
        }
        let record_type = u16::from_be_bytes([packet[offset], packet[offset + 1]]);
        let length = usize::from(u16::from_be_bytes([packet[offset + 8], packet[offset + 9]]));
        let data_offset = offset + 10;
        let data_end = data_offset
            .checked_add(length)
            .filter(|end| *end <= packet.len())
            .ok_or_else(|| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?;
        match (record_type, length) {
            (1, 4) => records.addresses.push(IpAddr::V4(Ipv4Addr::new(
                packet[data_offset],
                packet[data_offset + 1],
                packet[data_offset + 2],
                packet[data_offset + 3],
            ))),
            (28, 16) => {
                let octets: [u8; 16] = packet[data_offset..data_end]
                    .try_into()
                    .map_err(|_| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?;
                records.addresses.push(IpAddr::V6(Ipv6Addr::from(octets)));
            }
            (33, 6..) => {
                let (target, _) = read_dns_name(packet, data_offset + 6)?;
                if !target.is_empty() && !records.srv_targets.contains(&target) {
                    records.srv_targets.push(target);
                }
            }
            _ => {}
        }
        offset = data_end;
    }
    Ok(records)
}

fn read_dns_name(packet: &[u8], start: usize) -> Result<(String, usize), VolteError> {
    let mut labels = Vec::new();
    let mut offset = start;
    let mut end = None;
    for _ in 0..128 {
        let length = *packet
            .get(offset)
            .ok_or_else(|| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?;
        if length == 0 {
            return Ok((labels.join("."), end.unwrap_or(offset + 1)));
        }
        if length & 0xc0 == 0xc0 {
            let low = *packet
                .get(offset + 1)
                .ok_or_else(|| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?;
            end.get_or_insert(offset + 2);
            offset = (usize::from(length & 0x3f) << 8) | usize::from(low);
            continue;
        }
        if length & 0xc0 != 0 {
            return Err(VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED));
        }
        let label_start = offset + 1;
        let label_end = label_start + usize::from(length);
        let label = std::str::from_utf8(
            packet
                .get(label_start..label_end)
                .ok_or_else(|| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?,
        )
        .map_err(|_| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?;
        labels.push(label.to_string());
        offset = label_end;
    }
    Err(VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))
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

    #[test]
    fn parses_compressed_aaaa_dns_answer() {
        let id = 0x1234;
        let name = "pcscf.ims.example";
        let query = build_dns_query(id, name, 28).unwrap();
        let mut packet = query.clone();
        packet[2..4].copy_from_slice(&0x8180u16.to_be_bytes());
        packet[6..8].copy_from_slice(&1u16.to_be_bytes());
        packet.extend_from_slice(&[0xc0, 0x0c]);
        packet.extend_from_slice(&28u16.to_be_bytes());
        packet.extend_from_slice(&1u16.to_be_bytes());
        packet.extend_from_slice(&60u32.to_be_bytes());
        packet.extend_from_slice(&16u16.to_be_bytes());
        packet.extend_from_slice(&Ipv6Addr::LOCALHOST.octets());

        let records = parse_dns_response(id, &packet).unwrap();
        assert_eq!(records.addresses, vec![IpAddr::V6(Ipv6Addr::LOCALHOST)]);
    }

    #[test]
    fn pcscf_socket_uses_standard_sip_port() {
        assert_eq!(pcscf_socket(IpAddr::V4(Ipv4Addr::LOCALHOST)).port(), 5060);
    }
}
