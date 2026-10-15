//! Opt-in, information-only DHCP SIP discovery on an already established UE bearer.
//!
//! RFC 2131 §4.4.3 / RFC 3361 option 120 and RFC 8415 §18.2.6 /
//! RFC 3319 options 21/22. Never requests/installs addresses, routes, or DNS.
//! A correlated response is bearer-sourced information, NOT authenticated
//! server identity; SIP TLS/AKA and source selection remain the caller's job.
//! IPv4 deliberately requires direct ACKs (no unverifiable relay giaddr) and
//! RFC 6842 client-id echo. IPv6 relays must strip Relay-reply before delivery
//! to a client, as required by RFC 8415; relay envelopes are not accepted.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
use std::time::Duration;

use ring::rand::{SecureRandom, SystemRandom};
use tokio::net::UdpSocket;
use tokio::time::{timeout_at, Instant};

use crate::hardware::devices::transport::ImsPcscfDiscovery;
use crate::services::ue_worker::{UeSocket, UeSocketSpec, UeWorkerBinding};
use super::errors::{code, CellularImsError};

const MAX_WIRE: usize = 4096;
const MAX_OPTIONS: usize = 128;
const MAX_RESULTS: usize = 16;
const MAX_MESSAGES: usize = 32;
const ATTEMPTS: usize = 2;
const TOTAL_BUDGET: Duration = Duration::from_secs(12);
const FAMILY_BUDGET: Duration = Duration::from_secs(6);
const COOKIE: [u8; 4] = [99, 130, 83, 99];

type DecodeResult<T> = Result<T, &'static str>;

fn failure(reason: &'static str) -> CellularImsError {
    CellularImsError::with_detail(code::DHCP_PCSCF_FAILED, reason)
}

fn current(binding: &UeWorkerBinding) -> Result<(), CellularImsError> {
    if binding.is_current() { Ok(()) } else { Err(failure("dhcp_worker_changed")) }
}

/// Call only for explicit InformationRequest policy, after the captured
/// provider bearer address(es) have been configured in this exact worker.
/// At most one granted address per family, two sends/family, 32 receives/family,
/// 4096 bytes/message, 16 results and a 12-second total including socket creation.
/// Domain names preserve server order; RFC 3319 prefers domains over addresses.
pub(super) async fn discover(
    binding: &UeWorkerBinding,
    interface: &str,
    locals: &[IpAddr],
) -> Result<ImsPcscfDiscovery, CellularImsError> {
    discover_with(binding, interface, locals, &ExchangeSettings::default()).await
}

#[derive(Default)]
struct ExchangeSettings {
    #[cfg(test)]
    wire: Option<TestWire>,
}

#[cfg(test)]
struct TestWire {
    local_port: u16,
    peer: SocketAddr,
    budget: Duration,
}

impl ExchangeSettings {
    fn local_port(&self, local: IpAddr) -> u16 {
        #[cfg(test)]
        if let Some(wire) = &self.wire { return wire.local_port; }
        if local.is_ipv4() { 68 } else { 546 }
    }

    fn budget(&self) -> Duration {
        #[cfg(test)]
        if let Some(wire) = &self.wire { return wire.budget; }
        FAMILY_BUDGET
    }

    fn destination(&self, local: IpAddr, socket: &UdpSocket) -> Result<SocketAddr, CellularImsError> {
        #[cfg(test)]
        if let Some(wire) = &self.wire { return Ok(wire.peer); }
        if local.is_ipv4() {
            Ok(SocketAddr::new(Ipv4Addr::BROADCAST.into(), 67))
        } else {
            // Read the namespace-resolved interface index already set on this
            // fd by the UE worker. Never resolve a name in the parent netns.
            Ok(SocketAddr::V6(SocketAddrV6::new(
                "ff02::1:2".parse().expect("constant multicast address"),
                547, 0, multicast_index(socket)?,
            )))
        }
    }

    fn initial_delay(&self, random: u16, ipv6: bool) -> Duration {
        #[cfg(test)]
        if self.wire.is_some() { return Duration::ZERO; }
        if ipv6 { Duration::from_millis(u64::from(random) % 1001) } else { Duration::ZERO }
    }

