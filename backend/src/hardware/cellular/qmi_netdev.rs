//! Resolve which network device carries a QMI data session.
//!
//! # Why this is a probe and not a lookup
//!
//! A WDS session gives you an IP configuration and a packet data handle — but no
//! interface name. On a USB modem that does not matter: the `cdc-wdm` control
//! node has exactly one sibling `wwan` netdev, so the pairing is structural.
//!
//! On the bam-dmux target this project runs on, it is not. One baseband publishes
//! eight identical netdevs (`wwan0`…`wwan7`, all `POINTOPOINT,NOARP`, all under
//! the same `<addr>.remoteproc:bam-dmux` parent), and the firmware decides which
//! MUX channel a session lands on. Nothing in sysfs says which. The two QMI
//! commands that would tell us — `--wds-bind-data-port` and
//! `--wds-bind-mux-data-port` — are unsupported by the 2015 firmware here and
//! issuing either one restarts the baseband, so asking is not an option either.
//!
//! What is left is observation: configure a candidate, send a packet that the
//! network is obliged to answer, and see which interface counts the reply. That
//! is what this module does.
//!
//! # Cost of a wrong answer
//!
//! Silent breakage. SIP would bind to an address on an interface whose packets go
//! nowhere: REGISTER times out with no error from any layer, which looks exactly
//! like an unreachable P-CSCF. Verifying the interface here converts that into a
//! specific, reported failure.

use std::{
    io,
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::{
    net::UdpSocket,
    process::Command,
    time::{sleep, timeout},
};

use crate::platform::network_routing::{
    host_selector, network_address, route_table, rule_priority, source_selector, RouteDomain,
};
use tracing::{debug, info, warn};

/// How long to wait for a probe reply before moving to the next candidate.
const PROBE_REPLY_WAIT: Duration = Duration::from_millis(1200);
/// Settle time after configuring a link, before probing it.
const LINK_SETTLE: Duration = Duration::from_millis(300);
/// Port used for the throwaway DNS probe.
const DNS_PORT: u16 = 53;

/// Traffic counters for one interface, read from sysfs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkCounters {
    pub rx_packets: u64,
    pub tx_packets: u64,
}

impl LinkCounters {
    /// Did this interface receive anything between the two samples?
    ///
    /// Receive is the signal that matters. Transmit only proves the kernel handed
    /// the frame to the driver, which happens on every candidate regardless of
    /// whether the MUX channel is the right one — so a tx-based check would
    /// accept the first interface tried, every time.
    pub fn received_since(self, earlier: Self) -> bool {
        self.rx_packets > earlier.rx_packets
    }

    /// Packets received between the two samples, saturating on a counter reset.
    pub fn rx_delta(self, earlier: Self) -> u64 {
        self.rx_packets.saturating_sub(earlier.rx_packets)
    }
}

/// Outcome of resolving the data netdev for a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedNetdev {
    /// Interface that answered, e.g. `wwan3`.
    pub interface: String,
    /// Packets it received during the probe, kept for the attempt log.
    pub rx_packets: u64,
    /// How the interface was decided.
    pub method: ResolutionMethod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionMethod {
    /// Only one candidate existed, so no probe was needed.
    SoleCandidate,
    /// The interface answered a probe packet.
    ProbeAnswered,
    /// Nothing answered; a candidate was assumed. The caller should treat the
    /// resulting session as unverified.
    Assumed,
}

impl ResolutionMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SoleCandidate => "sole_candidate",
            Self::ProbeAnswered => "probe_answered",
            Self::Assumed => "assumed",
        }
    }

    /// Whether the answer was actually observed rather than guessed.
    #[cfg(test)]
    pub fn is_verified(self) -> bool {
        matches!(self, Self::SoleCandidate | Self::ProbeAnswered)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetdevError {
    /// No candidate interface belongs to this baseband.
    NoCandidates(String),
    /// Candidates exist but none could be configured with the session address.
    ConfigureFailed(String),
}

impl std::fmt::Display for NetdevError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCandidates(d) => write!(f, "qmi_netdev_no_candidates:{d}"),
            Self::ConfigureFailed(d) => write!(f, "qmi_netdev_configure_failed:{d}"),
        }
    }
}

