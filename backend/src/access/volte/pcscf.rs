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
//! The P-CSCF is normally delivered via PCO. DNS addresses are resolver
//! endpoints, not implicit SIP proxies; they are only used to resolve the
//! standard P-CSCF/SRV names.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use tokio::{net::UdpSocket, process::Command, time::sleep};

use crate::infra::config::VolteIpFamilyPreference;

use super::errors::{code, VolteError};
use super::plan::{ImsConnectionPlan, IpFamily};

const DNS_TIMEOUT: Duration = Duration::from_secs(4);
const DNS_PORT: u16 = 53;
const SIP_PORT: u16 = 5060;
const ENV_PCSCF: &str = "SIMADMIN_VOLTE_PCSCF";
const ENV_IMS_CID: &str = "SIMADMIN_VOLTE_IMS_CID";
const DEFAULT_IMS_CID: u8 = 2;
const AT_CONTEXT_SETTLE: Duration = Duration::from_secs(3);
const AT_DISCOVERY_ROUNDS: usize = 3;

/// An IMS PDP context kept alive while the dedicated bearer is negotiated.
///
/// Qualcomm exposes P-CSCF PCO only while this context is active. The
/// reference runtime retains CID 2 through WDS probing and bearer creation,
/// then restores the original context during session teardown.
#[derive(Debug, Clone)]
pub struct ImsAtContextLease {
    pub modem: String,
    pub cid: u8,
    restore_command: String,
}

impl ImsAtContextLease {
    pub async fn cleanup(self) {
        let _ = run_at(&self.modem, &format!("AT+CGACT=0,{}", self.cid)).await;
        let _ = run_at(&self.modem, &format!("AT$QCPDPIMSCFGE={},0,0,0", self.cid)).await;
        let _ = run_at(&self.modem, &self.restore_command).await;
    }
}