    fn retry_delay(&self, random: u16, ipv6: bool, attempt: usize) -> Duration {
        #[cfg(test)]
        if self.wire.is_some() { return Duration::from_millis(50); }
        let base = if ipv6 { 1000u64 << attempt } else { 4000u64 << attempt };
        Duration::from_millis(base * (900 + u64::from(random) % 201) / 1000)
    }
}

#[cfg(target_os = "linux")]
fn multicast_index(socket: &UdpSocket) -> Result<u32, CellularImsError> {
    use std::os::fd::AsRawFd;
    let mut index: libc::c_uint = 0;
    let mut len = std::mem::size_of_val(&index) as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(socket.as_raw_fd(), libc::IPPROTO_IPV6, libc::IPV6_MULTICAST_IF,
            (&mut index as *mut libc::c_uint).cast(), &mut len)
    };
    if rc != 0 || index == 0 || len as usize != std::mem::size_of_val(&index) {
        return Err(failure("dhcp_namespace_scope_unavailable"));
    }
    Ok(index)
}

#[cfg(not(target_os = "linux"))]
fn multicast_index(_socket: &UdpSocket) -> Result<u32, CellularImsError> {
    Err(failure("dhcp_namespace_scope_unavailable"))
}

async fn discover_with(
    binding: &UeWorkerBinding,
    interface: &str,
    locals: &[IpAddr],
    settings: &ExchangeSettings,
) -> Result<ImsPcscfDiscovery, CellularImsError> {
    current(binding)?;
    if interface.is_empty() || interface.len() >= 16
        || !interface.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        || locals.len() > 16
    {
        return Err(failure("dhcp_binding_arguments_invalid"));
    }
    #[cfg(test)]
    if let Some(wire) = &settings.wire {
        // No privileged-port or arbitrary external-peer injection even in tests.
        if wire.local_port < 1024 || wire.peer.port() < 1024 || !wire.peer.ip().is_loopback() {
            return Err(failure("dhcp_test_boundary_invalid"));
        }
    }
    let mut selected = Vec::with_capacity(2);
    for local in locals.iter().copied().filter(|ip| eligible_local(*ip)) {
        if !selected.iter().any(|old: &IpAddr| old.is_ipv4() == local.is_ipv4()) {
            selected.push(local);
        }
    }
    if selected.is_empty() { return Err(failure("dhcp_no_granted_family")); }
    let end = Instant::now() + TOTAL_BUDGET;
    let mut last = failure("dhcp_information_unavailable");
    for local in selected {
        current(binding)?;
        let deadline = end.min(Instant::now() + settings.budget());
        match timeout_at(deadline, exchange(binding, interface, local, settings, deadline)).await {
            Ok(Ok(result)) => { current(binding)?; return Ok(result); }
            Ok(Err(error)) => last = error,
            Err(_) => last = failure("dhcp_information_timeout"),
        }
        current(binding)?;
        if Instant::now() >= end { break; }
    }
    Err(last)
}

struct Transaction {
    xid: [u8; 4],
    duid: [u8; 18],
    client_v4: Vec<u8>,
    jitter: [u16; 3],
}

impl Transaction {
    fn random() -> Result<Self, CellularImsError> {
        let mut bytes = [0u8; 30];
        SystemRandom::new().fill(&mut bytes).map_err(|_| failure("dhcp_random_unavailable"))?;
        let mut duid = [0u8; 18];
        duid[..2].copy_from_slice(&4u16.to_be_bytes()); // DUID-UUID, RFC 6355
        duid[2..].copy_from_slice(&bytes[4..20]);
        duid[8] = (duid[8] & 0x0f) | 0x40; // random UUID v4
        duid[10] = (duid[10] & 0x3f) | 0x80;
        let mut client_v4 = vec![255]; // RFC 4361 IAID + DUID, no subscriber/MAC identity
        client_v4.extend_from_slice(&bytes[20..24]);
        client_v4.extend_from_slice(&duid);
        Ok(Self {
            xid: bytes[..4].try_into().expect("fixed slice"), duid, client_v4,
            jitter: [u16::from_be_bytes([bytes[24], bytes[25]]),
                u16::from_be_bytes([bytes[26], bytes[27]]), u16::from_be_bytes([bytes[28], bytes[29]])],
        })
    }
}