impl std::error::Error for NetdevError {}

/// The address configuration a session needs on its interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetdevConfig {
    pub address: IpAddr,
    pub prefix: u8,
    pub mtu: Option<u32>,
    /// A target the network will answer: the session's DNS server, or its
    /// gateway. Used only to generate return traffic.
    pub probe_target: Option<IpAddr>,
}

impl NetdevConfig {
    /// Build the probe configuration from a session's settings.
    ///
    /// Prefers a DNS server as the probe target: a DNS query to it produces a
    /// real reply. A gateway is a weaker fallback — on a point-to-point raw-IP
    /// link it may not answer anything, in which case the probe is inconclusive
    /// and resolution falls back to `Assumed`.
    pub fn from_session(
        address: IpAddr,
        prefix: Option<u8>,
        mtu: Option<u32>,
        dns: &[IpAddr],
        gateway: Option<IpAddr>,
    ) -> Self {
        let same_family = |candidate: &IpAddr| candidate.is_ipv4() == address.is_ipv4();
        let probe_target = dns
            .iter()
            .copied()
            .find(same_family)
            .or_else(|| gateway.filter(same_family));
        Self {
            address,
            prefix: prefix.unwrap_or(if address.is_ipv6() { 64 } else { 32 }),
            mtu,
            probe_target,
        }
    }
}

/// Candidate netdevs for a baseband, in probe order.
///
/// Ordering is a hint only — every candidate is probed until one answers. Lower
/// interface numbers first, because the firmware allocates MUX channels from the
/// bottom and the first data session usually lands early.
pub fn candidates_for_baseband(baseband: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/sys/class/net") else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Ok(resolved) = std::fs::canonicalize(entry.path()) else {
            continue;
        };
        if !resolved.to_string_lossy().contains(baseband) {
            continue;
        }
        candidates.push(name);
    }
    candidates.sort_by_key(|name| (trailing_number(name), name.clone()));
    candidates
}

/// Drop the caller's reserved interfaces from a candidate list.
///
/// Filtering happens once, at the top of resolution, rather than at each
/// decision point. Resolution has three exits -- sole candidate, probe answered,
/// and assumed -- and the assumed exit is the one that actually misfired on this
/// target: with no probe reply it takes the lowest-numbered candidate, which is
/// `wwan0`. A filter per exit would have to be correct three times; a filter on
/// the input is correct once.
fn usable_candidates(candidates: Vec<String>, reserved: ReservedNetdevs<'_>) -> Vec<String> {
    candidates
        .into_iter()
        .filter(|name| !reserved.contains(&name.as_str()))
        .collect()
}

/// Numeric suffix of an interface name, for natural ordering (`wwan10` after
/// `wwan9`).
fn trailing_number(name: &str) -> u32 {
    let digits: String = name
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.parse().unwrap_or(u32::MAX)
}

/// Read an interface's packet counters.
pub fn read_counters(interface: &str) -> LinkCounters {
    let read = |field: &str| -> u64 {
        std::fs::read_to_string(format!("/sys/class/net/{interface}/statistics/{field}"))
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0)
    };
    LinkCounters {
        rx_packets: read("rx_packets"),
        tx_packets: read("tx_packets"),
    }
}

