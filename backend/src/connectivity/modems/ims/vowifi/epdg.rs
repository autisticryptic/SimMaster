#![allow(dead_code)]

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use serde::Serialize;
use tokio::net::{lookup_host, UdpSocket};

use super::{
    profiles::CarrierProfile,
    transport::{
        choose_route_policy, DnsResolver, NetworkRoutePolicy, ProxyKind, ResolvedEpdgEndpoint,
        TransportError,
    },
};

const SYSTEM_DNS_TIMEOUT: Duration = Duration::from_secs(4);
const FALLBACK_DNS_TIMEOUT: Duration = Duration::from_secs(2);
/// Budget for standing up the SOCKS5 association used to carry a DNS query.
/// Kept short: resolution is on the connect path and a dead proxy should surface
/// quickly rather than stall the whole VoWiFi bring-up.
const PROXY_DNS_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PUBLIC_DNS_FALLBACKS: &[IpAddr] = &[
    IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
    IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    IpAddr::V4(Ipv4Addr::new(223, 5, 5, 5)),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EpdgConnectionPlan {
    pub profile_id: &'static str,
    pub plmn: &'static str,
    pub host: &'static str,
    pub port: u16,
    pub ip_stack: &'static str,
    pub apn: Option<&'static str>,
    pub dns_server: Option<&'static str>,
    pub route_policy: NetworkRoutePolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EpdgResolutionStatus {
    pub plan: EpdgConnectionPlan,
    pub addresses: Vec<SocketAddr>,
    pub ready: bool,
    pub degraded_reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SystemDnsResolver;

impl DnsResolver for SystemDnsResolver {
    async fn resolve_epdg(
        &self,
        profile: &'static super::profiles::CarrierProfileMeta,
        host: &str,
        port: u16,
    ) -> Result<ResolvedEpdgEndpoint, TransportError> {
        let addresses =
            match tokio::time::timeout(SYSTEM_DNS_TIMEOUT, lookup_host((host, port))).await {
                Ok(Ok(addresses)) => addresses.collect::<Vec<_>>(),
                Ok(Err(primary_error)) => resolve_epdg_via_dns_fallback(profile, host, port)
                    .await
                    .map_err(|fallback_error| {
                        TransportError::DnsFailed(format!(
                            "system_resolver={}; fallback={}",
                            primary_error.kind(),
                            fallback_error
                        ))
                    })?,
                Err(_) => resolve_epdg_via_dns_fallback(profile, host, port)
                    .await
                    .map_err(|fallback_error| {
                        TransportError::DnsFailed(format!(
                            "system_resolver=timeout; fallback={fallback_error}"
                        ))
                    })?,
            };

        if addresses.is_empty() {
            return Err(TransportError::DnsFailed("empty address set".to_string()));
        }

        Ok(ResolvedEpdgEndpoint {
            host: host.to_string(),
            port,
            addresses,
            route_policy: choose_route_policy(profile, host, None),
        })
    }
}

/// Resolve an ePDG through one explicitly selected DNS server. This bypasses
/// the host resolver so carrier geo-DNS answers remain under line control.
pub async fn resolve_epdg_with_dns_override(
    profile: &'static super::profiles::CarrierProfileMeta,
    host: &str,
    port: u16,
    dns_server: Option<IpAddr>,
) -> Result<ResolvedEpdgEndpoint, TransportError> {
    let Some(dns_server) = dns_server else {
        return SystemDnsResolver.resolve_epdg(profile, host, port).await;
    };
    let mut addresses = Vec::new();
    let mut last_error = None;
    match query_dns_records(dns_server, host, port, 1).await {
        Ok(mut v4) => addresses.append(&mut v4),
        Err(error) => last_error = Some(error),
    }
    match query_dns_records(dns_server, host, port, 28).await {
        Ok(mut v6) => addresses.append(&mut v6),
        Err(error) => last_error = Some(error),
    }
    if addresses.is_empty() {
        return Err(last_error
            .unwrap_or_else(|| TransportError::DnsFailed("custom_dns_empty_answer".to_string())));
    }
    addresses.sort();
    addresses.dedup();
    Ok(ResolvedEpdgEndpoint {
        host: host.to_string(),
        port,
        addresses,
        route_policy: choose_route_policy(profile, host, None),
    })
}

/// Resolve an ePDG with the DNS query itself sent through a SOCKS5 proxy.
///
/// DNS follows the proxy on purpose. Resolving directly while the IKE traffic is
/// proxied would (a) expose the real client IP to the resolver and (b) leave the
/// ePDG lookup subject to interception on the local network — the very things the
/// proxy is there to avoid. Because the query egresses where the tunnel does, the
/// answer also reflects the proxy's location, which is what geo-sensitive carrier
/// DNS should see.
///
/// Falls back to `resolve_epdg_with_dns_override` when no explicit DNS server is
/// configured, since there is then no query for us to route.
pub async fn resolve_epdg_via_socks5(
    profile: &'static super::profiles::CarrierProfileMeta,
    host: &str,
    port: u16,
    dns_server: Option<IpAddr>,
    proxy: &super::socks5::Socks5Endpoint,
) -> Result<ResolvedEpdgEndpoint, TransportError> {
    let Some(dns_server) = dns_server else {
        // No server to query through the proxy; keep the existing behaviour.
        return resolve_epdg_with_dns_override(profile, host, port, dns_server).await;
    };

    let client =
        super::socks5::Socks5UdpClient::connect(proxy, dns_server, PROXY_DNS_CONNECT_TIMEOUT)
            .await
            .map_err(|error| TransportError::UnsupportedProxy(error.to_string()))?
            .with_recv_timeout(FALLBACK_DNS_TIMEOUT);

    let server = SocketAddr::new(dns_server, 53);
    let mut addresses = Vec::new();
    let mut last_error = None;
    for qtype in [1u16, 28u16] {
        match query_dns_via_socks5(&client, server, host, port, qtype).await {
            Ok(mut found) => addresses.append(&mut found),
            Err(error) => last_error = Some(error),
        }
    }
    if addresses.is_empty() {
        return Err(last_error
            .unwrap_or_else(|| TransportError::DnsFailed("proxied_dns_empty_answer".to_string())));
    }
    addresses.sort();
    addresses.dedup();
    Ok(ResolvedEpdgEndpoint {
        host: host.to_string(),
        port,
        addresses,
        route_policy: choose_route_policy(profile, host, Some(ProxyKind::Socks5UdpAssociate)),
    })
}

async fn query_dns_via_socks5(
    client: &super::socks5::Socks5UdpClient,
    server: SocketAddr,
    host: &str,
    port: u16,
    qtype: u16,
) -> Result<Vec<SocketAddr>, TransportError> {
    let query = build_dns_query(host, qtype)?;
    client
        .send_to(server, &query)
        .await
        .map_err(|error| TransportError::Io(error.to_string()))?;
    let (_origin, response) = client
        .recv_from()
        .await
        .map_err(|error| TransportError::DnsFailed(error.to_string()))?;
    parse_dns_response(&response, port, qtype)
}

async fn resolve_epdg_via_dns_fallback(
    _profile: &'static super::profiles::CarrierProfileMeta,
    host: &str,
    port: u16,
) -> Result<Vec<SocketAddr>, TransportError> {
    let dns_servers = candidate_dns_servers();
    if dns_servers.is_empty() {
        return Err(TransportError::DnsFailed(
            "no_fallback_dns_server".to_string(),
        ));
    }

    let mut last_error = None;
    for dns_server in dns_servers {
        let mut addresses = Vec::new();
        match query_dns_records(dns_server, host, port, 1).await {
            Ok(mut v4) => addresses.append(&mut v4),
            Err(err) => last_error = Some(err),
        }
        match query_dns_records(dns_server, host, port, 28).await {
            Ok(mut v6) => addresses.append(&mut v6),
            Err(err) => last_error = Some(err),
        }
        if !addresses.is_empty() {
            addresses.sort();
            addresses.dedup();
            return Ok(addresses);
        }
    }

    Err(last_error
        .unwrap_or_else(|| TransportError::DnsFailed("empty_fallback_answer".to_string())))
}

fn candidate_dns_servers() -> Vec<IpAddr> {
    let mut servers = std::fs::read_to_string("/etc/resolv.conf")
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("nameserver"))
        .filter_map(|line| line.split_whitespace().nth(1))
        .filter_map(|value| value.parse::<IpAddr>().ok())
        .collect::<Vec<_>>();
    servers.extend(PUBLIC_DNS_FALLBACKS.iter().copied());
    servers.sort();
    servers.dedup();
    servers
}

async fn query_dns_records(
    dns_server: IpAddr,
    host: &str,
    port: u16,
    qtype: u16,
) -> Result<Vec<SocketAddr>, TransportError> {
    let query = build_dns_query(host, qtype)?;
    let bind_addr = match dns_server {
        IpAddr::V4(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
        IpAddr::V6(_) => SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
    };
    let socket = UdpSocket::bind(bind_addr)
        .await
        .map_err(|err| TransportError::Io(err.kind().to_string()))?;
    let server = SocketAddr::new(dns_server, 53);
    socket
        .send_to(&query, server)
        .await
        .map_err(|err| TransportError::Io(err.kind().to_string()))?;

    let mut buffer = vec![0u8; 1536];
    let (len, _remote) = tokio::time::timeout(FALLBACK_DNS_TIMEOUT, socket.recv_from(&mut buffer))
        .await
        .map_err(|_| TransportError::DnsFailed("fallback_timeout".to_string()))?
        .map_err(|err| TransportError::Io(err.kind().to_string()))?;
    buffer.truncate(len);
    parse_dns_response(&buffer, port, qtype)
}

fn build_dns_query(host: &str, qtype: u16) -> Result<Vec<u8>, TransportError> {
    let mut query = vec![
        0x53, 0x41, // transaction id: "SA" for SimAdmin
        0x01, 0x00, // recursion desired
        0x00, 0x01, // qdcount
        0x00, 0x00, // ancount
        0x00, 0x00, // nscount
        0x00, 0x00, // arcount
    ];
    for label in host.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(TransportError::DnsFailed("invalid_dns_label".to_string()));
        }
        query.push(label.len() as u8);
        query.extend_from_slice(label.as_bytes());
    }
    query.push(0);
    query.extend_from_slice(&qtype.to_be_bytes());
    query.extend_from_slice(&1u16.to_be_bytes()); // IN
    Ok(query)
}

fn parse_dns_response(
    response: &[u8],
    port: u16,
    expected_qtype: u16,
) -> Result<Vec<SocketAddr>, TransportError> {
    if response.len() < 12 {
        return Err(TransportError::DnsFailed("short_dns_response".to_string()));
    }
    if response[0] != 0x53 || response[1] != 0x41 {
        return Err(TransportError::DnsFailed(
            "dns_transaction_mismatch".to_string(),
        ));
    }
    let rcode = response[3] & 0x0f;
    if rcode != 0 {
        return Err(TransportError::DnsFailed(format!("dns_rcode_{rcode}")));
    }
    let qdcount = u16::from_be_bytes([response[4], response[5]]) as usize;
    let ancount = u16::from_be_bytes([response[6], response[7]]) as usize;
    let mut offset = 12usize;
    for _ in 0..qdcount {
        offset = skip_dns_name(response, offset)?;
        offset = offset
            .checked_add(4)
            .ok_or_else(|| TransportError::DnsFailed("dns_question_overflow".to_string()))?;
        if offset > response.len() {
            return Err(TransportError::DnsFailed(
                "truncated_dns_question".to_string(),
            ));
        }
    }

    let mut addresses = Vec::new();
    for _ in 0..ancount {
        offset = skip_dns_name(response, offset)?;
        if offset + 10 > response.len() {
            return Err(TransportError::DnsFailed(
                "truncated_dns_answer".to_string(),
            ));
        }
        let rr_type = u16::from_be_bytes([response[offset], response[offset + 1]]);
        let rr_class = u16::from_be_bytes([response[offset + 2], response[offset + 3]]);
        let rdlen = u16::from_be_bytes([response[offset + 8], response[offset + 9]]) as usize;
        offset += 10;
        if offset + rdlen > response.len() {
            return Err(TransportError::DnsFailed("truncated_dns_rdata".to_string()));
        }
        if rr_class == 1 && rr_type == expected_qtype {
            match (rr_type, rdlen) {
                (1, 4) => addresses.push(SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::new(
                        response[offset],
                        response[offset + 1],
                        response[offset + 2],
                        response[offset + 3],
                    )),
                    port,
                )),
                (28, 16) => {
                    let mut octets = [0u8; 16];
                    octets.copy_from_slice(&response[offset..offset + 16]);
                    addresses.push(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), port));
                }
                _ => {}
            }
        }
        offset += rdlen;
    }
    Ok(addresses)
}