#[derive(Debug, Clone)]
pub struct AtPcscfDiscovery {
    pub candidates: Vec<IpAddr>,
    pub context: Option<ImsAtContextLease>,
    pub cid: u8,
}

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

    /// Available bearer addresses in the plan's family order.
    pub fn ordered_local_addrs(&self, plan: &ImsConnectionPlan) -> Vec<IpAddr> {
        let mut addresses = Vec::with_capacity(2);
        for family in plan.pcscf_order() {
            let addr = match family {
                IpFamily::Ipv6 => self.ipv6_address,
                IpFamily::Ipv4 => self.ipv4_address,
            };
            push_optional_addr(&mut addresses, addr);
        }
        addresses
    }

    /// Return an explicit P-CSCF delivered by the bearer PCO.
    ///
    /// DNS server addresses must never be returned here. Some Qualcomm
    /// devices expose public carrier resolvers in the IMS bearer DNS slots;
    /// sending SIP REGISTER to those addresses produces a misleading timeout.
    pub fn resolve_pcscf_for(&self, local: IpAddr) -> Result<IpAddr, VolteError> {
        if let Some(p) = self
            .pcscf
            .iter()
            .copied()
            .find(|candidate| same_family(local, *candidate))
        {
            return Ok(p);
        }
        Err(VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))
    }

    /// Validate the family invariant: local addr and P-CSCF must share family.
    pub fn ensure_family_match(&self, local: IpAddr, pcscf: IpAddr) -> Result<IpAddr, VolteError> {
        if !same_family(local, pcscf) {
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

/// The IMS PDP context id used for P-CSCF discovery and the IPv6 WDS preflight.
/// Honors `SIMADMIN_VOLTE_IMS_CID` (1..=16), else falls back to CID 2. Exposed
/// so callers that skip the AT probe (e.g. when it is non-fatal and returns no
/// candidates) still have a stable CID hint for the preflight.
pub fn configured_ims_cid() -> u8 {
    std::env::var(ENV_IMS_CID)
        .ok()
        .and_then(|value| value.trim().parse::<u8>().ok())
        .filter(|value| (1..=16).contains(value))
        .unwrap_or(DEFAULT_IMS_CID)
}

/// Discover P-CSCF candidates and retain the successful Qualcomm IMS context.
///
/// Keeping the context is intentional: on the 410 firmware, cleaning CID 2
/// immediately after `+CGCONTRDP` makes the subsequent IPv6 WDS request fail
/// with `prefix-unavailable` even though the same request succeeds in the
/// reference runtime.
pub async fn discover_pcscf_via_at_with_context(
    modem: &str,
    plan: &ImsConnectionPlan,
) -> Result<AtPcscfDiscovery, VolteError> {
    let cid = configured_ims_cid();

    let mut last_error = None;
    for _ in 0..AT_DISCOVERY_ROUNDS {
        for pdp_type in plan.pdp_types() {
            match probe_pcscf_context(modem, cid, pdp_type).await {
                Ok((candidates, context)) if !candidates.is_empty() => {
                    return Ok(AtPcscfDiscovery {
                        candidates,
                        context: Some(context),
                        cid,
                    });
                }
                Ok((_, context)) => {
                    context.cleanup().await;
                    last_error = Some(VolteError::with_detail(
                        code::RUNTIME_ALL_PCSCF_FAILED,
                        format!("AT+CGCONTRDP={cid}:{pdp_type}:no-pcscf"),
                    ));
                }
                Err(error) => last_error = Some(error),
            }
        }
    }
    Err(last_error.unwrap_or_else(|| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED)))
}

async fn probe_pcscf_context(
    modem: &str,
    cid: u8,
    pdp_type: &str,
) -> Result<(Vec<IpAddr>, ImsAtContextLease), VolteError> {
    let restore_context = run_at(modem, "AT+CGDCONT?")
        .await
        .ok()
        .and_then(|output| cgdccont_restore_command(&output, cid))
        .unwrap_or_else(|| format!("AT+CGDCONT={cid},\"IPV4V6\",\"\""));
    let _ = run_at(modem, &format!("AT+CGACT=0,{cid}")).await;
    run_at(modem, &format!("AT+CGDCONT={cid},\"{pdp_type}\",\"ims\"")).await?;
    run_at(modem, &format!("AT$QCPDPIMSCFGE={cid},1,1,1")).await?;
    if let Err(error) = run_at(modem, &format!("AT+CGACT=1,{cid}")).await {
        cleanup_pcscf_context(modem, cid, &restore_context).await;
        return Err(error);
    }
    sleep(AT_CONTEXT_SETTLE).await;
    let settings = match run_at(modem, &format!("AT+CGCONTRDP={cid}")).await {
        Ok(settings) => settings,
        Err(error) => {
            cleanup_pcscf_context(modem, cid, &restore_context).await;
            return Err(error);
        }
    };
    Ok((
        parse_cgcontrdp_pcscf(&settings, cid),
        ImsAtContextLease {
            modem: modem.to_string(),
            cid,
            restore_command: restore_context,
        },
    ))
}

async fn cleanup_pcscf_context(modem: &str, cid: u8, restore_context: &str) {
    let _ = run_at(modem, &format!("AT+CGACT=0,{cid}")).await;
    let _ = run_at(modem, &format!("AT$QCPDPIMSCFGE={cid},0,0,0")).await;
    let _ = run_at(modem, restore_context).await;
}

async fn run_at(modem: &str, command: &str) -> Result<String, VolteError> {
    let argument = format!("--command={command}");
    let output = Command::new("mmcli")
        .args(["-m", modem, &argument])
        .output()
        .await
        .map_err(|error| {
            VolteError::with_detail(code::COMMAND_SPAWN_FAILED, format!("mmcli:{error}"))
        })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr)
            .trim()
            .replace('\n', " ");
        Err(VolteError::with_detail(
            code::COMMAND_FAILED,
            format!(
                "mmcli:{}:-m {modem} {argument}:{stderr}",
                output.status.code().unwrap_or(-1)
            ),
        ))
    }
}

fn cgdccont_restore_command(output: &str, expected_cid: u8) -> Option<String> {
    for line in output.lines() {
        let Some((_, values)) = line.split_once("+CGDCONT:") else {
            continue;
        };
        let fields: Vec<&str> = values.split(',').map(|field| field.trim()).collect();
        if fields.len() < 3 || fields[0].parse::<u8>().ok() != Some(expected_cid) {
            continue;
        }
        let pdp_type = fields[1].trim_matches('"');
        let apn = fields[2].trim_matches('"');
        if !pdp_type.is_empty() {
            return Some(format!(
                "AT+CGDCONT={expected_cid},\"{pdp_type}\",\"{apn}\""
            ));
        }
    }
    None
}

/// Parse the primary/secondary P-CSCF columns from a 3GPP +CGCONTRDP response.
/// Qualcomm renders IPv6 values as 16 dot-separated decimal octets.
pub fn parse_cgcontrdp_pcscf(output: &str, expected_cid: u8) -> Vec<IpAddr> {
    let mut candidates = Vec::new();
    for line in output.lines() {
        let Some((_, values)) = line.split_once("+CGCONTRDP:") else {
            continue;
        };
        let fields: Vec<&str> = values.split(',').map(|field| field.trim()).collect();
        if fields.len() < 8 || fields[0].parse::<u8>().ok() != Some(expected_cid) {
            continue;
        }
        for field in fields.iter().skip(7).take(2) {
            for address in parse_cgcontrdp_addresses(field) {
                if !candidates.contains(&address) {
                    candidates.push(address);
                }
            }
        }
    }
    candidates
}