async fn exchange(
    binding: &UeWorkerBinding,
    interface: &str,
    local: IpAddr,
    settings: &ExchangeSettings,
    deadline: Instant,
) -> Result<ImsPcscfDiscovery, CellularImsError> {
    let transaction = Transaction::random()?;
    let mut spec = UeSocketSpec::udp_bound(
        SocketAddr::new(local, settings.local_port(local)), Some(interface.to_owned()));
    spec.reuse_address = false; // conflict with an existing DHCP owner is explicit failure
    spec.dhcp_information = true;
    current(binding)?;
    let socket = binding.create_socket(spec).await.map_err(|_| failure("dhcp_worker_socket_unavailable"))?;
    current(binding)?;
    let UeSocket::Udp(socket) = socket else { return Err(failure("dhcp_worker_socket_kind")); };
    let destination = settings.destination(local, &socket)?;
    let ipv6 = local.is_ipv6();
    current(binding)?;
    tokio::time::sleep(settings.initial_delay(transaction.jitter[0], ipv6)).await;
    current(binding)?;
    let started = Instant::now();
    let mut received = 0;
    // One extra byte distinguishes an over-limit/truncated datagram from an
    // exactly MAX_WIRE response, even if the kernel discards the remaining tail.
    let mut buffer = [0u8; MAX_WIRE + 1];
    for attempt in 0..ATTEMPTS {
        current(binding)?;
        let elapsed = started.elapsed();
        let request = match local {
            IpAddr::V4(local) => inform_v4(local, &transaction, elapsed),
            IpAddr::V6(_) => information_v6(&transaction, elapsed),
        };
        let sent = socket.send_to(&request, destination).await.map_err(|_| failure("dhcp_send_failed"))?;
        current(binding)?;
        if sent != request.len() { return Err(failure("dhcp_send_truncated")); }
        let round_end = deadline.min(Instant::now() + settings.retry_delay(transaction.jitter[attempt + 1], ipv6, attempt));
        loop {
            current(binding)?;
            if received >= MAX_MESSAGES { return Err(failure("dhcp_message_limit")); }
            let response = timeout_at(round_end, socket.recv_from(&mut buffer)).await;
            current(binding)?;
            let (len, peer) = match response {
                Err(_) => break,
                Ok(Err(_)) => return Err(failure("dhcp_receive_failed")),
                Ok(Ok(response)) => response,
            };
            received += 1;
            if len > MAX_WIRE { continue; }
            // Test transport changes only endpoints/ports. The decoder still
            // sees production server-port and non-loopback source constraints.
            let peer = decoder_peer(settings, peer, destination, ipv6);
            let Some(peer) = peer else { continue; };
            let decoded = match local {
                IpAddr::V4(local) => decode_v4(&buffer[..len], &transaction, local, peer),
                IpAddr::V6(_) => decode_v6(&buffer[..len], &transaction, peer, destination),
            };
            if let Ok(result) = decoded {
                // Never merge different servers, relay paths or transactions.
                current(binding)?;
                return Ok(result);
            }
        }
    }
    Err(failure("dhcp_information_timeout"))
}

fn decoder_peer(settings: &ExchangeSettings, peer: SocketAddr, destination: SocketAddr, ipv6: bool) -> Option<SocketAddr> {
    #[cfg(test)]
    if settings.wire.is_some() {
        if peer != destination { return None; }
        return Some(if ipv6 { "[2001:db8::1]:547".parse().unwrap() } else { "192.0.2.1:67".parse().unwrap() });
    }
    let _ = (settings, destination, ipv6);
    Some(peer)
}

fn inform_v4(local: Ipv4Addr, transaction: &Transaction, elapsed: Duration) -> Vec<u8> {
    let mut packet = vec![0u8; 240];
    packet[0] = 1; // BOOTREQUEST, no fake hardware address on a WWAN link
    packet[4..8].copy_from_slice(&transaction.xid);
    packet[8..10].copy_from_slice(&(elapsed.as_secs().min(u64::from(u16::MAX)) as u16).to_be_bytes());
    packet[12..16].copy_from_slice(&local.octets()); // externally granted ciaddr
    packet[236..240].copy_from_slice(&COOKIE);
    push_v4(&mut packet, 53, &[8]); // DHCPINFORM only
    push_v4(&mut packet, 61, &transaction.client_v4);
    push_v4(&mut packet, 55, &[120]);
    push_v4(&mut packet, 57, &(MAX_WIRE as u16).to_be_bytes());
    packet.push(255);
    packet.resize(packet.len().max(300), 0);
    packet
}