fn skip_dns_name(response: &[u8], mut offset: usize) -> Result<usize, TransportError> {
    let mut jumps = 0u8;
    loop {
        if offset >= response.len() {
            return Err(TransportError::DnsFailed("truncated_dns_name".to_string()));
        }
        let len = response[offset];
        if len & 0xc0 == 0xc0 {
            if offset + 1 >= response.len() {
                return Err(TransportError::DnsFailed(
                    "truncated_dns_pointer".to_string(),
                ));
            }
            return Ok(offset + 2);
        }
        if len & 0xc0 != 0 {
            return Err(TransportError::DnsFailed("invalid_dns_pointer".to_string()));
        }
        offset += 1;
        if len == 0 {
            return Ok(offset);
        }
        offset = offset
            .checked_add(len as usize)
            .ok_or_else(|| TransportError::DnsFailed("dns_name_overflow".to_string()))?;
        jumps = jumps.saturating_add(1);
        if jumps > 128 {
            return Err(TransportError::DnsFailed("dns_name_too_deep".to_string()));
        }
    }
}

pub fn build_connection_plan(
    profile: &'static CarrierProfile,
    requested_proxy: Option<ProxyKind>,
) -> EpdgConnectionPlan {
    EpdgConnectionPlan {
        profile_id: profile.meta.profile_id,
        plmn: profile.meta.plmn,
        host: profile.epdg.host,
        port: profile.epdg.port,
        ip_stack: profile.epdg.ip_stack,
        apn: profile.epdg.apn,
        dns_server: profile.epdg.dns_server,
        route_policy: choose_route_policy(&profile.meta, profile.epdg.host, requested_proxy),
    }
}

