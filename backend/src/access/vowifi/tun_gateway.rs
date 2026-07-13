#![allow(dead_code)]

use std::{
    fmt,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::Instant,
};

use super::{ike_keys::ChildSaSecretPair, transport::UdpSocketDatagramTransport};

const IMS_ESP_CLIENT_FLOW: &str = "client_flow";
const IMS_ESP_SERVER_FLOW: &str = "server_flow";

#[derive(Clone)]
pub(crate) struct TunGatewayConfig {
    pub profile_id: &'static str,
    pub tun_name: String,
    pub inner_addr: IpAddr,
    pub inner_prefix_len: Option<u8>,
    pub pcscf_addr: IpAddr,
    pub pcscf_addrs: Vec<IpAddr>,
    pub inbound_sa_identifier: u32,
    pub outbound_sa_identifier: u32,
    pub secrets: ChildSaSecretPair,
    pub transport: UdpSocketDatagramTransport,
    pub remote: SocketAddr,
}

pub(crate) struct TunGatewayRuntime {
    profile_id: &'static str,
    tun_name: String,
    inner_addr: IpAddr,
    pcscf_addr: IpAddr,
    pcscf_addrs: Vec<IpAddr>,
    started_at: Instant,
    ims_esp_policy: Arc<StdMutex<Option<ImsEspRuntimePolicy>>>,
    shutdown: Arc<AtomicBool>,
    #[cfg(target_os = "linux")]
    _tun_file: std::fs::File,
}

impl Drop for TunGatewayRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl TunGatewayRuntime {
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        platform_shutdown_tun(&self.tun_name);
    }

    pub fn is_for_profile(&self, profile_id: &str) -> bool {
        self.profile_id == profile_id
    }

    pub fn tun_name(&self) -> &str {
        &self.tun_name
    }

    pub fn inner_addr(&self) -> IpAddr {
        self.inner_addr
    }

    pub fn pcscf_addr(&self) -> IpAddr {
        self.pcscf_addr
    }

    pub fn pcscf_addrs(&self) -> &[IpAddr] {
        &self.pcscf_addrs
    }

    pub fn age_ms(&self) -> u128 {
        self.started_at.elapsed().as_millis()
    }

    pub(crate) fn install_ims_esp_policy(
        &self,
        config: ImsEspPolicyConfig,
    ) -> Result<(), TunGatewayError> {
        if config.local_addr.is_ipv4() != config.remote_addr.is_ipv4() {
            return Err(tun_error("ims_esp_policy_address_family_mismatch"));
        }
        if config.local_port_c == 0
            || config.local_port_s == 0
            || config.remote_port_c == 0
            || config.remote_port_s == 0
            || config.client_flow.local_port == 0
            || config.client_flow.remote_port == 0
            || config.server_flow.local_port == 0
            || config.server_flow.remote_port == 0
        {
            return Err(tun_error("ims_esp_policy_port_invalid"));
        }
        if config.client_flow.outbound_sa_identifier == 0
            || config.client_flow.inbound_sa_identifier == 0
            || config.server_flow.outbound_sa_identifier == 0
            || config.server_flow.inbound_sa_identifier == 0
        {
            return Err(tun_error("ims_esp_policy_spi_invalid"));
        }
        let mut guard = self
            .ims_esp_policy
            .lock()
            .map_err(|_| tun_error("ims_esp_policy_lock_failed"))?;
        let local_port_c = config.local_port_c;
        let local_port_s = config.local_port_s;
        let remote_port_c = config.remote_port_c;
        let remote_port_s = config.remote_port_s;
        *guard = Some(ImsEspRuntimePolicy::new(config));
        tracing::info!(
            profile_id = self.profile_id,
            ip_family = ip_family_name(self.inner_addr),
            local_port_c = local_port_c,
            local_port_s = local_port_s,
            remote_port_c = remote_port_c,
            remote_port_s = remote_port_s,
            "IMS ipsec-3gpp userspace policy installed with client and server flows"
        );
        Ok(())
    }

    pub(crate) fn ims_client_tcp_route(&self) -> Result<ImsClientTcpRoute, TunGatewayError> {
        let guard = self
            .ims_esp_policy
            .lock()
            .map_err(|_| tun_error("ims_esp_policy_lock_failed"))?;
        let Some(policy) = guard.as_ref() else {
            return Err(tun_error("ims_esp_policy_missing"));
        };
        policy.client_tcp_route()
    }
}