fn push_v4(packet: &mut Vec<u8>, code: u8, value: &[u8]) {
    packet.extend_from_slice(&[code, value.len() as u8]);
    packet.extend_from_slice(value);
}

fn information_v6(transaction: &Transaction, elapsed: Duration) -> Vec<u8> {
    let mut packet = vec![11]; // INFORMATION-REQUEST only
    packet.extend_from_slice(&transaction.xid[..3]);
    push_v6(&mut packet, 1, &transaction.duid);
    push_v6(&mut packet, 6, &[0, 21, 0, 22, 0, 83, 0, 32]); // SIP, INF_MAX_RT, refresh time
    let centiseconds = elapsed.as_millis().saturating_div(10).min(u128::from(u16::MAX)) as u16;
    push_v6(&mut packet, 8, &centiseconds.to_be_bytes());
    packet
}

fn push_v6(packet: &mut Vec<u8>, code: u16, value: &[u8]) {
    packet.extend_from_slice(&code.to_be_bytes());
    packet.extend_from_slice(&(value.len() as u16).to_be_bytes());
    packet.extend_from_slice(value);
}

fn discovery() -> ImsPcscfDiscovery {
    ImsPcscfDiscovery { candidates: Vec::new(), domains: Vec::new(), context_id: None, source: "dhcp_information" }
}

fn complete(result: ImsPcscfDiscovery) -> DecodeResult<ImsPcscfDiscovery> {
    let count = result.candidates.len() + result.domains.len();
    if count == 0 || count > MAX_RESULTS { Err("dhcp_result_count") } else { Ok(result) }
}

/// RFC 3396 ordering is options, file, sname (not physical packet order).
/// Concatenate option 120 before interpreting its single encoding byte.
/// Mandatory identity/type/overload options must be singleton; duplicates are
/// rejected even if byte-identical rather than disguising ambiguous identities.
fn options_v4(packet: &[u8]) -> DecodeResult<Vec<Option<Vec<u8>>>> {
    let mut options = vec![None; 256];
    let mut count = 0;
    let mut parse = |bytes: &[u8], overloaded: bool, options: &mut Vec<Option<Vec<u8>>>| -> DecodeResult<()> {
        let mut pos = 0;
        while pos < bytes.len() {
            let code = bytes[pos] as usize;
            pos += 1;
            if code == 0 { continue; }
            if code == 255 {
                return if bytes[pos..].iter().all(|b| *b == 0) { Ok(()) } else { Err("dhcp_option_tail") };
            }
            count += 1;
            if count > MAX_OPTIONS { return Err("dhcp_option_limit"); }
            let len = usize::from(*bytes.get(pos).ok_or("dhcp_option_truncated")?);
            pos += 1;
            let value = bytes.get(pos..pos + len).ok_or("dhcp_option_truncated")?;
            pos += len;
            if (overloaded && code == 52) || (matches!(code, 52 | 53 | 54 | 61) && options[code].is_some()) {
                return Err("dhcp_duplicate_identity");
            }
            let joined = options[code].get_or_insert_with(Vec::new);
            if joined.len() + value.len() > MAX_WIRE { return Err("dhcp_option_limit"); }
            joined.extend_from_slice(value);
        }
        Err("dhcp_missing_end")
    };
    parse(&packet[240..], false, &mut options)?;
    let overload = match options[52].as_deref() {
        None => 0,
        Some([v]) if (1..=3).contains(v) => *v,
        _ => return Err("dhcp_overload_invalid"),
    };
    if overload & 1 != 0 { parse(&packet[108..236], true, &mut options)?; }
    if overload & 2 != 0 { parse(&packet[44..108], true, &mut options)?; }
    Ok(options)
}