pub async fn resolve_connection_plan<R>(
    resolver: &R,
    profile: &'static CarrierProfile,
    requested_proxy: Option<ProxyKind>,
) -> EpdgResolutionStatus
where
    R: DnsResolver + Sync,
{
    let plan = build_connection_plan(profile, requested_proxy);
    match resolver
        .resolve_epdg(&profile.meta, profile.epdg.host, profile.epdg.port)
        .await
    {
        Ok(mut endpoint) => {
            endpoint.route_policy = plan.route_policy.clone();
            EpdgResolutionStatus {
                plan,
                addresses: endpoint.addresses,
                ready: true,
                degraded_reason: None,
            }
        }
        Err(err) => EpdgResolutionStatus {
            plan,
            addresses: Vec::new(),
            ready: false,
            degraded_reason: Some(err.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::modems::ims::vowifi::{
        profiles::{GB_EE_23433, US_ATT_310410},
        transport::ProxyKind,
    };

    #[test]
    fn plan_preserves_clean_room_profile_metadata() {
        let plan = build_connection_plan(&GB_EE_23433, None);

        assert_eq!(plan.profile_id, "gb_ee_23433");
        assert_eq!(plan.plmn, "23433");
        assert_eq!(plan.host, "epdg.epc.mnc033.mcc234.pub.3gppnetwork.org");
        assert_eq!(plan.route_policy.kind, ProxyKind::Direct);
    }

    #[test]
    fn plan_can_select_future_udp_relay_without_plain_http_connect() {
        let plan = build_connection_plan(&US_ATT_310410, Some(ProxyKind::UdpRelay));

        assert_eq!(plan.route_policy.kind, ProxyKind::UdpRelay);
        assert_eq!(plan.route_policy.policy_id, "udp_relay_us_epdg");
    }

    #[test]
    fn parses_minimal_dns_a_response_without_serializing_query_payload() {
        let response = [
            0x53, 0x41, 0x81, 0x80, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x04, b'e',
            b'p', b'd', b'g', 0x03, b'e', b'p', b'c', 0x03, b'm', b'n', b'c', 0x03, b'0', b'3',
            b'3', 0x00, 0x00, 0x01, 0x00, 0x01, 0xc0, 0x0c, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00,
            0x00, 0x3c, 0x00, 0x04, 31, 94, 76, 10,
        ];

        let addresses = parse_dns_response(&response, 500, 1).expect("parse A response");

        assert_eq!(
            addresses,
            vec![SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(31, 94, 76, 10)),
                500
            )]
        );

        let json = serde_json::to_string(&addresses).expect("serialize addresses");
        for forbidden in ["payload", "query", "imsi", "iccid", "key"] {
            assert!(!json.to_ascii_lowercase().contains(forbidden));
        }
    }
}