#[derive(Clone)]
pub(crate) struct ImsEspPolicyConfig {
    pub profile_id: &'static str,
    pub local_addr: IpAddr,
    pub remote_addr: IpAddr,
    pub local_port_c: u16,
    pub local_port_s: u16,
    pub remote_port_c: u16,
    pub remote_port_s: u16,
    pub client_flow: ImsEspFlowConfig,
    pub server_flow: ImsEspFlowConfig,
}

#[derive(Clone)]
pub(crate) struct ImsEspFlowConfig {
    pub label: &'static str,
    pub local_port: u16,
    pub remote_port: u16,
    pub outbound_sa_identifier: u32,
    pub inbound_sa_identifier: u32,
    pub secrets: ChildSaSecretPair,
}

#[derive(Clone)]
struct ImsEspRuntimePolicy {
    profile_id: &'static str,
    local_addr: IpAddr,
    remote_addr: IpAddr,
    local_port_c: u16,
    local_port_s: u16,
    remote_port_c: u16,
    remote_port_s: u16,
    flows: [ImsEspRuntimeFlow; 2],
}

#[derive(Clone)]
struct ImsEspRuntimeFlow {
    label: &'static str,
    local_port: u16,
    remote_port: u16,
    outbound_sa_identifier: u32,
    inbound_sa_identifier: u32,
    secrets: ChildSaSecretPair,
    next_outbound_sequence: u64,
    inbound_replay: super::dataplane::AntiReplayWindow,
    outbound_logged: bool,
    inbound_logged: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImsClientTcpRoute {
    pub profile_id: &'static str,
    pub local_addr: IpAddr,
    pub remote_addr: IpAddr,
    pub local_port: u16,
    pub remote_port: u16,
}

impl ImsEspRuntimePolicy {
    fn new(config: ImsEspPolicyConfig) -> Self {
        Self {
            profile_id: config.profile_id,
            local_addr: config.local_addr,
            remote_addr: config.remote_addr,
            local_port_c: config.local_port_c,
            local_port_s: config.local_port_s,
            remote_port_c: config.remote_port_c,
            remote_port_s: config.remote_port_s,
            flows: [
                ImsEspRuntimeFlow::new(config.client_flow),
                ImsEspRuntimeFlow::new(config.server_flow),
            ],
        }
    }

    fn client_tcp_route(&self) -> Result<ImsClientTcpRoute, TunGatewayError> {
        let Some(flow) = self
            .flows
            .iter()
            .find(|flow| flow.label == IMS_ESP_CLIENT_FLOW)
        else {
            return Err(tun_error("ims_esp_client_flow_missing"));
        };
        Ok(ImsClientTcpRoute {
            profile_id: self.profile_id,
            local_addr: self.local_addr,
            remote_addr: self.remote_addr,
            local_port: flow.local_port,
            remote_port: flow.remote_port,
        })
    }
}

impl ImsEspRuntimeFlow {
    fn new(config: ImsEspFlowConfig) -> Self {
        Self {
            label: config.label,
            local_port: config.local_port,
            remote_port: config.remote_port,
            outbound_sa_identifier: config.outbound_sa_identifier,
            inbound_sa_identifier: config.inbound_sa_identifier,
            secrets: config.secrets,
            next_outbound_sequence: 1,
            inbound_replay: super::dataplane::AntiReplayWindow::new(64),
            outbound_logged: false,
            inbound_logged: false,
        }
    }