/// Interfaces a caller must never be given, even as a last-resort guess.
///
/// The MSM8916 firmware cannot keep IMS and Internet bearers alive through the
/// same data slot, so the two runtimes divide the netdevs between them. Handing
/// a caller an interface that belongs to the other one does not fail loudly: it
/// produces a session on a link whose packets belong to somebody else, which
/// looks like an unreachable peer several layers up.
pub type ReservedNetdevs<'a> = &'a [&'a str];

/// Find the interface carrying a session, and leave it configured.
///
/// Each candidate is configured, probed, and — if it does not answer — stripped
/// back down before the next is tried, so a failed probe leaves no stray address
/// behind to conflict with the interface that does work.
///
/// A single candidate is accepted without probing: there is nothing to
/// disambiguate, and spending a probe timeout on the USB/single-netdev case would
/// slow down every connection for no information.
///
/// `reserved` names interfaces this caller must not be given under any
/// circumstance — not as a probe candidate and not as the assumed fallback. The
/// fallback is the reason this parameter exists: when no candidate answers,
/// resolution picks the lowest-numbered one, which on this target is `wwan0`,
/// the port ModemManager holds for IMS. Filtering at the top means a caller can
/// never receive a reserved interface, however resolution ends up deciding.
pub async fn resolve(
    baseband: &str,
    config: &NetdevConfig,
    reserved: ReservedNetdevs<'_>,
) -> Result<ResolvedNetdev, NetdevError> {
    let candidates = usable_candidates(candidates_for_baseband(baseband), reserved);
    if candidates.is_empty() {
        return Err(NetdevError::NoCandidates(baseband.to_string()));
    }

    if let [only] = candidates.as_slice() {
        configure(only, config)
            .await
            .map_err(|error| NetdevError::ConfigureFailed(format!("{only}: {error}")))?;
        info!(interface = %only, "Data netdev resolved: sole candidate for this baseband");
        return Ok(ResolvedNetdev {
            interface: only.clone(),
            rx_packets: 0,
            method: ResolutionMethod::SoleCandidate,
        });
    }

    let mut configure_errors = Vec::new();
    for candidate in &candidates {
        if let Err(error) = configure(candidate, config).await {
            debug!(interface = %candidate, error = %error, "Candidate could not be configured");
            configure_errors.push(format!("{candidate}: {error}"));
            continue;
        }
        sleep(LINK_SETTLE).await;

        let before = read_counters(candidate);
        let socket_replied = send_probe(config, candidate).await;
        let after = read_counters(candidate);

        if probe_observed(socket_replied, before, after) {
            let rx_packets = after.rx_delta(before);
            info!(
                interface = %candidate,
                rx_packets,
                socket_replied,
                "Data netdev resolved: candidate answered the probe"
            );
            return Ok(ResolvedNetdev {
                interface: candidate.clone(),
                rx_packets,
                method: ResolutionMethod::ProbeAnswered,
            });
        }
        // Wrong channel (or a target that does not answer). Undo before the next
        // candidate so only one interface ever holds this address.
        deconfigure(candidate, config).await;
    }

    if configure_errors.len() == candidates.len() {
        return Err(NetdevError::ConfigureFailed(configure_errors.join("; ")));
    }

    // Nothing answered. This is expected when the session's network offers no
    // probe target that replies, so it is not fatal — but the caller must know
    // the interface is a guess, because a wrong guess makes SIP fail silently.
    let assumed = candidates
        .iter()
        .find(|candidate| {
            !configure_errors
                .iter()
                .any(|error| error.starts_with(&format!("{candidate}: ")))
        })
        .cloned()
        .unwrap_or_else(|| candidates[0].clone());
    if configure(&assumed, config).await.is_err() {
        return Err(NetdevError::ConfigureFailed(format!(
            "no candidate answered and {assumed} could not be reconfigured"
        )));
    }
    warn!(
        interface = %assumed,
        probe_target = ?config.probe_target,
        "No data netdev answered the probe; assuming a candidate — the session is unverified"
    );
    Ok(ResolvedNetdev {
        interface: assumed,
        rx_packets: 0,
        method: ResolutionMethod::Assumed,
    })
}

/// Bring an interface up with the session's address, MTU and probe route.
async fn configure(interface: &str, config: &NetdevConfig) -> Result<(), String> {
    if let Err(error) = run_ip(&["link", "set", "dev", interface, "up"]).await {
        debug!(
            interface,
            error = %error,
            "Data bearer netdev did not accept an administrative UP request; continuing"
        );
    }
    if let Some(mtu) = config.mtu {
        // A rejected MTU is not fatal; the link still carries traffic at its
        // default, and failing here would discard an otherwise working candidate.
        let mtu = mtu.to_string();
        if let Err(error) = run_ip(&["link", "set", "dev", interface, "mtu", &mtu]).await {
            debug!(interface, error = %error, "Setting the MTU failed; keeping the default");
        }
    }
    let address = format!("{}/{}", config.address, config.prefix);
    let family_arg: &[&str] = if config.address.is_ipv6() {
        &["-6"]
    } else {
        &[]
    };
    let mut args = family_arg.to_vec();
    args.extend_from_slice(&["address", "replace", &address, "dev", interface]);
    run_ip(&args).await?;

    // Keep bearer traffic out of the host's main routing table. A source rule
    // gives probes (and, for a data bearer, the proxy) a private table without
    // replacing Wi-Fi/Ethernet defaults or another PDP context's DNS routes.
    let table = route_table(RouteDomain::ModemData, interface, config.address).to_string();
    let priority = rule_priority(RouteDomain::ModemData, interface, config.address).to_string();
    let source = source_selector(config.address);
    let mut delete_rule = family_arg.to_vec();
    delete_rule.extend_from_slice(&["rule", "del", "priority", &priority]);
    let _ = run_ip(&delete_rule).await;
    let mut add_rule = family_arg.to_vec();
    add_rule.extend_from_slice(&[
        "rule", "add", "priority", &priority, "from", &source, "table", &table,
    ]);
    run_ip(&add_rule).await?;

    // Keep the connected network in the private table. The interface address
    // itself is a host address and cannot be used as an IPv4 /30 route target.
    let connected = format!(
        "{}/{}",
        network_address(config.address, config.prefix),
        config.prefix
    );
    let mut connected_route = family_arg.to_vec();
    connected_route.extend_from_slice(&[
        "route", "replace", &connected, "dev", interface, "table", &table,
    ]);
    run_ip(&connected_route).await?;

    if let Some(target) = config.probe_target {
        let destination = host_selector(target);
        let mut args = family_arg.to_vec();
        args.extend_from_slice(&[
            "route",
            "replace",
            &destination,
            "dev",
            interface,
            "table",
            &table,
        ]);
        // A route that will not install means this candidate cannot carry the
        // probe; report it so the candidate is skipped rather than silently
        // probed with no path.
        run_ip(&args).await?;
    }
    Ok(())
}

/// Restore a session interface in the host namespace after an attempted UE
/// namespace migration. This is the public counterpart of the resolver's
/// private candidate setup and includes the proxy default route.
pub async fn configure_host_data_path(
    interface: &str,
    config: &NetdevConfig,
) -> Result<(), String> {
    configure(interface, config).await?;
    install_default_route(interface, config).await
}

/// Remove what `configure` added, so a rejected candidate holds no state.
async fn deconfigure(interface: &str, config: &NetdevConfig) {
    let family_arg: &[&str] = if config.address.is_ipv6() {
        &["-6"]
    } else {
        &[]
    };
    let table = route_table(RouteDomain::ModemData, interface, config.address).to_string();
    let priority = rule_priority(RouteDomain::ModemData, interface, config.address).to_string();
    let mut args = family_arg.to_vec();
    args.extend_from_slice(&["route", "flush", "table", &table]);
    let _ = run_ip(&args).await;
    let mut args = family_arg.to_vec();
    args.extend_from_slice(&["rule", "del", "priority", &priority]);
    let _ = run_ip(&args).await;
    let address = format!("{}/{}", config.address, config.prefix);
    let mut args = family_arg.to_vec();
    args.extend_from_slice(&["address", "del", &address, "dev", interface]);
    let _ = run_ip(&args).await;
}

/// Install the private default route used by a user-data proxy after the
/// correct bam-dmux netdev has been observed. IMS callers do not call this and
/// therefore remain limited to their explicit P-CSCF routes.
pub async fn install_default_route(interface: &str, config: &NetdevConfig) -> Result<(), String> {
    let family_arg: &[&str] = if config.address.is_ipv6() {
        &["-6"]
    } else {
        &[]
    };
    let table = route_table(RouteDomain::ModemData, interface, config.address).to_string();
    let mut args = family_arg.to_vec();
    args.extend_from_slice(&[
        "route", "replace", "default", "dev", interface, "table", &table,
    ]);
    run_ip(&args).await
}

/// Remove only the address and policy routes created for this session. Never
/// flush an entire netdev: another bearer (notably IMS) may share the same
/// bam-dmux parent and must keep its own address and routes.
pub async fn teardown(interface: &str, config: &NetdevConfig) {
    deconfigure(interface, config).await;
}

/// Send one packet the network should answer, from the session address.
///
/// A DNS query is used because it is a plain UDP datagram that needs no
/// privileges (unlike ICMP) and gets a reply from any resolver. The reply content
/// is irrelevant — only that *some* bytes come back on the source-bound socket
/// (or, as a fallback, move the interface RX counter). Failures are ignored: an
/// unreachable target simply means this candidate does not answer.
async fn send_probe(config: &NetdevConfig, interface: &str) -> bool {
    let Some(target) = config.probe_target else {
        return false;
    };
    let bind = SocketAddr::new(config.address, 0);
    let Ok(socket) = bind_probe_socket(bind, interface) else {
        debug!(?bind, "Probe socket could not bind to the session address");
        return false;
    };
    // Minimal DNS query for `.` NS — smallest well-formed request that draws a
    // reply.
    let query: [u8; 17] = [
        0x5a, 0x17, // transaction id
        0x01, 0x00, // standard query, recursion desired
        0x00, 0x01, // one question
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // no answer/authority/additional
        0x00, // root name
        0x00, 0x02, // NS
        0x00, 0x01, // IN
    ];
    if socket
        .send_to(&query, SocketAddr::new(target, DNS_PORT))
        .await
        .is_err()
    {
        return false;
    }

    // Some bam-dmux kernels deliver packets correctly but never update the
    // per-netdev RX/TX counters. A reply on this source-bound socket is the
    // authoritative signal; counters remain a fallback for unusual responders.
    let mut response = [0u8; 2048];
    matches!(
        timeout(PROBE_REPLY_WAIT, socket.recv_from(&mut response)).await,
        Ok(Ok((length, _))) if length > 0
    )
}

fn bind_probe_socket(local: SocketAddr, interface: &str) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::for_address(local), Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    bind_probe_socket_to_device(&socket, interface)?;
    socket.bind(&local.into())?;
    socket.set_nonblocking(true)?;
    UdpSocket::from_std(socket.into())
}

