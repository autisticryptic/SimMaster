#![allow(dead_code)]

use std::{
    collections::HashMap,
    fmt,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::{Duration, Instant},
};

use super::{ike_keys::ChildSaSecretPair, transport::UdpSocketDatagramTransport};

const IMS_ESP_CLIENT_FLOW: &str = "client_flow";
const IMS_ESP_SERVER_FLOW: &str = "server_flow";

/// Inner IP packets larger than this (after the IMS ESP transform) are
/// fragmented in software before the outer tunnel encapsulation, so every
/// physical packet stays below the 1500-byte path MTU. 1356 bytes of inner
/// IP yields an outer ESP-in-UDP packet of ~1456 bytes with margin.
const AUTO_FRAGMENT_INNER_IP_MAX: usize = 1356;

/// Identification counter for IPv6 fragment headers (RFC 8200 §4.5). A
/// monotonic counter guarantees distinct values across fragmented packets,
/// which is all the receiver needs to disambiguate reassembly buffers.
static INNER_FRAGMENT_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

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
    /// RFC 4303 §3.3.2 includes the explicit IV in the ICV. Some IMS
    /// stacks omit it; candidates carry this flag for interop probing.
    pub icv_include_iv: bool,
    /// Wrap the ESP frame in a UDP header (RFC 3948 style) instead of raw
    /// ESP (IP protocol 50). Some P-CSCF deployments negotiate ipsec-3gpp
    /// but expect the UDP-encapsulated variant on the protected ports.
    pub udp_encapsulate: bool,
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
    icv_include_iv: bool,
    udp_encapsulate: bool,
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
            icv_include_iv: config.icv_include_iv,
            udp_encapsulate: config.udp_encapsulate,
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

    use crate::connectivity::modems::ims::vowifi::dataplane::{
        protect_inner_packet_for_esp, protect_inner_packet_for_esp_with_mode,
        unprotect_inner_packet_from_esp, unprotect_inner_packet_from_esp_with_mode,
        AntiReplayWindow,
    };

    #[cfg(target_env = "musl")]
    const TUNSETIFF: libc::c_int = 0x4004_54ca;
    #[cfg(not(target_env = "musl"))]
    const TUNSETIFF: libc::c_ulong = 0x4004_54ca;
    const IFF_TUN: i16 = 0x0001;
    const IFF_NO_PI: i16 = 0x1000;
    const IFREQ_BYTES: usize = 40;
    const IFNAMSIZ: usize = 16;
    // The inner SIP REGISTER (with Security-Client/Verify + Authorization +
    // contact features) routinely exceeds 1400 bytes. Keeping the TUN MTU at
    // 1360 made the kernel fragment it, and the gateway shipped the fragments
    // as separate ESP packets, which the P-CSCF could not reassemble. 1600
    // lets a full REGISTER through as one datagram; the outer ESP packet is
    // then fragmented at the physical link (IP_MTU_DISCOVER=DONT) and
    // reassembled by the ePDG.
    const DEFAULT_TUN_MTU: u16 = 1600;

    #[derive(Debug, Clone, Copy)]
    struct Ipv4FragmentInfo {
        identification: u32,
        offset_bytes: usize,
        more_fragments: bool,
    }

    fn ipv4_fragment_info(packet: &[u8]) -> Option<Ipv4FragmentInfo> {
        if packet.first().map(|byte| byte >> 4) != Some(4) || packet.len() < 20 {
            return None;
        }
        let flags_and_offset = u16::from_be_bytes([packet[6], packet[7]]);
        let more_fragments = flags_and_offset & 0x2000 != 0;
        let offset_bytes = usize::from(flags_and_offset & 0x1fff) * 8;
        if !more_fragments && offset_bytes == 0 {
            return None;
        }
        Some(Ipv4FragmentInfo {
            identification: u32::from_be_bytes([0, 0, packet[4], packet[5]]),
            offset_bytes,
            more_fragments,
        })
    }

    struct Ipv4FragmentBuffer {
        header: Vec<u8>,
        payload: Vec<u8>,
        created_at: Instant,
    }

    #[derive(Debug)]
    enum FragmentReassemblyOutcome {
        Forward(Vec<u8>),
        Buffered,
        Dropped,
    }

    /// Reassemble an IPv4 fragment stream before the ipsec-3gpp transform.
    ///
    /// The kernel fragments an oversized SIP datagram at the TUN MTU. Shipping
    /// the fragments separately would make the P-CSCF concatenate two ESP
    /// frames into one bogus payload, so the gateway waits for the full set and
    /// re-emits one complete IP packet. The outer ESP packet then carries the
    /// whole REGISTER; the physical link fragments it and the ePDG reassembles.
    fn reassemble_outbound_ip_fragment(
        packet: Vec<u8>,
        buffers: &mut HashMap<(IpAddr, IpAddr, u32), Ipv4FragmentBuffer>,
    ) -> FragmentReassemblyOutcome {
        let Some(fragment) = ipv4_fragment_info(&packet) else {
            return FragmentReassemblyOutcome::Forward(packet);
        };
        if packet.len() < 20 {
            return FragmentReassemblyOutcome::Dropped;
        }
        let src = IpAddr::V4(Ipv4Addr::new(
            packet[12], packet[13], packet[14], packet[15],
        ));
        let dst = IpAddr::V4(Ipv4Addr::new(
            packet[16], packet[17], packet[18], packet[19],
        ));
        let key = (src, dst, fragment.identification);

        let now = Instant::now();
        if buffers.len() >= 32 {
            buffers
                .retain(|_, buffer| now.duration_since(buffer.created_at) < Duration::from_secs(3));
        }

        if fragment.offset_bytes == 0 {
            let ihl = usize::from(packet[0] & 0x0f) * 4;
            buffers.insert(
                key,
                Ipv4FragmentBuffer {
                    header: packet[..ihl].to_vec(),
                    payload: packet[ihl..].to_vec(),
                    created_at: now,
                },
            );
            return FragmentReassemblyOutcome::Buffered;
        }

        let Some(buffer) = buffers.get_mut(&key) else {
            warn!(
                src = %src,
                dst = %dst,
                identification = fragment.identification,
                offset_bytes = fragment.offset_bytes,
                "IMS TUN outbound continuation fragment without fragment 0; dropping"
            );
            return FragmentReassemblyOutcome::Dropped;
        };
        let ihl = usize::from(packet[0] & 0x0f) * 4;
        buffer.payload.extend_from_slice(&packet[ihl..]);
        if fragment.more_fragments {
            return FragmentReassemblyOutcome::Buffered;
        }

        let mut header = buffer.header.clone();
        let payload = std::mem::take(&mut buffer.payload);
        buffers.remove(&key);
        let total_len = header.len().checked_add(payload.len());
        let Some(total_len) = total_len.filter(|len| *len <= usize::from(u16::MAX)) else {
            return FragmentReassemblyOutcome::Dropped;
        };
        header[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
        header[6..8].copy_from_slice(&[0, 0]);
        header[10] = 0;
        header[11] = 0;
        let checksum = ipv4_header_checksum(&header);
        header[10..12].copy_from_slice(&checksum.to_be_bytes());
        let mut reassembled = header;
        reassembled.extend_from_slice(&payload);
        FragmentReassemblyOutcome::Forward(reassembled)
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct InboundFragmentKey {
        src: IpAddr,
        dst: IpAddr,
        identification: u32,
    }

    #[derive(Clone)]
    struct InboundFragmentPiece {
        offset_bytes: usize,
        payload: Vec<u8>,
    }

    struct InboundFragmentBuffer {
        /// IPv4: complete first-fragment IP header (flags/offset cleared and
        /// total length set on reassembly). IPv6: 40-byte base header with
        /// the original next header (the Fragment Header's next header)
        /// restored in byte 6. None until fragment 0 arrives (continuation
        /// fragments may arrive first on the network).
        base_header: Option<Vec<u8>>,
        pieces: Vec<InboundFragmentPiece>,
        expected_len: Option<usize>,
        created_at: Instant,
    }

    struct InboundFragmentInfo {
        src: IpAddr,
        dst: IpAddr,
        identification: u32,
        offset_bytes: usize,
        more_fragments: bool,
        base_header: Vec<u8>,
        payload: Vec<u8>,
    }

    /// Extract fragment metadata for an inbound IP packet. IPv4 fragments
    /// use the MF/offset fields in the base header; IPv6 fragments carry a
    /// Fragment Header (RFC 8200 §4.5). Non-fragmented packets return None.
    fn inbound_fragment_info(packet: &[u8]) -> Option<InboundFragmentInfo> {
        match packet.first().map(|byte| byte >> 4) {
            Some(4) => {
                if packet.len() < 20 {
                    return None;
                }
                let flags_and_offset = u16::from_be_bytes([packet[6], packet[7]]);
                let more_fragments = flags_and_offset & 0x2000 != 0;
                let offset_bytes = usize::from(flags_and_offset & 0x1fff) * 8;
                if !more_fragments && offset_bytes == 0 {
                    return None;
                }
                let ihl = usize::from(packet[0] & 0x0f) * 4;
                Some(InboundFragmentInfo {
                    src: IpAddr::V4(Ipv4Addr::new(
                        packet[12], packet[13], packet[14], packet[15],
                    )),
                    dst: IpAddr::V4(Ipv4Addr::new(
                        packet[16], packet[17], packet[18], packet[19],
                    )),
                    identification: u32::from_be_bytes([0, 0, packet[4], packet[5]]),
                    offset_bytes,
                    more_fragments,
                    base_header: packet[..ihl].to_vec(),
                    payload: packet[ihl..].to_vec(),
                })
            }
            Some(6) => {
                if packet.len() < 48 || packet[6] != 44 {
                    return None;
                }
                let offset_m = u16::from_be_bytes([packet[42], packet[43]]);
                let offset_bytes = usize::from(offset_m >> 3) * 8;
                let more_fragments = offset_m & 1 == 1;
                let mut base_header = packet[..40].to_vec();
                base_header[6] = packet[40]; // restore the original next header
                Some(InboundFragmentInfo {
                    src: IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(&packet[8..24]).ok()?)),
                    dst: IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(&packet[24..40]).ok()?)),
                    identification: u32::from_be_bytes([
                        packet[44], packet[45], packet[46], packet[47],
                    ]),
                    offset_bytes,
                    more_fragments,
                    base_header,
                    payload: packet[48..].to_vec(),
                })
            }
            _ => None,
        }
    }

    /// Reassemble inbound IP fragments (from the P-CSCF) before the
    /// ipsec-3gpp inbound transform. The P-CSCF may split a large protected
    /// response (e.g. an INVITE carrying SDP) into several ESP packets; the
    /// IP layer must reassemble them into one ESP frame before ICV/decrypt.
    /// Handles IPv4 fragments and IPv6 Fragment Headers, including
    /// out-of-order arrival and basic overlap rejection.
    fn reassemble_inbound_ip_fragment(
        packet: Vec<u8>,
        buffers: &mut HashMap<InboundFragmentKey, InboundFragmentBuffer>,
    ) -> FragmentReassemblyOutcome {
        let Some(info) = inbound_fragment_info(&packet) else {
            return FragmentReassemblyOutcome::Forward(packet);
        };
        let key = InboundFragmentKey {
            src: info.src,
            dst: info.dst,
            identification: info.identification,
        };
        let now = Instant::now();
        if buffers.len() >= 32 {
            buffers
                .retain(|_, buffer| now.duration_since(buffer.created_at) < Duration::from_secs(3));
        }
        let piece = InboundFragmentPiece {
            offset_bytes: info.offset_bytes,
            payload: info.payload,
        };
        let last = !info.more_fragments;
        let buffer = buffers.entry(key).or_insert_with(|| InboundFragmentBuffer {
            base_header: None,
            pieces: Vec::new(),
            expected_len: None,
            created_at: now,
        });
        // Reject overlapping fragments (RFC 8200 §4.5.4 / classic reassembly
        // hardening): ranges must be disjoint.
        let start = piece.offset_bytes;
        let end = start + piece.payload.len();
        if buffer.pieces.iter().any(|piece| {
            let piece_end = piece.offset_bytes + piece.payload.len();
            start < piece_end && piece.offset_bytes < end
        }) {
            warn!(
                src = %info.src,
                dst = %info.dst,
                identification = info.identification,
                "IMS ESP inbound overlapping fragment; dropping"
            );
            buffers.remove(&key);
            return FragmentReassemblyOutcome::Dropped;
        }
        if info.offset_bytes == 0 {
            // Fragment 0 carries the base header; replace any stale piece at
            // offset 0 and record the header for final assembly.
            buffer.base_header = Some(info.base_header);
            buffer.pieces.retain(|piece| piece.offset_bytes != 0);
        }
        let piece_end = piece.offset_bytes + piece.payload.len();
        buffer.pieces.push(piece);
        if last {
            buffer.expected_len = Some(piece_end);
        }
        let Some(expected) = buffer.expected_len else {
            return FragmentReassemblyOutcome::Buffered;
        };
        let Some(header) = &buffer.base_header else {
            return FragmentReassemblyOutcome::Buffered;
        };
        let mut pieces = buffer.pieces.clone();
        pieces.sort_by_key(|piece| piece.offset_bytes);
        let mut cursor = 0usize;
        for piece in &pieces {
            if piece.offset_bytes > cursor {
                return FragmentReassemblyOutcome::Buffered;
            }
            cursor = cursor.max(piece.offset_bytes + piece.payload.len());
        }
        if cursor != expected {
            return FragmentReassemblyOutcome::Buffered;
        }
        let mut header = header.clone();
        match header.first().map(|byte| byte >> 4) {
            Some(4) => {
                let ihl = usize::from(header[0] & 0x0f) * 4;
                let total_len = ihl + expected;
                if total_len > usize::from(u16::MAX) {
                    return FragmentReassemblyOutcome::Dropped;
                }
                header[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
                header[6..8].copy_from_slice(&[0, 0]);
                header[10] = 0;
                header[11] = 0;
                let checksum = ipv4_header_checksum(&header[..ihl]);
                header[10..12].copy_from_slice(&checksum.to_be_bytes());
            }
            Some(6) => {
                if expected > usize::from(u16::MAX) {
                    return FragmentReassemblyOutcome::Dropped;
                }
                header[4..6].copy_from_slice(&(expected as u16).to_be_bytes());
            }
            _ => return FragmentReassemblyOutcome::Dropped,
        }
        let mut reassembled = header;
        for piece in &pieces {
            reassembled.extend_from_slice(&piece.payload);
        }
        buffers.remove(&key);
        FragmentReassemblyOutcome::Forward(reassembled)
    }

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
        // The interface name is stable per line (see tun_name_for_line), so a
        // reconnect must be able to recreate it. TUNSETIFF fails with EEXIST
        // while the old device is still around, so delete it best-effort.
        let _ = Command::new("ip")
            .args(["link", "del", "dev", tun_name])
            .output();
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
            let mut fragment_buffers = HashMap::<(IpAddr, IpAddr, u32), Ipv4FragmentBuffer>::new();
            while let Some(packet) = inner_rx.recv().await {
                if outbound_shutdown.load(Ordering::SeqCst) {
                    break;
                }
                let packet = match reassemble_outbound_ip_fragment(packet, &mut fragment_buffers) {
                    FragmentReassemblyOutcome::Forward(packet) => packet,
                    FragmentReassemblyOutcome::Buffered => continue,
                    FragmentReassemblyOutcome::Dropped => continue,
                };
                log_tun_outbound_packet(&packet);
                let packet =
                    match protect_ims_esp_outbound_if_needed(packet, &outbound_ims_esp_policy) {
                        Ok(packet) => packet,
                        Err(err) => {
                            warn!(reason = %err, "IMS ESP outbound protection failed");
                            continue;
                        }
                    };
                // Software fragmentation is the default: it keeps the full
                // REGISTER headers while guaranteeing every physical packet
                // stays under the path MTU. Set SIMADMIN_AUTO_FRAGMENT=0 to
                // disable (then either the packet must fit, or the carrier
                // must handle IP-layer fragmentation of the outer ESP).
                let auto_fragment = std::env::var("SIMADMIN_AUTO_FRAGMENT")
                    .map(|value| value != "0")
                    .unwrap_or(true);
                let outbound_fragments = if auto_fragment {
                    let fragments = fragment_inner_packet(&packet, AUTO_FRAGMENT_INNER_IP_MAX);
                    if fragments.len() > 1 {
                        info!(
                            inner_packet_bytes = packet.len(),
                            fragment_count = fragments.len(),
                            "IMS ESP inner packet software-fragmented for outer tunnel MTU"
                        );
                    }
                    fragments
                } else {
                    vec![packet]
                };
                for fragment in outbound_fragments {
                    let Some(next_header) = inner_next_header(&fragment) else {
                        continue;
                    };
                    let current_sequence = sequence_number;
                    sequence_number = sequence_number.saturating_add(1);
                    match protect_inner_packet_for_esp(
                        outbound_spi,
                        current_sequence,
                        &fragment,
                        next_header,
                        &outbound_secrets,
                    ) {
                        Ok((frame, summary)) => {
                            info!(
                                outer_sequence = current_sequence,
                                outer_frame_bytes = summary.outer_frame_bytes,
                                inner_packet_bytes = summary.inner_packet_bytes,
                                "IMS ESP outbound frame sent through outer tunnel"
                            );
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
            }
        });

        fn log_tun_outbound_packet(packet: &[u8]) {
            let Ok(parsed) = ParsedIpPacket::parse(packet) else {
                return;
            };
            if parsed.next_header == 50 {
                info!(
                    ip_proto = 50,
                    packet_bytes = packet.len(),
                    src = %parsed.src,
                    dst = %parsed.dst,
                    "IMS ESP-wrapped packet re-entered TUN for outer tunnel"
                );
                return;
            }
            if parsed.next_header == 17 {
                let payload = parsed.payload(packet);
                if payload.len() >= 4 {
                    let src_port = u16::from_be_bytes([payload[0], payload[1]]);
                    let dst_port = u16::from_be_bytes([payload[2], payload[3]]);
                    if src_port != 5060 || dst_port != 5060 {
                        info!(
                            ip_proto = 17,
                            packet_bytes = packet.len(),
                            src = %parsed.src,
                            dst = %parsed.dst,
                            src_port,
                            dst_port,
                            "IMS TUN outbound UDP packet on non-5060 ports"
                        );
                    }
                }
            }
        }

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
            let mut inbound_fragment_buffers =
                HashMap::<InboundFragmentKey, InboundFragmentBuffer>::new();
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
                        let inner = match reassemble_inbound_ip_fragment(
                            inner,
                            &mut inbound_fragment_buffers,
                        ) {
                            FragmentReassemblyOutcome::Forward(inner) => inner,
                            FragmentReassemblyOutcome::Buffered => continue,
                            FragmentReassemblyOutcome::Dropped => continue,
                        };
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
        let Some(matched) = ims_outbound_flow_index(&packet, policy) else {
            return Ok(packet);
        };
        let flow = &mut policy.flows[matched.flow_index];
        let sequence = flow.allocate_outbound_sequence()?;
        let parsed = ParsedIpPacket::parse(&packet)?;
        let payload = parsed.payload(&packet);
        let (esp, summary) = protect_inner_packet_for_esp_with_mode(
            flow.outbound_sa_identifier,
            sequence,
            payload,
            matched.next_header,
            &flow.secrets,
            flow.icv_include_iv,
        )
        .map_err(|_| tun_error("ims_esp_protect_failed"))?;
        if std::env::var("SIMADMIN_DEBUG_ESP_FRAMES").is_ok() {
            tracing::info!(
                profile_id = policy.profile_id,
                flow = flow.label,
                outbound_sa_identifier = flow.outbound_sa_identifier,
                sequence_number = summary.sequence_number,
                icv_include_iv = flow.icv_include_iv,
                udp_encapsulate = flow.udp_encapsulate,
                inner_ip_packet_hex = hex_bytes(&packet),
                esp_frame_hex = hex_bytes(&esp),
                "IMS ESP frame debug dump (SIMADMIN_DEBUG_ESP_FRAMES)"
            );
        }
        if !flow.outbound_logged {
            info!(
                profile_id = policy.profile_id,
                flow = flow.label,
                outbound_sa_identifier = flow.outbound_sa_identifier,
                sequence_number = summary.sequence_number,
                protected_bytes = summary.protected_bytes,
                icv_include_iv = flow.icv_include_iv,
                udp_encapsulate = flow.udp_encapsulate,
                "IMS ESP outbound packet protected"
            );
            flow.outbound_logged = true;
        } else {
            debug!(
                profile_id = policy.profile_id,
                flow = flow.label,
                sequence_number = summary.sequence_number,
                protected_bytes = summary.protected_bytes,
                icv_include_iv = flow.icv_include_iv,
                udp_encapsulate = flow.udp_encapsulate,
                "IMS ESP outbound packet protected"
            );
        }
        if flow.udp_encapsulate {
            // RFC 3948 ESP-in-UDP: insert an 8-byte UDP header between the
            // IP header and the ESP frame. Source/destination are the flow's
            // protected client/server ports; the checksum is computed over
            // the pseudo-header so both IPv4 and IPv6 receivers accept it.
            let mut encapsulated = Vec::with_capacity(8 + esp.len());
            encapsulated.extend_from_slice(&flow.local_port.to_be_bytes());
            encapsulated.extend_from_slice(&flow.remote_port.to_be_bytes());
            let udp_len = u16::try_from(8 + esp.len())
                .map_err(|_| tun_error("ims_esp_udp_length_invalid"))?;
            encapsulated.extend_from_slice(&udp_len.to_be_bytes());
            encapsulated.extend_from_slice(&[0, 0]);
            encapsulated.extend_from_slice(&esp);
            let checksum = udp_checksum(parsed.src, parsed.dst, &encapsulated);
            encapsulated[6..8].copy_from_slice(&checksum.to_be_bytes());
            parsed.rebuild_with_payload(&packet, 17, &encapsulated)
        } else {
            parsed.rebuild_with_payload(&packet, 50, &esp)
        }
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
        let esp_payload = if flow.udp_encapsulate {
            if payload.len() < 8 {
                return Err(tun_error("ims_esp_udp_short"));
            }
            &payload[8..]
        } else {
            payload
        };
        if esp_payload.len() < 8 {
            return Err(tun_error("ims_esp_packet_short"));
        }
        let sequence = u32::from_be_bytes([
            esp_payload[4],
            esp_payload[5],
            esp_payload[6],
            esp_payload[7],
        ]) as u64;
        let unprotect_result = unprotect_inner_packet_from_esp_with_mode(
            esp_payload,
            &flow.secrets,
            flow.icv_include_iv,
        );
        if unprotect_result.is_err() && std::env::var("SIMADMIN_DEBUG_ESP_FRAMES").is_ok() {
            tracing::warn!(
                profile_id = policy.profile_id,
                flow = flow.label,
                inbound_sa_identifier = flow.inbound_sa_identifier,
                udp_encapsulate = flow.udp_encapsulate,
                icv_include_iv = flow.icv_include_iv,
                esp_packet_hex = hex_bytes(esp_payload),
                "IMS ESP inbound frame debug dump (SIMADMIN_DEBUG_ESP_FRAMES)"
            );
        }
        let (transport_payload, summary) =
            unprotect_result.map_err(|_| tun_error("ims_esp_unprotect_failed"))?;
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

    struct ImsOutboundFlowMatch {
        flow_index: usize,
        next_header: u8,
    }

    /// Match an inner packet that carries IMS SIP traffic to the negotiated
    /// ipsec-3gpp flow. Both TCP (protocol 6) and UDP (protocol 17) are valid
    /// inside the tunnel; the outer ESP header must record which one so the
    /// peer can rebuild the inner packet.
    fn ims_outbound_flow_index(
        packet: &[u8],
        policy: &ImsEspRuntimePolicy,
    ) -> Option<ImsOutboundFlowMatch> {
        let Ok(parsed) = ParsedIpPacket::parse(packet) else {
            return None;
        };
        if !matches!(parsed.next_header, 6 | 17)
            || parsed.src != policy.local_addr
            || parsed.dst != policy.remote_addr
        {
            return None;
        }
        let payload = parsed.payload(packet);
        let (src, dst) = transport_ports(payload)?;
        policy
            .flows
            .iter()
            .position(|flow| src == flow.local_port && dst == flow.remote_port)
            .map(|flow_index| ImsOutboundFlowMatch {
                flow_index,
                next_header: parsed.next_header,
            })
    }

    fn ims_inbound_esp_flow_index(packet: &[u8], policy: &ImsEspRuntimePolicy) -> Option<usize> {
        let Ok(parsed) = ParsedIpPacket::parse(packet) else {
            return None;
        };
        if parsed.src != policy.remote_addr || parsed.dst != policy.local_addr {
            return None;
        }
        let payload = parsed.payload(packet);
        match parsed.next_header {
            50 => {
                if payload.len() < 8 {
                    return None;
                }
                let inbound_sa_identifier =
                    u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                policy.flows.iter().position(|flow| {
                    !flow.udp_encapsulate && inbound_sa_identifier == flow.inbound_sa_identifier
                })
            }
            17 => {
                // UDP-encapsulated ESP (RFC 3948): 8-byte UDP header then
                // the ESP frame. Match the flow on the protected ports plus
                // the inbound SPI carried in the ESP header.
                if payload.len() < 16 {
                    return None;
                }
                let src_port = u16::from_be_bytes([payload[0], payload[1]]);
                let dst_port = u16::from_be_bytes([payload[2], payload[3]]);
                let inbound_sa_identifier =
                    u32::from_be_bytes([payload[8], payload[9], payload[10], payload[11]]);
                policy.flows.iter().position(|flow| {
                    flow.udp_encapsulate
                        && flow.local_port == dst_port
                        && flow.remote_port == src_port
                        && inbound_sa_identifier == flow.inbound_sa_identifier
                })
            }
            _ => None,
        }
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

    fn transport_ports(payload: &[u8]) -> Option<(u16, u16)> {
        (payload.len() >= 4).then(|| {
            (
                u16::from_be_bytes([payload[0], payload[1]]),
                u16::from_be_bytes([payload[2], payload[3]]),
            )
        })
    }

    fn hex_bytes(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use std::fmt::Write;
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    fn udp_checksum(src: IpAddr, dst: IpAddr, udp_packet: &[u8]) -> u16 {
        let mut sum = 0u32;
        let mut accumulate = |data: &[u8]| {
            let mut chunks = data.chunks_exact(2);
            for chunk in &mut chunks {
                sum = sum.wrapping_add(u16::from_be_bytes([chunk[0], chunk[1]]) as u32);
            }
            if let [last] = chunks.remainder() {
                sum = sum.wrapping_add(u32::from(*last) << 8);
            }
        };
        match src {
            IpAddr::V4(src) => {
                if let IpAddr::V4(dst) = dst {
                    accumulate(&src.octets());
                    accumulate(&dst.octets());
                    accumulate(&[0, 17]);
                    accumulate(&(udp_packet.len() as u16).to_be_bytes());
                }
            }
            IpAddr::V6(src) => {
                if let IpAddr::V6(dst) = dst {
                    accumulate(&src.octets());
                    accumulate(&dst.octets());
                    accumulate(&(udp_packet.len() as u32).to_be_bytes());
                    accumulate(&[0, 0, 0, 0, 0, 0, 0, 17]);
                }
            }
        }
        accumulate(udp_packet);
        while sum >> 16 != 0 {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        (!(sum as u16)).to_be()
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

    /// Fragment an inner IP packet (IPv4 or IPv6) in software so each
    /// fragment's total IP length is at most `max_ip_bytes`. IPv4 keeps the
    /// id and clears DF; IPv6 emits a Fragment Header (RFC 8200 §4.5) with
    /// offset/M and a per-packet identification. Already-small packets pass
    /// through unchanged.
    fn fragment_inner_packet(packet: &[u8], max_ip_bytes: usize) -> Vec<Vec<u8>> {
        if packet.len() <= max_ip_bytes {
            return vec![packet.to_vec()];
        }
        match packet.first().map(|byte| byte >> 4) {
            Some(4) => fragment_ipv4_packet(packet, max_ip_bytes),
            Some(6) => fragment_ipv6_packet(packet, max_ip_bytes),
            _ => vec![packet.to_vec()],
        }
    }

    fn fragment_ipv4_packet(packet: &[u8], max_ip_bytes: usize) -> Vec<Vec<u8>> {
        let ihl = usize::from(packet[0] & 0x0f) * 4;
        if ihl < 20 || packet.len() < ihl {
            return vec![packet.to_vec()];
        }
        let payload = &packet[ihl..];
        let chunk = ((max_ip_bytes.saturating_sub(ihl)) / 8) * 8;
        if chunk == 0 {
            return vec![packet.to_vec()];
        }
        let mut fragments = Vec::new();
        let mut offset = 0usize;
        loop {
            let end = (offset + chunk).min(payload.len());
            let last = end == payload.len();
            let mut fragment = packet[..ihl].to_vec();
            fragment[2..4].copy_from_slice(&((ihl + end - offset) as u16).to_be_bytes());
            // Preserve the id, clear DF (0x4000), keep MF on non-last.
            let id = u16::from_be_bytes([packet[4], packet[5]]);
            let flags_offset = u16::from((offset / 8) as u16) | if last { 0 } else { 0x2000 };
            fragment[4..6].copy_from_slice(&id.to_be_bytes());
            fragment[6..8].copy_from_slice(&flags_offset.to_be_bytes());
            fragment[10] = 0;
            fragment[11] = 0;
            let checksum = ipv4_header_checksum(&fragment[..ihl]);
            fragment[10..12].copy_from_slice(&checksum.to_be_bytes());
            fragment.extend_from_slice(&payload[offset..end]);
            fragments.push(fragment);
            if last {
                break;
            }
            offset = end;
        }
        fragments
    }

    fn fragment_ipv6_packet(packet: &[u8], max_ip_bytes: usize) -> Vec<Vec<u8>> {
        if packet.len() < 40 {
            return vec![packet.to_vec()];
        }
        let payload = &packet[40..];
        // The Fragment Header (8 bytes) rides in every fragment, so the
        // fragmentable payload chunk leaves room for it under the cap.
        let chunk = ((max_ip_bytes.saturating_sub(48)) / 8) * 8;
        if chunk == 0 {
            return vec![packet.to_vec()];
        }
        let identification = INNER_FRAGMENT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let original_next_header = packet[6];
        let mut fragments = Vec::new();
        let mut offset = 0usize;
        loop {
            let end = (offset + chunk).min(payload.len());
            let last = end == payload.len();
            let mut fragment = Vec::with_capacity(40 + 8 + (end - offset));
            fragment.extend_from_slice(&packet[..40]);
            fragment[4..6].copy_from_slice(&((8 + end - offset) as u16).to_be_bytes());
            fragment[6] = 44; // Fragment Header
            let mut fragment_header = [0u8; 8];
            fragment_header[0] = original_next_header;
            // 13-bit offset (8-octet units) in the high bits, M flag in the
            // low bit (RFC 8200 §4.5).
            let offset_m = (((offset / 8) as u16) << 3) | if last { 0 } else { 1 };
            fragment_header[2..4].copy_from_slice(&offset_m.to_be_bytes());
            fragment_header[4..8].copy_from_slice(&identification.to_be_bytes());
            fragment.extend_from_slice(&fragment_header);
            fragment.extend_from_slice(&payload[offset..end]);
            fragments.push(fragment);
            if last {
                break;
            }
            offset = end;
        }
        fragments
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

    #[cfg(test)]
    mod tests {
        use super::*;

        fn build_ipv4_udp_packet(payload: &[u8]) -> Vec<u8> {
            let mut packet = vec![0u8; 20 + 8 + payload.len()];
            packet[0] = 0x45;
            let total_len = packet.len() as u16;
            packet[2..4].copy_from_slice(&total_len.to_be_bytes());
            packet[4] = 0x12;
            packet[5] = 0x34;
            packet[8] = 64;
            packet[9] = 17;
            packet[12..16].copy_from_slice(&[2, 31, 105, 44]);
            packet[16..20].copy_from_slice(&[172, 20, 110, 221]);
            packet[20..22].copy_from_slice(&5064u16.to_be_bytes());
            packet[22..24].copy_from_slice(&7777u16.to_be_bytes());
            packet[24..26].copy_from_slice(&((8 + payload.len()) as u16).to_be_bytes());
            packet[26..28].copy_from_slice(&[0, 0]);
            packet[28..].copy_from_slice(payload);
            let checksum = ipv4_header_checksum(&packet[..20]);
            packet[10..12].copy_from_slice(&checksum.to_be_bytes());
            packet
        }

        fn fragment_packet(packet: &[u8], mtu: usize) -> Vec<Vec<u8>> {
            let ihl = usize::from(packet[0] & 0x0f) * 4;
            let payload = &packet[ihl..];
            let first_len = (mtu - ihl) & !7;
            let mut fragments = Vec::new();
            let mut offset = 0usize;
            loop {
                let end = (offset + first_len).min(payload.len());
                let last = end == payload.len();
                let mut frag = packet[..ihl].to_vec();
                frag[2..4].copy_from_slice(&((ihl + end - offset) as u16).to_be_bytes());
                let flags_and_offset =
                    u16::from((offset / 8) as u16) | if last { 0 } else { 0x2000 };
                frag[6..8].copy_from_slice(&flags_and_offset.to_be_bytes());
                let checksum = ipv4_header_checksum(&frag[..ihl]);
                frag[10..12].copy_from_slice(&checksum.to_be_bytes());
                frag.extend_from_slice(&payload[offset..end]);
                fragments.push(frag);
                if last {
                    break;
                }
                offset = end;
            }
            fragments
        }

        #[test]
        fn outbound_fragment_stream_reassembles_before_esp() {
            let sip = vec![b'R'; 1535];
            let original = build_ipv4_udp_packet(&sip);
            let fragments = fragment_packet(&original, 1360);
            assert!(fragments.len() >= 2, "REGISTER-sized packet must fragment");
            eprintln!(
                "original len={} fragments={} sizes={:?} offsets={:?}",
                original.len(),
                fragments.len(),
                fragments.iter().map(|f| f.len()).collect::<Vec<_>>(),
                fragments
                    .iter()
                    .map(|f| u16::from_be_bytes([f[6], f[7]]) & 0x1fff)
                    .collect::<Vec<_>>(),
            );

            let mut buffers = HashMap::new();
            let mut reassembled = None;
            for fragment in fragments {
                match reassemble_outbound_ip_fragment(fragment, &mut buffers) {
                    FragmentReassemblyOutcome::Forward(packet) => reassembled = Some(packet),
                    FragmentReassemblyOutcome::Buffered | FragmentReassemblyOutcome::Dropped => {}
                }
            }

            let reassembled = reassembled.expect("last fragment must yield a complete packet");
            eprintln!(
                "reassembled len={} total_len_field={}",
                reassembled.len(),
                u16::from_be_bytes([reassembled[2], reassembled[3]])
            );
            // The IP header checksum is recomputed on reassembly (total length
            // and fragment fields change), so compare header fields except the
            // checksum and require the payload to be byte-identical.
            assert_eq!(reassembled.len(), original.len());
            assert_eq!(reassembled[..10], original[..10]);
            assert_eq!(reassembled[12..], original[12..]);
            let mut header_without_checksum = reassembled[..20].to_vec();
            header_without_checksum[10] = 0;
            header_without_checksum[11] = 0;
            let computed = ipv4_header_checksum(&header_without_checksum);
            assert_eq!(
                computed.to_be_bytes(),
                [reassembled[10], reassembled[11]],
                "reassembled header checksum must validate"
            );
        }

        #[test]
        fn software_fragment_ipv4_packet_preserves_id_offsets_and_checksums() {
            // Build an inner IMS-ESP-sized packet: IPv4 header + 1588 bytes
            // of ESP frame (mimics the full REGISTER after the transform).
            let mut packet = vec![0u8; 20 + 1588];
            packet[0] = 0x45;
            let total_len = packet.len() as u16;
            packet[2..4].copy_from_slice(&total_len.to_be_bytes());
            packet[4..6].copy_from_slice(&0xabcd_u16.to_be_bytes());
            packet[8] = 64;
            packet[9] = 50; // ESP
            packet[12..16].copy_from_slice(&[2, 30, 238, 251]);
            packet[16..20].copy_from_slice(&[172, 20, 110, 221]);
            let checksum = ipv4_header_checksum(&packet[..20]);
            packet[10..12].copy_from_slice(&checksum.to_be_bytes());

            let fragments = fragment_inner_packet(&packet, AUTO_FRAGMENT_INNER_IP_MAX);
            assert!(fragments.len() >= 2, "1608B packet must fragment");
            let mut offsets = Vec::new();
            for (index, fragment) in fragments.iter().enumerate() {
                let ihl = usize::from(fragment[0] & 0x0f) * 4;
                let flags_offset = u16::from_be_bytes([fragment[6], fragment[7]]);
                offsets.push((flags_offset & 0x1fff, flags_offset & 0x2000 != 0));
                // Every fragment validates its own header checksum.
                let mut header = fragment[..ihl].to_vec();
                header[10] = 0;
                header[11] = 0;
                let computed = ipv4_header_checksum(&header);
                assert_eq!(
                    computed.to_be_bytes(),
                    [fragment[10], fragment[11]],
                    "fragment {index} checksum"
                );
                // IP id preserved.
                assert_eq!(&fragment[4..6], &[0xab, 0xcd]);
                // Fragment payload multiple of 8 except the last.
                let payload_len = fragment.len() - ihl;
                if index + 1 < fragments.len() {
                    assert_eq!(payload_len % 8, 0, "non-last fragment payload");
                }
            }
            let last = fragments.last().expect("has last fragment");
            assert_eq!(
                last.len() % 8,
                0,
                "last fragment also ends at 8-boundary payload"
            );
            assert!(!offsets.last().map(|(_, mf)| *mf).unwrap_or(true));
            assert_eq!(offsets.first().map(|(o, _)| *o), Some(0));
            assert_eq!(offsets[0].1, true, "first fragment must set MF");

            // Reassemble through the existing buffer and compare payloads.
            let mut buffers = HashMap::new();
            let mut reassembled = None;
            for fragment in fragments {
                match reassemble_outbound_ip_fragment(fragment, &mut buffers) {
                    FragmentReassemblyOutcome::Forward(packet) => reassembled = Some(packet),
                    FragmentReassemblyOutcome::Buffered | FragmentReassemblyOutcome::Dropped => {}
                }
            }
            let reassembled = reassembled.expect("reassembles");
            assert_eq!(reassembled, packet);
        }

        #[test]
        fn software_fragment_ipv6_packet_emits_rfc8200_fragment_headers() {
            // Build an inner IPv6 packet: 40-byte base header (next header =
            // 50 ESP) + 1588-byte ESP frame.
            let mut packet = vec![0u8; 40 + 1588];
            packet[0] = 0x60;
            packet[4..6].copy_from_slice(&(1588u16).to_be_bytes());
            packet[6] = 50; // ESP
            packet[8..24].copy_from_slice(
                b"\x20\x01\x0d\xb8\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x01",
            );
            packet[24..40].copy_from_slice(
                b"\x20\x01\x0d\xb8\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02",
            );

            let fragments = fragment_inner_packet(&packet, AUTO_FRAGMENT_INNER_IP_MAX);
            assert!(fragments.len() >= 2, "1628B IPv6 packet must fragment");
            let mut seen_offsets = Vec::new();
            let mut identification = None;
            for (index, fragment) in fragments.iter().enumerate() {
                assert_eq!(fragment[0] >> 4, 6, "fragment stays IPv6");
                assert_eq!(
                    fragment[6], 44,
                    "base header next header must be Fragment Header"
                );
                let next_header = fragment[40];
                assert_eq!(next_header, 50, "Fragment Header next header preserves ESP");
                let offset_m = u16::from_be_bytes([fragment[42], fragment[43]]);
                let offset = (offset_m >> 3) as usize;
                let m_flag = offset_m & 1 == 1;
                seen_offsets.push((offset, m_flag));
                let payload_len = u16::from_be_bytes([fragment[4], fragment[5]]) as usize;
                assert_eq!(payload_len, 8 + fragment.len() - 48);
                let id =
                    u32::from_be_bytes([fragment[44], fragment[45], fragment[46], fragment[47]]);
                match identification {
                    Some(prev) => assert_eq!(prev, id, "same identification across fragments"),
                    None => identification = Some(id),
                }
                // Fragmentable payload multiple of 8 except the last.
                let frag_payload = fragment.len() - 48;
                if index + 1 < fragments.len() {
                    assert_eq!(frag_payload % 8, 0, "non-last IPv6 fragment payload");
                    assert!(m_flag, "non-last must set M");
                }
            }
            assert_eq!(seen_offsets.first().map(|(o, _)| *o), Some(0));
            assert!(!seen_offsets.last().map(|(_, m)| *m).unwrap_or(true));
            let total_frag_payload: usize = fragments.iter().map(|f| f.len() - 48).sum();
            assert_eq!(
                total_frag_payload, 1588,
                "all fragment payloads concatenate to the ESP frame"
            );
        }

        fn build_ipv6_esp_packet() -> Vec<u8> {
            let mut packet = vec![0u8; 40 + 1588];
            packet[0] = 0x60;
            packet[4..6].copy_from_slice(&(1588u16).to_be_bytes());
            packet[6] = 50; // ESP
            packet[8..24].copy_from_slice(
                b"\x20\x01\x0d\xb8\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x01",
            );
            packet[24..40].copy_from_slice(
                b"\x20\x01\x0d\xb8\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02",
            );
            packet
        }

        #[test]
        fn inbound_ipv4_fragments_reassemble_out_of_order() {
            let mut original = vec![0u8; 20 + 1588];
            original[0] = 0x45;
            let total_len = original.len() as u16;
            original[2..4].copy_from_slice(&total_len.to_be_bytes());
            original[4..6].copy_from_slice(&0xbeef_u16.to_be_bytes());
            original[8] = 64;
            original[9] = 50;
            original[12..16].copy_from_slice(&[172, 20, 110, 221]);
            original[16..20].copy_from_slice(&[2, 30, 238, 251]);
            let checksum = ipv4_header_checksum(&original[..20]);
            original[10..12].copy_from_slice(&checksum.to_be_bytes());

            let fragments = fragment_inner_packet(&original, AUTO_FRAGMENT_INNER_IP_MAX);
            assert!(fragments.len() >= 2);
            let mut buffers = HashMap::new();
            let mut forwards = Vec::new();
            for fragment in fragments.iter().rev() {
                match reassemble_inbound_ip_fragment(fragment.clone(), &mut buffers) {
                    FragmentReassemblyOutcome::Forward(packet) => forwards.push(packet),
                    FragmentReassemblyOutcome::Buffered => {}
                    FragmentReassemblyOutcome::Dropped => panic!("IPv4 fragment dropped"),
                }
            }
            assert_eq!(
                forwards.len(),
                1,
                "out-of-order IPv4 fragments reassemble once"
            );
            assert_eq!(forwards[0], original);
        }

        #[test]
        fn inbound_ipv6_fragments_reassemble_out_of_order() {
            let original = build_ipv6_esp_packet();
            let fragments = fragment_inner_packet(&original, AUTO_FRAGMENT_INNER_IP_MAX);
            assert!(fragments.len() >= 2);
            let mut buffers = HashMap::new();
            let mut forwards = Vec::new();
            for fragment in fragments.iter().rev() {
                match reassemble_inbound_ip_fragment(fragment.clone(), &mut buffers) {
                    FragmentReassemblyOutcome::Forward(packet) => forwards.push(packet),
                    FragmentReassemblyOutcome::Buffered => {}
                    FragmentReassemblyOutcome::Dropped => panic!("IPv6 fragment dropped"),
                }
            }
            assert_eq!(
                forwards.len(),
                1,
                "out-of-order IPv6 fragments reassemble once"
            );
            assert_eq!(forwards[0], original);
        }

        #[test]
        fn inbound_overlapping_ipv4_fragments_are_rejected() {
            // Fragment 0: offset 0, MF=1, 100 payload bytes.
            let mut first = vec![0u8; 20 + 100];
            first[0] = 0x45;
            first[2..4].copy_from_slice(&120u16.to_be_bytes());
            first[4..6].copy_from_slice(&0x1234_u16.to_be_bytes());
            first[6] = 0x20; // MF
            first[8] = 64;
            first[9] = 50;
            first[12..16].copy_from_slice(&[172, 20, 110, 221]);
            first[16..20].copy_from_slice(&[2, 30, 238, 251]);
            let checksum = ipv4_header_checksum(&first[..20]);
            first[10..12].copy_from_slice(&checksum.to_be_bytes());
            first[20..].fill(0xaa);

            // Overlapping continuation: offset 8, MF=0, 100 payload bytes
            // overlaps [0,100).
            let mut second = vec![0u8; 20 + 100];
            second[0] = 0x45;
            second[2..4].copy_from_slice(&120u16.to_be_bytes());
            second[4..6].copy_from_slice(&0x1234_u16.to_be_bytes());
            second[6..8].copy_from_slice(&8u16.to_be_bytes()); // offset 8
            second[8] = 64;
            second[9] = 50;
            second[12..16].copy_from_slice(&[172, 20, 110, 221]);
            second[16..20].copy_from_slice(&[2, 30, 238, 251]);
            let checksum = ipv4_header_checksum(&second[..20]);
            second[10..12].copy_from_slice(&checksum.to_be_bytes());
            second[20..].fill(0xbb);

            let mut buffers = HashMap::new();
            match reassemble_inbound_ip_fragment(first, &mut buffers) {
                FragmentReassemblyOutcome::Buffered => {}
                other => panic!("expected buffered, got {other:?}"),
            }
            match reassemble_inbound_ip_fragment(second, &mut buffers) {
                FragmentReassemblyOutcome::Dropped => {}
                other => panic!("expected dropped for overlap, got {other:?}"),
            }
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