    fn allocate_outbound_sequence(&mut self) -> Result<u64, TunGatewayError> {
        let sequence = self.next_outbound_sequence;
        if sequence > u64::from(u32::MAX) {
            return Err(tun_error("ims_esp_sequence_exhausted"));
        }
        self.next_outbound_sequence = self.next_outbound_sequence.saturating_add(1);
        Ok(sequence)
    }
}

fn ip_family_name(addr: IpAddr) -> &'static str {
    match addr {
        IpAddr::V4(_) => "ipv4",
        IpAddr::V6(_) => "ipv6",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TunGatewayError {
    reason: &'static str,
}

impl TunGatewayError {
    pub fn reason(&self) -> &'static str {
        self.reason
    }
}

impl fmt::Display for TunGatewayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.reason)
    }
}

impl std::error::Error for TunGatewayError {}

fn tun_error(reason: &'static str) -> TunGatewayError {
    TunGatewayError { reason }
}

#[cfg(target_os = "linux")]
fn platform_shutdown_tun(tun_name: &str) {
    imp::shutdown_tun(tun_name);
}

#[cfg(not(target_os = "linux"))]
fn platform_shutdown_tun(_tun_name: &str) {}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::{
        fs::{File, OpenOptions},
        io::{ErrorKind, Read, Write},
        net::{Ipv4Addr, Ipv6Addr},
        os::fd::AsRawFd,
        process::Command,
    };

    use tokio::sync::mpsc;
    use tracing::{debug, info, warn};

    use crate::access::vowifi::dataplane::{
        protect_inner_packet_for_esp, unprotect_inner_packet_from_esp, AntiReplayWindow,
    };

    #[cfg(target_env = "musl")]
    const TUNSETIFF: libc::c_int = 0x4004_54ca;
    #[cfg(not(target_env = "musl"))]
    const TUNSETIFF: libc::c_ulong = 0x4004_54ca;
    const IFF_TUN: i16 = 0x0001;
    const IFF_NO_PI: i16 = 0x1000;
    const IFREQ_BYTES: usize = 40;
    const IFNAMSIZ: usize = 16;
    const DEFAULT_TUN_MTU: u16 = 1360;

    pub(crate) async fn start_gateway(
        config: TunGatewayConfig,
    ) -> Result<Arc<TunGatewayRuntime>, TunGatewayError> {
        if config.inbound_sa_identifier == 0 || config.outbound_sa_identifier == 0 {
            return Err(tun_error("tun_gateway_child_sa_identifier_invalid"));
        }
        if config.inner_addr.is_ipv4() != config.pcscf_addr.is_ipv4() {
            return Err(tun_error("tun_gateway_inner_pcscf_family_mismatch"));
        }
        if config
            .pcscf_addrs
            .iter()
            .any(|addr| addr.is_ipv4() != config.inner_addr.is_ipv4())
        {
            return Err(tun_error("tun_gateway_inner_pcscf_family_mismatch"));
        }

        let tun_file = open_tun(&config.tun_name)?;
        configure_tun(&config)?;
        let read_file = tun_file
            .try_clone()
            .map_err(|_| tun_error("tun_gateway_clone_failed"))?;
        let write_file = tun_file
            .try_clone()
            .map_err(|_| tun_error("tun_gateway_clone_failed"))?;

        let ims_esp_policy = Arc::new(StdMutex::new(None));
        let shutdown = Arc::new(AtomicBool::new(false));
        spawn_forwarders(
            &config,
            read_file,
            write_file,
            Arc::clone(&ims_esp_policy),
            Arc::clone(&shutdown),
        );

        info!(
            tun_name = %config.tun_name,
            inner_family = ip_family(config.inner_addr),
            pcscf_family = ip_family(config.pcscf_addr),
            "VoWiFi outer ESP TUN gateway started"
        );

        Ok(Arc::new(TunGatewayRuntime {
            profile_id: config.profile_id,
            tun_name: config.tun_name,
            inner_addr: config.inner_addr,
            pcscf_addr: config.pcscf_addr,
            pcscf_addrs: config.pcscf_addrs,
            started_at: Instant::now(),
            ims_esp_policy,
            shutdown,
            _tun_file: tun_file,
        }))
    }

    pub(crate) fn shutdown_tun(tun_name: &str) {
        if tun_name.is_empty()
            || tun_name.len() >= IFNAMSIZ
            || !tun_name.bytes().all(valid_ifname_byte)
        {
            return;
        }
        let _ = Command::new("ip")
            .args(["link", "set", "dev", tun_name, "down"])
            .output();
        let _ = Command::new("ifconfig").args([tun_name, "down"]).output();
    }

    fn open_tun(name: &str) -> Result<File, TunGatewayError> {
        if name.is_empty() || name.len() >= IFNAMSIZ || !name.bytes().all(valid_ifname_byte) {
            return Err(tun_error("tun_gateway_invalid_name"));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/net/tun")
            .map_err(|_| tun_error("tun_gateway_open_failed"))?;

        let mut ifreq = [0u8; IFREQ_BYTES];
        ifreq[..name.len()].copy_from_slice(name.as_bytes());
        let flags = (IFF_TUN | IFF_NO_PI).to_ne_bytes();
        ifreq[IFNAMSIZ..IFNAMSIZ + flags.len()].copy_from_slice(&flags);

        let rc = unsafe { libc::ioctl(file.as_raw_fd(), TUNSETIFF, ifreq.as_mut_ptr()) };
        if rc < 0 {
            return Err(tun_error("tun_gateway_ioctl_failed"));
        }
        Ok(file)
    }

    fn configure_tun(config: &TunGatewayConfig) -> Result<(), TunGatewayError> {
        run_command(
            &["ifconfig", "/sbin/ifconfig", "/usr/sbin/ifconfig"],
            &[&config.tun_name, "mtu", &DEFAULT_TUN_MTU.to_string(), "up"],
            "tun_gateway_ifconfig_mtu_failed",
            false,
        )?;

        match config.inner_addr {
            IpAddr::V6(addr) => {
                let prefix = config.inner_prefix_len.unwrap_or(128).clamp(1, 128);
                let cidr = format!("{addr}/{prefix}");
                run_command(
                    &["ifconfig", "/sbin/ifconfig", "/usr/sbin/ifconfig"],
                    &[&config.tun_name, "inet6", "add", &cidr, "up"],
                    "tun_gateway_ifconfig_address_failed",
                    true,
                )?;
                for pcscf_addr in route_targets(config) {
                    let route_target = format!("{pcscf_addr}/128");
                    run_command(
                        &["route", "/sbin/route", "/usr/sbin/route"],
                        &["-A", "inet6", "add", &route_target, "dev", &config.tun_name],
                        "tun_gateway_route_failed",
                        true,
                    )?;
                }
            }
            IpAddr::V4(addr) => {
                let addr_text = addr.to_string();
                run_command(
                    &["ifconfig", "/sbin/ifconfig", "/usr/sbin/ifconfig"],
                    &[
                        &config.tun_name,
                        &addr_text,
                        "netmask",
                        "255.255.255.255",
                        "up",
                    ],
                    "tun_gateway_ifconfig_address_failed",
                    true,
                )?;
                for pcscf_addr in route_targets(config) {
                    let route_target = pcscf_addr.to_string();
                    run_command(
                        &["route", "/sbin/route", "/usr/sbin/route"],
                        &["add", "-host", &route_target, "dev", &config.tun_name],
                        "tun_gateway_route_failed",
                        true,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn route_targets(config: &TunGatewayConfig) -> Vec<IpAddr> {
        let mut targets = Vec::new();
        targets.push(config.pcscf_addr);
        targets.extend(config.pcscf_addrs.iter().copied());
        targets.sort();
        targets.dedup();
        targets
    }

    fn run_command(
        candidates: &[&str],
        args: &[&str],
        reason: &'static str,
        allow_existing: bool,
    ) -> Result<(), TunGatewayError> {
        for command in candidates {
            let Ok(output) = Command::new(command).args(args).output() else {
                continue;
            };
            if output.status.success() {
                return Ok(());
            }
            let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
            if allow_existing
                && (stderr.contains("file exists")
                    || stderr.contains("exists")
                    || stderr.contains("already"))
            {
                return Ok(());
            }
            debug!(command = %command, "VoWiFi TUN configuration command failed");
            return Err(tun_error(reason));
        }
        Err(tun_error(reason))
    }

    fn spawn_forwarders(
        config: &TunGatewayConfig,
        read_file: File,
        write_file: File,
        ims_esp_policy: Arc<StdMutex<Option<ImsEspRuntimePolicy>>>,
        shutdown: Arc<AtomicBool>,
    ) {
        let (inner_tx, mut inner_rx) = mpsc::channel::<Vec<u8>>(128);
        spawn_tun_reader(read_file, inner_tx, Arc::clone(&shutdown));

        let outbound_transport = config.transport.clone();
        let outbound_remote = config.remote;
        let outbound_spi = config.outbound_sa_identifier;
        let outbound_secrets = config.secrets.clone();
        let outbound_ims_esp_policy = Arc::clone(&ims_esp_policy);
        let outbound_shutdown = Arc::clone(&shutdown);
        tokio::spawn(async move {
            let mut sequence_number = 1u64;
            while let Some(packet) = inner_rx.recv().await {
                if outbound_shutdown.load(Ordering::SeqCst) {
                    break;
                }
                let packet =
                    match protect_ims_esp_outbound_if_needed(packet, &outbound_ims_esp_policy) {
                        Ok(packet) => packet,
                        Err(err) => {
                            warn!(reason = %err, "IMS ESP outbound protection failed");
                            continue;
                        }
                    };
                let Some(next_header) = inner_next_header(&packet) else {
                    continue;
                };
                let current_sequence = sequence_number;
                sequence_number = sequence_number.saturating_add(1);
                match protect_inner_packet_for_esp(
                    outbound_spi,
                    current_sequence,
                    &packet,
                    next_header,
                    &outbound_secrets,
                ) {
                    Ok((frame, _summary)) => {
                        if let Err(err) = outbound_transport
                            .send_esp_nat_t_metadata(outbound_remote, &frame)
                            .await
                        {
                            warn!(reason = %err, "VoWiFi ESP outbound send failed");
                        }
                    }
                    Err(err) => {
                        warn!(reason = %err, "VoWiFi ESP outbound protection failed");
                    }
                }
            }
        });

        let inbound_transport = config
            .transport
            .clone()
            .with_recv_timeout(std::time::Duration::from_secs(2));
        let inbound_spi = config.inbound_sa_identifier;
        let inbound_secrets = config.secrets.clone();
        let inbound_ims_esp_policy = Arc::clone(&ims_esp_policy);
        let writer = Arc::new(StdMutex::new(write_file));
        let inbound_shutdown = Arc::clone(&shutdown);
        tokio::spawn(async move {
            let mut replay = AntiReplayWindow::new(64);
            loop {
                if inbound_shutdown.load(Ordering::SeqCst) {
                    break;
                }
                let packet = match inbound_transport.recv_nat_t_raw_metadata().await {
                    Ok((_remote, packet, _metadata)) => packet,
                    Err(super::super::transport::TransportError::Timeout(_)) => continue,
                    Err(err) => {
                        warn!(reason = %err, "VoWiFi ESP inbound receive failed");
                        continue;
                    }
                };
                if packet == [0xff] || packet.starts_with(&[0, 0, 0, 0]) {
                    continue;
                }
                if packet.len() < 8
                    || u32::from_be_bytes([packet[0], packet[1], packet[2], packet[3]])
                        != inbound_spi
                {
                    continue;
                }
                let sequence =
                    u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]) as u64;
                match unprotect_inner_packet_from_esp(&packet, &inbound_secrets) {
                    Ok((inner, _summary)) => {
                        if !replay.accept(sequence).accepted {
                            continue;
                        }
                        let inner = match unprotect_ims_esp_inbound_if_needed(
                            inner,
                            &inbound_ims_esp_policy,
                        ) {
                            Ok(inner) => inner,
                            Err(err) => {
                                warn!(reason = %err, "IMS ESP inbound unprotect failed");
                                continue;
                            }
                        };
                        if let Ok(mut file) = writer.lock() {
                            if let Err(err) = file.write_all(&inner) {
                                warn!(reason = %err, "VoWiFi TUN inbound write failed");
                            }
                        }
                    }
                    Err(err) => {
                        warn!(reason = %err, "VoWiFi ESP inbound unprotect failed");
                    }
                }
            }
        });
    }

    fn protect_ims_esp_outbound_if_needed(
        packet: Vec<u8>,
        policy_lock: &Arc<StdMutex<Option<ImsEspRuntimePolicy>>>,
    ) -> Result<Vec<u8>, TunGatewayError> {
        let mut guard = policy_lock
            .lock()
            .map_err(|_| tun_error("ims_esp_policy_lock_failed"))?;
        let Some(policy) = guard.as_mut() else {
            return Ok(packet);
        };
        let Some(flow_index) = ims_outbound_tcp_flow_index(&packet, policy) else {
            return Ok(packet);
        };
        let flow = &mut policy.flows[flow_index];
        let sequence = flow.allocate_outbound_sequence()?;
        let parsed = ParsedIpPacket::parse(&packet)?;
        let payload = parsed.payload(&packet);
        let (esp, summary) = protect_inner_packet_for_esp(
            flow.outbound_sa_identifier,
            sequence,
            payload,
            6,
            &flow.secrets,
        )
        .map_err(|_| tun_error("ims_esp_protect_failed"))?;
        if !flow.outbound_logged {
            info!(
                profile_id = policy.profile_id,
                flow = flow.label,
                sequence_number = summary.sequence_number,
                protected_bytes = summary.protected_bytes,
                "IMS ESP outbound packet protected"
            );
            flow.outbound_logged = true;
        } else {
            debug!(
                profile_id = policy.profile_id,
                flow = flow.label,
                sequence_number = summary.sequence_number,
                protected_bytes = summary.protected_bytes,
                "IMS ESP outbound packet protected"
            );
        }
        parsed.rebuild_with_payload(&packet, 50, &esp)
    }

    fn unprotect_ims_esp_inbound_if_needed(
        packet: Vec<u8>,
        policy_lock: &Arc<StdMutex<Option<ImsEspRuntimePolicy>>>,
    ) -> Result<Vec<u8>, TunGatewayError> {
        let mut guard = policy_lock
            .lock()
            .map_err(|_| tun_error("ims_esp_policy_lock_failed"))?;
        let Some(policy) = guard.as_mut() else {
            return Ok(packet);
        };
        let Some(flow_index) = ims_inbound_esp_flow_index(&packet, policy) else {
            return Ok(packet);
        };
        let flow = &mut policy.flows[flow_index];
        let parsed = ParsedIpPacket::parse(&packet)?;
        let payload = parsed.payload(&packet);
        let sequence = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]) as u64;
        let (transport_payload, summary) = unprotect_inner_packet_from_esp(payload, &flow.secrets)
            .map_err(|_| tun_error("ims_esp_unprotect_failed"))?;
        if !flow.inbound_replay.accept(sequence).accepted {
            return Err(tun_error("ims_esp_replay_rejected"));
        }
        if !flow.inbound_logged {
            info!(
                profile_id = policy.profile_id,
                flow = flow.label,
                sequence_number = summary.sequence_number,
                protected_bytes = summary.protected_bytes,
                "IMS ESP inbound packet unprotected"
            );
            flow.inbound_logged = true;
        } else {
            debug!(
                profile_id = policy.profile_id,
                flow = flow.label,
                sequence_number = summary.sequence_number,
                protected_bytes = summary.protected_bytes,
                "IMS ESP inbound packet unprotected"
            );
        }
        parsed.rebuild_with_payload(&packet, summary.next_header, &transport_payload)
    }

    fn ims_outbound_tcp_flow_index(packet: &[u8], policy: &ImsEspRuntimePolicy) -> Option<usize> {
        let Ok(parsed) = ParsedIpPacket::parse(packet) else {
            return None;
        };
        if parsed.next_header != 6
            || parsed.src != policy.local_addr
            || parsed.dst != policy.remote_addr
        {
            return None;
        }
        let payload = parsed.payload(packet);
        let (src, dst) = tcp_ports(payload)?;
        policy
            .flows
            .iter()
            .position(|flow| src == flow.local_port && dst == flow.remote_port)
    }

    fn ims_inbound_esp_flow_index(packet: &[u8], policy: &ImsEspRuntimePolicy) -> Option<usize> {
        let Ok(parsed) = ParsedIpPacket::parse(packet) else {
            return None;
        };
        if parsed.next_header != 50
            || parsed.src != policy.remote_addr
            || parsed.dst != policy.local_addr
        {
            return None;
        }
        let payload = parsed.payload(packet);
        if payload.len() < 8 {
            return None;
        }
        let inbound_sa_identifier =
            u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
        policy
            .flows
            .iter()
            .position(|flow| inbound_sa_identifier == flow.inbound_sa_identifier)
    }

    #[derive(Debug, Clone)]
    struct ParsedIpPacket {
        version: u8,
        header_len: usize,
        payload_start: usize,
        payload_end: usize,
        next_header: u8,
        src: IpAddr,
        dst: IpAddr,
    }

    impl ParsedIpPacket {
        fn parse(packet: &[u8]) -> Result<Self, TunGatewayError> {
            match packet.first().map(|byte| byte >> 4) {
                Some(4) => Self::parse_v4(packet),
                Some(6) => Self::parse_v6(packet),
                _ => Err(tun_error("ims_esp_ip_packet_unsupported")),
            }
        }

        fn parse_v4(packet: &[u8]) -> Result<Self, TunGatewayError> {
            if packet.len() < 20 {
                return Err(tun_error("ims_esp_ipv4_packet_too_short"));
            }
            let ihl = usize::from(packet[0] & 0x0f) * 4;
            if ihl < 20 || packet.len() < ihl {
                return Err(tun_error("ims_esp_ipv4_header_invalid"));
            }
            let total_len = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
            if total_len < ihl || total_len > packet.len() {
                return Err(tun_error("ims_esp_ipv4_length_invalid"));
            }
            Ok(Self {
                version: 4,
                header_len: ihl,
                payload_start: ihl,
                payload_end: total_len,
                next_header: packet[9],
                src: IpAddr::V4(Ipv4Addr::new(
                    packet[12], packet[13], packet[14], packet[15],
                )),
                dst: IpAddr::V4(Ipv4Addr::new(
                    packet[16], packet[17], packet[18], packet[19],
                )),
            })
        }

        fn parse_v6(packet: &[u8]) -> Result<Self, TunGatewayError> {
            if packet.len() < 40 {
                return Err(tun_error("ims_esp_ipv6_packet_too_short"));
            }
            let payload_len = usize::from(u16::from_be_bytes([packet[4], packet[5]]));
            let payload_end = 40usize
                .checked_add(payload_len)
                .ok_or_else(|| tun_error("ims_esp_ipv6_length_invalid"))?;
            if payload_end > packet.len() {
                return Err(tun_error("ims_esp_ipv6_length_invalid"));
            }
            let src: [u8; 16] = packet[8..24]
                .try_into()
                .expect("IPv6 source address has fixed length");
            let dst: [u8; 16] = packet[24..40]
                .try_into()
                .expect("IPv6 destination address has fixed length");
            Ok(Self {
                version: 6,
                header_len: 40,
                payload_start: 40,
                payload_end,
                next_header: packet[6],
                src: IpAddr::V6(Ipv6Addr::from(src)),
                dst: IpAddr::V6(Ipv6Addr::from(dst)),
            })
        }

        fn payload<'a>(&self, packet: &'a [u8]) -> &'a [u8] {
            &packet[self.payload_start..self.payload_end]
        }

        fn rebuild_with_payload(
            &self,
            packet: &[u8],
            next_header: u8,
            payload: &[u8],
        ) -> Result<Vec<u8>, TunGatewayError> {
            match self.version {
                4 => self.rebuild_v4(packet, next_header, payload),
                6 => self.rebuild_v6(packet, next_header, payload),
                _ => Err(tun_error("ims_esp_ip_packet_unsupported")),
            }
        }

        fn rebuild_v4(
            &self,
            packet: &[u8],
            next_header: u8,
            payload: &[u8],
        ) -> Result<Vec<u8>, TunGatewayError> {
            let total_len = self
                .header_len
                .checked_add(payload.len())
                .filter(|len| *len <= usize::from(u16::MAX))
                .ok_or_else(|| tun_error("ims_esp_ipv4_length_invalid"))?;
            let mut out = Vec::with_capacity(total_len);
            out.extend_from_slice(&packet[..self.header_len]);
            out[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
            out[9] = next_header;
            out[10] = 0;
            out[11] = 0;
            let checksum = ipv4_header_checksum(&out);
            out[10..12].copy_from_slice(&checksum.to_be_bytes());
            out.extend_from_slice(payload);
            Ok(out)
        }

        fn rebuild_v6(
            &self,
            packet: &[u8],
            next_header: u8,
            payload: &[u8],
        ) -> Result<Vec<u8>, TunGatewayError> {
            if payload.len() > usize::from(u16::MAX) {
                return Err(tun_error("ims_esp_ipv6_length_invalid"));
            }
            let mut out = Vec::with_capacity(40 + payload.len());
            out.extend_from_slice(&packet[..40]);
            out[4..6].copy_from_slice(&(payload.len() as u16).to_be_bytes());
            out[6] = next_header;
            out.extend_from_slice(payload);
            Ok(out)
        }
    }

    fn tcp_ports(payload: &[u8]) -> Option<(u16, u16)> {
        (payload.len() >= 4).then(|| {
            (
                u16::from_be_bytes([payload[0], payload[1]]),
                u16::from_be_bytes([payload[2], payload[3]]),
            )
        })
    }

    fn ipv4_header_checksum(header: &[u8]) -> u16 {
        let mut sum = 0u32;
        for chunk in header.chunks(2) {
            let word = if chunk.len() == 2 {
                u16::from_be_bytes([chunk[0], chunk[1]]) as u32
            } else {
                u32::from(chunk[0]) << 8
            };
            sum = sum.wrapping_add(word);
        }
        while (sum >> 16) != 0 {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        !(sum as u16)
    }

    fn spawn_tun_reader(mut file: File, tx: mpsc::Sender<Vec<u8>>, shutdown: Arc<AtomicBool>) {
        tokio::task::spawn_blocking(move || {
            let mut buffer = vec![0u8; 4096];
            loop {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                match file.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(bytes) => {
                        if tx.blocking_send(buffer[..bytes].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(err) if err.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });
    }

    fn inner_next_header(packet: &[u8]) -> Option<u8> {
        match packet.first().map(|byte| byte >> 4) {
            Some(4) => Some(4),
            Some(6) => Some(41),
            _ => None,
        }
    }

    fn valid_ifname_byte(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
    }

    fn ip_family(addr: IpAddr) -> &'static str {
        match addr {
            IpAddr::V4(_) => "ipv4",
            IpAddr::V6(_) => "ipv6",
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::*;

    pub(crate) async fn start_gateway(
        _config: TunGatewayConfig,
    ) -> Result<Arc<TunGatewayRuntime>, TunGatewayError> {
        Err(tun_error("tun_gateway_platform_unsupported"))
    }
}

pub(crate) use imp::start_gateway;