fn decode_v4(packet: &[u8], transaction: &Transaction, local: Ipv4Addr, peer: SocketAddr) -> DecodeResult<ImsPcscfDiscovery> {
    let SocketAddr::V4(peer) = peer else { return Err("dhcp_server_family"); };
    if peer.port() != 67 || !suitable_address((*peer.ip()).into()) { return Err("dhcp_server_address"); }
    if packet.len() < 241 || packet.len() > MAX_WIRE { return Err("dhcp_packet_size"); }
    if packet[..4] != [2, 0, 0, 0] || packet[4..8] != transaction.xid
        || packet[10..12] != [0, 0]
        || (packet[12..16] != local.octets() && packet[12..16] != [0; 4])
        || packet[16..20] != [0; 4] || packet[24..44] != [0; 20]
        || packet[236..240] != COOKIE
    { return Err("dhcp_reply_correlation"); }
    let options = options_v4(packet)?;
    if options[53].as_deref() != Some(&[5][..])
        || options[61].as_deref() != Some(transaction.client_v4.as_slice())
        || options[54].as_deref() != Some(peer.ip().octets().as_slice())
    { return Err("dhcp_reply_identity"); }
    if [50, 51, 58, 59, 82].iter().any(|code| options[*code].is_some()) {
        return Err("dhcp_assignment_or_relay");
    }
    let sip = options[120].as_deref().ok_or("dhcp_sip_missing")?;
    let mut result = discovery();
    match sip.split_first() {
        Some((&0, names)) => result.domains = domain_list(names, true)?,
        Some((&1, addresses)) if !addresses.is_empty() && addresses.len() % 4 == 0 => {
            for bytes in addresses.chunks_exact(4) {
                add_address(&mut result, Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]).into())?;
            }
        }
        _ => return Err("dhcp_sip_encoding"),
    }
    complete(result)
}

fn decode_v6(packet: &[u8], transaction: &Transaction, peer: SocketAddr, destination: SocketAddr) -> DecodeResult<ImsPcscfDiscovery> {
    let SocketAddr::V6(peer) = peer else { return Err("dhcp_server_family"); };
    if peer.port() != 547 || !eligible_local((*peer.ip()).into()) { return Err("dhcp_server_address"); }
    if peer.ip().is_unicast_link_local() {
        let SocketAddr::V6(destination) = destination else { return Err("dhcp_server_scope"); };
        if peer.scope_id() == 0 || peer.scope_id() != destination.scope_id() { return Err("dhcp_server_scope"); }
    }
    if packet.len() < 4 || packet.len() > MAX_WIRE || packet[0] != 7 || packet[1..4] != transaction.xid[..3] {
        return Err("dhcp_reply_correlation");
    }
    let mut seen = Vec::new();
    let mut client = None;
    let mut server = None;
    let mut result = discovery();
    let mut pos = 4;
    while pos < packet.len() {
        let header = packet.get(pos..pos + 4).ok_or("dhcp_option_truncated")?;
        let code = u16::from_be_bytes([header[0], header[1]]);
        let len = usize::from(u16::from_be_bytes([header[2], header[3]]));
        pos += 4;
        let value = packet.get(pos..pos + len).ok_or("dhcp_option_truncated")?;
        pos += len;
        if seen.len() >= MAX_OPTIONS || seen.contains(&code) { return Err("dhcp_duplicate_option"); }
        seen.push(code);
        match code {
            1 => client = Some(value),
            2 => server = Some(value),
            3 | 4 | 5 | 9 | 18 | 25 | 26 => return Err("dhcp_assignment_or_relay"),
            13 => if value.len() < 2 || value[..2] != [0, 0] { return Err("dhcp_status_failure"); },
            21 => result.domains = domain_list(value, false)?,
            22 => {
                if value.is_empty() || value.len() % 16 != 0 { return Err("dhcp_sip_encoding"); }
                for bytes in value.chunks_exact(16) {
                    let octets: [u8; 16] = bytes.try_into().map_err(|_| "dhcp_sip_encoding")?;
                    add_address(&mut result, Ipv6Addr::from(octets).into())?;
                }
            }
            32 | 83 => if value.len() != 4 { return Err("dhcp_timer_length"); },
            _ => {} // unsolicited configuration never leaves this decoder
        }
    }
    let server = server.ok_or("dhcp_server_missing")?;
    if client != Some(transaction.duid.as_slice()) || server == transaction.duid.as_slice() || !valid_duid(server) {
        return Err("dhcp_reply_identity");
    }
    complete(result)
}