#[cfg(target_os = "linux")]
fn bind_probe_socket_to_device(socket: &Socket, interface: &str) -> io::Result<()> {
    use std::{ffi::CString, os::fd::AsRawFd};

    let name = CString::new(interface)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "interface contains NUL"))?;
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            name.as_ptr().cast(),
            name.as_bytes_with_nul().len() as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn bind_probe_socket_to_device(_socket: &Socket, _interface: &str) -> io::Result<()> {
    Ok(())
}

fn probe_observed(socket_replied: bool, before: LinkCounters, after: LinkCounters) -> bool {
    socket_replied || after.received_since(before)
}

async fn run_ip(args: &[&str]) -> Result<(), String> {
    let output = Command::new("ip")
        .args(args)
        .output()
        .await
        .map_err(|error| format!("spawn ip: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&output.stderr)
        .trim()
        .replace('\n', " "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn only_receive_counts_as_an_answer() {
        let before = LinkCounters {
            rx_packets: 10,
            tx_packets: 4,
        };
        // Transmit alone must not count: the kernel increments tx on every
        // candidate, so accepting it would pick whichever was tried first.
        let tx_only = LinkCounters {
            rx_packets: 10,
            tx_packets: 9,
        };
        assert!(!tx_only.received_since(before));
        let rx_moved = LinkCounters {
            rx_packets: 12,
            tx_packets: 9,
        };
        assert!(rx_moved.received_since(before));
        assert_eq!(rx_moved.rx_delta(before), 2);
    }

    #[test]
    fn socket_reply_verifies_a_link_with_static_driver_counters() {
        let unchanged = LinkCounters {
            rx_packets: 0,
            tx_packets: 0,
        };
        assert!(probe_observed(true, unchanged, unchanged));
        assert!(!probe_observed(false, unchanged, unchanged));
    }

    #[test]
    fn counter_reset_does_not_underflow() {
        let before = LinkCounters {
            rx_packets: 500,
            tx_packets: 0,
        };
        let after = LinkCounters {
            rx_packets: 3,
            tx_packets: 0,
        };
        assert!(!after.received_since(before));
        assert_eq!(after.rx_delta(before), 0);
    }

    #[test]
    fn dns_is_preferred_over_gateway_as_a_probe_target() {
        let address = IpAddr::V4(Ipv4Addr::new(10, 129, 39, 207));
        let dns = vec![
            IpAddr::V4(Ipv4Addr::new(172, 17, 163, 218)),
            IpAddr::V4(Ipv4Addr::new(172, 17, 167, 218)),
        ];
        let gateway = Some(IpAddr::V4(Ipv4Addr::new(10, 129, 39, 208)));
        let config = NetdevConfig::from_session(address, Some(27), Some(1500), &dns, gateway);
        assert_eq!(config.probe_target, Some(dns[0]));
        assert_eq!(config.prefix, 27);
        assert_eq!(config.mtu, Some(1500));
    }

    #[test]
    fn gateway_is_the_fallback_probe_target() {
        let address = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        let gateway = Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        let config = NetdevConfig::from_session(address, None, None, &[], gateway);
        assert_eq!(config.probe_target, gateway);
        // No prefix reported: a raw-IP v4 session is a /32 host address.
        assert_eq!(config.prefix, 32);
    }

    #[test]
    fn probe_target_never_crosses_family() {
        // A v6 session with only v4 resolvers has nothing to probe with; mixing
        // families would bind a v6 socket to a v4 destination and fail anyway.
        let address = IpAddr::V6("2001:db8::20".parse::<Ipv6Addr>().unwrap());
        let v4_dns = vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 53))];
        let v4_gateway = Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        let config = NetdevConfig::from_session(address, None, None, &v4_dns, v4_gateway);
        assert_eq!(config.probe_target, None);
        assert_eq!(config.prefix, 64, "v6 sessions default to a /64");

        let v6_dns = vec![IpAddr::V6("2001:db8::53".parse::<Ipv6Addr>().unwrap())];
        let config = NetdevConfig::from_session(address, None, None, &v6_dns, None);
        assert_eq!(config.probe_target, Some(v6_dns[0]));
    }

    #[test]
    fn candidates_sort_numerically_not_lexically() {
        let mut names = vec![
            "wwan10".to_string(),
            "wwan2".to_string(),
            "wwan1".to_string(),
        ];
        names.sort_by_key(|name| (trailing_number(name), name.clone()));
        assert_eq!(names, vec!["wwan1", "wwan2", "wwan10"]);
        // A name with no numeric suffix sorts last rather than panicking.
        assert_eq!(trailing_number("eth"), u32::MAX);
        assert_eq!(trailing_number("wwan0"), 0);
    }

    #[test]
    fn data_policy_tables_are_stable_and_family_separated() {
        let v4 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        let v6 = IpAddr::V6("2001:db8::2".parse().unwrap());
        assert_eq!(route_table(RouteDomain::ModemData, "wwan0", v4), 12_000);
        assert_eq!(route_table(RouteDomain::ModemData, "wwan0", v6), 12_001);
        assert_eq!(route_table(RouteDomain::ModemData, "wwan7", v4), 12_014);
        assert_eq!(source_selector(v4), "10.0.0.2/32");
        assert_eq!(source_selector(v6), "2001:db8::2/128");
    }

    #[test]
    fn resolution_method_reports_whether_the_answer_was_observed() {
        assert!(ResolutionMethod::ProbeAnswered.is_verified());
        assert!(ResolutionMethod::SoleCandidate.is_verified());
        // The whole point of tracking this: an assumed interface may be wrong,
        // and a wrong interface makes SIP time out with no visible cause.
        assert!(!ResolutionMethod::Assumed.is_verified());
        assert_eq!(ResolutionMethod::ProbeAnswered.as_str(), "probe_answered");
        assert_eq!(ResolutionMethod::Assumed.as_str(), "assumed");
        assert_eq!(ResolutionMethod::SoleCandidate.as_str(), "sole_candidate");
    }

    #[test]
    fn reserved_netdevs_are_removed_from_resolution() {
        let all = vec![
            "wwan0".to_string(),
            "wwan2".to_string(),
            "wwan3".to_string(),
        ];
        assert_eq!(
            usable_candidates(all.clone(), &["wwan0"]),
            vec!["wwan2", "wwan3"]
        );
        // Nothing reserved leaves the list untouched: the IMS caller owns the
        // primary netdev and must still be able to resolve onto it.
        assert_eq!(usable_candidates(all.clone(), &[]), all);
    }

    #[test]
    fn a_reserved_netdev_can_never_become_the_assumed_fallback() {
        // The actual failure this guards. No candidate answered the probe, so
        // resolution falls back to the lowest-numbered one -- wwan0, the netdev
        // ModemManager holds for IMS. DATA6 taking it stops the IMS PDN from
        // establishing, and the VoLTE REGISTER then leaves over Wi-Fi toward a
        // carrier-private P-CSCF that cannot answer. Filtering the input means
        // the fallback has no way to select it.
        let candidates = usable_candidates(
            vec!["wwan0".to_string(), "wwan2".to_string()],
            &["wwan0"],
        );
        let assumed = candidates.first().cloned();
        assert_eq!(assumed.as_deref(), Some("wwan2"));
    }

    #[test]
    fn reserving_every_candidate_reports_no_candidates_rather_than_guessing() {
        // Better a named failure than a session on somebody else's link: a wrong
        // interface produces no error at any layer, just a peer that never replies.
        assert!(usable_candidates(vec!["wwan0".to_string()], &["wwan0"]).is_empty());
    }

    #[test]
    fn error_codes_are_stable_and_prefixed() {
        assert_eq!(
            NetdevError::NoCandidates("4080000.remoteproc".into()).to_string(),
            "qmi_netdev_no_candidates:4080000.remoteproc"
        );
        assert_eq!(
            NetdevError::ConfigureFailed("wwan0: EINVAL".into()).to_string(),
            "qmi_netdev_configure_failed:wwan0: EINVAL"
        );
    }

    #[test]
    fn reference_device_session_yields_a_usable_probe_config() {
        // The verified session from the reference device (Maxis, DATA6 endpoint).
        let config = NetdevConfig::from_session(
            IpAddr::V4(Ipv4Addr::new(10, 129, 39, 207)),
            Some(27),
            Some(1500),
            &[IpAddr::V4(Ipv4Addr::new(172, 17, 163, 218))],
            Some(IpAddr::V4(Ipv4Addr::new(10, 129, 39, 208))),
        );
        assert!(
            config.probe_target.is_some(),
            "must have something to probe"
        );
        assert_eq!(config.address.to_string(), "10.129.39.207");
    }
}
