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
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use tokio::{net::UdpSocket, process::Command, time::sleep};
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

/// Find the interface carrying a session, and leave it configured.
///
/// Each candidate is configured, probed, and — if it does not answer — stripped
/// back down before the next is tried, so a failed probe leaves no stray address
/// behind to conflict with the interface that does work.
///
/// A single candidate is accepted without probing: there is nothing to
/// disambiguate, and spending a probe timeout on the USB/single-netdev case would
/// slow down every connection for no information.
pub async fn resolve(baseband: &str, config: &NetdevConfig) -> Result<ResolvedNetdev, NetdevError> {
    let candidates = candidates_for_baseband(baseband);
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
        send_probe(config).await;
        sleep(PROBE_REPLY_WAIT).await;
        let after = read_counters(candidate);

        if after.received_since(before) {
            let rx_packets = after.rx_delta(before);
            info!(
                interface = %candidate,
                rx_packets,
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
    run_ip(&["link", "set", "dev", interface, "up"]).await?;
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

    if let Some(target) = config.probe_target {
        let destination = format!("{}/{}", target, if target.is_ipv6() { 128 } else { 32 });
        let mut args = family_arg.to_vec();
        args.extend_from_slice(&["route", "replace", &destination, "dev", interface]);
        // A route that will not install means this candidate cannot carry the
        // probe; report it so the candidate is skipped rather than silently
        // probed with no path.
        run_ip(&args).await?;
    }
    Ok(())
}

/// Remove what `configure` added, so a rejected candidate holds no state.
async fn deconfigure(interface: &str, config: &NetdevConfig) {
    let family_arg: &[&str] = if config.address.is_ipv6() {
        &["-6"]
    } else {
        &[]
    };
    let mut args = family_arg.to_vec();
    args.extend_from_slice(&["route", "flush", "dev", interface]);
    let _ = run_ip(&args).await;
    let mut args = family_arg.to_vec();
    args.extend_from_slice(&["address", "flush", "dev", interface]);
    let _ = run_ip(&args).await;
    let _ = run_ip(&["link", "set", "dev", interface, "down"]).await;
}

/// Send one packet the network should answer, from the session address.
///
/// A DNS query is used because it is a plain UDP datagram that needs no
/// privileges (unlike ICMP) and gets a reply from any resolver. The reply content
/// is irrelevant — only that *some* bytes come back on the interface, which the
/// counters report. Failures are ignored: an unreachable target simply means this
/// candidate does not answer, which is the information being gathered.
async fn send_probe(config: &NetdevConfig) {
    let Some(target) = config.probe_target else {
        return;
    };
    let bind = SocketAddr::new(config.address, 0);
    let Ok(socket) = UdpSocket::bind(bind).await else {
        debug!(?bind, "Probe socket could not bind to the session address");
        return;
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
    let _ = socket
        .send_to(&query, SocketAddr::new(target, DNS_PORT))
        .await;
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