fn valid_duid(value: &[u8]) -> bool {
    if !(3..=130).contains(&value.len()) { return false; }
    match u16::from_be_bytes([value[0], value[1]]) {
        1 => value.len() >= 9 && value[2..4] != [0, 0],
        2 => value.len() >= 7,
        3 => value.len() >= 5 && value[2..4] != [0, 0],
        4 => value.len() == 18 && value[2..].iter().any(|b| *b != 0),
        _ => false, // unknown identity format is not guessed
    }
}

fn add_address(result: &mut ImsPcscfDiscovery, address: IpAddr) -> DecodeResult<()> {
    if !suitable_address(address) || result.candidates.contains(&address)
        || result.candidates.len() >= MAX_RESULTS
    { return Err("dhcp_sip_address_invalid"); }
    result.candidates.push(address);
    Ok(())
}

fn eligible_local(address: IpAddr) -> bool {
    suitable_address(address) || matches!(address, IpAddr::V6(ip) if ip.is_unicast_link_local())
}

fn suitable_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            let b = ip.octets();
            b[0] != 0 && b[0] != 127 && b[0] < 224 && !(b[0] == 169 && b[1] == 254)
        }
        IpAddr::V6(ip) => {
            let b = ip.octets();
            !ip.is_unspecified() && !ip.is_loopback() && !ip.is_multicast()
                && !ip.is_unicast_link_local() && !(b[0] == 0xfe && b[1] & 0xc0 == 0xc0)
                && b[..12] != [0; 12] && !(b[..10] == [0; 10] && b[10..12] == [255, 255])
        }
    }
}

/// RFC 3361 pointers are relative to the concatenated domain-list, excluding
/// the encoding octet. RFC 3319/RFC 8415 prohibit compression for DHCPv6.
/// Only earlier, known label boundaries are targets; no loops, forward jumps,
/// interior-label pointers, escaped/binary labels or resolver search names.
fn domain_list(bytes: &[u8], compression: bool) -> DecodeResult<Vec<String>> {
    if bytes.is_empty() || bytes.len() > MAX_WIRE { return Err("dhcp_domain_size"); }
    let mut names = Vec::new();
    let mut boundaries = vec![false; bytes.len()];
    let mut next = 0;
    while next < bytes.len() {
        if names.len() >= MAX_RESULTS { return Err("dhcp_domain_limit"); }
        let start = next;
        let mut pos = start;
        let mut resume = None;
        let mut labels = Vec::new();
        let mut expanded = 1usize; // root label
        let mut steps = 0;
        loop {
            steps += 1;
            if steps > 128 { return Err("dhcp_domain_cycle"); }
            let size = *bytes.get(pos).ok_or("dhcp_domain_truncated")?;
            if size & 0xc0 == 0xc0 {
                let low = *bytes.get(pos + 1).ok_or("dhcp_domain_truncated")?;
                let target = (usize::from(size & 0x3f) << 8) | usize::from(low);
                if !compression || target >= start || !boundaries.get(target).copied().unwrap_or(false) {
                    return Err("dhcp_domain_pointer");
                }
                boundaries[pos] = true;
                resume.get_or_insert(pos + 2);
                pos = target;
                continue;
            }
            if size & 0xc0 != 0 { return Err("dhcp_domain_label"); }
            boundaries[pos] = true;
            pos += 1;
            if size == 0 { break; }
            let label = bytes.get(pos..pos + usize::from(size)).ok_or("dhcp_domain_truncated")?;
            if !label[0].is_ascii_alphanumeric() || !label[label.len() - 1].is_ascii_alphanumeric()
                || !label.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-')
            { return Err("dhcp_domain_label"); }
            expanded += label.len() + 1;
            if expanded > 255 { return Err("dhcp_domain_size"); }
            labels.push(std::str::from_utf8(label).map_err(|_| "dhcp_domain_label")?.to_ascii_lowercase());
            pos += label.len();
        }
        next = resume.unwrap_or(pos);
        let name = labels.join(".");
        let tld = labels.last().ok_or("dhcp_domain_root")?;
        if labels.len() < 2 || tld.bytes().all(|b| b.is_ascii_digit())
            || matches!(tld.as_str(), "localhost" | "local" | "invalid") || names.contains(&name)
        { return Err("dhcp_domain_unsuitable"); }
        names.push(name);
    }
    Ok(names)
}

#[cfg(test)]
#[path = "dhcp_pcscf_tests.rs"]
mod tests;