fn parse_cgcontrdp_addresses(field: &str) -> Vec<IpAddr> {
    field
        .trim_matches(|character| character == '\'' || character == '"')
        .split_whitespace()
        .filter_map(parse_cgcontrdp_address)
        .collect()
}

fn parse_cgcontrdp_address(value: &str) -> Option<IpAddr> {
    if let Ok(address) = value.parse::<IpAddr>() {
        return Some(address);
    }
    let octets: Vec<u8> = value
        .split('.')
        .map(str::parse::<u8>)
        .collect::<Result<_, _>>()
        .ok()?;
    match octets.len() {
        4 => Some(IpAddr::V4(Ipv4Addr::new(
            octets[0], octets[1], octets[2], octets[3],
        ))),
        16 => {
            let bytes: [u8; 16] = octets.try_into().ok()?;
            Some(IpAddr::V6(Ipv6Addr::from(bytes)))
        }
        _ => None,
    }
}

/// Discover a P-CSCF without changing the system resolver. IMS APNs commonly
/// provide private DNS servers that are reachable only through the dedicated
/// bearer, so queries are sent directly from the bearer address.
pub async fn discover_pcscf(
    settings: &ImsIpSettings,
    home_domain: &str,
    local: IpAddr,
) -> Result<IpAddr, VolteError> {
    if let Ok(explicit) = std::env::var(ENV_PCSCF) {
        if let Some(address) = parse_pcscf_override(&explicit)
            .into_iter()
            .find(|candidate| same_family(local, *candidate))
        {
            return settings.ensure_family_match(local, address);
        }
    }
    if let Ok(address) = settings.resolve_pcscf_for(local) {
        return settings.ensure_family_match(local, address);
    }

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
            if let Some(address) = records
                .addresses
                .into_iter()
                .find(|item| item.is_ipv4() == local.is_ipv4())
            {
                return Ok(address);
            }
        }

        for srv_name in &srv_names {
            let Ok(records) = query_dns(local, *server, srv_name, 33).await else {
                continue;
            };
            for target in records.srv_targets {
                if let Ok(target_records) = query_dns(local, *server, &target, address_type).await {
                    if let Some(address) = target_records
                        .addresses
                        .into_iter()
                        .find(|item| item.is_ipv4() == local.is_ipv4())
                    {
                        return Ok(address);
                    }
                }
            }
        }
    }
    Err(VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))
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

fn push_optional_addr(list: &mut Vec<IpAddr>, value: Option<IpAddr>) {
    if let Some(value) = value {
        if !list.contains(&value) {
            list.push(value);
        }
    }
}

fn same_family(left: IpAddr, right: IpAddr) -> bool {
    left.is_ipv4() == right.is_ipv4()
}

