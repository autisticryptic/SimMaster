//! Strict configured endpoints and bounded, bearer-local UDP DNS discovery.
//! No system resolver, search suffixes, NAPTR, TCP or TLS fallback is used.

use std::{
    future::Future,
    net::{IpAddr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use super::super::{
    errors::{code, CellularImsError},
    pcscf_dns::{DnsRecords, SrvTarget},
};
use super::{same_family, ImsIpSettings, ENV_PCSCF, LEGACY_ENV_PCSCF, SIP_PORT};

pub const MAX_PCSCF_ENDPOINTS: usize = 16;
const MAX_CONFIGURED_TARGETS: usize = 32;
const MAX_CONFIGURED_TEXT: usize = 4096;
const MAX_DNS_SERVERS: usize = 3;
const MAX_DNS_QUERIES: usize = 16;
const DNS_TOTAL_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcscfSource {
    Environment,
    BearerPco,
    Profile,
    Isim,
    StandardDns,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcscfTransport {
    Udp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcscfEndpoint {
    pub socket: SocketAddr,
    pub source: PcscfSource,
    /// Canonical configured hostname or SRV target; absent for IP literals.
    pub host: Option<String>,
    pub transport: PcscfTransport,
    /// DNS TTL in seconds, limited by CNAME and (where present) SRV TTLs.
    /// This is metadata, not a cache or an authorization to reuse a bearer.
    pub ttl: Option<u32>,
    pub srv_priority: Option<u16>,
    pub srv_weight: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PcscfHost {
    Address(IpAddr),
    Name(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPcscfEndpoint {
    pub host: PcscfHost,
    pub port: u16,
    pub transport: PcscfTransport,
}

fn invalid(reason: &'static str) -> CellularImsError {
    CellularImsError::with_detail(code::PCSCF_ENDPOINT_INVALID, reason)
}

fn unavailable() -> CellularImsError {
    CellularImsError::new(code::RUNTIME_ALL_PCSCF_FAILED)
}

fn unsupported() -> CellularImsError {
    CellularImsError::new(code::PCSCF_TRANSPORT_UNSUPPORTED)
}

fn port(value: &str) -> Result<u16, CellularImsError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid("port_invalid"));
    }
    value.parse::<u16>().ok().filter(|port| *port != 0)
        .ok_or_else(|| invalid("port_invalid"))
}

fn hostname(value: &str) -> Result<String, CellularImsError> {
    let name = value.strip_suffix('.').unwrap_or(value);
    if name.is_empty() || name.len() > 253 || !name.is_ascii()
        || name.split('.').any(|label| {
            label.is_empty() || label.len() > 63
                || !label.as_bytes()[0].is_ascii_alphanumeric()
                || !label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                || !label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        // Numeric/dotted malformed IPs must not turn into DNS queries.
        || name.bytes().all(|b| b.is_ascii_digit() || b == b'.')
    {
        return Err(invalid("host_invalid"));
    }
    Ok(name.to_ascii_lowercase())
}

/// Parse exactly one endpoint, without IO or lossy URI normalization.
/// Bare IPv6 is always an address (never an inferred host:port). Semicolons
/// are URI parameters, not list separators. Only SIP's UDP transport exists.
pub fn parse_pcscf_endpoint(value: &str) -> Result<ParsedPcscfEndpoint, CellularImsError> {
    if value.is_empty() || value.len() > 512 || !value.is_ascii()
        || value.bytes().any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
    {
        return Err(invalid("endpoint_text_invalid"));
    }
    if value.get(..5).is_some_and(|prefix| prefix.eq_ignore_ascii_case("sips:")) {
        return Err(unsupported());
    }
    let sip = value.get(..4).is_some_and(|prefix| prefix.eq_ignore_ascii_case("sip:"));
    let value = if sip { &value[4..] } else { value };
    if value.bytes().any(|b| matches!(b, b'@' | b'?' | b'#' | b'/' | b'\\' | b'%' | b'<' | b'>' | b'"' | b'\'')) {
        return Err(invalid("uri_component_unsupported"));
    }
    let mut parts = value.split(';');
    let authority = parts.next().unwrap_or_default();
    let mut seen_transport = false;
    for parameter in parts {
        if !sip || seen_transport {
            return Err(invalid("uri_parameter_invalid"));
        }
        let (key, value) = parameter.split_once('=')
            .ok_or_else(|| invalid("uri_parameter_invalid"))?;
        if !key.eq_ignore_ascii_case("transport") || value.is_empty()
            || !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(invalid("uri_parameter_invalid"));
        }
        if !value.eq_ignore_ascii_case("udp") {
            return Err(unsupported());
        }
        seen_transport = true;
    }
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (address, suffix) = bracketed.split_once(']')
            .ok_or_else(|| invalid("ipv6_brackets_invalid"))?;
        let address = address.parse::<Ipv6Addr>().map_err(|_| invalid("ipv6_invalid"))?;
        let port = if suffix.is_empty() { SIP_PORT } else {
            port(suffix.strip_prefix(':').ok_or_else(|| invalid("ipv6_suffix_invalid"))?)?
        };
        (PcscfHost::Address(IpAddr::V6(address)), port)
    } else if let Ok(address) = authority.parse::<IpAddr>() {
        if sip && address.is_ipv6() {
            return Err(invalid("sip_ipv6_brackets_required"));
        }
        (PcscfHost::Address(address), SIP_PORT)
    } else {
        let (host, port) = match authority.split_once(':') {
            Some((host, value)) => (host, port(value)?),
            None => (authority, SIP_PORT),
        };
        let host = match host.parse::<IpAddr>() {
            Ok(address) => PcscfHost::Address(address),
            Err(_) => PcscfHost::Name(hostname(host)?),
        };
        (host, port)
    };
    Ok(ParsedPcscfEndpoint { host, port, transport: PcscfTransport::Udp })
}

fn parse_list(value: &str) -> Result<Vec<ParsedPcscfEndpoint>, CellularImsError> {
    if value.len() > MAX_CONFIGURED_TEXT || !value.is_ascii()
        || value.bytes().any(|b| b.is_ascii_control())
    {
        return Err(invalid("endpoint_list_invalid"));
    }
    let mut targets = Vec::new();
    // Retain comma/space-separated overrides, but never split a SIP parameter.
    // Empty comma entries are errors rather than silently missing candidates.
    for group in value.split(',') {
        let group = group.trim_matches(' ');
        if group.is_empty() {
            return Err(invalid("endpoint_list_empty"));
        }
        for value in group.split(' ').filter(|value| !value.is_empty()) {
            if targets.len() == MAX_CONFIGURED_TARGETS {
                return Err(invalid("endpoint_list_limit"));
            }
            targets.push(parse_pcscf_endpoint(value)?);
        }
    }
    Ok(targets)
}

pub(super) fn environment_override_with(
    mut read: impl FnMut(&str) -> Result<String, std::env::VarError>,
) -> Result<Option<String>, CellularImsError> {
    for name in [ENV_PCSCF, LEGACY_ENV_PCSCF] {
        match read(name) {
            Ok(value) => return Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => {},
            Err(std::env::VarError::NotUnicode(_)) => return Err(invalid("environment_not_unicode")),
        }
    }
    Ok(None)
}

#[derive(Debug)]
pub(super) enum Selection {
    Explicit(PcscfSource, Vec<ParsedPcscfEndpoint>),
    StandardDns,
}

/// Select by presence, not by parse success, resolution success or family.
/// A family mismatch is returned to the caller's existing granted-family loop;
/// it is never permission to bypass an explicitly selected source.
pub(super) fn select_source(
    environment: Option<&str>,
    settings: &ImsIpSettings,
    configured: Option<&str>,
    isim: &[String],
) -> Result<Selection, CellularImsError> {
    if let Some(value) = environment {
        return Ok(Selection::Explicit(PcscfSource::Environment, parse_list(value)?));
    }
    if !settings.pcscf.is_empty() {
        if settings.pcscf.len() > MAX_CONFIGURED_TARGETS {
            return Err(invalid("pco_endpoint_limit"));
        }
        return Ok(Selection::Explicit(PcscfSource::BearerPco,
            settings.pcscf.iter().map(|address| ParsedPcscfEndpoint {
                host: PcscfHost::Address(*address), port: SIP_PORT, transport: PcscfTransport::Udp,
            }).collect()));
    }
    if let Some(value) = configured {
        return Ok(Selection::Explicit(PcscfSource::Profile, parse_list(value)?));
    }
    if !isim.is_empty() {
        if isim.len() > MAX_CONFIGURED_TARGETS {
            return Err(invalid("isim_endpoint_limit"));
        }
        let targets = isim.iter().map(|value| parse_pcscf_endpoint(value))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Selection::Explicit(PcscfSource::Isim, targets));
    }
    Ok(Selection::StandardDns)
}

#[derive(Clone, Copy)]
pub(super) struct DiscoveryLimits {
    candidates: usize,
    queries: usize,
    total: Duration,
    per_query: Duration,
}

impl Default for DiscoveryLimits {
    fn default() -> Self {
        Self {
            candidates: MAX_PCSCF_ENDPOINTS, queries: MAX_DNS_QUERIES,
            total: DNS_TOTAL_TIMEOUT, per_query: super::DNS_TIMEOUT,
        }
    }
}

struct CachedQuery {
    server: IpAddr,
    name: String,
    kind: u16,
    records: Option<DnsRecords>,
}

struct Resolver<Query> {
    query: Query,
    cache: Vec<CachedQuery>,
    deadline: tokio::time::Instant,
    limits: DiscoveryLimits,
}

impl<Query> Resolver<Query> {
    async fn records<Fut>(&mut self, server: IpAddr, name: &str, kind: u16) -> Option<DnsRecords>
    where
        Query: FnMut(IpAddr, String, u16) -> Fut,
        Fut: Future<Output = Result<DnsRecords, CellularImsError>>,
    {
        if let Some(cached) = self.cache.iter().find(|entry| {
            entry.server == server && entry.name == name && entry.kind == kind
        }) {
            return cached.records.clone();
        }
        let now = tokio::time::Instant::now();
        if self.cache.len() >= self.limits.queries || now >= self.deadline {
            return None;
        }
        // Includes socket creation, send and receive, not just recv(). The
        // shared deadline applies across every hostname, server and SRV target.
        let deadline = self.deadline.min(now + self.limits.per_query);
        let records = tokio::time::timeout_at(deadline, (self.query)(server, name.to_string(), kind))
            .await.ok().and_then(Result::ok);
        self.cache.push(CachedQuery { server, name: name.to_string(), kind, records: records.clone() });
        records
    }
}

fn add_endpoint(endpoints: &mut Vec<PcscfEndpoint>, candidate: PcscfEndpoint, limit: usize) {
    if let Some(existing) = endpoints.iter_mut().find(|entry| {
        entry.socket == candidate.socket && entry.transport == candidate.transport
    }) {
        // Keep the first source/priority/order, but never extend its TTL when
        // another alias or RR supplies the same socket with a shorter lifetime.
        if let (Some(old), Some(new)) = (existing.ttl, candidate.ttl) {
            existing.ttl = Some(old.min(new));
        }
    } else if endpoints.len() < limit {
        endpoints.push(candidate);
    }
}

fn add_addresses(
    endpoints: &mut Vec<PcscfEndpoint>, records: DnsRecords, local: IpAddr,
    source: PcscfSource, host: &str, port: u16, srv: Option<&SrvTarget>, limit: usize,
) {
    for address in records.addresses {
        if !same_family(local, address) { continue; }
        let ttl = records.address_ttls.get(&address).copied();
        let ttl = match (ttl, srv) {
            (Some(ttl), Some(srv)) => Some(ttl.min(srv.ttl)),
            (ttl, _) => ttl,
        };
        add_endpoint(endpoints, PcscfEndpoint {
            socket: SocketAddr::new(address, port), source, host: Some(host.to_string()),
            transport: PcscfTransport::Udp, ttl,
            srv_priority: srv.map(|s| s.priority), srv_weight: srv.map(|s| s.weight),
        }, limit);
    }
}

/// RFC 2782 ordering within each priority group. The injected inclusive draw
/// makes weighted selection testable without statistical/flaky assertions.
fn order_srv_targets_with(
    mut targets: Vec<SrvTarget>,
    mut draw: impl FnMut(u32) -> Result<u32, CellularImsError>,
) -> Result<Vec<SrvTarget>, CellularImsError> {
    targets.sort_by_key(|target| target.priority);
    let mut ordered = Vec::with_capacity(targets.len());
    while !targets.is_empty() {
        let priority = targets[0].priority;
        let count = targets.iter().take_while(|target| target.priority == priority).count();
        let mut group = targets.drain(..count).collect::<Vec<_>>();
        for index in (1..group.len()).rev() {
            let chosen = draw(index as u32)? as usize;
            group.swap(index, chosen);
        }
        while !group.is_empty() {
            // RFC 2782 gives zero-weight RRs a small chance when nonzero
            // weights exist by drawing from the inclusive range 0..=sum.
            group.sort_by_key(|target| target.weight != 0);
            let sum: u32 = group.iter().map(|target| u32::from(target.weight)).sum();
            let ticket = draw(sum)?;
            let mut cumulative = 0;
            let index = group.iter().position(|target| {
                cumulative += u32::from(target.weight);
                cumulative >= ticket
            }).unwrap_or(0);
            ordered.push(group.remove(index));
        }
    }
    Ok(ordered)
}

fn srv_draw(max: u32) -> Result<u32, CellularImsError> {
    use ring::rand::{SecureRandom, SystemRandom};
    if max == 0 { return Ok(0); }
    let range = u64::from(max) + 1;
    let space = 1u64 << 32;
    let ceiling = space - space % range;
    // Rejection sampling without modulo bias, with a finite RNG budget.
    for _ in 0..8 {
        let mut bytes = [0u8; 4];
        SystemRandom::new().fill(&mut bytes)
            .map_err(|_| CellularImsError::new(code::RANDOM_FAILED))?;
        let value = u64::from(u32::from_ne_bytes(bytes));
        if value < ceiling { return Ok((value % range) as u32); }
    }
    Err(CellularImsError::new(code::RANDOM_FAILED))
}

pub(super) async fn discover_with<Query, Fut>(
    selection: Selection,
    settings: &ImsIpSettings,
    home_domain: &str,
    local: IpAddr,
    query: Query,
    limits: DiscoveryLimits,
) -> Result<Vec<PcscfEndpoint>, CellularImsError>
where
    Query: FnMut(IpAddr, String, u16) -> Fut,
    Fut: Future<Output = Result<DnsRecords, CellularImsError>>,
{
    let deadline = tokio::time::Instant::now() + limits.total;
    let mut resolver = Resolver { query, cache: Vec::new(), deadline, limits };
    let dns = if local.is_ipv6() { &settings.ipv6_dns } else { &settings.ipv4_dns };
    let mut servers = Vec::new();
    for server in dns.iter().take(MAX_CONFIGURED_TARGETS) {
        if same_family(local, *server) && !servers.contains(server) {
            servers.push(*server);
            if servers.len() == MAX_DNS_SERVERS { break; }
        }
    }
    let kind = if local.is_ipv6() { 28 } else { 1 };
    let mut endpoints = Vec::new();
    match selection {
        Selection::Explicit(source, targets) => {
            let mut eligible = false;
            for target in targets {
                match target.host {
                    PcscfHost::Address(address) if same_family(local, address) => {
                        eligible = true;
                        add_endpoint(&mut endpoints, PcscfEndpoint {
                            socket: SocketAddr::new(address, target.port), source, host: None,
                            transport: target.transport, ttl: None, srv_priority: None, srv_weight: None,
                        }, limits.candidates);
                    }
                    PcscfHost::Name(host) => {
                        eligible = true;
                        if endpoints.len() >= limits.candidates { continue; }
                        for server in &servers {
                            if let Some(records) = resolver.records(*server, &host, kind).await {
                                let has_addresses = records.addresses.iter().any(|address| same_family(local, *address));
                                add_addresses(&mut endpoints, records, local, source, &host, target.port, None, limits.candidates);
                                if has_addresses { break; }
                            }
                        }
                    }
                    _ => {},
                }
            }
            if !eligible {
                return Err(CellularImsError::new(code::PCSCF_FAMILY_MISMATCH));
            }
        }
        Selection::StandardDns => {
            let domain = hostname(home_domain)?;
            let name = format!("pcscf.{domain}");
            let srv_names = super::pcscf_srv_names(&domain);
            // Preserve the existing direct-name then UDP-SRV fallback. Use a
            // whole bounded RRset, not a first IP/target; do not union unrelated
            // service names or conflicting responses from different resolvers.
            for server in &servers {
                if let Some(records) = resolver.records(*server, &name, kind).await {
                    add_addresses(&mut endpoints, records, local, PcscfSource::StandardDns, &name, SIP_PORT, None, limits.candidates);
                    if !endpoints.is_empty() { break; }
                }
                for srv_name in &srv_names {
                    let Some(records) = resolver.records(*server, srv_name, 33).await else { continue };
                    let targets = order_srv_targets_with(records.srv_targets, srv_draw)?;
                    for target in targets {
                        if endpoints.len() >= limits.candidates { break; }
                        // A service RR's target is a hostname, never URI syntax.
                        let host = hostname(&target.target)?;
                        if let Some(records) = resolver.records(*server, &host, kind).await {
                            add_addresses(&mut endpoints, records, local, PcscfSource::StandardDns,
                                &host, target.port, Some(&target), limits.candidates);
                        }
                    }
                    if !endpoints.is_empty() { break; }
                }
                if !endpoints.is_empty() { break; }
            }
        }
    }
    if endpoints.is_empty() { Err(unavailable()) } else { Ok(endpoints) }
}

#[cfg(test)]
#[path = "pcscf_endpoint_tests.rs"]
mod tests;