fn parse_pcscf_override(value: &str) -> Vec<IpAddr> {
    value
        .split(|character: char| character == ',' || character == ';' || character.is_whitespace())
        .filter_map(|candidate| candidate.trim().parse::<IpAddr>().ok())
        .fold(Vec::new(), |mut addresses, address| {
            if !addresses.contains(&address) {
                addresses.push(address);
            }
            addresses
        })
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
        assert_eq!(s.ipv4_address, Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))));
    }

    #[test]
    fn address_order_honors_preference_and_strict_modes() {
        use crate::access::volte::plan::ImsConnectionPlan;
        let s = parse_ip_settings(SAMPLE);
        assert_eq!(
            s.ordered_local_addrs(&ImsConnectionPlan::from_preference(
                VolteIpFamilyPreference::Ipv6First
            )),
            vec![
                IpAddr::V6("2001:db8::2".parse::<Ipv6Addr>().unwrap()),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
            ]
        );
        assert_eq!(
            s.ordered_local_addrs(&ImsConnectionPlan::from_preference(
                VolteIpFamilyPreference::Ipv4First
            )),
            vec![
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
                IpAddr::V6("2001:db8::2".parse::<Ipv6Addr>().unwrap()),
            ]
        );
        assert_eq!(
            s.ordered_local_addrs(&ImsConnectionPlan::from_preference(
                VolteIpFamilyPreference::Ipv6Only
            )),
            vec![IpAddr::V6("2001:db8::2".parse::<Ipv6Addr>().unwrap())]
        );
        assert_eq!(
            s.ordered_local_addrs(&ImsConnectionPlan::from_preference(
                VolteIpFamilyPreference::Ipv4Only
            )),
            vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))]
        );
    }

    #[test]
    fn resolve_pcscf_accepts_only_explicit_pco_address() {
        let mut s = parse_ip_settings(SAMPLE);
        assert_eq!(
            s.resolve_pcscf_for(IpAddr::V6("2001:db8::2".parse().unwrap()))
                .unwrap_err()
                .code(),
            code::RUNTIME_ALL_PCSCF_FAILED
        );
        s.pcscf
            .push(IpAddr::V6("2001:db8::99".parse::<Ipv6Addr>().unwrap()));
        assert_eq!(
            s.resolve_pcscf_for(IpAddr::V6("2001:db8::2".parse().unwrap()))
                .unwrap(),
            IpAddr::V6("2001:db8::99".parse::<Ipv6Addr>().unwrap())
        );
    }

    #[test]
    fn resolve_pcscf_errors_when_nothing() {
        let s = ImsIpSettings::default();
        assert_eq!(
            s.resolve_pcscf_for(IpAddr::V6(Ipv6Addr::LOCALHOST))
                .unwrap_err()
                .code(),
            code::RUNTIME_ALL_PCSCF_FAILED
        );
    }

    #[test]
    fn family_mismatch_detected() {
        let s = parse_ip_settings("IPv6 address: 2001:db8::2");
        let v4 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        assert_eq!(
            s.ensure_family_match(s.local_addr().unwrap(), v4)
                .unwrap_err()
                .code(),
            code::PCSCF_FAMILY_MISMATCH
        );
        let v6 = IpAddr::V6("2001:db8::1".parse::<Ipv6Addr>().unwrap());
        assert_eq!(
            s.ensure_family_match(s.local_addr().unwrap(), v6).unwrap(),
            v6
        );
    }

    #[test]
    fn explicit_pcscf_and_override_candidates_are_filtered_by_family() {
        let mut s = parse_ip_settings(SAMPLE);
        let v4 = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
        let v6 = IpAddr::V6("2001:db8::99".parse().unwrap());
        s.pcscf.extend([v6, v4]);

        assert_eq!(s.resolve_pcscf_for(s.ipv4_address.unwrap()).unwrap(), v4);
        assert_eq!(s.resolve_pcscf_for(s.ipv6_address.unwrap()).unwrap(), v6);
        assert_eq!(
            parse_pcscf_override("2001:db8::99, 192.0.2.10;invalid 192.0.2.10"),
            vec![v6, v4]
        );
    }

    #[test]
    fn parses_qualcomm_cgcontrdp_pcscf_columns() {
        let response = "response: '+CGCONTRDP: 2,5,ims,36.14.87.128.10.128.45.91.1.2.3.4.5.6.7.8,36.14.87.128.10.128.45.91.8.7.6.5.4.3.2.1,36.14.0.90.0.0.0.0.0.0.0.0.0.102.102.36,36.14.0.91.0.0.0.0.0.0.0.0.0.102.102.254,36.14.0.46.130.1.192.0.0.9.0.0.0.0.0.1,36.14.0.46.130.1.192.0.0.9.0.0.0.0.0.2'";
        assert_eq!(
            parse_cgcontrdp_pcscf(response, 2),
            vec![
                "240e:2e:8201:c000:9::1".parse::<IpAddr>().unwrap(),
                "240e:2e:8201:c000:9::2".parse::<IpAddr>().unwrap(),
            ]
        );
        assert!(parse_cgcontrdp_pcscf(response, 3).is_empty());
    }

    #[test]
    fn at_probe_family_order_matches_runtime_preference() {
        use crate::access::volte::plan::ImsConnectionPlan;
        assert_eq!(
            ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv6First).pdp_types(),
            vec!["IPV4V6", "IPV6", "IP"]
        );
        assert_eq!(
            ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv4First).pdp_types(),
            vec!["IPV4V6", "IP", "IPV6"]
        );
        assert_eq!(
            ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv6Only).pdp_types(),
            vec!["IPV6"]
        );
        assert_eq!(
            ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv4Only).pdp_types(),
            vec!["IP"]
        );
    }

    #[test]
    fn temporary_at_probe_restores_original_pdp_context() {
        let contexts = "response: '+CGDCONT: 1,\"IPV4V6\",\"ctnet\",\"0.0.0.0\",0,0\n+CGDCONT: 2,\"IPV6\",\"private-ims\",\"0.0.0.0\",0,0'";
        assert_eq!(
            cgdccont_restore_command(contexts, 2).as_deref(),
            Some("AT+CGDCONT=2,\"IPV6\",\"private-ims\"")
        );
        assert!(cgdccont_restore_command(contexts, 3).is_none());
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
